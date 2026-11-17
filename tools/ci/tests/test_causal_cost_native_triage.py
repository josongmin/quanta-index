"""Native capture triage fixtures derived from the P4 Large Off artifact shape."""

from __future__ import annotations

import hashlib
import json
from pathlib import Path

import pytest

from tools.benchmark.retrieval.causal_cost_native_triage import (
    build_native_triage_report,
    format_native_triage,
)

SOURCE = "824629e4f72b835788bc31c7f1bed5715f6e4f9c"
PHASES = {
    "full_stage_sources": {"completed": True, "wall_ms": 8222.280166},
    "full_event_ack": {
        "completed": True,
        "wall_ms": 27281.457875,
        "observer_ms": 145.513167,
    },
    "first_search": {"completed": True, "wall_ms": 43.854459},
    "reopen": {"completed": True, "wall_ms": 1303.107583},
    "reopen_search": {"completed": True, "wall_ms": 50.152166},
}


def _write_json(path: Path, value: dict) -> bytes:
    path.parent.mkdir(parents=True, exist_ok=True)
    raw = json.dumps(value, sort_keys=True, separators=(",", ":")).encode() + b"\n"
    path.write_bytes(raw)
    return raw


def _native_success(run: Path) -> None:
    # Same envelope and digest binding as P4/n2-large-off. Phase values are
    # deliberately fixed independently of the triage implementation.
    summary = {
        "schema_version": 2,
        "dimension": "scale-index-native",
        "mode": "cold",
        "concurrency": 1,
        "provenance": {"git_head": SOURCE},
        "detail": {
            "backend": "index-native-configured-daemon-uds",
            "tier": "large",
            "file_count": 4096,
            "phases": PHASES,
        },
    }
    raw = _write_json(run / "artifact" / "summary.json", summary)
    manifest = _write_json(
        run / "artifact" / "tier_manifest.json",
        {
            "kind": "quanta-index-scale-tier-manifest",
            "dimension": "scale",
            "manifest_schema_version": 2,
            "tiers": [{"tier": "large", "total_files": 4096}],
        },
    )
    _write_json(
        run / "execution.json",
        {
            "backend": "index-native",
            "status": "PARTIAL_DIAGNOSTIC",
            "product_status": "PRODUCT_ARTIFACT_VERIFIED",
            "exit_code": 0,
            "timed_out": False,
            "source_revision": SOURCE,
            "native_trace_mode": "off",
            "diagnostic_status": "NOT_APPLICABLE",
            "summary_sha256": hashlib.sha256(raw).hexdigest(),
            "tier_manifest_sha256": hashlib.sha256(manifest).hexdigest(),
            "build_binding": "UNPROVEN",
        },
    )


def _first_child_trace(run: Path) -> tuple[Path, bytes, str]:
    capture_id = "0" * 32
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
            "coverage": [],
        },
        {
            "record_type": "event",
            "output_seq": 1,
            "capture_id": capture_id,
            "ts_ns": 10,
            "kind": {
                "type": "span_begin",
                "span_id": 1,
                "root_kind": "operation",
                "stage": "operation",
                "lane_id": 0,
            },
        },
        {
            "record_type": "event",
            "output_seq": 2,
            "capture_id": capture_id,
            "ts_ns": 40,
            "kind": {"type": "span_end", "span_id": 1, "outcome": "ok"},
        },
        {
            "record_type": "footer",
            "schema_version": 1,
            "capture_id": capture_id,
            "closed_ns": 50,
            "status": "complete",
            "close_reason": "normal",
            "last_output_seq": 2,
            "producer_leases": 0,
            "events_attempted": 2,
            "events_accepted": 2,
            "events_dropped": 0,
            "events_written": 2,
            "events_unwritten": 0,
            "open_spans": 0,
        },
    ]
    raw = b"".join(json.dumps(row, separators=(",", ":")).encode() + b"\n" for row in rows)
    path = run / "causal-timeline.jsonl.native-large-children" / "first.jsonl"
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(raw)
    return path, raw, capture_id


