"""Frontdoor history policy flags and refusal before creating an output root."""

from __future__ import annotations

import argparse
import hashlib
from pathlib import Path
from types import SimpleNamespace

import pytest

from tools.benchmark.retrieval.causal_cost_capture import _history_inputs, _scale_command, capture
from tools.benchmark.retrieval import causal_cost_capture


@pytest.fixture
def capture_fixture(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> SimpleNamespace:
    source = tmp_path / "source"
    source.mkdir()
    binary = tmp_path / "scale_matrix"
    binary.write_bytes(b"fixed executable fixture\n")
    binary.chmod(0o700)
    state = {"head": "a" * 40, "dirty": "", "replays": 0}
    args = argparse.Namespace(
        cwd=source,
        binary=binary,
        out_root=tmp_path / "capture",
        source_revision=state["head"],
        binary_sha256=hashlib.sha256(binary.read_bytes()).hexdigest(),
        tier="small",
        seed=7,
        client_timeout_ms=None,
        history_max_bytes=None,
        history_max_total_bytes=None,
        max_seconds=10,
    )

    def git(_cwd: Path, *command: str) -> str:
        return state["head"] if command == ("rev-parse", "HEAD") else state["dirty"]

    def run(_command: list[str], **kwargs: object) -> SimpleNamespace:
        artifact = args.out_root / "artifact"
        artifact.mkdir()
        (artifact / "summary.json").write_bytes(b'{"fixture":"summary"}\n')
        (artifact / "tier_manifest.json").write_bytes(b'{"fixture":"manifest"}\n')
        kwargs["stdout"].write(b"fixed stdout\n")
        kwargs["stderr"].write(b"fixed trace\n")
        return SimpleNamespace(returncode=0)

    def replay(summary: bytes, trace: bytes, executable: bytes, manifest: bytes, **_: object) -> dict:
        state["replays"] += 1
        assert summary == b'{"fixture":"summary"}\n'
        assert trace == b"fixed trace\n"
        assert executable == b"fixed executable fixture\n"
        assert manifest == b'{"fixture":"manifest"}\n'
        return {
            "runtime_config": {
                "history_policy_id": "scale-supported-v1",
                "requested_history_max_bytes": None,
                "history_max_bytes": 1_073_741_824,
                "requested_history_max_total_bytes": None,
                "history_max_total_bytes": 2_147_483_648,
            },
            "scope": {},
        }

    monkeypatch.setattr(causal_cost_capture, "_git", git)
    monkeypatch.setattr(causal_cost_capture.subprocess, "run", run)
    monkeypatch.setattr(causal_cost_capture, "replay", replay)
    return SimpleNamespace(args=args, state=state, replay=replay)


def test_capture_accepts_one_unchanged_executable_and_input_epoch(capture_fixture: SimpleNamespace):
    result = capture(capture_fixture.args)
    assert result["status"] == "VERIFIED_DIAGNOSTIC"
    assert result["binary_sha256"] == result["binary_sha256_after"]
    assert capture_fixture.state["replays"] == 1
    assert (capture_fixture.args.out_root / "causal-profile.json").is_file()


def test_capture_refuses_binary_mutation_after_post_execution_epoch(
    capture_fixture: SimpleNamespace, monkeypatch: pytest.MonkeyPatch
) -> None:
    original = causal_cost_capture.capture_executable
    calls = 0

    def capture_then_mutate(path: Path) -> dict:
        nonlocal calls
        epoch = original(path)
        calls += 1
        if calls == 3:
            path.write_bytes(b"changed executable fixture\n")
        return epoch

    monkeypatch.setattr(causal_cost_capture, "capture_executable", capture_then_mutate)
    result = capture(capture_fixture.args)
    assert result["status"] == "FAILED"
    assert capture_fixture.state["replays"] == 0
    assert not (capture_fixture.args.out_root / "causal-profile.json").exists()


@pytest.mark.parametrize(
    "mutation", ["binary", "same-bytes-replacement", "summary", "manifest", "trace", "head", "dirty"]
)
def test_capture_refuses_input_drift_during_replay(
    mutation: str, capture_fixture: SimpleNamespace, monkeypatch: pytest.MonkeyPatch
) -> None:
    args = capture_fixture.args

    def replay_then_mutate(*values: bytes, **kwargs: object) -> dict:
        profile = capture_fixture.replay(*values, **kwargs)
        if mutation == "binary":
            args.binary.write_bytes(b"changed executable\n")
        elif mutation == "same-bytes-replacement":
            replacement = args.binary.with_suffix(".replacement")
            replacement.write_bytes(args.binary.read_bytes())
            replacement.chmod(0o700)
            replacement.replace(args.binary)
        elif mutation in {"summary", "manifest"}:
            name = "summary.json" if mutation == "summary" else "tier_manifest.json"
            (args.out_root / "artifact" / name).write_bytes(b"changed artifact\n")
        elif mutation == "trace":
            (args.out_root / "stderr").write_bytes(b"changed trace\n")
        elif mutation == "head":
            capture_fixture.state["head"] = "b" * 40
        else:
            capture_fixture.state["dirty"] = " M source.rs"
        return profile

    monkeypatch.setattr(causal_cost_capture, "replay", replay_then_mutate)
    result = capture(args)
    assert result["status"] == "FAILED"
    assert not (args.out_root / "causal-profile.json").exists()
    assert (args.out_root / "execution.json").is_file()


def test_capture_removes_own_profile_if_epoch_changes_during_publication(
    capture_fixture: SimpleNamespace, monkeypatch: pytest.MonkeyPatch
) -> None:
    original = causal_cost_capture.capture_executable

    def mutate_published_epoch(path: Path) -> dict:
        if (capture_fixture.args.out_root / "causal-profile.json").exists():
            path.write_bytes(b"changed executable after publication\n")
        return original(path)

    monkeypatch.setattr(causal_cost_capture, "capture_executable", mutate_published_epoch)
    result = capture(capture_fixture.args)
    assert result["status"] == "FAILED"
    assert "profile_sha256" not in result
    assert not (capture_fixture.args.out_root / "causal-profile.json").exists()


def test_scale_command_forwards_pair_and_total_as_distinct_flags() -> None:
    args = argparse.Namespace(
        tier="xlarge",
        seed=7,
        client_timeout_ms=None,
        history_max_bytes=300_000_000,
        history_max_total_bytes=600_000_000,
    )
    assert _history_inputs(args.history_max_bytes, args.history_max_total_bytes) == {
        "history_policy_id": "explicit-pair-total-diagnostic-v1",
        "requested_history_max_bytes": 300_000_000,
        "history_max_bytes": 300_000_000,
        "requested_history_max_total_bytes": 600_000_000,
        "history_max_total_bytes": 600_000_000,
    }
    assert _scale_command(args, Path("/tmp/scale_matrix"), Path("/tmp/new/artifact")) == [
        "/tmp/scale_matrix",
        "--tier",
        "xlarge",
        "--seed",
        "7",
        "--out-dir",
        "/tmp/new/artifact",
        "--history-max-bytes",
        "300000000",
        "--history-max-total-bytes",
        "600000000",
    ]
    args.history_max_bytes = None
    args.history_max_total_bytes = None
    assert _scale_command(args, Path("/tmp/scale_matrix"), Path("/tmp/new/artifact"))[-2:] == [
        "--out-dir",
        "/tmp/new/artifact",
    ]


@pytest.mark.parametrize(
    ("pair", "total"),
    [
        (None, 600_000_000),
        (600_000_001, 600_000_000),
        (2_147_483_649, None),
        (0, None),
        (True, None),
        (300_000_000, 0),
        (300_000_000, 600_000_000.0),
        (300_000_000, 1 << 64),
    ],
)
def test_invalid_history_inputs_refuse_without_output(
    pair: object, total: object, tmp_path: Path
) -> None:
    output = tmp_path / "must-remain-absent"
    with pytest.raises(ValueError):
        capture(
            argparse.Namespace(
                history_max_bytes=pair, history_max_total_bytes=total, out_root=output
            )
        )
    assert not output.exists()
