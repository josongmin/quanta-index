"""Frontdoor history policy flags and refusal before creating an output root."""

from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path
from types import SimpleNamespace

import pytest

from tools.benchmark.retrieval import causal_cost_capture
from tools.benchmark.retrieval.causal_cost_capture import (
    _history_inputs,
    _scale_command,
    capture,
    offline_export,
)


def _timeline() -> bytes:
    capture_id = "0" * 32
    phases = (
        "full_ingest_seal",
        "full_activate",
        "delta_ingest_seal",
        "delta_activate",
        "noop_seal",
        "noop_activate",
    )
    rows = [
        {
            "record_type": "header",
            "schema_version": 1,
            "capture_id": capture_id,
            "stream_id": capture_id,
            "clock_domain": capture_id,
            "process_instance": capture_id,
            "started_ns": 0,
            "max_record_bytes": 4096,
            "max_counters_per_event": 1,
            "coverage": ["harness"],
        }
    ]
    sequence = 0
    for span_id, phase in enumerate(phases, 1):
        for ts, kind in (
            (
                span_id * 10,
                {
                    "type": "span_begin",
                    "span_id": span_id,
                    "root_kind": "operation",
                    "stage": "harness_phase",
                    "lane_id": 1,
                },
            ),
            (span_id * 10 + 1, {"type": "phase_identity", "span_id": span_id, "phase": phase}),
            (span_id * 10 + 2, {"type": "span_end", "span_id": span_id, "outcome": "ok"}),
        ):
            sequence += 1
            rows.append(
                {
                    "record_type": "event",
                    "output_seq": sequence,
                    "capture_id": capture_id,
                    "ts_ns": ts,
                    "kind": kind,
                }
            )
    rows.append(
        {
            "record_type": "footer",
            "schema_version": 1,
            "capture_id": capture_id,
            "closed_ns": 100,
            "status": "complete",
            "close_reason": "normal",
            "last_output_seq": sequence,
            "producer_leases": 0,
            "events_attempted": sequence,
            "events_accepted": sequence,
            "events_dropped": 0,
            "events_written": sequence,
            "events_unwritten": 0,
            "open_spans": 0,
        }
    )
    return b"".join(json.dumps(row, separators=(",", ":")).encode() + b"\n" for row in rows)


@pytest.fixture
def capture_fixture(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> SimpleNamespace:
    monkeypatch.setenv("QUANTA_INDEX_CAUSAL_PROFILE_V1", "1")
    source = tmp_path / "source"
    source.mkdir()
    binary = tmp_path / "scale_matrix"
    binary.write_bytes(b"fixed executable fixture\n")
    binary.chmod(0o700)
    state = {
        "head": "a" * 40,
        "dirty": "",
        "replays": 0,
        "product_validations": 0,
        "timeline": _timeline(),
    }
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
        disk_observation=None,
        max_seconds=10,
        timeline=True,
    )

    def git(_cwd: Path, *command: str) -> str:
        return state["head"] if command == ("rev-parse", "HEAD") else state["dirty"]

    def run(_command: list[str], **kwargs: object) -> SimpleNamespace:
        assert "QUANTA_INDEX_CAUSAL_PROFILE_V1" not in kwargs["env"]
        artifact = args.out_root / "artifact"
        artifact.mkdir()
        (artifact / "summary.json").write_bytes(b'{"fixture":"summary"}\n')
        (artifact / "tier_manifest.json").write_bytes(b'{"fixture":"manifest"}\n')
        kwargs["stdout"].write(b"fixed stdout\n")
        kwargs["stderr"].write(b"fixed trace\n")
        (args.out_root / "causal-timeline.jsonl").write_bytes(state["timeline"])
        return SimpleNamespace(returncode=0)

    def replay(
        summary: bytes, trace: bytes, executable: bytes, manifest: bytes, **options: object
    ) -> dict:
        state["replays"] += 1
        assert summary == b'{"fixture":"summary"}\n'
        assert trace == b"fixed trace\n"
        assert executable == b"fixed executable fixture\n"
        assert manifest == b'{"fixture":"manifest"}\n'
        return {
            "status": "diagnostic_unqualified",
            "source_revision": options["source_revision"],
            "binary_sha256": hashlib.sha256(executable).hexdigest(),
            "summary_sha256": hashlib.sha256(summary).hexdigest(),
            "trace_sha256": hashlib.sha256(trace).hexdigest(),
            "runtime_config": {
                "history_policy_id": "scale-supported-v1",
                "requested_history_max_bytes": None,
                "history_max_bytes": 1_073_741_824,
                "requested_history_max_total_bytes": None,
                "history_max_total_bytes": 2_147_483_648,
                "requested_disk_observation_mode": args.disk_observation,
                "disk_observation_mode": args.disk_observation or "boundary-only",
            },
            "scope": {},
            "phases": {
                name: {
                    "status": "ok",
                    "sync": {},
                    "exact_live_token_scan": {"calls": 0, "elapsed_ns": 0},
                    "bm25_census_build": {"calls": 0, "elapsed_ns": 0},
                    "bm25_boundary": {},
                    "bm25_live_build": [],
                    "process_write_io": {},
                    "process_write_io_unavailable_reason": None,
                }
                for name in (
                    "full_ingest_seal",
                    "full_activate",
                    "delta_ingest_seal",
                    "delta_activate",
                    "noop_seal",
                    "noop_activate",
                )
            },
        }

    def validate_product(*_values: object, **_options: object) -> dict:
        state["product_validations"] += 1
        return {"status": "product_artifact_verified"}

    monkeypatch.setattr(causal_cost_capture, "_git", git)
    monkeypatch.setattr(causal_cost_capture.subprocess, "run", run)
    monkeypatch.setattr(causal_cost_capture, "replay", replay)
    monkeypatch.setattr(causal_cost_capture, "validate_product_artifacts", validate_product)
    return SimpleNamespace(args=args, state=state, replay=replay)