def test_native_off_success_reports_product_and_fixed_phases_without_trace_parse(
    tmp_path: Path,
) -> None:
    run = tmp_path / "n2-large-off"
    _native_success(run)
    # Neither Off's unavailable timeline nor a malformed child is product
    # evidence. Default triage must not parse either one.
    (run / "causal-timeline.jsonl").write_bytes(b"not a qtrace")
    child = run / "causal-timeline.jsonl.native-large-children" / "first.jsonl"
    child.parent.mkdir()
    child.write_bytes(b"not a child qtrace")
    report = build_native_triage_report(run)
    assert report["status"] == "PRODUCT_VERIFIED"
    assert report["execution"]["source_revision"] == SOURCE
    assert report["trace"]["status"] == "OFF"
    assert report["phases"]["full_event_ack"] == PHASES["full_event_ack"]
    assert report["phases"]["first_search"] == PHASES["first_search"]
    assert report["phases"]["reopen_search"] == PHASES["reopen_search"]
    assert "observer=145.51 ms" in format_native_triage(report)


def test_preflight_blocked_control_folder_needs_no_producer(tmp_path: Path) -> None:
    control = tmp_path / "n2-xl-off-control"
    _write_json(
        control / "host-preflight.json",
        {
            "kind": "quanta-index-timing-preflight",
            "run_id": "n2-xl-off",
            "status": "blocked",
            "host_contention": {"over_limit": True},
        },
    )
    _write_json(
        control / "terminal.json",
        {
            "run": "n2-xl-off",
            "status": "BLOCKED",
            "reason": "canonical timing preflight refused",
        },
    )
    assert not (tmp_path / "n2-xl-off").exists()
    report = build_native_triage_report(control)
    assert report["status"] == "HOST_PREFLIGHT_BLOCKED"
    assert report["trace"]["status"] == "NOT_RUN"
    assert not report.get("phases")


def test_outer_timeout_without_refusal_is_not_missing_artifact(tmp_path: Path) -> None:
    run = tmp_path / "n2-xl-off"
    _write_json(
        run / "execution.json",
        {
            "backend": "index-native",
            "status": "FAILED",
            "product_status": "UNVERIFIED",
            "exit_code": None,
            "timed_out": True,
            "source_revision": SOURCE,
            "native_trace_mode": "off",
        },
    )
    assert not (run / "artifact" / "refusal.json").exists()
    report = build_native_triage_report(run)
    assert report["status"] == "OUTER_TIMEOUT"
    assert not report.get("phases")


def test_summary_digest_mismatch_withholds_phase_values(tmp_path: Path) -> None:
    run = tmp_path / "n2-large-off"
    _native_success(run)
    summary = run / "artifact" / "summary.json"
    # A previously valid artifact can drift after execution; only the raw
    # digest from execution.json is authoritative for phase projection.
    summary.write_bytes(summary.read_bytes().replace(b"27281.457875", b"1.0"))
    report = build_native_triage_report(run)
    assert report["status"] == "ARTIFACT_MISMATCH"
    assert not report.get("phases")


def test_missing_or_tampered_manifest_withholds_product_phases(tmp_path: Path) -> None:
    for mutation in ("missing", "tampered", "missing_digest"):
        run = tmp_path / mutation / "n2-large-off"
        _native_success(run)
        manifest = run / "artifact" / "tier_manifest.json"
        if mutation == "missing":
            manifest.unlink()
        elif mutation == "tampered":
            manifest.write_bytes(manifest.read_bytes().replace(b"4096", b"4095"))
        else:
            execution = run / "execution.json"
            record = json.loads(execution.read_text())
            del record["tier_manifest_sha256"]
            _write_json(execution, record)
        report = build_native_triage_report(run)
        assert report["status"] == "ARTIFACT_MISMATCH"
        assert report["phase_source"] is None
        assert not report["phases"]


