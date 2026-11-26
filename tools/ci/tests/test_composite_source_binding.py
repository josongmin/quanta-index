"""Source-only composite custody regressions; no ranking admission."""

from __future__ import annotations

import hashlib
import json
import subprocess
from pathlib import Path

import pytest

from tools.benchmark.retrieval import composite_source_binding
from tools.benchmark.retrieval import evaluator as ev


def _write_json(path: Path, value: dict) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, sort_keys=True), encoding="utf-8")


@pytest.mark.parametrize("raw", [b'{"a":1,"a":2}', b'{"a":NaN}', b"\xff"])
def test_digest_bound_json_loader_preserves_strict_rejections(tmp_path: Path, raw: bytes) -> None:
    path = tmp_path / "source.json"
    path.write_bytes(raw)
    with pytest.raises(ev.EvidenceError):
        ev.read_json_with_bytes(path)


def test_digest_bound_json_loader_returns_exact_parsed_bytes(tmp_path: Path) -> None:
    path = tmp_path / "source.json"
    raw = b' {"a": 1} \n'
    path.write_bytes(raw)
    assert ev.read_json_with_bytes(path) == (raw, {"a": 1})


def _git_commit(root: Path) -> str:
    subprocess.run(["git", "init", "-q", str(root)], check=True)
    subprocess.run(["git", "-C", str(root), "add", "--all"], check=True)
    subprocess.run(
        [
            "git",
            "-C",
            str(root),
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "commit",
            "-qm",
            "source",
        ],
        check=True,
    )
    return subprocess.check_output(["git", "-C", str(root), "rev-parse", "HEAD"], text=True).strip()


def test_duplicate_repository_in_twelve_slot_roster_is_rejected(
    tmp_path: Path, monkeypatch
) -> None:
    """Twelve slots can repeat one release repository under a fresh namespace."""
    monkeypatch.syspath_prepend(str(Path(ev.__file__).resolve().parents[1]))
    import corpus_release

    base = tmp_path / "base"
    base.mkdir()
    git = tmp_path / "source"
    git.mkdir()
    digest = "sha256:" + "a" * 64
    rows = []
    roster = []
    files = []
    for index in range(11):
        repo = f"repo{index:02d}"
        content = f"package {repo}\n".encode()
        sha = hashlib.sha256(content).hexdigest()
        manifest = f"manifests/{repo}.json"
        _write_json(base / manifest, {"files": [{"path": "main.go", "file_sha256": sha}]})
        original = base / "views" / repo / "code_only" / "main.go"
        original.parent.mkdir(parents=True)
        original.write_bytes(content)
        rows.append(
            {
                "recipe": {"name": repo, "revision": "b" * 40},
                "views": {
                    "code_only": {
                        "manifest": manifest,
                        "file_universe_digest": digest,
                        "file_count": 1,
                    }
                },
            }
        )
        namespaces = [f"r{index:02d}"] + (["r11"] if index == 0 else [])
        for namespace in namespaces:
            path = f"{namespace}/main.go"
            target = git / path
            target.parent.mkdir(parents=True)
            target.write_bytes(content)
            roster.append(
                {
                    "repository": repo,
                    "namespace": namespace,
                    "release_digest": digest,
                    "commit": "b" * 40,
                    "file_universe_digest": digest,
                    "file_count": 1,
                }
            )
            files.append({"path": path, "file_sha256": sha})
    files.sort(key=lambda row: row["path"])
    roster.sort(key=lambda row: row["namespace"])
    commit = _git_commit(git)
    source = ev.SourceSnapshot(git, commit)
    monkeypatch.setattr(
        corpus_release, "validate", lambda path: {"digest": digest, "repositories": rows}
    )
    case = {
        "family_id": "case",
        "release_roster": roster,
        "target_repository": "repo00",
        "target_commit": "b" * 40,
        "file_count": 12,
        "files": files,
        "file_universe_digest": "sha256:" + ev.universe_digest(files),
    }
    universe_path = tmp_path / "universe.json"
    binding_path = tmp_path / "binding.json"
    universe = {
        "kind": "provisional_source_only_twelve_repo_universes_v1",
        "status": "NOT_ADMITTED_NO_QRELS_NO_SDK_RANKS",
        "base_release_digest": digest,
        "cases": [case],
    }
    _write_json(universe_path, universe)
    binding = {
        "kind": "provisional_composite_git_source_binding_v1",
        "status": "SOURCE_ONLY_NOT_FORMAL_EVAL",
        "family_id": "case",
        "universe_sha256": hashlib.sha256(universe_path.read_bytes()).hexdigest(),
        "file_universe_digest": case["file_universe_digest"],
        "file_count": 12,
        "repository_count": 12,
        "source_commit": commit,
    }
    _write_json(binding_path, binding)
    capture = {
        "source_universe_sha256": binding["universe_sha256"],
        "source_binding_sha256": hashlib.sha256(binding_path.read_bytes()).hexdigest(),
    }
    suite = {
        "repository_commit": commit,
        "file_universe": files,
        "file_universe_digest": ev.universe_digest(files),
    }
    original_bytes, original_text = Path.read_bytes, Path.read_text
    reads = {universe_path: 0, binding_path: 0}

    def one_source_read(path, *args, **kwargs):
        if path in reads:
            reads[path] += 1
        return original_bytes(path, *args, **kwargs)

    def no_second_source_read(path, *args, **kwargs):
        if path in reads:
            raise AssertionError("source JSON was reopened after digesting its bytes")
        return original_text(path, *args, **kwargs)

    monkeypatch.setattr(Path, "read_bytes", one_source_read)
    monkeypatch.setattr(Path, "read_text", no_second_source_read)
    with pytest.raises(ev.EvidenceError, match="unique|roster|repository"):
        composite_source_binding.verify(
            source,
            suite,
            suite,
            capture,
            universe_path,
            binding_path,
            base,
            tmp_path / "targets",
            "case",
        )
    assert reads == {universe_path: 1, binding_path: 1}


