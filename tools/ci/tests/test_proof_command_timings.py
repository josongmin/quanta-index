"""Diagnostic timings cannot substitute for portable execution evidence."""

import json
import sys

import pytest

from tools.benchmark.retrieval import portable_proof as proof


def test_command_timings_separate_execution_and_custody(monkeypatch, tmp_path):
    ticks = iter([100, 120, 170, 200, 210])
    monkeypatch.setattr(proof.time, "monotonic_ns", lambda: next(ticks))
    monkeypatch.setattr(proof, "execute", lambda *a, **kw: (b"out", b"err", {}))
    commands = []
    assert proof._run("example", [sys.executable], tmp_path, commands) == b"out"
    timing = json.loads((tmp_path / "example.timing.json").read_bytes())
    assert timing == {
        "schema_version": 1, "kind": "proof_command_timing_diagnostic",
        "command": "example", "prepare_ns": 20, "execute_ns": 50,
        "verify_ns": 30, "record_ns": 10, "total_ns": 110,
        "excludes": "timing-file write and work outside this command",
    }
    assert not any("timing" in key or key.endswith("_ns") for key in commands[0])
    assert (tmp_path / "example.stdout").read_bytes() == b"out"
    assert (tmp_path / "example.stderr").read_bytes() == b"err"


def test_failed_command_never_publishes_success_or_success_timing(monkeypatch, tmp_path):
    def fail(*args, **kwargs):
        raise ValueError("producer failed")

    monkeypatch.setattr(proof, "execute", fail)
    commands = []
    with pytest.raises(ValueError, match="producer failed"):
        proof._run("example", [sys.executable], tmp_path, commands)
    assert commands == []
    assert not list(tmp_path.iterdir())