def test_bound_manifest_shape_or_source_conflict_withholds_phases(tmp_path: Path) -> None:
    for mutation in ("wrong_tier", "wrong_file_count", "wrong_version", "wrong_source"):
        run = tmp_path / mutation / "n2-large-off"
        _native_success(run)
        execution = run / "execution.json"
        record = json.loads(execution.read_text())
        if mutation == "wrong_source":
            summary = run / "artifact" / "summary.json"
            changed = json.loads(summary.read_text())
            changed["provenance"]["git_head"] = "0" * 40
            record["summary_sha256"] = hashlib.sha256(_write_json(summary, changed)).hexdigest()
        else:
            manifest = run / "artifact" / "tier_manifest.json"
            changed = json.loads(manifest.read_text())
            if mutation == "wrong_tier":
                changed["tiers"][0]["tier"] = "xlarge"
            elif mutation == "wrong_file_count":
                changed["tiers"][0]["total_files"] = 32768
            else:
                changed["manifest_schema_version"] = 3
            record["tier_manifest_sha256"] = hashlib.sha256(
                _write_json(manifest, changed)
            ).hexdigest()
        _write_json(execution, record)
        report = build_native_triage_report(run)
        assert report["status"] == "ARTIFACT_MISMATCH"
        assert report["phase_source"] is None
        assert not report["phases"]


