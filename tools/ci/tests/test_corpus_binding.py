"""Real Git release/view/query binding; no product search or index attestation."""

from __future__ import annotations

import hashlib
import io
import json
import shutil
import sys
import zipfile
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[3] / "tools/benchmark"))
import corpus_binding as binding
from evidence import EvidenceError, write_raw_file

from tools.benchmark.retrieval.retrieval_contract import canonical
from tools.ci.tests.test_corpus_release import (  # noqa: F401
    release,
    release_seed,
    source,
    source_seed,
)


@pytest.fixture
def inputs(release_seed, tmp_path):  # noqa: F811
    # Validation remains per call; only immutable fixture construction is shared.
    root = tmp_path / "release"
    shutil.copytree(release_seed, root, symlinks=True)
    return inputs_from_release(root)


def inputs_from_release(root):
    document = json.loads((root / "release.json").read_bytes())
    selected = document["repositories"][0]["views"]["code_only"]
    manifest = json.loads((root / selected["manifest"]).read_bytes())
    suite = {
        "repository_commit": manifest["repository_commit"],
        "file_universe": manifest["files"],
        "file_universe_digest": selected["file_universe_digest"][7:],
        "tasks": [{"task_id": "one", "gold": [{"path": "src/main.rs"}]}],
    }
    pack = {
        **{
            key: suite[key]
            for key in ("repository_commit", "file_universe", "file_universe_digest")
        },
        "suite_commitment_sha256": hashlib.sha256(canonical(suite)).hexdigest(),
        "tasks": [{"task_id": "one", "query": "main"}],
    }
    selection = {
        "release_path": str(root),
        "release_digest": document["digest"],
        "repository": "fixture",
        "view": "code_only",
    }
    return root, selection, suite, pack


def test_mutated_consumer_copy_cannot_poison_next_case(inputs, release_seed, tmp_path):  # noqa: F811
    root, selection, suite, pack = inputs
    relative = "views/fixture/code_only/src/main.rs"
    pristine = (release_seed / relative).read_bytes()
    view = root / relative
    view.chmod(0o644)
    view.write_bytes(b"corrupted independent test input")
    with pytest.raises(EvidenceError):
        binding.capture(
            root, selection, canonical(suite), canonical(pack), root.with_suffix(".zip")
        )
    assert (release_seed / relative).read_bytes() == pristine
    fresh = tmp_path / "next-consumer"
    shutil.copytree(release_seed, fresh, symlinks=True)
    selection = {**selection, "release_path": str(fresh)}
    result, capsule = binding.capture(
        fresh, selection, canonical(suite), canonical(pack), fresh.with_suffix(".zip")
    )
    assert binding.replay(capsule, selection, canonical(suite), canonical(pack)) == result


@pytest.mark.parametrize("view", ["code_only", "developer_search"])
def test_capsule_replays_after_original_sources_are_unavailable(source, tmp_path, view):  # noqa: F811
    # This oracle must remove the actual producer checkout, not a spare seed copy.
    original, _ = release(source, tmp_path)
    root, selection, suite, pack = inputs_from_release(original)
    if view != selection["view"]:
        document = json.loads((root / "release.json").read_bytes())
        metadata = document["repositories"][0]["views"][view]
        manifest = json.loads((root / metadata["manifest"]).read_bytes())
        selection["view"] = view
        for payload in (suite, pack):
            payload["file_universe"] = manifest["files"]
            payload["file_universe_digest"] = metadata["file_universe_digest"][7:]
        pack["suite_commitment_sha256"] = hashlib.sha256(canonical(suite)).hexdigest()
    result, capsule = binding.capture(
        root, selection, canonical(suite), canonical(pack), root.with_suffix(".zip")
    )
    assert result["index_universe_attested"] is False
    assert result["file_universe_digest"] == "sha256:" + suite["file_universe_digest"]
    shutil.rmtree(source[0])
    root.rename(root.with_name("original-unavailable"))
    assert not source[0].exists() and not root.exists()
    assert binding.replay(capsule, selection, canonical(suite), canonical(pack)) == result
    with zipfile.ZipFile(capsule.path) as archive:
        assert archive.namelist() == ["bundles/fixture.bundle", "recipe.json", "release.json"]


