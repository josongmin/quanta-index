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
from evidence import EvidenceError

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
    pack = {**{key: suite[key] for key in ("repository_commit", "file_universe",
                                         "file_universe_digest")},
            "suite_commitment_sha256": hashlib.sha256(canonical(suite)).hexdigest(),
            "tasks": [{"task_id": "one", "query": "main"}]}
    selection = {"release_path": str(root), "release_digest": document["digest"],
                 "repository": "fixture", "view": "code_only"}
    return root, selection, suite, pack


def test_mutated_consumer_copy_cannot_poison_next_case(inputs, release_seed, tmp_path):  # noqa: F811
    root, selection, suite, pack = inputs
    relative = "views/fixture/code_only/src/main.rs"
    pristine = (release_seed / relative).read_bytes()
    view = root / relative
    view.chmod(0o644)
    view.write_bytes(b"corrupted independent test input")
    with pytest.raises(EvidenceError):
        binding.capture(root, selection, canonical(suite), canonical(pack))
    assert (release_seed / relative).read_bytes() == pristine
    fresh = tmp_path / "next-consumer"
    shutil.copytree(release_seed, fresh, symlinks=True)
    selection = {**selection, "release_path": str(fresh)}
    result, capsule = binding.capture(fresh, selection, canonical(suite), canonical(pack))
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
    result, capsule = binding.capture(root, selection, canonical(suite), canonical(pack))
    assert result["index_universe_attested"] is False
    assert result["file_universe_digest"] == "sha256:" + suite["file_universe_digest"]
    shutil.rmtree(source[0])
    root.rename(root.with_name("original-unavailable"))
    assert not source[0].exists() and not root.exists()
    assert binding.replay(capsule, selection, canonical(suite), canonical(pack)) == result
    with zipfile.ZipFile(io.BytesIO(capsule)) as archive:
        assert archive.namelist() == ["bundles/fixture.bundle", "recipe.json", "release.json"]


@pytest.mark.parametrize("mutation", ["suite_commit", "pack_commit", "suite_files",
                                     "pack_files", "suite_hash", "pack_hash", "pack_binding",
                                     "gold", "view", "release", "repository"])
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
        binding.capture(root, selection, canonical(suite), canonical(pack))


@pytest.mark.parametrize("mutation", ["extra", "missing", "duplicate", "reordered",
                                     "traversal", "symlink", "compression", "metadata", "bundle"])
def test_capsule_refuses_corrupt_or_forged_custody(inputs, mutation):
    root, selection, suite, pack = inputs
    _, capsule = binding.capture(root, selection, canonical(suite), canonical(pack))
    with zipfile.ZipFile(io.BytesIO(capsule)) as archive:
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
            info.compress_type = zipfile.ZIP_DEFLATED if mutation == "compression" else zipfile.ZIP_STORED
            archive.writestr(info, raw)
    with pytest.raises(EvidenceError):
        binding.replay(out.getvalue(), selection, canonical(suite), canonical(pack))


def test_capsule_resource_limit_is_explicit(inputs, monkeypatch):
    root, selection, suite, pack = inputs
    monkeypatch.setattr(binding, "MAX_CAPSULE_BYTES", 1)
    with pytest.raises(EvidenceError, match="limit"):
        binding.capture(root, selection, canonical(suite), canonical(pack))


@pytest.mark.parametrize("mutation", ["legacy", "partial", "extra", "relative", "bad_digest"])
def test_common_spec_requires_explicit_release_and_closed_roles(inputs, mutation):
    root, selection, _, _ = inputs
    value = {"schema_version": 2, "corpus": selection,
             "inputs": {"suite": str(root / "suite"), "query_pack": str(root / "pack")}}
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


@pytest.mark.parametrize("raw", [b"\xff", b"{}", b"{", b"null",
                                 b'{"schema_version":2,"schema_version":2}'])
def test_common_spec_rejects_invalid_json_bytes(raw):
    with pytest.raises(EvidenceError):
        binding.read_spec(raw, ("suite", "query_pack"))