@pytest.mark.parametrize(
    ("diagnostic", "expected_denial", "expected_location"),
    [
        (
            'WorkBudget { first_denial: "writes", failed_stage: Some(BuildPrepare) }',
            "writes",
            None,
        ),
        (
            'WorkBudget { first_denial: "reads", denied_checkpoint: None, '
            'denied_charge: Some(DeniedCharge { axis: "reads", used: 999982, '
            "attempted: 27, limit: 1000000, overflow: false, caller: Location { "
            'file: "crates/quanta-index-catalog/src/native/authority/managed_install.rs", '
            "line: 108, column: 12 } }), failed_stage: Some(BuildPrepare) }",
            "reads",
            {
                "file": "crates/quanta-index-catalog/src/native/authority/managed_install.rs",
                "line": 108,
                "column": 12,
            },
        ),
        (
            'WorkBudget { first_denial: "bytes", denied_checkpoint: None, '
            'denied_charge: Some(DeniedCharge { axis: "bytes", used: 536870900, '
            "attempted: 28, limit: 536870912, overflow: false, caller: Location { "
            'file: "crates/quanta-index-lexical/src/native/text/cold_tokens.rs", '
            "line: 398, column: 20 } }), failed_stage: Some(BuildPrepare) }",
            "bytes",
            {
                "file": "crates/quanta-index-lexical/src/native/text/cold_tokens.rs",
                "line": 398,
                "column": 20,
            },
        ),
        (
            'WorkBudget { first_denial: "deadline", denied_checkpoint: Some(Location { '
            'file: "source.rs", line: 42, column: 7 }), denied_charge: None, '
            "failed_stage: Some(BuildPrepare) }",
            "deadline",
            {"file": "source.rs", "line": 42, "column": 7},
        ),
        (
            'WorkBudget { first_denial: "bytes", denied_checkpoint: None, '
            'denied_charge: None, unrelated: Location { file: "other.rs", '
            "line: 42, column: 7 }, failed_stage: Some(BuildPrepare) }",
            "bytes",
            None,
        ),
        (
            'WorkBudget { first_denial: "bytes", denied_checkpoint: None, '
            'denied_charge: Some(DeniedCharge { axis: "reads", used: 999982, '
            "attempted: 27, limit: 1000000, overflow: false, caller: Location { "
            'file: "other.rs", line: 42, column: 7 } }), '
            "failed_stage: Some(BuildPrepare) }",
            "bytes",
            None,
        ),
        (
            'WorkBudget { first_denial: "reads", denied_checkpoint: Some(Location { '
            'file: "other.rs", line: 42, column: 7 }), denied_charge: None, '
            "failed_stage: Some(BuildPrepare) }",
            "reads",
            None,
        ),
    ],
    ids=[
        "unobserved",
        "caddy-charge",
        "etcd-charge",
        "checkpoint",
        "unrelated",
        "wrong-axis",
        "wrong-checkpoint-axis",
    ],
)
def test_failed_capture_selects_only_refusal_for_bound_operation(
    tmp_path: Path, diagnostic: str, expected_denial: str, expected_location: dict | None
) -> None:
    run = tmp_path / "n2-xl-off"
    _write_json(
        run / "execution.json",
        {
            "backend": "index-native",
            "status": "FAILED",
            "product_status": "UNVERIFIED",
            "exit_code": 1,
            "timed_out": False,
            "source_revision": SOURCE,
            "native_trace_mode": "off",
            "command": ["scale_matrix", "--tier", "xlarge"],
        },
    )
    key = {"sequence": 2, "producer": [5] * 16}
    _write_json(
        run / "artifact" / "refusal.json",
        {
            "kind": "quanta-index-scale-index-native-failure",
            "provenance": {"git_head": SOURCE},
            "failure": {
                "stage": "full_event_ack",
                "phase_prefix": {
                    "publication_outcomes": [{"phase": "full_event_ack", "operation_key": key}]
                },
            },
        },
    )
    child = run / "causal-timeline.jsonl.native-xlarge-children" / "first.stderr"
    child.parent.mkdir()
    records = [
        {
            "event": "native_work_refusal",
            "operation_key": operation,
            "budget_diagnostic": diagnostic,
            "outcome": "rejected",
        }
        for operation in ({"sequence": 3, "producer": [5] * 16}, key)
    ]
    child.write_text(
        json.dumps({**records[-1], "budget_diagnostic": 'first_denial: "deadline"'}) + "\n"
    )
    unbound = build_native_triage_report(run)
    assert unbound["status"] == "PRODUCER_FAILED"
    assert unbound["refusal"]["evidence_binding"] == "UNBOUND"
    assert unbound["refusal"]["operation_match"] == "matched"

    harness = ("\n".join(json.dumps(row) for row in records) + "\n").encode()
    (run / "stderr").write_bytes(harness)
    execution = run / "execution.json"
    record = json.loads(execution.read_bytes())
    record["stderr_sha256"] = hashlib.sha256(harness).hexdigest()
    _write_json(execution, record)
    report = build_native_triage_report(run)
    assert report["status"] == "WORK_REFUSED"
    assert report["refusal"]["evidence"] == "harness_stderr"
    assert report["refusal"]["evidence_binding"] == "BOUND"
    assert report["refusal"]["operation_match"] == "matched"
    assert report["refusal"]["operation_sequence"] == 2
    assert report["refusal"]["first_denial"] == expected_denial
    assert report["refusal"]["checkpoint"] == expected_location