@pytest.mark.parametrize(
    "mutation",
    [
        "suite_commit",
        "pack_commit",
        "suite_files",
        "pack_files",
        "suite_hash",
        "pack_hash",
        "pack_binding",
        "gold",
        "view",
        "release",
        "repository",
    ],
)
def test_binding_refuses_cross_corpus_query_inputs(inputs, mutation):
    root, selection, suite, pack = inputs
    if mutation.endswith("commit"):
        (suite if mutation.startswith("suite") else pack)["repository_commit"] = "a" * 40
    elif mutation.endswith("files"):
        (suite if mutation.startswith("suite") else pack)["file_universe"] = []
    elif mutation.endswith("hash"):
        (suite if mutation.startswith("suite") else pack)["file_universe_digest"] = "b" * 64
    elif mutation == "pack_binding":
        pack["suite_commitment_sha256"] = "c" * 64
    elif mutation == "gold":
        suite["tasks"][0]["gold"] = [{"path": "README.md"}]
        pack["suite_commitment_sha256"] = hashlib.sha256(canonical(suite)).hexdigest()
    elif mutation == "view":
        selection["view"] = "developer_search"
    elif mutation == "release":
        selection["release_digest"] = "sha256:" + "d" * 64
    else:
        selection["repository"] = "absent"
    with pytest.raises(EvidenceError):
        binding.capture(
            root, selection, canonical(suite), canonical(pack), root.with_suffix(".zip")
        )


@pytest.mark.parametrize(
    "mutation",
    [
        "extra",
        "missing",
        "duplicate",
        "reordered",
        "traversal",
        "symlink",
        "compression",
        "metadata",
        "bundle",
    ],
)
def test_capsule_refuses_corrupt_or_forged_custody(inputs, mutation):
    root, selection, suite, pack = inputs
    _, capsule = binding.capture(
        root, selection, canonical(suite), canonical(pack), root.with_suffix(".zip")
    )
    with zipfile.ZipFile(capsule.path) as archive:
        entries = [(name, archive.read(name)) for name in archive.namelist()]
    if mutation == "extra":
        entries.append(("unexpected", b"foreign"))
    elif mutation == "missing":
        entries.pop()
    elif mutation == "duplicate":
        entries.append(entries[-1])
    elif mutation == "reordered":
        entries.reverse()
    elif mutation == "traversal":
        entries[0] = ("../escape", entries[0][1])
    elif mutation == "metadata":
        document = json.loads(entries[-1][1])
        document["policy"]["max_file_bytes"] += 1
        entries[-1] = (entries[-1][0], canonical(document))
    elif mutation == "bundle":
        entries[0] = (entries[0][0], b"not a Git bundle")
    out = io.BytesIO()
    with zipfile.ZipFile(out, "w") as archive:
        for name, raw in entries:
            info = zipfile.ZipInfo(name)
            info.external_attr = (0o120777 if mutation == "symlink" else 0o100644) << 16
            info.compress_type = (
                zipfile.ZIP_DEFLATED if mutation == "compression" else zipfile.ZIP_STORED
            )
            archive.writestr(info, raw)
    with pytest.raises(EvidenceError):
        binding.replay(
            write_raw_file(root.with_suffix(".bad.zip"), [out.getvalue()]),
            selection,
            canonical(suite),
            canonical(pack),
        )


def test_capsule_resource_limit_is_explicit(inputs, monkeypatch):
    root, selection, suite, pack = inputs
    monkeypatch.setattr(binding, "MAX_CAPSULE_BYTES", 1)
    with pytest.raises(EvidenceError, match="limit"):
        binding.capture(
            root, selection, canonical(suite), canonical(pack), root.with_suffix(".zip")
        )


