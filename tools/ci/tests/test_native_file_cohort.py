"""CLI admission for independently replayed Native file families."""

from __future__ import annotations

import copy
import json
import subprocess
from pathlib import Path

import pytest

from tools.benchmark.retrieval import composite_source_binding
from tools.benchmark.retrieval import evaluator as ev
from tools.ci.tests.test_native_file_evidence import _native_family_cohort


def _git_checkout(path: Path) -> None:
    path.mkdir(parents=True)
    (path / "source.rs").write_text("fn original() {}\n", encoding="utf-8")
    subprocess.run(["git", "init", "-q", str(path)], check=True)
    subprocess.run(["git", "-C", str(path), "add", "--all"], check=True)
    subprocess.run(
        [
            "git",
            "-C",
            str(path),
            "-c",
            "user.name=Native fixture",
            "-c",
            "user.email=native@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-qm",
            "original",
        ],
        check=True,
    )


def _advance_git_head(path: Path) -> None:
    (path / "source.rs").write_text("fn successor() {}\n", encoding="utf-8")
    subprocess.run(["git", "-C", str(path), "add", "--all"], check=True)
    subprocess.run(
        [
            "git",
            "-C",
            str(path),
            "-c",
            "user.name=Native fixture",
            "-c",
            "user.email=native@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-qm",
            "successor",
        ],
        check=True,
    )


def _bind_fixture_to_git(item: dict, repo: Path) -> dict:
    replay = copy.deepcopy(item)
    commit = subprocess.check_output(
        ["git", "-C", str(repo), "rev-parse", "HEAD"],
        text=True,
    ).strip()
    replay["suite"]["repository_commit"] = commit
    replay["capture"]["repository_commit"] = commit
    replay["report"]["repository_commit"] = commit
    binding = replay["report"]["composite_source_binding"]
    binding["source_commit"] = commit
    suite_digest = ev.digest(ev.canonical(replay["suite"]))
    replay["capture"]["suite_commitment_sha256"] = suite_digest
    replay["report"]["suite_commitment_sha256"] = suite_digest
    replay["report"]["capture_commitment_sha256"] = ev.digest(ev.canonical(replay["capture"]))
    return replay


def _write_replay_inputs(manifest_path: Path) -> None:
    manifest = json.loads(manifest_path.read_bytes())
    for family in manifest["families"]:
        for key in ("suite", "query_pack", "capture", "source_universe", "source_binding"):
            path = manifest_path.parent / family[key]
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(json.dumps({"input": key}), encoding="utf-8")


def _manifest(tmp_path: Path) -> tuple[Path, list[dict]]:
    replayed, repositories, strata = _native_family_cohort()
    manifest = {
        "schema_version": 1,
        "profile": "native_file_family_cohort",
        "repositories": repositories,
        "repository_strata": strata,
        "families": [
            {
                "repo": f"inputs/{index}/repo",
                "suite": f"inputs/{index}/suite.json",
                "query_pack": f"inputs/{index}/pack.json",
                "capture": f"inputs/{index}/capture.json",
                "source_universe": f"inputs/{index}/universe.json",
                "source_binding": f"inputs/{index}/binding.json",
                "base_release": f"inputs/{index}/base.json",
                "target_releases": f"inputs/{index}/targets.json",
                "family_id": item["report"]["composite_source_binding"]["family_id"],
            }
            for index, item in enumerate(replayed)
        ],
    }
    path = tmp_path / "cohort.json"
    path.write_text(json.dumps(manifest), encoding="utf-8")
    return path, replayed


def _runtime_manifest(tmp_path: Path, *, selected: bool) -> Path:
    path, _ = _manifest(tmp_path)
    value = json.loads(path.read_text(encoding="utf-8"))
    value["schema_version"] = 2
    for index, family in enumerate(value["families"]):
        family["daemon_config"] = f"runtime/{index}/daemon.json"
        family["daemon_config_sha256"] = f"{index + 1:064x}"
        if selected:
            family["selection_path"] = f"runtime/{index}/selection.json"
            family["selection_sha256"] = f"{index + 101:064x}"
    path.write_text(json.dumps(value), encoding="utf-8")
    return path