def test_capture_accepts_one_unchanged_executable_and_input_epoch(
    capture_fixture: SimpleNamespace,
) -> None:
    result = capture(capture_fixture.args)
    assert result["status"] == "PARTIAL_DIAGNOSTIC"
    assert result["diagnostic_status"] == "PARTIAL"
    assert result["product_status"] == "PRODUCT_ARTIFACT_VERIFIED"
    assert result["binary_sha256"] == result["binary_sha256_after"]
    assert result["build_binding"] == "UNPROVEN"
    assert result["summary_sha256"] == hashlib.sha256(b'{"fixture":"summary"}\n').hexdigest()
    assert result["tier_manifest_sha256"] == hashlib.sha256(b'{"fixture":"manifest"}\n').hexdigest()
    assert capture_fixture.state["replays"] == 1
    assert capture_fixture.state["product_validations"] == 1
    assert result["requested_disk_observation_mode"] is None
    assert result["disk_observation_mode"] == "boundary-only"
    assert result["command"][-2:] == ["--disk-observation", "boundary-only"]
    assert not (capture_fixture.args.out_root / "causal-profile.json").exists()
    assert (capture_fixture.args.out_root / "causal-waterfall.json").is_file()
    status = json.loads((capture_fixture.args.out_root / "timeline-status.json").read_text())
    assert status["capture_status"] == "complete"
    assert status["coverage"] == ["harness"]
    assert status["viewer_available"] is True
    assert status["source_binding"] == "UNPROVEN"
    assert status["execution"]["summary_sha256"] == result["summary_sha256"]
    assert status["execution"]["tier_manifest_sha256"] == result["tier_manifest_sha256"]
    assert status["execution"]["stderr_sha256"] == result["stderr_sha256"]
    assert status["execution"]["timeline_capture_id"] == "0" * 32
    summary = json.loads((capture_fixture.args.out_root / "timeline-summary.json").read_text())
    assert summary["legacy_reconciliation"]["phase_parity"] == "verified"
    waterfall = json.loads((capture_fixture.args.out_root / "causal-waterfall.json").read_text())
    assert waterfall["quantaIndexSummary"]["legacy_reconciliation"]["phase_parity"] == "verified"
    assert "legacyAggregate" not in waterfall