def _native_cli_fixture(tmp_path: Path, monkeypatch):
    from tools.ci.tests.test_native_file_evidence import complete_capture

    suite, pack, capture = complete_capture()
    suite_file, pack_file, capture_file = (
        tmp_path / name for name in ("suite.json", "pack.json", "capture.json")
    )
    _write_json(suite_file, suite)
    _write_json(pack_file, pack)
    capture["query_pack_file_sha256"] = ev.digest(pack_file.read_bytes())
    monkeypatch.setattr(ev, "validate_suite", lambda *_: (suite, pack, None))
    return suite, pack, capture, suite_file, pack_file, capture_file


def _write_bound_native_inputs(suite, pack, capture, suite_file, pack_file, capture_file):
    pack["suite_commitment_sha256"] = ev.digest(ev.canonical(suite))
    capture["suite_commitment_sha256"] = pack["suite_commitment_sha256"]
    capture["query_pack_sha256"] = ev.digest(ev.canonical(pack))
    _write_json(suite_file, suite)
    _write_json(pack_file, pack)
    capture["query_pack_file_sha256"] = ev.digest(pack_file.read_bytes())
    _write_json(capture_file, capture)


def _run_native_cli(tmp_path: Path, suite_file: Path, pack_file: Path, capture_file: Path) -> int:
    return ev.main(
        [
            "evaluate-native-file",
            "--repo",
            str(tmp_path),
            "--suite",
            str(suite_file),
            "--runner",
            str(capture_file),
            "--query-pack",
            str(pack_file),
        ]
    )


def test_marked_capture_requires_source_inputs_before_scoring(tmp_path: Path, monkeypatch) -> None:
    suite, pack, capture, suite_file, pack_file, capture_file = _native_cli_fixture(
        tmp_path, monkeypatch
    )
    suite["source_provenance"] = {
        "kind": "composite_git_releases_v1",
        "source_universe_sha256": "0" * 64,
        "source_binding_sha256": "1" * 64,
    }
    capture["source_universe_sha256"] = "0" * 64
    capture["source_binding_sha256"] = "1" * 64
    _write_bound_native_inputs(suite, pack, capture, suite_file, pack_file, capture_file)
    assert _run_native_cli(tmp_path, suite_file, pack_file, capture_file) == 2