@pytest.mark.parametrize("selected", [False, True])
def test_native_file_cohort_v2_accepts_one_runtime_mode(selected, tmp_path):
    path = _runtime_manifest(tmp_path, selected=selected)
    value = json.loads(path.read_text(encoding="utf-8"))
    value["families"][0]["daemon_config"] = str(tmp_path / "external-daemon.json")
    if selected:
        value["families"][0]["selection_path"] = str(tmp_path / "external-selection.json")
    path.write_text(json.dumps(value), encoding="utf-8")
    _raw, manifest = ev._native_cohort_manifest(path)
    assert manifest["schema_version"] == 2
    assert all(("selection_path" in row) == selected for row in manifest["families"])


@pytest.mark.parametrize(
    "mutation",
    [
        "mixed_mode",
        "partial_selection",
        "duplicate_config",
        "duplicate_selection",
        "config_alias",
        "selection_alias",
        "malformed_config_sha",
        "malformed_selection_sha",
    ],
)
def test_native_file_cohort_v2_rejects_runtime_path_or_mode_drift(mutation, tmp_path):
    path = _runtime_manifest(tmp_path, selected=True)
    value = json.loads(path.read_text(encoding="utf-8"))
    first, second = value["families"][:2]
    if mutation == "mixed_mode":
        del second["selection_path"], second["selection_sha256"]
    elif mutation == "partial_selection":
        del first["selection_sha256"]
    elif mutation == "duplicate_config":
        second["daemon_config"] = first["daemon_config"]
    elif mutation == "duplicate_selection":
        second["selection_path"] = first["selection_path"]
    elif mutation == "config_alias":
        second["daemon_config"] = "runtime/1/../0/daemon.json"
    elif mutation == "selection_alias":
        second["selection_path"] = "runtime/1/../0/selection.json"
    elif mutation == "malformed_config_sha":
        first["daemon_config_sha256"] = "0" * 63
    else:
        first["selection_sha256"] = "z" * 64
    path.write_text(json.dumps(value), encoding="utf-8")
    with pytest.raises(ev.EvidenceError, match="Native cohort"):
        ev._native_cohort_manifest(path)


