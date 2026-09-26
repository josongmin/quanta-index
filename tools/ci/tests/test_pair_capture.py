"""Paired common bridge: real native oracle, declared fixture producer only."""

from __future__ import annotations

import json
import os
import shutil
import subprocess
import sys
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[3] / "tools/benchmark"))
import pair_capture as bridge
from evidence import RunStore, sample_evidence
from registry import load_registry

from tools.ci.tests.test_retrieval_benchmark import _pair_stage, _stage_verdict


def fixture(tmp_path):
    stage = _pair_stage(tmp_path)
    (stage["stage"] / "verdict.json").write_text(json.dumps(_stage_verdict(stage)))
    return stage


def test_native_owner_replay_and_separate_metric_spaces(tmp_path):
    stage = fixture(tmp_path)
    manifest, verdict = bridge.derive(stage["stage"], stage["repo"])
    assert verdict["states"]["PAIR_VALID"] == "pass"
    payloads = bridge.typed_payloads(stage["stage"], manifest)
    assert len(payloads) == 6
    assert {payload["metric_space"] for payload in payloads.values()} == {"file", "context", "span"}
    assert all(payload["universe_attested"] is False for payload in payloads.values())
    for payload in payloads.values():
        assert [row["query_id"] for row in payload["rows"]] == ["T1", "T2"]
        assert all(row["value"] is None for row in payload["rows"] if row["state"] == "unsupported")


def test_bundle_replay_restores_real_commit_and_executable_mode(tmp_path):
    stage = fixture(tmp_path)
    source = stage["repo"]
    (source / "a.txt").chmod(0o755)
    subprocess.run(["git", "-C", str(source), "add", "a.txt"], check=True)
    subprocess.run(["git", "-C", str(source), "commit", "-qm", "executable fixture"], check=True)
    commit = (
        subprocess.check_output(["git", "-C", str(source), "rev-parse", "HEAD"]).decode().strip()
    )
    clone = tmp_path / "clone"
    bridge.clone_corpus(source, clone, commit, 60)
    bundle = tmp_path / "corpus.bundle"
    bridge.execute(
        ["git", "-C", str(clone), "bundle", "create", str(bundle), "HEAD"],
        cwd=bridge.ROOT,
        env={**os.environ, **bridge.GIT_ENV},
        timeout=60,
    )
    (source / "a.txt").write_bytes(b"mutated original corpus")
    restored = tmp_path / "restored"
    bridge.restore_corpus(bundle, restored, 60)
    assert bridge.evaluator.verify_repo(restored, commit) == restored.resolve()
    assert (restored / "a.txt").stat().st_mode & 0o111
    assert (restored / "a.txt").read_bytes() != (source / "a.txt").read_bytes()


def test_native_report_tamper_refuses_before_typed_scoring(tmp_path):
    stage = fixture(tmp_path)
    ref = stage["manifest"]["artifacts"]["reports"][0]
    path = stage["stage"] / ref
    report = json.loads(path.read_text())
    report["per_query"][0]["file_recall_at_10"] = 0.5
    path.write_text(json.dumps(report))
    with pytest.raises(ValueError, match="verdict differs|PAIR_VALID"):
        bridge.derive(stage["stage"], stage["repo"])


def test_native_raw_tree_refuses_links_and_special_files(tmp_path):
    root = tmp_path / "native"
    root.mkdir()
    (root / "record").write_bytes(b"raw")
    (root / "alias").symlink_to(root / "record")
    with pytest.raises(ValueError):
        bridge.tree_bytes(root)
    (root / "alias").unlink()
    os.mkfifo(root / "fifo")
    with pytest.raises(ValueError):
        bridge.tree_bytes(root)


@pytest.mark.parametrize(
    "changes", [{"scorer": "none"}, {"validator": "retrieval-proof"}, {"native_schema": "wrong"}]
)
def test_pair_registration_drift_refuses(changes):
    registry = load_registry()
    registry["families"][bridge.FAMILY].update(changes)
    with pytest.raises(ValueError, match="registration differs"):
        bridge.require_registration(registry)


def test_capture_complete_profile_and_replay_with_original_corpus_changed(tmp_path, monkeypatch):
    import benchctl

    stage = fixture(tmp_path)
    output = tmp_path / "pair-output"
    searchd = tmp_path / "searchd"
    searchd.write_bytes(b"fixture executable identity; never launched")
    spec = {
        **stage["spec"],
        "output_root": str(output),
        "searchd_binary": str(searchd),
        "searchd_expected_sha256": bridge.owner.sha_file(searchd),
        "top_k": 10,
        "strategies": [{"name": "whole_file"}],
        "routes": ["lexical"],
        "semble_python": sys.executable,
        "semble_lockfile_sha256": bridge.owner.sha_file(Path(stage["spec"]["semble_lockfile"])),
    }
    spec_path = tmp_path / "spec.json"
    spec_path.write_text(json.dumps(spec))
    source = sample_evidence()["source"]
    source["revision"] = stage["manifest"]["provenance"]["quanta"]["source_sha"]
    source["dirty"] = False
    monkeypatch.setattr(benchctl, "require_clean_worktree", lambda _: None)
    monkeypatch.setattr(benchctl, "resolve_checkout_head", lambda _: source["revision"])
    monkeypatch.setattr(benchctl, "require_frozen_source", lambda *_: None)
    monkeypatch.setattr(bridge, "source_identity", lambda *_: source)
    real_execute = bridge.execute

    def fixture_producer(argv, **kwargs):
        if argv[1:4] == ["-m", bridge.MODULE, "pair"]:
            shutil.copytree(stage["stage"], output)
            command = {**sample_evidence()["command"], "argv": argv, "cwd": str(bridge.ROOT)}
            return b"declared fixture producer", b"", command
        return real_execute(argv, **kwargs)

    monkeypatch.setattr(bridge, "execute", fixture_producer)
    root = tmp_path / "evidence"
    document = bridge.capture(bridge.ROOT, root, load_registry(), spec_path, 60)
    assert len(document["runs"]) == 6
    (stage["repo"] / "a.txt").write_bytes(b"original source no longer authoritative")
    shutil.rmtree(output)
    shutil.rmtree(root / "work")
    assert bridge.validate(bridge.ROOT, root, load_registry()) == document
    store = RunStore(root)
    evidence = store.load(document["runs"][0]["run_id"])
    evidence["payload"]["rows"][0]["value"] = 0.12345
    with pytest.raises(ValueError, match="typed metrics"):
        bridge.replay_run(store, evidence)