def test_unmarked_native_capture_retains_existing_single_source_path(
    tmp_path: Path, monkeypatch
) -> None:
    from tools.ci.tests.test_native_file_evidence import _git_backed_native_replay_inputs

    repo, suite_file, pack_file, capture_file = _git_backed_native_replay_inputs(tmp_path)
    original_bytes, original_text = Path.read_bytes, Path.read_text
    reads = []

    def one_pack_read(path, *args, **kwargs):
        if path == pack_file:
            reads.append(path)
        return original_bytes(path, *args, **kwargs)

    def no_second_pack_read(path, *args, **kwargs):
        if path == pack_file:
            raise AssertionError("blind pack was reopened after parsing its bytes")
        return original_text(path, *args, **kwargs)

    monkeypatch.setattr(Path, "read_bytes", one_pack_read)
    monkeypatch.setattr(Path, "read_text", no_second_pack_read)
    assert _run_native_cli(repo, suite_file, pack_file, capture_file) == 0
    assert reads == [pack_file]


def test_frozen_composite_source_cannot_score_without_capture_markers(
    tmp_path: Path, monkeypatch
) -> None:
    suite, pack, capture, suite_file, pack_file, capture_file = _native_cli_fixture(
        tmp_path, monkeypatch
    )
    suite["source_provenance"] = {
        "kind": "composite_git_releases_v1",
        "source_universe_sha256": "0" * 64,
        "source_binding_sha256": "1" * 64,
    }
    _write_bound_native_inputs(suite, pack, capture, suite_file, pack_file, capture_file)
    assert _run_native_cli(tmp_path, suite_file, pack_file, capture_file) == 2


def test_untyped_source_is_not_accepted_as_native_score(tmp_path: Path, monkeypatch) -> None:
    suite, pack, capture, suite_file, pack_file, capture_file = _native_cli_fixture(
        tmp_path, monkeypatch
    )
    del suite["source_provenance"]
    _write_bound_native_inputs(suite, pack, capture, suite_file, pack_file, capture_file)
    assert _run_native_cli(tmp_path, suite_file, pack_file, capture_file) == 2