def test_native_file_cohort_cli_replays_and_publishes_repository_equal_gold(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    tmp_path = tmp_path.resolve()
    manifest_path, replayed = _manifest(tmp_path)
    _write_replay_inputs(manifest_path)
    by_family = {item["report"]["composite_source_binding"]["family_id"]: item for item in replayed}
    calls = []

    def replay(repo, suite, query_pack, capture, source_inputs, **commitments):
        calls.append((repo, suite, query_pack, capture, source_inputs, commitments))
        return copy.deepcopy(by_family[source_inputs[-1]])

    monkeypatch.setattr(ev, "load_native_file_replay", replay, raising=False)
    monkeypatch.setattr(ev, "verify_repo", lambda repo, _commit: repo.resolve())
    output = tmp_path / "result.json"
    assert (
        ev.main(
            [
                "evaluate-native-file-cohort",
                "--manifest",
                str(manifest_path),
                "--output",
                str(output),
            ]
        )
        == 0
    )
    value = json.loads(output.read_text(encoding="utf-8"))
    assert len(calls) == 13
    assert calls[0][0] == tmp_path / "inputs/0/repo"
    assert calls[0][4][0] == tmp_path / "inputs/0/universe.json"
    for _repo, suite, pack, capture, source, commitments in calls:
        inputs = dict(
            zip(
                ("suite", "query_pack", "capture", "source_universe", "source_binding"),
                (suite, pack, capture, *source[:2]),
                strict=True,
            )
        )
        for key, path in inputs.items():
            raw = path.read_bytes()
            assert commitments[f"expected_{key}"] == ("sha256:" + ev.digest(raw), len(raw))
    assert value["manifest_file_sha256"] == ev.digest(manifest_path.read_bytes())
    assert value["status"] == "native_repository_cluster_diagnostic_unqualified"
    assert value["product_default_decision"] is False
    assert value["runtime_qualified"] is False
    ci = value["repository_cluster_ci"]
    assert (ci["repository_count"], ci["family_count"], ci["sample_count"]) == (12, 13, 31)
    assert ci["mean"] == pytest.approx(-11 / 12)
    assert ci["mean"] != pytest.approx(7 / 31)


@pytest.mark.parametrize(
    "mutation",
    [
        "injected_report",
        "injected_qualification",
        "missing_input",
        "duplicate_family",
        "duplicate_capture",
        "wrong_profile",
        "duplicate_json_key",
    ],
)
def test_native_file_cohort_cli_rejects_invalid_manifest_before_replay(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, mutation: str
) -> None:
    path, _ = _manifest(tmp_path)
    manifest = json.loads(path.read_text(encoding="utf-8"))
    if mutation == "injected_report":
        manifest["families"][0]["report"] = {"status": "qualified"}
    elif mutation == "injected_qualification":
        manifest["qualification"] = "verified"
    elif mutation == "missing_input":
        del manifest["families"][0]["source_binding"]
    elif mutation == "duplicate_family":
        manifest["families"][1]["family_id"] = manifest["families"][0]["family_id"]
    elif mutation == "duplicate_capture":
        manifest["families"][1]["capture"] = manifest["families"][0]["capture"]
    elif mutation == "wrong_profile":
        manifest["profile"] = "qualified_native_file"
    if mutation == "duplicate_json_key":
        path.write_text(
            path.read_text(encoding="utf-8").replace(
                '"schema_version": 1', '"schema_version": 1, "schema_version": 1', 1
            ),
            encoding="utf-8",
        )
    else:
        path.write_text(json.dumps(manifest), encoding="utf-8")
    monkeypatch.setattr(
        ev, "load_native_file_replay", lambda *_: pytest.fail("invalid manifest reached replay")
    )
    output = tmp_path / "result.json"
    assert (
        ev.main(
            [
                "evaluate-native-file-cohort",
                "--manifest",
                str(path),
                "--output",
                str(output),
            ]
        )
        == 2
    )
    assert not output.exists()


def test_native_file_cohort_cli_rejects_replay_from_wrong_family(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture
) -> None:
    path, replayed = _manifest(tmp_path)
    _write_replay_inputs(path)
    monkeypatch.setattr(ev, "load_native_file_replay", lambda *_, **__: replayed[1])
    assert ev.main(["evaluate-native-file-cohort", "--manifest", str(path)]) == 2
    assert "replay family differs" in capsys.readouterr().err


@pytest.mark.parametrize("changed", ["manifest", "earlier_suite", "later_capture", "capture_alias"])
def test_native_file_cohort_refuses_inputs_changed_during_replay(
    changed: str, tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture
) -> None:
    tmp_path = tmp_path.resolve()
    manifest_path, replayed = _manifest(tmp_path)
    _write_replay_inputs(manifest_path)
    by_family = {row["report"]["composite_source_binding"]["family_id"]: row for row in replayed}
    alias = tmp_path / "inputs/0/capture.json"
    alternate = tmp_path / "alternate-capture.json"
    if changed == "capture_alias":
        alternate.write_bytes(alias.read_bytes())
        original = alias.rename(alias.with_suffix(".original"))
        alias.symlink_to(original)
    calls = []

    def replay(_repo, suite, _pack, capture, source_inputs, **commitments):
        ev.read_json_with_bytes(suite, expected_bytes=commitments.get("expected_suite"))
        ev.read_json_with_bytes(capture, expected_bytes=commitments.get("expected_capture"))
        calls.append(source_inputs[-1])
        if len(calls) == 1:
            if changed == "capture_alias":
                alias.unlink()
                alias.symlink_to(alternate)
            else:
                target = {
                    "manifest": manifest_path,
                    "earlier_suite": suite,
                    "later_capture": tmp_path / "inputs/1/capture.json",
                }[changed]
                target.write_bytes(target.read_bytes().replace(b" ", b"\t", 1))
        return copy.deepcopy(by_family[source_inputs[-1]])

    monkeypatch.setattr(ev, "load_native_file_replay", replay)
    output = tmp_path / "report.json"
    assert (
        ev.main(
            [
                "evaluate-native-file-cohort",
                "--manifest",
                str(manifest_path),
                "--output",
                str(output),
            ]
        )
        == 2
    )
    assert calls
    assert "commitment" in capsys.readouterr().err
    assert not output.exists()


@pytest.mark.parametrize("role", ["base_release", "target_releases"])
def test_native_file_cohort_keeps_release_alias_visible_to_validator(
    role: str, tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture
) -> None:
    tmp_path = tmp_path.resolve()
    manifest_path, replayed = _manifest(tmp_path)
    _write_replay_inputs(manifest_path)
    family = json.loads(manifest_path.read_bytes())["families"][0]
    release = tmp_path / "release"
    release.mkdir()
    (tmp_path / family[role]).symlink_to(release, target_is_directory=True)
    by_family = {row["report"]["composite_source_binding"]["family_id"]: row for row in replayed}

    def replay(_repo, _suite, _pack, _capture, source_inputs, **_):
        if source_inputs[-1] == family["family_id"]:
            path = source_inputs[2 if role == "base_release" else 3]
            composite_source_binding._corpus_release().regular_tree(path)
        return copy.deepcopy(by_family[source_inputs[-1]])

    monkeypatch.setattr(ev, "load_native_file_replay", replay)
    assert ev.main(["evaluate-native-file-cohort", "--manifest", str(manifest_path)]) == 2
    assert "root or ancestor is a symlink" in capsys.readouterr().err


@pytest.mark.parametrize("output_kind", ["manifest", "runner", "dangling_symlink"])
def test_native_evaluation_cli_rejects_existing_output_before_replay(
    output_kind: str, tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture
) -> None:
    output = tmp_path / "output.json"
    if output_kind == "dangling_symlink":
        output.symlink_to(tmp_path / "absent.json")
    else:
        output.write_bytes(b"original evidence\n")
    if output_kind == "runner":
        arguments = [
            "evaluate-native-file",
            "--repo",
            str(tmp_path),
            "--suite",
            "suite.json",
            "--query-pack",
            "pack.json",
            "--runner",
            str(output),
        ]
    else:
        arguments = ["evaluate-native-file-cohort", "--manifest", str(output)]
    for attr in ("evaluate_native_file_cohort", "load_native_file_replay"):
        monkeypatch.setattr(
            ev, attr, lambda *_, **__: pytest.fail("existing output reached replay")
        )
    assert ev.main([*arguments, "--output", str(output)]) == 2
    assert "output already exists" in capsys.readouterr().err
    if output_kind == "dangling_symlink":
        assert output.is_symlink() and not output.exists()
    else:
        assert output.read_bytes() == b"original evidence\n"


def test_native_evaluation_cli_does_not_truncate_output_created_during_replay(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    output = tmp_path / "result.json"

    def replay(_manifest, *, output):
        output.path.write_bytes(b"concurrent report\n")
        return {"result": "new"}

    monkeypatch.setattr(ev, "evaluate_native_file_cohort", replay)
    assert (
        ev.main(
            [
                "evaluate-native-file-cohort",
                "--manifest",
                "manifest.json",
                "--output",
                str(output),
            ]
        )
        == 2
    )
    assert output.read_bytes() == b"concurrent report\n"


@pytest.mark.parametrize("case_alias", [False, True])
def test_native_file_evaluation_refuses_output_inside_source_checkout(
    tmp_path: Path,
    capsys: pytest.CaptureFixture,
    case_alias: bool,
) -> None:
    from tools.ci.tests.test_native_file_evidence import _git_backed_native_replay_inputs

    repo, suite, pack, capture = _git_backed_native_replay_inputs(tmp_path)
    destination = repo
    if case_alias:
        destination = repo.with_name(repo.name.swapcase())
        if not destination.is_dir() or not destination.samefile(repo):
            pytest.skip("test volume does not resolve case aliases")
    output = destination / "report.json"
    assert (
        ev.main(
            [
                "evaluate-native-file",
                "--repo",
                str(repo),
                "--suite",
                str(suite),
                "--query-pack",
                str(pack),
                "--runner",
                str(capture),
                "--output",
                str(output),
            ]
        )
        == 2
    )
    assert "Native report output overlaps source repository" in capsys.readouterr().err
    assert not output.exists()
    assert ev.load_native_file_replay(repo, suite, pack, capture)["report"]


def test_native_file_evaluation_rechecks_git_head_after_scoring(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    from tools.benchmark.retrieval import native_file_evidence
    from tools.ci.tests.test_native_file_evidence import _git_backed_native_replay_inputs

    repo, suite, pack, capture = _git_backed_native_replay_inputs(tmp_path)
    original_evaluate = native_file_evidence.evaluate

    def evaluate(*args, **options):
        result = original_evaluate(*args, **options)
        _advance_git_head(repo)
        return result

    monkeypatch.setattr(native_file_evidence, "evaluate", evaluate)
    output = tmp_path / "report.json"
    assert (
        ev.main(
            [
                "evaluate-native-file",
                "--repo",
                str(repo),
                "--suite",
                str(suite),
                "--query-pack",
                str(pack),
                "--runner",
                str(capture),
                "--output",
                str(output),
            ]
        )
        == 2
    )
    assert not output.exists()


def test_native_cohort_refuses_output_inside_release_input(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    manifest_path, replayed = _manifest(tmp_path)
    _write_replay_inputs(manifest_path)
    by_family = {item["report"]["composite_source_binding"]["family_id"]: item for item in replayed}
    monkeypatch.setattr(
        ev,
        "load_native_file_replay",
        lambda *args, **_options: copy.deepcopy(by_family[args[-1][-1]]),
    )
    release = tmp_path / json.loads(manifest_path.read_bytes())["families"][0]["base_release"]
    release.mkdir(parents=True)
    output = release / "report.json"
    assert (
        ev.main(
            [
                "evaluate-native-file-cohort",
                "--manifest",
                str(manifest_path),
                "--output",
                str(output),
            ]
        )
        == 2
    )
    assert not output.exists()


def test_native_cohort_refuses_output_parent_alias_changed_during_replay(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    first, second = tmp_path / "first", tmp_path / "second"
    first.mkdir()
    second.mkdir()
    alias = tmp_path / "reports"
    alias.symlink_to(first, target_is_directory=True)
    output = alias / "report.json"

    def replay(_manifest, *, output):
        alias.unlink()
        alias.symlink_to(second, target_is_directory=True)
        return {"result": "new"}

    monkeypatch.setattr(ev, "evaluate_native_file_cohort", replay)
    assert (
        ev.main(
            [
                "evaluate-native-file-cohort",
                "--manifest",
                str(tmp_path / "manifest.json"),
                "--output",
                str(output),
            ]
        )
        == 2
    )
    assert not (first / "report.json").exists()
    assert not (second / "report.json").exists()


def test_native_cohort_rechecks_git_head_after_aggregation(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    from tools.benchmark.retrieval import native_file_evidence

    manifest_path, replayed = _manifest(tmp_path)
    _write_replay_inputs(manifest_path)
    entries = json.loads(manifest_path.read_bytes())["families"]
    by_family = {}
    for entry, item in zip(entries, replayed, strict=True):
        repo = tmp_path / entry["repo"]
        _git_checkout(repo)
        by_family[entry["family_id"]] = _bind_fixture_to_git(item, repo)
    monkeypatch.setattr(
        ev,
        "load_native_file_replay",
        lambda *args, **_options: copy.deepcopy(by_family[args[-1][-1]]),
    )
    original_aggregate = native_file_evidence.repository_cluster_ci_from_native_families
    changed_repo = tmp_path / entries[0]["repo"]

    def aggregate(*args):
        result = original_aggregate(*args)
        _advance_git_head(changed_repo)
        return result

    monkeypatch.setattr(
        native_file_evidence, "repository_cluster_ci_from_native_families", aggregate
    )
    with pytest.raises(ev.EvidenceError, match="HEAD|checkout"):
        ev.evaluate_native_file_cohort(manifest_path)


@pytest.mark.parametrize("command", ["evaluate-native-file", "evaluate-native-file-cohort"])
def test_native_evaluation_cli_refuses_cyclic_input_without_traceback(
    tmp_path: Path, capsys: pytest.CaptureFixture, command: str
) -> None:
    cyclic = tmp_path / "cyclic"
    cyclic.symlink_to(cyclic)
    output = tmp_path / "report.json"
    if command == "evaluate-native-file":
        from tools.ci.tests.test_native_file_evidence import _git_backed_native_replay_inputs

        _repo, suite, pack, capture = _git_backed_native_replay_inputs(tmp_path)
        args = [
            command,
            "--repo",
            str(cyclic),
            "--suite",
            str(suite),
            "--query-pack",
            str(pack),
            "--runner",
            str(capture),
        ]
    else:
        args = [command, "--manifest", str(cyclic)]
    assert ev.main([*args, "--output", str(output)]) == 2
    assert not output.exists()
    assert "Traceback" not in capsys.readouterr().err