@pytest.mark.parametrize("mutation", ["legacy", "partial", "extra", "relative", "bad_digest"])
def test_common_spec_requires_explicit_release_and_closed_roles(inputs, mutation):
    root, selection, _, _ = inputs
    value = {
        "schema_version": 2,
        "corpus": selection,
        "inputs": {"suite": str(root / "suite"), "query_pack": str(root / "pack")},
    }
    if mutation == "legacy":
        value["schema_version"] = 1
    elif mutation == "partial":
        del value["inputs"]["suite"]
    elif mutation == "extra":
        value["corpus"]["index_universe_attested"] = True
    elif mutation == "relative":
        value["corpus"]["release_path"] = "relative"
    else:
        value["corpus"]["release_digest"] = "sha256:" + "g" * 64
    with pytest.raises(EvidenceError):
        binding.read_spec(canonical(value), ("suite", "query_pack"))


@pytest.mark.parametrize(
    "raw", [b"\xff", b"{}", b"{", b"null", b'{"schema_version":2,"schema_version":2}']
)
def test_common_spec_rejects_invalid_json_bytes(raw):
    with pytest.raises(EvidenceError):
        binding.read_spec(raw, ("suite", "query_pack"))


def test_gold_capture_checks_staged_bytes_without_rederiving_source(tmp_path, monkeypatch):
    identity = {
        "producer_source_digests": binding._gold_producer_source_digests(),
    }
    material = {
        "selection.json": b"{}\n",
        "recipe.json": b"{}\n",
        "gold.json": b"{}\n",
        "blind.json": b"{}\n",
        "identity.json": canonical(identity) + b"\n",
    }
    calls = 0

    def derive_once(*_args):
        nonlocal calls
        calls += 1
        return material

    monkeypatch.setattr(binding, "_gold_material", derive_once)
    root = tmp_path / "release"
    root.mkdir()
    target = tmp_path / "gold"
    assert binding.capture_gold(root, {}, b"{}", target) == identity
    assert calls == 1
    assert set(binding.corpus.regular_tree(target)) == set(material)
    (target / "gold.json").write_bytes(b'{"forged":true}\n')
    with pytest.raises(EvidenceError, match="differs from source-derived oracle: gold.json"):
        binding._verify_gold_material(target, material)


def test_gold_capture_refuses_source_drift_before_publication(tmp_path, monkeypatch):
    material = {
        "selection.json": b"{}\n",
        "recipe.json": b"{}\n",
        "gold.json": b"{}\n",
        "blind.json": b"{}\n",
        "identity.json": canonical({"producer_source_digests": {"source_oracle": "stale"}}) + b"\n",
    }
    monkeypatch.setattr(binding, "_gold_material", lambda *_args: material)
    root = tmp_path / "release"
    root.mkdir()
    target = tmp_path / "gold"
    with pytest.raises(EvidenceError, match="source changed before publication"):
        binding.capture_gold(root, {}, b"{}", target)
    assert not target.exists()


# Repository-disjoint split manifests. Source bodies are fixed text; leakage
# expectations come from how each fixture was written, not from the validator.
RUST_BODY = "\n".join(
    f"pub fn handler_{index}(input: &str, limit: usize) -> Option<String> {{\n"
    f"    let trimmed = input.trim_start_matches('{chr(97 + index % 26)}');\n"
    f"    if trimmed.len() > limit + {index} {{ return None; }}\n"
    f"    Some(trimmed.repeat({index % 7 + 1}))\n}}"
    for index in range(12)
)
PYTHON_BODY = "\n".join(
    f"def worker_{index}(items, scale={index}):\n"
    f"    total = sum(item * scale for item in items if item % {index + 2})\n"
    f"    return total or {index * 3}\n"
    for index in range(12)
)
SPLIT_REPOSITORIES = {
    "alpha": {"src/lib.rs": RUST_BODY},
    "beta": {"app/core.py": PYTHON_BODY},
    "epsilon": {"pkg/run.py": PYTHON_BODY.replace("worker_", "runner_").replace("total", "acc")},
    # An exact copied development file inside a holdout repository.
    "gamma": {"vendored_copy/lib.rs": RUST_BODY, "app/core.py": PYTHON_BODY},
    # The same tokens re-laid out plus one new function: a near duplicate.
    "delta": {
        "src/lib.rs": RUST_BODY.replace("\n    ", "\n\t").replace(" {", "\n{")
        + "\npub fn extra(value: u8) -> u8 { value.wrapping_mul(3) }\n"
    },
}