def test_verified_composite_source_exposes_exact_target_release_without_qualification(
    tmp_path: Path, monkeypatch
) -> None:
    monkeypatch.syspath_prepend(str(Path(ev.__file__).resolve().parents[1]))
    import corpus_release

    base = tmp_path / "base"
    targets = tmp_path / "targets"
    target = targets / "one"
    git = tmp_path / "source"
    base_digest = "sha256:" + "b" * 64
    target_digest = "sha256:" + "c" * 64
    base_commit = "b" * 40
    target_commit = "c" * 40
    base_rows, roster, files = [], [], []
    for index in range(12):
        repository = f"repo{index:02d}"
        namespace = f"r{index:02d}"
        revision = target_commit if index == 0 else base_commit
        release_digest = target_digest if index == 0 else base_digest
        release_root = target if index == 0 else base
        content = f"package {repository} at {revision}\n".encode()
        sha = hashlib.sha256(content).hexdigest()
        manifest = f"manifests/{repository}.json"
        _write_json(release_root / manifest, {"files": [{"path": "main.go", "file_sha256": sha}]})
        original = release_root / "views" / repository / "code_only" / "main.go"
        original.parent.mkdir(parents=True, exist_ok=True)
        original.write_bytes(content)
        source_file = git / namespace / "main.go"
        source_file.parent.mkdir(parents=True, exist_ok=True)
        source_file.write_bytes(content)
        view = {
            "manifest": manifest,
            "file_universe_digest": "sha256:" + f"{index + 1:064x}",
            "file_count": 1,
        }
        row = {
            "recipe": {
                "name": repository,
                "revision": revision,
                "url": f"https://example.invalid/official/{repository}",
            },
            "views": {"code_only": view},
        }
        if index != 0:
            base_rows.append(row)
        roster.append(
            {
                "repository": repository,
                "namespace": namespace,
                "release_digest": release_digest,
                "commit": revision,
                "file_universe_digest": view["file_universe_digest"],
                "file_count": 1,
            }
        )
        files.append({"path": f"{namespace}/main.go", "file_sha256": sha})
    base_rows.insert(
        0,
        {
            "recipe": {
                "name": "repo00",
                "revision": base_commit,
                "url": "https://example.invalid/official/repo00",
            },
            "views": {
                "code_only": {
                    "manifest": "unused",
                    "file_universe_digest": "unused",
                    "file_count": 1,
                }
            },
        },
    )
    _write_json(target / "release.json", {"digest": target_digest})
    # The target release's exact manifest is selected only for repo00.
    target_rows = [
        {
            "recipe": {
                "name": "repo00",
                "revision": target_commit,
                "url": "https://example.invalid/official/repo00",
            },
            "views": {
                "code_only": {
                    "manifest": "manifests/repo00.json",
                    "file_universe_digest": roster[0]["file_universe_digest"],
                    "file_count": 1,
                }
            },
        }
    ]
    monkeypatch.setattr(
        corpus_release,
        "validate",
        lambda path: (
            {"digest": target_digest, "repositories": target_rows}
            if path == target
            else {"digest": base_digest, "repositories": base_rows}
        ),
    )
    commit = _git_commit(git)
    case = {
        "family_id": "family-00",
        "release_roster": roster,
        "target_repository": "repo00",
        "target_commit": target_commit,
        "file_count": 12,
        "files": files,
        "file_universe_digest": "sha256:" + ev.universe_digest(files),
    }
    universe_path, binding_path = tmp_path / "universe.json", tmp_path / "binding.json"
    _write_json(
        universe_path,
        {
            "kind": "provisional_source_only_twelve_repo_universes_v1",
            "status": "NOT_ADMITTED_NO_QRELS_NO_SDK_RANKS",
            "base_release_digest": base_digest,
            "cases": [case],
        },
    )
    binding = {
        "kind": "provisional_composite_git_source_binding_v1",
        "status": "SOURCE_ONLY_NOT_FORMAL_EVAL",
        "family_id": case["family_id"],
        "universe_sha256": hashlib.sha256(universe_path.read_bytes()).hexdigest(),
        "file_universe_digest": case["file_universe_digest"],
        "file_count": 12,
        "repository_count": 12,
        "source_commit": commit,
    }
    _write_json(binding_path, binding)
    capture = {
        "source_universe_sha256": binding["universe_sha256"],
        "source_binding_sha256": hashlib.sha256(binding_path.read_bytes()).hexdigest(),
    }
    query = "Find the package entry point"
    suite = {
        "schema_version": ev.SCHEMA_VERSION,
        "suite_id": "composite-native-review",
        "repository_commit": commit,
        "file_universe": files,
        "file_universe_digest": ev.universe_digest(files),
        "source_provenance": {"kind": "composite_git_releases_v1", **capture},
        "routes": ["lexical", "hybrid"],
        "comparison_contract": {
            "top_k": 10,
            "tokenizer": ev.TOKENIZER,
            "tokenizer_budget_version": ev.TOKENIZER_BUDGET_VERSION,
            "output_unit_policy": "rank_prefix",
            "span_unit": ev.SPAN_UNIT,
        },
        "tasks": [
            {
                "task_id": "review-entry-point",
                "split": "eval",
                "query": query,
                "query_sha256": ev.digest(query.encode()),
                "query_family_id": case["family_id"],
                "answerable": False,
                "gold": [],
            }
        ],
    }
    suite, pack, source = ev.validate_suite(git, suite)
    result = composite_source_binding.verify(
        source, suite, pack, capture, universe_path, binding_path, base, targets, case["family_id"]
    )
    assert result["target_repository"] == "repo00"
    assert result["target_commit"] == target_commit
    assert result["target_release_digest"] == target_digest
    assert result["base_release_digest"] == base_digest
    assert result["source_commit"] == commit
    assert result["qualification"] == "SOURCE_ONLY_NOT_FORMAL_EVAL"
    assert result["file_universe_digest"] == case["file_universe_digest"]
    direct = composite_source_binding.verify(
        source, suite, pack, capture, universe_path, binding_path, base, target, case["family_id"]
    )
    assert direct == result
    _write_json(target / "nested" / "release.json", {"digest": target_digest})
    with pytest.raises(ev.EvidenceError, match="ambiguous"):
        composite_source_binding.verify(
            source,
            suite,
            pack,
            capture,
            universe_path,
            binding_path,
            base,
            target,
            case["family_id"],
        )
    mismatched_pack = {**pack, "file_universe_digest": "0" * 64}
    with pytest.raises(ev.EvidenceError, match="source/query/capture binding differs"):
        composite_source_binding.verify(
            source,
            suite,
            mismatched_pack,
            capture,
            universe_path,
            binding_path,
            base,
            targets,
            case["family_id"],
        )

    base_target = base_rows[0]
    target_recipe = target_rows[0]["recipe"]
    target_recipe["url"] = "https://example.invalid/fork/repo00"
    with pytest.raises(ev.EvidenceError, match="target upstream repository differs from base"):
        composite_source_binding.verify(
            source,
            suite,
            pack,
            capture,
            universe_path,
            binding_path,
            base,
            targets,
            case["family_id"],
        )
    target_recipe["url"] = base_target["recipe"]["url"]
    base_target["recipe"]["revision"] = target_commit
    with pytest.raises(ev.EvidenceError, match="target commit must differ"):
        composite_source_binding.verify(
            source,
            suite,
            pack,
            capture,
            universe_path,
            binding_path,
            base,
            targets,
            case["family_id"],
        )
    base_target["recipe"]["revision"] = base_commit
    base_target["views"]["code_only"]["file_universe_digest"] = roster[0]["file_universe_digest"]
    with pytest.raises(ev.EvidenceError, match="target code-only universe must differ"):
        composite_source_binding.verify(
            source,
            suite,
            pack,
            capture,
            universe_path,
            binding_path,
            base,
            targets,
            case["family_id"],
        )


