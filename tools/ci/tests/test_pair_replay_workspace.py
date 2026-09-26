"""Pair replay shares restored input bytes, never per-run verdicts."""

from __future__ import annotations

import subprocess

import pytest

from tools.ci.tests import test_pair_capture as pair_tests

bridge = pair_tests.bridge


@pytest.fixture
def raw_archives(tmp_path):
    stage = pair_tests.fixture(tmp_path)
    raw = tmp_path / "raw"
    raw.mkdir()
    subprocess.run(
        ["git", "-C", str(stage["repo"]), "bundle", "create", str(raw / "corpus.bundle"), "HEAD"],
        check=True,
        capture_output=True,
    )
    (raw / "native-tree.zip").write_bytes(bridge.pack_native(stage["stage"]))
    return raw


def test_repeated_restoration_is_local_and_rechecks_actual_archive_bytes(raw_archives, monkeypatch):
    calls = []
    original = bridge.restore_corpus

    def counted(*args, **kwargs):
        calls.append(args[0])
        return original(*args, **kwargs)

    monkeypatch.setattr(bridge, "restore_corpus", counted)
    with bridge._ReplayWorkspace() as workspace:
        first = workspace.restore(raw_archives)
        assert workspace.restore(raw_archives) == first
        assert len(calls) == 1
        archive = raw_archives / "native-tree.zip"
        original_bytes = archive.read_bytes()
        archive.write_bytes(original_bytes[:-1] + bytes([original_bytes[-1] ^ 1]))
        with pytest.raises(bridge.EvidenceError, match="archives differ"):
            workspace.restore(raw_archives)
        archive.write_bytes(original_bytes)
        assert workspace.restore(raw_archives) == first
    assert not first[0].exists()
    with bridge._ReplayWorkspace() as second:
        second.restore(raw_archives)
        assert len(calls) == 2


@pytest.mark.parametrize("mutation", ["corpus", "native", "git", "mode", "link", "extra"])
def test_restored_workspace_mutation_cannot_leak_to_next_case(raw_archives, mutation):
    with bridge._ReplayWorkspace() as workspace:
        corpus, native = workspace.restore(raw_archives)
        if mutation == "corpus":
            (corpus / "a.txt").write_bytes(b"changed corpus")
        elif mutation == "native":
            (native / "verdict.json").write_bytes(b"{}")
        elif mutation == "git":
            (corpus / ".git" / "HEAD").write_bytes(b"changed Git authority")
        elif mutation == "mode":
            path = corpus / "a.txt"
            path.chmod(path.stat().st_mode ^ 0o111)
        elif mutation == "link":
            (native / "verdict.json").unlink()
            (native / "verdict.json").symlink_to(corpus / "a.txt")
        else:
            (corpus / "extra").write_bytes(b"untracked")
        with pytest.raises(bridge.EvidenceError, match="workspace changed"):
            workspace.verify_unchanged()
        with pytest.raises(bridge.EvidenceError, match="workspace changed"):
            workspace.restore(raw_archives)


def test_changed_corpus_bundle_is_not_hidden_by_identical_native_archive(raw_archives):
    with bridge._ReplayWorkspace() as workspace:
        workspace.restore(raw_archives)
        (raw_archives / "corpus.bundle").write_bytes(b"different corpus")
        with pytest.raises(bridge.EvidenceError, match="archives differ"):
            workspace.restore(raw_archives)

