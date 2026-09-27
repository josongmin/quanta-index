"""Pair replay shares restored input bytes, never per-run verdicts."""

from __future__ import annotations

import shutil
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
    bridge.pack_native(stage["stage"], raw / "native-tree.zip")
    return raw


def test_repeated_restoration_shares_only_corpus_and_rechecks_archives(
    raw_archives, monkeypatch, tmp_path
):
    calls = []
    original = bridge.restore_corpus

    def counted(*args, **kwargs):
        calls.append(args[0])
        return original(*args, **kwargs)

    monkeypatch.setattr(bridge, "restore_corpus", counted)
    second_raw = tmp_path / "second-raw"
    shutil.copytree(raw_archives, second_raw)
    changed_native = tmp_path / "changed-native"
    bridge.unpack_native(bridge.RawFile.capture(second_raw / "native-tree.zip"), changed_native)
    (changed_native / "case-specific.txt").write_text("second run", encoding="utf-8")
    (second_raw / "native-tree.zip").unlink()
    bridge.pack_native(changed_native, second_raw / "native-tree.zip")
    with bridge._ReplayWorkspace() as workspace:
        first = workspace.restore(raw_archives)
        assert workspace.restore(raw_archives) == first
        assert len(calls) == 1
        assert workspace.restore(second_raw) == first
        assert (first[1] / "case-specific.txt").read_text() == "second run"
        assert len(calls) == 1
        assert workspace.restore(raw_archives) == first
        assert not (first[1] / "case-specific.txt").exists()
        (second_raw / "corpus.bundle").write_bytes(b"different corpus")
        with pytest.raises(bridge.EvidenceError, match="corpus archives differ"):
            workspace.restore(second_raw)
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
        with pytest.raises(bridge.EvidenceError, match="corpus archives differ"):
            workspace.restore(raw_archives)


def test_replay_discards_staging_bundle_but_rechecks_raw_archive(raw_archives, monkeypatch):
    raw_bundle = raw_archives / "corpus.bundle"
    expected_head = subprocess.run(
        ["git", "bundle", "list-heads", str(raw_bundle), "HEAD"],
        check=True,
        capture_output=True,
        text=True,
    ).stdout.split()[0]
    captured = []
    original_capture = bridge.RawFile.capture

    def capture(cls, path):
        captured.append(path)
        return original_capture(path)

    monkeypatch.setattr(bridge.RawFile, "capture", classmethod(capture))
    with bridge._ReplayWorkspace() as workspace:
        corpus, native = workspace.restore(raw_archives)
        assert not (workspace.root / "corpus.bundle").exists()
        assert "corpus.bundle" not in workspace.identity
        assert not (corpus / ".git" / "objects" / "info" / "alternates").exists()
        actual_head = subprocess.run(
            ["git", "-C", str(corpus), "rev-parse", "HEAD"],
            check=True,
            capture_output=True,
            text=True,
        ).stdout.strip()
        assert actual_head == expected_head
        # The restored object database remains usable without the staging input.
        subprocess.run(
            ["git", "-C", str(corpus), "fsck", "--full"],
            check=True,
            capture_output=True,
        )
        assert workspace.restore(raw_archives) == (corpus, native)
        assert captured.count(raw_bundle) == 2
        original_bytes = raw_bundle.read_bytes()
        raw_bundle.write_bytes(original_bytes[:-1] + bytes([original_bytes[-1] ^ 1]))
        with pytest.raises(bridge.EvidenceError, match="corpus archives differ"):
            workspace.restore(raw_archives)
        assert captured.count(raw_bundle) == 3