def test_build_materializes_exactly_one_changed_repository_per_family(tmp_path, monkeypatch):
    corpus_release = composite_source_binding._corpus_release()
    base, targets = tmp_path / "base", tmp_path / "targets"
    target = targets / "official"
    releases = {}
    for root, role in ((base, "base"), (target, "target")):
        rows = []
        for index in range(12):
            name = f"repo{index:02d}"
            path = root / "views" / name / "code_only" / "mod.py"
            path.parent.mkdir(parents=True)
            content = f"VALUE = '{role}-{index}'\n".encode()
            path.write_bytes(content)
            files = [{"path": "mod.py", "file_sha256": ev.digest(content)}]
            manifest = f"manifests/{name}.json"
            _write_json(root / manifest, {"files": files})
            rows.append(
                {
                    "recipe": {
                        "name": name,
                        "url": f"https://example.invalid/{name}",
                        "revision": ev.digest(f"{role}:{name}".encode())[:40],
                    },
                    "views": {
                        "code_only": {
                            "manifest": manifest,
                            "file_count": 1,
                            "manifest_digest": f"sha256:{ev.digest((root / manifest).read_bytes())}",
                            "file_universe_digest": f"sha256:{ev.universe_digest(files)}",
                        }
                    },
                }
            )
        release = {"digest": f"sha256:{ev.digest(role.encode())}", "repositories": rows}
        releases[root] = release
        _write_json(root / "release.json", release)
    monkeypatch.setattr(corpus_release, "validate", releases.__getitem__)
    output = tmp_path / "composite"
    assert (
        composite_source_binding.main(
            [
                "--base-release",
                str(base),
                "--target-release",
                str(target),
                "--output",
                str(output),
            ]
        )
        == 0
    )
    universe_path = output / "source-universe.json"
    universe = ev.read_json(universe_path)
    assert len(universe["cases"]) == 12
    for index, case in enumerate(universe["cases"]):
        name = f"repo{index:02d}"
        root = output / name / "source"
        binding_path = output / name / "source-binding.json"
        source = ev.SourceSnapshot(root, ev.read_json(binding_path)["source_commit"])
        assert sorted(source.tracked) == [f"r{i:02d}/mod.py" for i in range(12)]
        for i in range(12):
            role = "target" if i == index else "base"
            assert source.file(f"r{i:02d}/mod.py")[0] == f"VALUE = '{role}-{i}'\n".encode()
        suite = {
            "repository_commit": source.commit,
            "file_universe": case["files"],
            "file_universe_digest": ev.universe_digest(case["files"]),
        }
        capture = {
            "source_universe_sha256": ev.digest(universe_path.read_bytes()),
            "source_binding_sha256": ev.digest(binding_path.read_bytes()),
        }
        result = composite_source_binding.verify(
            source,
            suite,
            suite,
            capture,
            universe_path,
            binding_path,
            base,
            targets,
            name,
        )
        assert result["target_repository"] == name
        assert (
            result["target_commit"] == releases[target]["repositories"][index]["recipe"]["revision"]
        )
    with pytest.raises(FileExistsError):
        composite_source_binding.build(base, target, output)
    _write_json(base / "manifests/repo00.json", {"files": []})
    failed = tmp_path / "rejected-composite"
    with pytest.raises(ev.EvidenceError, match="view manifest changed"):
        composite_source_binding.build(base, target, failed)
    assert not failed.exists()