def test_mixed_failure_keeps_pin_release_distinct_from_unsettled_read(tmp_path: Path) -> None:
    run = tmp_path / "m2"
    raw_log = "".join(
        json.dumps({"event": "native_read_settlement_blocked", **row}) + "\n"
        for row in (
            {"target_sequence": 5, "status": "Pending(UnretiredView)", "head_sequence": 288},
            {
                "target_sequence": 5,
                "status": "Pending(TerminalCustody)",
                "head_sequence": 310,
                "exact_floor": 310,
                "oldest_ready_sequence": 27342,
            },
            {"target_sequence": 6, "status": "Pending(TargetPin)", "head_sequence": 318},
        )
    ).encode()
    _write_json(
        run / "execution.json",
        {
            "backend": "index-native",
            "status": "FAILED",
            "product_status": "UNVERIFIED",
            "exit_code": 1,
            "timed_out": False,
            "source_revision": SOURCE,
            "native_trace_mode": "off",
            "native_mixed_load": True,
            "command": ["scale_matrix", "--tier", "large"],
            "stderr_sha256": hashlib.sha256(raw_log).hexdigest(),
        },
    )
    (run / "stderr").write_bytes(raw_log)
    _write_json(
        run / "artifact" / "refusal.json",
        {
            "kind": "quanta-index-scale-index-native-failure",
            "provenance": {"git_head": SOURCE},
            "source": {"tier": "large"},
            "failure": {
                "stage": "execution_unclassified",
                "phase_prefix": {
                    "mixed_load": {
                        "first_failure_owner": "held_read",
                        "held_worker_error": "mixed held settlement exceeded 60 seconds",
                        "retained_read": {
                            "initial_view_sequence": 5,
                            "last_status": "Released",
                            "confirmed_last_pin": True,
                            "confirmed_release_ms": 600485.719166,
                            "last_settlement_state": "Pending",
                            "settlement_polls": 61,
                            "settlement_head_sequence": 318,
                            "settlement_exact_floor": 318,
                            "settlement_has_remaining_target_pins": False,
                            "settled_ms": None,
                        },
                    }
                },
            },
        },
    )
    report = build_native_triage_report(run)
    assert report["status"] == "PRODUCER_FAILED"
    assert report["failed_phase"] == "execution_unclassified"
    assert report["mixed_failure"] == {
        "first_failure_owner": "held_read",
        "held_worker_error": "mixed held settlement exceeded 60 seconds",
        "pin_status": "Released",
        "confirmed_last_pin": True,
        "confirmed_release_ms": 600485.719166,
        "last_settlement_state": "Pending",
        "settlement_polls": 61,
        "settlement_head_sequence": 318,
        "settlement_exact_floor": 318,
        "settlement_has_remaining_target_pins": False,
        "settled_ms": None,
        "settled_field_present": True,
        "settlement_blocker_samples": {
            "target_sequence": 5,
            "observed": ["Pending(UnretiredView)", "Pending(TerminalCustody)"],
            "last_logged": {
                "status": "Pending(TerminalCustody)",
                "head_sequence": 310,
                "exact_floor": 310,
                "oldest_ready_sequence": 27342,
            },
        },
    }
    text = format_native_triage(report)
    assert "Mixed first failure owner=held_read" in text
    assert "mixed held settlement exceeded 60 seconds" in text
    assert "Pin status=Released" in text
    assert "Settlement state=Pending  polls=61  settled_ms=null" in text
    assert "Settlement head=318  exact floor=318  remaining target pins=false" in text
    assert "Last logged blocker Pending(TerminalCustody)  head=310  oldest ready=27342" in text

    refusal = run / "artifact" / "refusal.json"
    zero = json.loads(refusal.read_text())
    retained = zero["failure"]["phase_prefix"]["mixed_load"]["retained_read"]
    retained["settlement_head_sequence"] = 0
    retained["settlement_exact_floor"] = 0
    retained["settlement_polls"] = 0
    _write_json(refusal, zero)
    zero_report = build_native_triage_report(run)
    assert zero_report["mixed_failure"]["settlement_head_sequence"] == 0
    assert zero_report["mixed_failure"]["settlement_exact_floor"] == 0
    assert zero_report["mixed_failure"]["settlement_has_remaining_target_pins"] is False
    assert "Settlement head=0  exact floor=0  remaining target pins=false" in format_native_triage(
        zero_report
    )