@pytest.fixture(scope="session")
def split_releases(tmp_path_factory):
    import corpus_release as corpus

    base = tmp_path_factory.mktemp("split-seed")
    checkouts = base / "checkouts"
    revisions = {}
    for name, files in SPLIT_REPOSITORIES.items():
        root = checkouts / name
        root.mkdir(parents=True)
        for command in (
            ["init", "-q"],
            ["config", "user.name", "Fixture"],
            ["config", "user.email", "fixture@localhost"],
        ):
            subprocess_git(root, *command)
        for path, text in {**files, "LICENSE": f"{name} fixture license\n"}.items():
            (root / path).parent.mkdir(parents=True, exist_ok=True)
            (root / path).write_text(text)
        subprocess_git(root, "add", ".")
        subprocess_git(root, "commit", "-qm", name)
        revisions[name] = subprocess_git(root, "rev-parse", "HEAD").decode().strip()

    def build(label, names, urls=None):
        spec = {
            "source_revision": "split fixture",
            "repositories": [
                {
                    "name": name,
                    "language": "rust" if name in ("alpha", "delta") else "python",
                    "url": (urls or {}).get(name, f"https://example.invalid/{name}.git"),
                    "revision": revisions[name],
                    "benchmark_root": "",
                    "upstream_semble_benchmark_overlap": False,
                }
                for name in names
            ],
        }
        spec_path = base / f"{label}.json"
        spec_path.write_text(json.dumps(spec))
        target = base / "releases" / label
        document = corpus.create(spec_path, checkouts, target)
        return target, document

    return {
        "disjoint": build("disjoint", ["alpha", "beta", "epsilon"]),
        "copy": build("copy", ["alpha", "gamma"]),
        "near": build("near", ["alpha", "delta"]),
        "dev_only": build("dev-only", ["alpha"]),
        "hold_only": build("hold-only", ["beta"]),
        "same_url": build("same-url", ["beta"], {"beta": "https://EXAMPLE.invalid/alpha.git/"}),
    }


def subprocess_git(root, *args):
    import subprocess

    return subprocess.check_output(["git", "-C", str(root), *args])


def split_manifest(assignments):
    """assignments: [(release root, document, repository, split, families)]."""
    rows = []
    for _root, document, repository, split, families in assignments:
        row = next(r for r in document["repositories"] if r["recipe"]["name"] == repository)
        rows.append(
            {
                "release_digest": document["digest"],
                "repository": repository,
                "repository_commit": row["recipe"]["revision"],
                "code_only_universe_digest": row["views"]["code_only"]["file_universe_digest"],
                "split": split,
                "query_family_ids": sorted(families),
            }
        )
    rows.sort(key=lambda r: (r["release_digest"], r["repository"]))
    manifest = {
        "schema_version": 1,
        "kind": "repository_disjoint_split_manifest",
        "leakage_policy": binding.SPLIT_LEAKAGE_POLICY,
        "repositories": rows,
    }
    releases = {document["digest"]: root for root, document, *_ in assignments}
    return manifest, releases


def disjoint_assignments(split_releases):
    root, document = split_releases["disjoint"]
    return [
        (root, document, "alpha", "development", ["dev-alpha-1", "dev-alpha-2"]),
        (root, document, "beta", "holdout", ["hold-beta-1"]),
        (root, document, "epsilon", "holdout", ["hold-epsilon-1"]),
    ]


def test_split_manifest_accepts_disjoint_repositories_in_one_or_two_releases(split_releases):
    manifest, releases = split_manifest(disjoint_assignments(split_releases))
    assert binding.validate_split_manifest(canonical(manifest), releases) == manifest
    dev_root, dev_doc = split_releases["dev_only"]
    hold_root, hold_doc = split_releases["hold_only"]
    manifest, releases = split_manifest(
        [
            (dev_root, dev_doc, "alpha", "development", ["dev-alpha-1"]),
            (hold_root, hold_doc, "beta", "holdout", ["hold-beta-1"]),
        ]
    )
    assert len(releases) == 2
    assert binding.validate_split_manifest(canonical(manifest), releases) == manifest