def test_export_uses_the_committed_timeline_snapshot_after_path_changes(tmp_path: Path) -> None:
    raw = _timeline()
    input_path = tmp_path / "causal-timeline.jsonl"
    output = tmp_path / "export"
    input_path.write_bytes(raw)
    output.mkdir()
    input_path.write_bytes(b"changed after input commitment\n")

    status = causal_cost_capture.export_timeline(
        input_path,
        output,
        viewer=True,
        raw_input=raw,
    )

    expected_sha = hashlib.sha256(raw).hexdigest()
    assert status["capture_status"] == "complete"
    assert status["input_sha256"] == expected_sha
    summary = json.loads((output / "timeline-summary.json").read_bytes())
    waterfall = json.loads((output / "causal-waterfall.json").read_bytes())
    assert summary["stream_sha256"] == expected_sha
    assert waterfall["quantaIndexSummary"]["stream_sha256"] == expected_sha


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


def test_failed_producer_preserves_raw_timeline_and_exports_viewer(
    capture_fixture: SimpleNamespace, monkeypatch: pytest.MonkeyPatch
) -> None:
    original = causal_cost_capture.subprocess.run

    def failed(command: list[str], **kwargs: object) -> SimpleNamespace:
        original(command, **kwargs)
        return SimpleNamespace(returncode=7)

    monkeypatch.setattr(causal_cost_capture.subprocess, "run", failed)
    result = capture(capture_fixture.args)
    assert result["status"] == "FAILED"
    assert (capture_fixture.args.out_root / "causal-timeline.jsonl").is_file()
    assert (capture_fixture.args.out_root / "causal-waterfall.json").is_file()
    assert not (capture_fixture.args.out_root / "causal-profile.json").exists()


def test_recorder_start_failure_keeps_explicit_unavailable_artifacts(
    capture_fixture: SimpleNamespace, monkeypatch: pytest.MonkeyPatch
) -> None:
    def failed_without_timeline(_command: list[str], **kwargs: object) -> SimpleNamespace:
        kwargs["stderr"].write(b"recorder start failed\n")
        return SimpleNamespace(returncode=7)

    monkeypatch.setattr(causal_cost_capture.subprocess, "run", failed_without_timeline)
    result = capture(capture_fixture.args)
    output = capture_fixture.args.out_root
    assert result["status"] == "FAILED"
    assert (output / "causal-timeline.jsonl").read_bytes() == b""
    status = json.loads((output / "timeline-status.json").read_bytes())
    summary = json.loads((output / "timeline-summary.json").read_bytes())
    assert status["capture_status"] == "unavailable"
    assert status["coverage"] is None
    assert status["raw_placeholder"] is True
    assert status["input_sha256"] is None
    assert status["viewer_available"] is False
    assert summary["status"] == "unavailable"
    assert summary["spans"] == []
    assert summary["bm25_census_aggregates"] == []
    assert summary["bm25_boundary_aggregates"] == []
    assert summary["bm25_live_build_aggregates"] == []
    assert summary["unavailable_reason"] == status["reason"]
    assert (output / "execution.json").is_file()
    assert not (output / "causal-waterfall.json").exists()