def test_mixed_failure_message_is_bounded_and_cannot_inject_lines(tmp_path: Path) -> None:
    run = tmp_path / "m2"
    _write_json(
        run / "execution.json",
        {
            "backend": "index-native",
            "status": "FAILED",
            "product_status": "UNVERIFIED",
            "exit_code": 1,
            "timed_out": False,
            "source_revision": SOURCE,
            "native_trace_mode": "off",
        },
    )
    _write_json(
        run / "artifact" / "refusal.json",
        {
            "kind": "quanta-index-scale-index-native-failure",
            "provenance": {"git_head": SOURCE},
            "failure": {
                "stage": "execution_unclassified",
                "phase_prefix": {
                    "mixed_load": {
                        "first_failure_owner": "held_read",
                        "held_worker_error": "A" * 300 + "\nFORGED_STATUS",
                    }
                },
            },
        },
    )
    report = build_native_triage_report(run)
    error = report["mixed_failure"]["held_worker_error"]
    assert len(error.encode("utf-8")) <= 259
    assert "\n" not in error
    assert "FORGED_STATUS" not in format_native_triage(report)


def test_failed_record_complete_child_is_unbound_diagnostic(tmp_path: Path) -> None:
    run = tmp_path / "xr56r"
    _first_child_trace(run)
    _write_json(
        run / "execution.json",
        {
            "backend": "index-native",
            "status": "FAILED",
            "product_status": "UNVERIFIED",
            "exit_code": 1,
            "timed_out": False,
            "source_revision": SOURCE,
            "native_trace_mode": "record",
            "command": ["scale_matrix", "--tier", "large"],
        },
    )
    _write_json(
        run / "artifact" / "refusal.json",
        {
            "kind": "quanta-index-scale-index-native-failure",
            "provenance": {"git_head": SOURCE},
            "failure": {
                "stage": "full_event_ack",
                "phase_prefix": {},
                "child_trace_files": {
                    "captures": [{"role": "first", "current_integrity": "unverified"}]
                },
            },
        },
    )
    report = build_native_triage_report(run, include_trace=True)
    assert report["status"] == "PRODUCER_FAILED"
    assert report["trace"]["status"] == "UNBOUND_DIAGNOSTIC"
    assert report["trace"]["parse_status"] == "COMPLETE_DIAGNOSTIC"
    assert report["trace"]["capture_status"] == "complete"
    assert "run binding UNBOUND" in format_native_triage(report)


def test_bound_child_trace_requires_exact_saved_bytes(tmp_path: Path) -> None:
    run = tmp_path / "record"
    path, raw, capture_id = _first_child_trace(run)
    execution = run / "execution.json"
    _write_json(
        execution,
        {
            "backend": "index-native",
            "status": "FAILED",
            "product_status": "UNVERIFIED",
            "exit_code": 1,
            "timed_out": False,
            "source_revision": SOURCE,
            "native_trace_mode": "record",
            "command": ["scale_matrix", "--tier", "large"],
            "native_child_inputs": {
                "first": {
                    "capture_id": capture_id,
                    "trace_path": str(path),
                    "trace_bytes": len(raw),
                    "trace_sha256": hashlib.sha256(raw).hexdigest(),
                }
            },
        },
    )
    bound = build_native_triage_report(run, include_trace=True)
    assert bound["status"] == "PRODUCER_FAILED"
    assert bound["trace"]["status"] == "BOUND_COMPLETE_DIAGNOSTIC"
    assert bound["trace"]["parse_status"] == "COMPLETE_DIAGNOSTIC"
    changed = raw.replace(b'"started_ns":0', b'"started_ns":1')
    assert changed != raw
    path.write_bytes(changed)
    mismatch = build_native_triage_report(run, include_trace=True)
    assert mismatch["status"] == "PRODUCER_FAILED"
    assert mismatch["trace"]["status"] == "BINDING_MISMATCH"
    assert mismatch["trace"]["parse_status"] == "COMPLETE_DIAGNOSTIC"
    assert mismatch["trace"]["binding_mismatch_fields"] == ["trace_sha256"]
    assert mismatch["trace"]["requests"] == []
    assert "run binding MISMATCH" in format_native_triage(mismatch)