def test_leakage_fingerprints_ignore_layout_but_not_tokens():
    assert binding._fingerprints(b"fn a() { b(1, 2); c(3) }\n" * 4) == binding._fingerprints(
        b"fn a()\n{\n\tb(1,2);\n  c(3)\n}" * 4
    )
    assert binding._fingerprints(b"fn a() { b(1, 2); c(3) }") != binding._fingerprints(
        b"fn z() { y(9, 8); x(7) }"
    )


@pytest.mark.parametrize(
    "mutation,error",
    [
        ("swapped_commit", "commit or source differs"),
        ("swapped_repository", "commit or source differs"),
        ("repeated_family", "repeats a query family"),
        ("missing_repository", "omits or invents"),
        ("stale_release", "stale or different release"),
        ("one_side_only", "development and holdout"),
        ("policy", "identity, policy"),
        ("unsorted_families", "entry is malformed"),
        ("extra_release_path", "release paths differ"),
    ],
)
def test_split_manifest_refuses_forged_assignment(split_releases, mutation, error):
    manifest, releases = split_manifest(disjoint_assignments(split_releases))
    rows = {row["repository"]: row for row in manifest["repositories"]}
    if mutation == "swapped_commit":
        rows["alpha"]["repository_commit"], rows["beta"]["repository_commit"] = (
            rows["beta"]["repository_commit"],
            rows["alpha"]["repository_commit"],
        )
    elif mutation == "swapped_repository":
        rows["alpha"]["code_only_universe_digest"], rows["beta"]["code_only_universe_digest"] = (
            rows["beta"]["code_only_universe_digest"],
            rows["alpha"]["code_only_universe_digest"],
        )
    elif mutation == "repeated_family":
        rows["beta"]["query_family_ids"] = ["dev-alpha-1"]
    elif mutation == "missing_repository":
        manifest["repositories"].remove(rows["epsilon"])
    elif mutation == "stale_release":
        stale = "sha256:" + "0" * 64
        for row in manifest["repositories"]:
            row["release_digest"] = stale
        releases = {stale: next(iter(releases.values()))}
    elif mutation == "one_side_only":
        rows["alpha"]["split"] = "holdout"
    elif mutation == "policy":
        manifest["leakage_policy"] = {
            **manifest["leakage_policy"],
            "exact_file_min_bytes": 1 << 30,
        }
    elif mutation == "unsorted_families":
        rows["alpha"]["query_family_ids"].reverse()
    else:
        releases = {**releases, "sha256:" + "1" * 64: Path("/nonexistent")}
    with pytest.raises(EvidenceError, match=error):
        binding.validate_split_manifest(canonical(manifest), releases)


@pytest.mark.parametrize(
    "release,holdout,error",
    [
        ("copy", "gamma", "identical source alpha:src/lib.rs / gamma:vendored_copy/lib.rs"),
        ("near", "delta", "near-duplicate source alpha:src/lib.rs / delta:src/lib.rs"),
    ],
)
def test_split_manifest_refuses_copied_or_near_duplicate_source(
    split_releases, release, holdout, error
):
    root, document = split_releases[release]
    manifest, releases = split_manifest(
        [
            (root, document, "alpha", "development", ["dev-alpha-1"]),
            (root, document, holdout, "holdout", ["hold-1"]),
        ]
    )
    with pytest.raises(EvidenceError, match=error):
        binding.validate_split_manifest(canonical(manifest), releases)


def test_split_manifest_refuses_one_upstream_on_both_sides(split_releases):
    dev_root, dev_doc = split_releases["dev_only"]
    hold_root, hold_doc = split_releases["same_url"]
    manifest, releases = split_manifest(
        [
            (dev_root, dev_doc, "alpha", "development", ["dev-alpha-1"]),
            (hold_root, hold_doc, "beta", "holdout", ["hold-beta-1"]),
        ]
    )
    with pytest.raises(EvidenceError, match="repeats one upstream"):
        binding.validate_split_manifest(canonical(manifest), releases)