@pytest.mark.parametrize(
    "mutation",
    [
        "binary",
        "same-bytes-replacement",
        "summary",
        "manifest",
        "trace",
        "timeline",
        "head",
        "dirty",
    ],
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
        elif mutation == "timeline":
            (args.out_root / "causal-timeline.jsonl").write_bytes(b"changed timeline\n")
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


def test_phase_only_parity_never_publishes_qualified_profile(
    capture_fixture: SimpleNamespace,
) -> None:
    result = capture(capture_fixture.args)
    assert result["status"] == "PARTIAL_DIAGNOSTIC"
    assert "profile_sha256" not in result
    assert not (capture_fixture.args.out_root / "causal-profile.json").exists()


def test_missing_legacy_markers_leave_a_bound_canonical_partial_capture(
    capture_fixture: SimpleNamespace, monkeypatch: pytest.MonkeyPatch
) -> None:
    def no_legacy(*_values: bytes, **_kwargs: object) -> dict:
        raise ValueError("missing QI_CAUSAL_V1 phase markers")

    monkeypatch.setattr(causal_cost_capture, "replay", no_legacy)
    result = capture(capture_fixture.args)
    assert result["status"] == "PARTIAL_DIAGNOSTIC"
    assert result["legacy_replay_error"] == "missing QI_CAUSAL_V1 phase markers"
    assert result["product_status"] == "PRODUCT_ARTIFACT_VERIFIED"
    assert capture_fixture.state["product_validations"] == 1
    assert result["build_binding"] == "UNPROVEN"
    assert (
        result["timeline_sha256"]
        == hashlib.sha256(
            (capture_fixture.args.out_root / "causal-timeline.jsonl").read_bytes()
        ).hexdigest()
    )
    summary = json.loads((capture_fixture.args.out_root / "timeline-summary.json").read_bytes())
    assert summary["status"] == "complete"
    assert "legacy_reconciliation" not in summary
    assert not (capture_fixture.args.out_root / "causal-profile.json").exists()


def test_invalid_product_artifact_is_a_failed_capture(
    capture_fixture: SimpleNamespace, monkeypatch: pytest.MonkeyPatch
) -> None:
    def reject_product(*_values: object, **_options: object) -> dict:
        raise ValueError("scale artifact is failed")

    monkeypatch.setattr(causal_cost_capture, "validate_product_artifacts", reject_product)
    result = capture(capture_fixture.args)
    assert result["status"] == "FAILED"
    assert result["product_status"] == "EXIT_ZERO_UNQUALIFIED"
    assert result["diagnostic_status"] == "UNAVAILABLE"
    assert result["reason"] == "scale artifact is failed"


@pytest.mark.parametrize(
    ("timeline", "capture_status"),
    [
        (b"not JSON\n", "invalid"),
        (_timeline().rsplit(b'\n{"record_type":"footer"', 1)[0] + b"\n", "partial"),
    ],
)
def test_invalid_or_partial_timeline_preserves_verified_product_artifacts(
    capture_fixture: SimpleNamespace, timeline: bytes, capture_status: str
) -> None:
    capture_fixture.state["timeline"] = timeline
    result = capture(capture_fixture.args)
    assert result["status"] == "PARTIAL_DIAGNOSTIC"
    assert result["product_status"] == "PRODUCT_ARTIFACT_VERIFIED"
    assert result["diagnostic_status"] == "PARTIAL"
    assert result["timeline_sha256"] == hashlib.sha256(timeline).hexdigest()
    assert capture_fixture.state["product_validations"] == 1
    assert capture_fixture.state["replays"] == 0
    timeline_status = json.loads(
        (capture_fixture.args.out_root / "timeline-status.json").read_bytes()
    )
    assert timeline_status["capture_status"] == capture_status
    assert timeline_status["execution"]["product_status"] == "PRODUCT_ARTIFACT_VERIFIED"
    assert not (capture_fixture.args.out_root / "causal-profile.json").exists()


def test_viewer_is_opt_in_but_raw_and_status_are_always_recorded(
    capture_fixture: SimpleNamespace,
) -> None:
    capture_fixture.args.timeline = False
    result = capture(capture_fixture.args)
    assert result["status"] == "PARTIAL_DIAGNOSTIC"
    assert (capture_fixture.args.out_root / "causal-timeline.jsonl").is_file()
    assert (capture_fixture.args.out_root / "timeline-status.json").is_file()
    assert (capture_fixture.args.out_root / "timeline-summary.json").is_file()
    assert not (capture_fixture.args.out_root / "causal-waterfall.json").exists()


def test_offline_partial_export_preserves_raw_and_status(tmp_path: Path) -> None:
    raw = tmp_path / "raw.jsonl"
    raw.write_bytes(_timeline().rsplit(b'\n{"record_type":"footer"', 1)[0] + b"\n")
    original = raw.read_bytes()
    output = tmp_path / "offline"
    status = offline_export(raw, output)
    assert status["capture_status"] == "partial"
    assert status["coverage"] is None
    assert status["source_binding"] == "offline_unverified"
    assert status["viewer_available"] is True
    assert raw.read_bytes() == original
    preserved = output / "causal-timeline.jsonl"
    assert preserved.read_bytes() == original
    raw.write_bytes(b"source changed after offline export\n")
    assert preserved.read_bytes() == original
    execution = json.loads((output / "execution.json").read_bytes())
    assert execution["status"] == "OFFLINE_EXPORT_UNVERIFIED"
    assert execution["product_status"] == "UNVERIFIED"
    assert execution["diagnostic_status"] == "UNVERIFIED"
    assert execution["capture_status"] == "partial"
    assert execution["producer_executed"] is False
    assert execution["timeline_sha256"] == hashlib.sha256(original).hexdigest()
    assert (output / "timeline-summary.json").is_file()
    assert (output / "causal-waterfall.json").is_file()


@pytest.mark.parametrize("oversized", [False, True])
def test_offline_invalid_or_oversized_input_retains_required_artifacts(
    tmp_path: Path, oversized: bool
) -> None:
    source = tmp_path / "raw.jsonl"
    if oversized:
        with source.open("wb") as stream:
            stream.truncate(128 * 1024 * 1024 + 1)
    else:
        source.write_bytes(b"not JSON\n")
    output = tmp_path / "offline"
    status = offline_export(source, output)
    summary = json.loads((output / "timeline-summary.json").read_bytes())
    execution = json.loads((output / "execution.json").read_bytes())
    assert (output / "causal-timeline.jsonl").stat().st_size == source.stat().st_size
    if not oversized:
        assert (output / "causal-timeline.jsonl").read_bytes() == b"not JSON\n"
    assert status["capture_status"] == "invalid"
    assert status["coverage"] is None
    assert status["viewer_available"] is False
    assert status["raw_placeholder"] is False
    assert summary["status"] == "invalid"
    assert summary["spans"] == []
    assert summary["unavailable_reason"] == status["reason"]
    assert execution["capture_status"] == "invalid"
    assert not (output / "causal-waterfall.json").exists()


def test_offline_missing_source_records_unavailable_placeholder(tmp_path: Path) -> None:
    output = tmp_path / "offline"
    status = offline_export(tmp_path / "missing.jsonl", output)
    assert status["capture_status"] == "unavailable"
    assert status["raw_placeholder"] is True
    assert (output / "causal-timeline.jsonl").read_bytes() == b""
    assert (output / "timeline-summary.json").is_file()
    assert (output / "execution.json").is_file()


def test_scale_command_forwards_pair_and_total_as_distinct_flags() -> None:
    args = argparse.Namespace(
        tier="xlarge",
        seed=7,
        client_timeout_ms=None,
        history_max_bytes=300_000_000,
        history_max_total_bytes=600_000_000,
        disk_observation="periodic",
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
        "--trace-mode",
        "record",
        "--trace-out",
        "/tmp/new/causal-timeline.jsonl",
        "--disk-observation",
        "periodic",
        "--history-max-bytes",
        "300000000",
        "--history-max-total-bytes",
        "600000000",
    ]
    args.history_max_bytes = None
    args.history_max_total_bytes = None
    args.disk_observation = None
    assert "--disk-observation" in _scale_command(
        args, Path("/tmp/scale_matrix"), Path("/tmp/new/artifact")
    )


def test_capture_refuses_replayed_disk_mode_mismatch(
    capture_fixture: SimpleNamespace, monkeypatch: pytest.MonkeyPatch
) -> None:
    args = capture_fixture.args
    args.disk_observation = "periodic"

    def wrong_mode(*values: bytes, **kwargs: object) -> dict:
        profile = capture_fixture.replay(*values, **kwargs)
        profile["runtime_config"]["disk_observation_mode"] = "boundary-only"
        return profile

    monkeypatch.setattr(causal_cost_capture, "replay", wrong_mode)
    result = capture(args)
    assert result["status"] == "PARTIAL_DIAGNOSTIC"
    assert result["product_status"] == "PRODUCT_ARTIFACT_VERIFIED"
    assert result["diagnostic_status"] == "PARTIAL"
    assert "disk observation mode" in result["reason"]
    assert result["command"][-2:] == ["--disk-observation", "periodic"]
    assert not (args.out_root / "causal-profile.json").exists()


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
