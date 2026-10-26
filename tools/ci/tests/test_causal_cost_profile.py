"""Independent refusal controls for the optional lifecycle cost profile."""

from __future__ import annotations

import hashlib
import json
import os
from copy import deepcopy
from pathlib import Path

import pytest

from tools.benchmark.evidence import RawFile
from tools.benchmark.retrieval.causal_cost_profile import (
    _native_child_trace,
    parse_trace,
    replay,
    validate_native_product_artifacts,
    validate_product_artifacts,
)
from tools.benchmark.retrieval.causal_cost_timeline import parse as parse_timeline
from tools.benchmark.retrieval.conditional_proof import canonical

SOURCE = "a" * 40
# Fixed Rust Native wire gold. Keep independent from the Python validator's
# accepted phase/field sets so a producer or validator omission fails here.
NATIVE_PHASE_GOLD = (
    "corpus_generation",
    "config_setup",
    "startup",
    "workspace_create",
    "profile_configure",
    "full_begin_input",
    "full_stage_sources",
    "full_event_ack",
    "first_search",
    "delta_stage_source",
    "delta_event_ack",
    "delta_search",
    "noop_event_ack",
    "noop_stable_search",
    "delete_event_ack",
    "deleted_search",
    "old_pin_search",
    "drain",
    "reopen",
    "reopen_read_head",
    "reopen_deleted_search",
    "reopen_search",
    "final_drain",
)
NATIVE_IO_GOLD = {
    "write_calls": 5,
    "written_bytes": 4096,
    "failed_writes": 0,
    "enospc_writes": 0,
    "file_sync_calls": 3,
    "failed_file_syncs": 0,
    "directory_sync_calls": 0,
    "failed_directory_syncs": 0,
    "barrier_calls": 0,
    "failed_barriers": 0,
    "reserve_calls": 1,
    "reserve_objects": 1,
    "single_object_reserves": 1,
    "failed_reserves": 0,
    "confirm_calls": 1,
    "confirm_objects": 1,
    "single_object_confirms": 1,
    "failed_confirms": 0,
    "write_elapsed_ns": 0,
    "file_sync_elapsed_ns": 27,
    "directory_sync_elapsed_ns": 0,
    "barrier_elapsed_ns": 0,
    "reserve_elapsed_ns": 0,
    "confirm_elapsed_ns": 0,
}
PHASES = (
    "full_ingest",
    "full_seal",
    "full_activate",
    "delta_ingest_seal",
    "delta_activate",
    "noop_seal",
    "noop_activate",
    "delete_seal",
    "delete_activate",
    "same_process_reopen",
)
BM25_FULL = (
    b"QI_CAUSAL_V1 kind=bm25_live_build ok=1 elapsed_ns=1000 "
    b"reused_segments=0 changed_segments=0 new_segments=1 mask_docs=0 "
    b"new_census_docs=2 newly_dead_docs=0 logical_census_bytes=30 "
    b"changed_ns=0 new_ns=700 death_ns=0 correction_keys=0 "
    b"segment_fanout=1 retained_valid=1 retained_estimate_bytes=4096\n"
)
BM25_DELTA = (
    b"QI_CAUSAL_V1 kind=bm25_live_build ok=1 elapsed_ns=1200 "
    b"reused_segments=0 changed_segments=1 new_segments=1 mask_docs=2 "
    b"new_census_docs=1 newly_dead_docs=1 logical_census_bytes=40 "
    b"changed_ns=500 new_ns=500 death_ns=200 correction_keys=2 "
    b"segment_fanout=2 retained_valid=1 retained_estimate_bytes=5000\n"
)
BM25_REUSE = (
    b"QI_CAUSAL_V1 kind=bm25_live_build ok=1 elapsed_ns=300 "
    b"reused_segments=1 changed_segments=0 new_segments=0 mask_docs=0 "
    b"new_census_docs=0 newly_dead_docs=0 logical_census_bytes=0 "
    b"changed_ns=0 new_ns=0 death_ns=0 correction_keys=0 "
    b"segment_fanout=1 retained_valid=1 retained_estimate_bytes=4096\n"
)
BM25_ENCODE = b"QI_CAUSAL_V1 kind=bm25_boundary label=encode ok=1 elapsed_ns=70 logical_bytes=128 index_files=1\n"
BM25_BASE = b"QI_CAUSAL_V1 kind=bm25_boundary label=base_read ok=1 elapsed_ns=90 logical_bytes=256 index_files=1\n"
BM25_CENSUS = b"QI_CAUSAL_V1 kind=bm25_census_build ok=1 elapsed_ns=80 docs=2 encoded_bytes=32\n"
SEAL_PHASES = {"full_seal", "delta_ingest_seal", "noop_seal", "delete_seal"}


def _trace_phase(name: str) -> bytes:
    rows = [f"QI_CAUSAL_V1 kind=phase_start name={name}\n".encode()]
    if name == "full_ingest":
        rows.append(BM25_CENSUS)
    if name == "full_seal":
        rows.extend((b"QI_CAUSAL_V1 kind=sync label=atomic_file ok=1 elapsed_ns=400\n", BM25_FULL))
        rows.append(
            b"QI_CAUSAL_V1 kind=exact_live_token_scan ok=1 elapsed_ns=800 terms=2 postings=4 live_postings=3 live_tokens=7\n"
        )
    elif name in SEAL_PHASES:
        rows.extend((BM25_BASE, BM25_REUSE if name == "noop_seal" else BM25_DELTA))
    if name in SEAL_PHASES:
        rows.append(BM25_ENCODE)
    rows.append(f"QI_CAUSAL_V1 kind=phase_end name={name} ok=1\n".encode())
    return b"".join(rows)


SINGLE_TRACE = _trace_phase("full_seal")
TRACE = b"".join(_trace_phase(name) for name in PHASES)


def _observed_disk_snapshot(allocated: int) -> dict:
    return {
        "status": "observed",
        "allocated_root_bytes": allocated,
        "filesystem_free_bytes": 1000,
        "filesystem_available_bytes": 900,
        "unavailable_reason": None,
    }


def _summary() -> dict:
    phase = {
        "cpu_process_user_ms": 1.0,
        "cpu_process_system_ms": 2.0,
        "observation_span_ms": 10.0,
        "process_write_io": {
            "write_bytes": 4096,
            "cancelled_write_bytes": 0,
            "syscw": 2,
            "wchar": 100,
        },
        "process_write_io_unavailable_reason": None,
        "disk_sample_interval_ms": None,
        "disk": {
            "observation_mode": "boundary-only",
            "start": _observed_disk_snapshot(100),
            "end": _observed_disk_snapshot(120),
            "sampled_max_allocated_root_bytes": None,
            "sampled_high_water_status": "unavailable_periodic_sampling_disabled",
            "sampled": [],
            "periodic_probe_wall_ms": 0.0,
            "observed_max_gap_ms": 10.0,
            "sampled_max_is_true_peak": False,
        },
    }
    return {
        "schema_version": 2,
        "provenance": {
            "git_head": SOURCE,
            "corpus_digest": "sha256:" + "c" * 64,
            "config_digest": "sha256:" + "d" * 64,
        },
        "detail": {
            "passed": True,
            "measured_tiers": [
                {
                    "status": "measured",
                    "tier": "large",
                    "seed": 5,
                    "file_count": 4096,
                    "source_repo_count": 16,
                    "disk_observation_mode": "boundary-only",
                    "client_request_timeout_ms": 600_000,
                    "query_dispatch_budget_ms": 600_000,
                    "requested_client_request_timeout_ms": None,
                    "history_max_generations": 2,
                    "history_max_bytes": 1024 * 1024 * 1024,
                    "requested_history_max_bytes": None,
                    "requested_history_max_total_bytes": None,
                    "history_policy_id": "scale-supported-v1",
                    "history_max_revision_pairs": 128,
                    "ingest_max_records": 100_000,
                    "ingest_max_text_bytes": 134_217_728,
                    "ingest_max_vector_bytes": 268_435_456,
                    "coverage_max_encoded_bytes": 67_108_864,
                    "coverage_max_decode_heap_bytes": 268_435_456,
                    "source_publication_part_bytes": 1_048_576,
                    "source_publication_max_bytes": 536_870_912,
                    "process_memory_ceiling_bytes": 4_294_967_296,
                    "history_max_total_bytes": 2 * 1024 * 1024 * 1024,
                    "retained_index_bytes": {
                        "method": "product_retention_receipt_gauge_v1",
                        "scope": "one serving repo/revision pair; exact unique-inode regular-file bytes admitted by each seal; total equals pair; unavailable after daemon restart",
                        "by_seal": {
                            "full": 1_000_000,
                            "delta": 1_100_000,
                            "noop": 1_200_000,
                            "delete": 1_300_000,
                        },
                    },
                    "phase_resources": {name: deepcopy(phase) for name in PHASES},
                }
            ],
        },
    }


def _manifest() -> dict:
    return {
        "kind": "quanta-index-scale-tier-manifest",
        "manifest_schema_version": 2,
        "dimension": "scale",
        "query_token": "scale_needle_token",
        "tiers": [
            {
                "tier": name,
                "repo_count": repos,
                "files_per_repo": per_repo,
                "total_files": repos * per_repo,
                "source_repo_count": repos,
                "default_run": name == "small",
                "selectable": True,
                "avg_file_lines": lines,
                "hit_density_per_mille": hits,
                "symbol_density_per_mille": symbols,
                "serving_owner_count": 1,
                "qualification_owner": "canonical-quiet-host-run",
            }
            for name, repos, per_repo, lines, hits, symbols in (
                ("small", 1, 16, 40, 60, 120),
                ("medium", 4, 64, 80, 40, 100),
                ("large", 16, 256, 120, 25, 80),
                ("xlarge", 64, 512, 200, 15, 60),
            )
        ],
    }


def _native_child(
    path: Path, capture_id: str, *, publication: bool, schema_version: int = 2
) -> tuple[RawFile, dict]:
    header = {
        "record_type": "header",
        "schema_version": schema_version,
        "capture_id": capture_id,
        "stream_id": capture_id,
        "clock_domain": capture_id,
        "process_instance": capture_id,
        "started_ns": 0,
        "max_record_bytes": 4096,
        "max_counters_per_event": 1,
        "coverage": [],
    }
    events = []

    def event(kind: dict) -> None:
        events.append(
            {
                "record_type": "event",
                "output_seq": len(events) + 1,
                "capture_id": capture_id,
                "ts_ns": len(events) + 1,
                "kind": kind,
            }
        )

    event(
        {
            "type": "span_begin",
            "span_id": 1,
            "root_kind": "operation",
            "stage": "operation",
            "lane_id": 1,
        }
    )
    event(
        {
            "type": "span_begin",
            "span_id": 2,
            "parent_span_id": 1,
            "stage": "runtime_boot",
            "lane_id": 1,
        }
    )
    event({"type": "span_end", "span_id": 2, "outcome": "ok"})
    event(
        {
            "type": "span_begin",
            "span_id": 3,
            "root_kind": "request",
            "stage": "request",
            "lane_id": 1,
        }
    )
    event(
        {
            "type": "request_identity",
            "span_id": 3,
            "request_id": 1,
            "plane": "control",
            "connection_id": 1,
        }
    )
    if publication:
        for span_id, stage in ((4, "build_prepare"), (5, "native_publish")):
            event(
                {
                    "type": "span_begin",
                    "span_id": span_id,
                    "parent_span_id": 3,
                    "stage": stage,
                    "lane_id": 1,
                }
            )
            event({"type": "span_end", "span_id": span_id, "outcome": "ok"})
    event({"type": "span_end", "span_id": 3, "outcome": "ok"})
    for key, value in (("sync_calls", 3), ("sync_failures", 0), ("sync_elapsed_ns", 27)):
        event({"type": "counter", "span_id": 1, "key": key, "value": value})
    event({"type": "span_end", "span_id": 1, "outcome": "ok"})
    footer = {
        "record_type": "footer",
        "schema_version": schema_version,
        "capture_id": capture_id,
        "closed_ns": len(events) + 1,
        "status": "complete",
        "close_reason": "normal",
        "last_output_seq": len(events),
        "producer_leases": 0,
        "events_attempted": len(events),
        "events_accepted": len(events),
        "events_dropped": 0,
        "events_written": len(events),
        "events_unwritten": 0,
        "open_spans": 0,
    }
    path.write_bytes(
        b"".join(json.dumps(row).encode() + b"\n" for row in (header, *events, footer))
    )
    path.chmod(0o600)
    return RawFile.capture(path), parse_timeline(path.read_bytes())


def _native_operation_key(sequence: int) -> dict:
    # Serde newtype [u8; 16] writes JSON arrays without a wrapper object.
    return {
        "workspace": {
            "tenant": [1] * 16,
            "collection": [2] * 16,
            "worktree": [3] * 16,
            "slot": [4] * 16,
        },
        "epoch": 1,
        "producer": [5] * 16,
        "stream": [6] * 16,
        "stream_epoch": 1,
        "sequence": sequence,
    }


def _native_product_fixture(
    tmp_path: Path,
    *,
    sampling: bool = False,
    host_os: str = "linux",
    enospc_writes: int = 0,
    timeline_schema_version: int = 2,
) -> tuple[dict, dict, dict]:
    child_dir = tmp_path / "children"
    child_dir.mkdir()
    children = {}
    entries = []
    epochs = {}
    for role, capture_id, publication, epoch in (
        ("first", "1" * 32, True, "drain"),
        ("reopen", "2" * 32, False, "final_drain"),
    ):
        trace_path = child_dir / f"{role}.jsonl"
        trace_file, trace = _native_child(
            trace_path,
            capture_id,
            publication=publication,
            schema_version=timeline_schema_version,
        )
        counts = dict(NATIVE_IO_GOLD)
        counts["failed_writes"] = enospc_writes
        counts["enospc_writes"] = enospc_writes
        line = (
            "native-object-io "
            + " ".join(
                [f"capture_id={capture_id}", "status=quiescent"]
                + [f"{key}={value}" for key, value in sorted(counts.items())]
            )
            + "\n"
        )
        stderr_path = child_dir / f"{role}.stderr"
        stderr_path.write_bytes(line.encode())
        stderr_path.chmod(0o600)
        stderr_file = RawFile.capture(stderr_path)
        metadata = os.stat(stderr_path)
        entries.append(
            {
                "role": role,
                "capture_id": capture_id,
                "file": f"{role}.jsonl",
                "path": str(trace_path),
                "bytes": trace_file.size,
                "sha256": trace_file.sha256.removeprefix("sha256:"),
                "publication_witness_required": publication,
                "stderr_tail": {
                    "path": str(stderr_path),
                    "whole_file_bytes_at_drain": stderr_file.size,
                    "tail_offset": 0,
                    "tail_bytes": stderr_file.size,
                    "tail_sha256": hashlib.sha256(line.encode()).hexdigest(),
                    "file_device": metadata.st_dev,
                    "file_inode": metadata.st_ino,
                },
            }
        )
        epochs[epoch] = {"capture_id": capture_id, "status": "quiescent", **counts}
        children[role] = {"trace_file": trace_file, "trace": trace, "stderr_file": stderr_file}
    workload = {
        "mode": "fixed",
        "work_limits": {
            "reads": 1_000_000,
            "writes": 1_000_000,
            "bytes": 536_870_912,
            "mutations": 1_000_000,
        },
    }
    phases = {
        name: {
            "wall_ms": 1.0,
            "observer_ms": 0.0,
            "completed": True,
            "root_bytes_before": None,
            "root_bytes_after": None,
            "root_net_bytes": None,
        }
        for name in NATIVE_PHASE_GOLD
    }
    publication_phases = (
        "profile_configure",
        "full_event_ack",
        "delta_event_ack",
        "noop_event_ack",
        "delete_event_ack",
    )
    summary = {
        "schema_version": 2,
        "dimension": "scale-index-native",
        "mode": "cold",
        "concurrency": 1,
        "provenance": {
            "git_head": SOURCE,
            "corpus_digest": "sha256:" + "c" * 64,
            "config_digest": "sha256:" + "d" * 64,
        },
        "detail": {
            "backend": "index-native-configured-daemon-uds",
            "scope": "index-only-source-event-to-ACK-to-searchability",
            "tier": "small",
            "seed": 7,
            "file_count": 16,
            "source_bytes": 4096,
            "daemon_sha256": "e" * 64,
            "native_config_sha256": "f" * 64,
            "foreground_workload": workload,
            "child_trace_verified": True,
            "phases": phases,
            "total_ms": 30.0,
            "publication_outcomes": [
                {
                    "phase": phase,
                    "cycle": None,
                    "submit_count": 1,
                    "completion": "immediate_ack",
                    "submit_outcome": "terminal_published",
                    "status_checks": 0,
                    "last_status": None,
                    "operation_key": _native_operation_key(ordinal + 1),
                }
                for ordinal, phase in enumerate(publication_phases)
            ],
            "child_trace_files": {"directory": str(child_dir), "captures": entries},
            "child_object_io": {"epochs": epochs},
            "finite_churn": None,
            "mixed_load": None,
        },
    }
    if sampling:
        summary["host"] = {"os": host_os}
        detail = summary["detail"]
        detail["native_resource_observation"] = _native_resource_fixture(host_os)
    return summary, children, workload


def _native_resource_fixture(host_os: str) -> dict:
    def probe(method: str, interval: int, *, bytes_observed: int | None) -> dict:
        available = bytes_observed is not None
        return {
            "method": method,
            "sample_interval_ms": interval,
            "first_bytes": bytes_observed,
            "last_bytes": bytes_observed,
            "sampled_max_bytes": bytes_observed,
            "sample_count": 2 if available else 0,
            "missing_count": 0 if available else 2,
            "last_missing_reason": None if available else "unsupported on this platform",
            "max_observed_gap_ms": 1.0,
            "observer_wall_ms": 1.0,
        }

    linux = host_os == "linux"
    process_io = {
        "method": "proc_pid_io" if linux else "unsupported",
        "scope": "unreaped_child_after_socket_ready_until_shutdown_signal; kernel_process_accounting_not_device_media_bytes",
        "sample_interval_ms": 100,
        "read_bytes": {"first": 10, "last": 20, "delta": 10}
        if linux
        else {
            "first": None,
            "last": None,
            "delta": None,
        },
        "write_bytes": {"first": 30, "last": 50, "delta": 20}
        if linux
        else {
            "first": None,
            "last": None,
            "delta": None,
        },
        "cancelled_write_bytes": {"first": 0, "last": 0, "delta": 0}
        if linux
        else {
            "first": None,
            "last": None,
            "delta": None,
        },
        "first_sample_offset_ms": 0.0 if linux else None,
        "last_sample_offset_ms": 1.0 if linux else None,
        "observed_window_ms": 1.0 if linux else None,
        "sample_count": 2 if linux else 0,
        "missing_count": 0 if linux else 2,
        "last_missing_reason": None if linux else "child process IO unsupported",
        "max_observed_gap_ms": 1.0,
        "observer_wall_ms": 1.0,
        "regression_error": None,
    }
    epochs = []
    for name in ("first_daemon", "reopened_daemon"):
        epochs.append(
            {
                "epoch": name,
                "observed_elapsed_ms": 2.0,
                "child_rss": probe(
                    "proc_pid_status_vmrss" if linux else "ps_o_rss_kib",
                    100,
                    bytes_observed=4096,
                ),
                "aggregate_process_rss": probe(
                    "proc_stat_identity_and_status_vmrss_sum" if linux else "unsupported",
                    100,
                    bytes_observed=8192 if linux else None,
                ),
                "allocated_fixture_root": probe(
                    "unique_inode_st_blocks_x_512",
                    500,
                    bytes_observed=12288,
                ),
                "allocated_state_root": probe(
                    "unique_inode_st_blocks_x_512",
                    500,
                    bytes_observed=8192,
                ),
                "allocated_native_scratch": probe(
                    "unique_inode_st_blocks_x_512",
                    500,
                    bytes_observed=0,
                ),
                "child_process_io": deepcopy(process_io),
                "worker_failure": None,
            }
        )
    return {
        "mode": "opt_in_diagnostic_v2",
        "scope": "child_daemon_after_socket_ready_until_shutdown_signal; startup and drain completion excluded",
        "sampling": "periodic and boundary observations; sampled maximum is not a continuous peak",
        "observer_cost_scope": "probe and traversal wall time; scheduling and aggregation excluded",
        "stop_limit": "cooperative between filesystem entries; an individual metadata syscall may block",
        "continuous_peak": False,
        "sampled_only": True,
        "aggregate_rss_scope": "harness and daemon plus observed descendants, including previously seen reparented children; per-process RSS sum may double-count shared pages",
        "aggregate_rss_census": "non-atomic, bounded; processes entirely between observations may be missed; numeric PID reuse is excluded by start-time identity",
        "aggregate_rss_max_processes": 4096,
        "disk_observer_cost_scope": "single fixture traversal charged to allocated_fixture_root; state and scratch are subsets with zero additional traversal cost",
        "fixture_scope": "owned native temporary fixture including state, scratch, config and in-fixture diagnostics; external retained JSONL/stderr and other temporary roots excluded",
        "scratch_scope": "state_root/native-scratch-v1 allocated blocks; linked objects and other temporary roots excluded",
        "epochs": epochs,
        "unavailable": [],
    }


def _native_validate(
    summary: dict,
    children: dict,
    workload: dict,
    *,
    expected_churn_cycles: int = 0,
    expected_resource_sampling: bool = False,
    expected_trace_max_output_bytes: int | None = None,
) -> dict:
    return validate_native_product_artifacts(
        canonical(summary),
        canonical(_manifest()),
        source_revision=SOURCE,
        expected_tier="small",
        expected_seed=7,
        expected_daemon_sha256="e" * 64,
        expected_foreground_workload=workload,
        expected_churn_cycles=expected_churn_cycles,
        expected_trace_max_output_bytes=expected_trace_max_output_bytes,
        expected_resource_sampling=expected_resource_sampling,
        child_records=children,
    )


def _native_off_fixture(tmp_path: Path) -> tuple[dict, dict]:
    summary, children, workload = _native_product_fixture(tmp_path, sampling=True)
    for role in ("first", "reopen"):
        children[role]["trace_file"].path.unlink()
        children[role]["stderr_file"].path.unlink()
    (tmp_path / "children").rmdir()
    detail = summary["detail"]
    detail["child_trace_verified"] = False
    detail["child_trace_files"] = None
    detail["child_object_io"] = None
    return summary, workload


def _native_validate_off(summary: dict, workload: dict) -> dict:
    return validate_native_product_artifacts(
        canonical(summary),
        canonical(_manifest()),
        source_revision=SOURCE,
        expected_tier="small",
        expected_seed=7,
        expected_daemon_sha256="e" * 64,
        expected_foreground_workload=workload,
        expected_churn_cycles=0,
        expected_trace_mode="off",
        expected_resource_sampling=True,
        child_records={},
    )


def test_native_ordinary_off_keeps_fixed_product_and_cold_reopen_gold(tmp_path: Path) -> None:
    summary, workload = _native_off_fixture(tmp_path)
    result = _native_validate_off(summary, workload)
    assert result["native_trace_mode"] == "off"
    assert result["child_capture_status"] == "off_no_canonical_capture"
    assert result["children"] == {}
    assert set(result["phases"]) == set(NATIVE_PHASE_GOLD)
    assert [
        row["operation_key"]["sequence"]
        for row in result["summary"]["detail"]["publication_outcomes"]
    ] == [1, 2, 3, 4, 5]


@pytest.mark.parametrize(
    "fault",
    (
        "verified_flag",
        "child_custody",
        "child_object_io",
        "child_records",
        "partial_publication",
        "cold_reopen",
        "wrong_daemon",
        "wrong_source",
        "mixed_claim",
    ),
)
def test_native_ordinary_off_refuses_trace_forgeries_and_partial_product(
    tmp_path: Path, fault: str
) -> None:
    summary, workload = _native_off_fixture(tmp_path)
    if fault == "verified_flag":
        summary["detail"]["child_trace_verified"] = True
    elif fault == "child_custody":
        summary["detail"]["child_trace_files"] = {"captures": []}
    elif fault == "child_object_io":
        summary["detail"]["child_object_io"] = {"epochs": {}}
    elif fault == "child_records":
        children = {"first": object()}
    elif fault == "partial_publication":
        summary["detail"]["publication_outcomes"].pop()
    elif fault == "cold_reopen":
        summary["detail"]["phases"]["reopen"]["completed"] = False
    elif fault == "wrong_daemon":
        summary["detail"]["daemon_sha256"] = "0" * 64
    elif fault == "wrong_source":
        summary["provenance"]["git_head"] = "0" * 40
    else:
        summary["detail"]["mixed_load"] = {"forged": True}
    if fault != "child_records":
        with pytest.raises(ValueError):
            _native_validate_off(summary, workload)
    else:
        with pytest.raises(ValueError, match="Off must have no child capture"):
            validate_native_product_artifacts(
                canonical(summary),
                canonical(_manifest()),
                source_revision=SOURCE,
                expected_tier="small",
                expected_seed=7,
                expected_daemon_sha256="e" * 64,
                expected_foreground_workload=workload,
                expected_churn_cycles=0,
                expected_trace_mode="off",
                expected_resource_sampling=True,
                child_records=children,
            )


def _native_mixed_fixture(tmp_path: Path, *, churn_cycles: int = 0) -> tuple[dict, dict]:
    if churn_cycles == 2:
        summary, children, _fixed = _native_two_cycle_fixture(tmp_path)
        summary["host"] = {"os": "linux"}
        summary["detail"]["native_resource_observation"] = _native_resource_fixture("linux")
    elif churn_cycles == 0:
        summary, children, _fixed = _native_product_fixture(tmp_path, sampling=True)
    else:
        raise ValueError("mixed fixture supports only zero or two churn cycles")
    for role in ("first", "reopen"):
        children[role]["trace_file"].path.unlink()
        children[role]["stderr_file"].path.unlink()
    (tmp_path / "children").rmdir()
    detail = summary["detail"]
    detail["tier"] = "large"
    detail["seed"] = 42
    detail["file_count"] = 4096
    detail["source_bytes"] = 1_048_576
    detail["child_trace_verified"] = False
    detail["child_trace_files"] = None
    detail["child_object_io"] = None
    detail["total_ms"] = 970_000.0
    detail["phases"]["reopen_churn_search"] = {
        "wall_ms": 1.0,
        "observer_ms": 0.0,
        "completed": True,
        "root_bytes_before": None,
        "root_bytes_after": None,
        "root_net_bytes": None,
    }
    detail["native_resource_observation"]["epochs"][0]["observed_elapsed_ms"] = 965_000.0
    workload = {
        "mode": "explicit",
        "work_limits": {
            "reads": 67_108_864,
            "writes": 67_108_864,
            "bytes": 549_755_813_888,
            "mutations": 67_108_864,
        },
    }
    detail["foreground_workload"] = deepcopy(workload)

    def window(name: str, seconds: int, first_sequence: int) -> dict:
        classes = {}
        for label, per_second in (("unique", 35), ("broad_top20", 10), ("miss", 5)):
            count = seconds * per_second
            classes[label] = {
                "success": count,
                "failed": 0,
                "latency_sample_count": count,
                "p50_ms": 1.0,
                "p95_ms": 2.0,
                "p99_ms": 3.0,
            }
        records = []
        for offset in range(seconds):
            sequence = first_sequence + offset
            records.append(
                {
                    "operation_sequence": sequence,
                    "status": "published",
                    "audit": [
                        {
                            "phase": "mixed_load",
                            "cycle": None,
                            "operation_key": _native_operation_key(sequence),
                            "submit_count": 1,
                            "submit_outcome": "terminal_published",
                            "status_checks": 0,
                            "last_status": None,
                            "completion": "immediate_ack",
                        }
                    ],
                    "timing": {
                        "input_ready_from_scheduled_ms": 10.0,
                        "ack_from_scheduled_ms": 20.0,
                        "current_gold_from_scheduled_ms": 30.0 + offset,
                        "old_view_witness_ms": 5.0 + offset,
                    },
                }
            )
        return {
            "name": name,
            "queries": {
                "window_seconds": seconds,
                "offered": seconds * 50,
                "dropped_queue_full": 0,
                "dropped_scheduler_late": 0,
                "drain_secs": float(seconds) + 0.5,
                "max_queue_ms": 1.0,
                "classes": classes,
                "first_query_error": None,
                "latency_scope": (
                    "scheduled arrival to decoded and validated reply, includes failed completions; "
                    "drops have no latency and are separate failures"
                ),
                "qualification": "observation_only",
            },
            "publisher": {
                "scheduled": seconds,
                "attempted": seconds,
                "acked": seconds,
                "published": seconds,
                "missed_late": 0,
                "last_operation_sequence": first_sequence + seconds - 1,
                "last_error": None,
                "publication_records": records,
                "current_gold_from_scheduled_p95_ms": 86.0 if name == "warmup" else 884.0,
                "current_gold_from_scheduled_p99_ms": 89.0 if name == "warmup" else 920.0,
                "old_view_witness_p95_ms": 61.0 if name == "warmup" else 859.0,
                "old_view_witness_p99_ms": 64.0 if name == "warmup" else 895.0,
            },
        }

    detail["mixed_load"] = {
        "scope": (
            "Native UDS mixed offered load with product retained-read witness; "
            "sampled resource observations are not continuous peaks"
        ),
        "warmup": window("warmup", 60, 6 + churn_cycles),
        "measure": window("measure", 900, 66 + churn_cycles),
        "retained_read": {
            "scope": (
                "product ReadSession Exact; monotonic 600-second held ticket and bounded "
                "settlement, not continuous resource peak"
            ),
            "lease_millis": 300_000,
            "renew_period_ms": 120_000,
            "max_renew_gap_ms": 180_000,
            "initial_view_sequence": 5 + churn_cycles,
            "initial_revision": 1,
            "final_revision": 5,
            "initial_expiry_ms": 1_000_300_000,
            "renewals_ms": [float(step * 120_000) for step in range(1, 5)],
            "renewal_expiries_ms": [1_000_300_000 + step * 120_000 for step in range(1, 5)],
            "held_queries": 6,
            "last_held_query_ms": 600_000.0,
            "first_release_attempt_ms": 600_000.0,
            "confirmed_release_ms": 600_010.0,
            "confirmed_last_pin": True,
            "settled_ms": 602_000.0,
            "settlement_polls": 3,
            "last_settlement_state": "SettledAtSnapshot",
            "settlement_head_sequence": 606 + churn_cycles,
            "settlement_exact_floor": 6 + churn_cycles,
            "last_status": "Released",
        },
    }
    return summary, workload


def _native_validate_mixed(
    summary: dict,
    workload: dict,
    *,
    churn_cycles: int = 0,
    linux_resource_observation: dict | None = None,
) -> dict:
    return validate_native_product_artifacts(
        canonical(summary),
        canonical(_manifest()),
        source_revision=SOURCE,
        expected_tier="large",
        expected_seed=42,
        expected_daemon_sha256="e" * 64,
        expected_foreground_workload=workload,
        expected_churn_cycles=churn_cycles,
        expected_mixed_load=True,
        expected_resource_sampling=True,
        child_records={},
        linux_resource_observation=linux_resource_observation,
    )


def _linux_outer_resource_gold() -> dict:
    # Fixed owner record, independent of the capture-side projection function.
    return {
        "owner": "linux-process-owner-v1",
        "scope": "scale_harness_and_owned_descendants_until_cgroup_empty",
        "cgroup_parent": "/sys/fs/cgroup/benchmark-owner",
        "cgroup_parent_dev": 29,
        "cgroup_parent_ino": 1001,
        "cgroup_path": "/sys/fs/cgroup/benchmark-owner/run-1",
        "root_pid": 321,
        "root_start_ticks": 98765,
        "sample_interval_ms": 50,
        "sample_count": 3,
        "aggregate_rss_sampled_max_bytes": 4096,
        "aggregate_rss_method": "sampled_smaps_rollup_rss_sum_shared_pages_may_repeat",
        "cgroup_memory_peak_bytes": 8192,
        "cgroup_memory_method": "cgroup_v2_memory.peak_kernel_accounted_memory_not_rss",
        "cgroup_cpu_usage_ns": 300,
        "ownership_complete": True,
        "migration_caveat": "unobserved_cgroup_migration_requires_external_prevention",
        "continuous_rss_peak": False,
        "allocated_disk_peak": None,
    }


@pytest.mark.parametrize("outer_rss", (0, 2 * 1024**3))
def test_native_mixed_outer_linux_rss_at_or_under_cap_retains_custody(
    tmp_path: Path, outer_rss: int
) -> None:
    summary, workload = _native_mixed_fixture(tmp_path)
    outer = _linux_outer_resource_gold()
    outer["aggregate_rss_sampled_max_bytes"] = outer_rss
    outer["cgroup_memory_peak_bytes"] = 3 * 1024**3
    result = _native_validate_mixed(summary, workload, linux_resource_observation=outer)
    assert result["linux_resource_observation"] == outer
    assert result["linux_resource_observation"] is not outer
    outer["sample_count"] = 999
    assert result["linux_resource_observation"]["sample_count"] == 3
    inner_rss = result["native_resource_observation"]["epochs"][0]["aggregate_process_rss"]
    assert inner_rss["sampled_max_bytes"] < 2 * 1024**3


def test_native_mixed_outer_linux_rss_over_cap_refuses_even_if_inner_under(
    tmp_path: Path,
) -> None:
    summary, workload = _native_mixed_fixture(tmp_path)
    outer = _linux_outer_resource_gold()
    outer["aggregate_rss_sampled_max_bytes"] = 2 * 1024**3 + 1
    inner_rss = summary["detail"]["native_resource_observation"]["epochs"][0][
        "aggregate_process_rss"
    ]
    assert inner_rss["sampled_max_bytes"] < 2 * 1024**3
    with pytest.raises(ValueError, match="outer Linux sampled aggregate RSS exceeds"):
        _native_validate_mixed(summary, workload, linux_resource_observation=outer)


@pytest.mark.parametrize(
    ("field", "bad"),
    (
        ("owner", "other-owner"),
        ("scope", "daemon-only"),
        ("aggregate_rss_method", "continuous_rss_peak"),
        ("aggregate_rss_method", "sampled_process_rss_sum_shared_pages_may_repeat"),
        ("cgroup_memory_method", "rss"),
        ("migration_caveat", "none"),
        ("ownership_complete", False),
        ("continuous_rss_peak", True),
        ("allocated_disk_peak", 0),
        ("cgroup_parent", "relative/parent"),
        ("cgroup_path", "/sys/fs/cgroup/other-owner/run-1"),
        ("cgroup_path", "/sys/fs/cgroup/benchmark-owner/nested/run-1"),
        ("cgroup_path", "/sys/fs/cgroup/benchmark-owner/../other/run-1"),
        ("root_pid", 0),
        ("root_start_ticks", 0),
        ("sample_interval_ms", 0),
        ("sample_count", 0),
        ("sample_count", True),
        ("aggregate_rss_sampled_max_bytes", -1),
        ("aggregate_rss_sampled_max_bytes", True),
        ("cgroup_memory_peak_bytes", -1),
        ("cgroup_cpu_usage_ns", False),
    ),
)
def test_native_outer_linux_resource_refuses_malformed_owner_record(
    tmp_path: Path, field: str, bad: object
) -> None:
    summary, workload = _native_mixed_fixture(tmp_path)
    outer = _linux_outer_resource_gold()
    outer[field] = bad
    with pytest.raises(ValueError, match="outer Linux"):
        _native_validate_mixed(summary, workload, linux_resource_observation=outer)


@pytest.mark.parametrize("extra", ("absent", "forged"))
def test_native_outer_linux_resource_requires_exact_record_shape(
    tmp_path: Path, extra: str
) -> None:
    summary, workload = _native_mixed_fixture(tmp_path)
    outer = _linux_outer_resource_gold()
    if extra == "absent":
        del outer["aggregate_rss_sampled_max_bytes"]
    else:
        outer["rss_peak_qualified"] = True
    with pytest.raises(ValueError, match="outer Linux"):
        _native_validate_mixed(summary, workload, linux_resource_observation=outer)


def test_native_ordinary_outer_linux_rss_does_not_take_mixed_ceiling(tmp_path: Path) -> None:
    summary, workload = _native_off_fixture(tmp_path)
    outer = _linux_outer_resource_gold()
    outer["aggregate_rss_sampled_max_bytes"] = 3 * 1024**3
    result = validate_native_product_artifacts(
        canonical(summary),
        canonical(_manifest()),
        source_revision=SOURCE,
        expected_tier="small",
        expected_seed=7,
        expected_daemon_sha256="e" * 64,
        expected_foreground_workload=workload,
        expected_churn_cycles=0,
        expected_trace_mode="off",
        expected_resource_sampling=True,
        child_records={},
        linux_resource_observation=outer,
    )
    assert result["linux_resource_observation"]["aggregate_rss_sampled_max_bytes"] == 3 * 1024**3


@pytest.mark.parametrize("scope", ("base", "mixed"))
@pytest.mark.parametrize("submit_outcome", ("accepted", "unknown_response", "unknown_error"))
def test_native_reconciled_status_binds_original_key_for_actual_submit_outcomes(
    tmp_path: Path, scope: str, submit_outcome: str
) -> None:
    if scope == "base":
        summary, children, workload = _native_product_fixture(tmp_path)
        audit = summary["detail"]["publication_outcomes"][0]
    else:
        summary, workload = _native_mixed_fixture(tmp_path)
        audit = summary["detail"]["mixed_load"]["warmup"]["publisher"]["publication_records"][0][
            "audit"
        ][0]
    audit.update(
        completion="reconciled_status",
        submit_outcome=submit_outcome,
        status_checks=1,
        last_status="published",
    )
    result = (
        _native_validate(summary, children, workload)
        if scope == "base"
        else _native_validate_mixed(summary, workload)
    )
    assert result["summary"]["detail"]["publication_outcomes"][0]["operation_key"]["sequence"] == 1
    assert audit["operation_key"]["sequence"] == (1 if scope == "base" else 6)


@pytest.mark.parametrize("scope", ("base", "mixed"))
@pytest.mark.parametrize("obsolete", ("unknown", "pending"))
def test_native_reconciled_status_refuses_nonproducer_submit_label(
    tmp_path: Path, scope: str, obsolete: str
) -> None:
    if scope == "base":
        summary, children, workload = _native_product_fixture(tmp_path)
        audit = summary["detail"]["publication_outcomes"][0]
    else:
        summary, workload = _native_mixed_fixture(tmp_path)
        audit = summary["detail"]["mixed_load"]["warmup"]["publisher"]["publication_records"][0][
            "audit"
        ][0]
    audit.update(
        completion="reconciled_status",
        submit_outcome=obsolete,
        status_checks=1,
        last_status="published",
    )
    with pytest.raises(ValueError, match="reconciled ACK"):
        if scope == "base":
            _native_validate(summary, children, workload)
        else:
            _native_validate_mixed(summary, workload)


def test_native_mixed_off_product_binds_fixed_offered_load_without_child_capture(
    tmp_path: Path,
) -> None:
    summary, workload = _native_mixed_fixture(tmp_path)
    result = _native_validate_mixed(summary, workload)
    assert not (tmp_path / "children").exists()
    assert result["child_capture_status"] == "off_no_canonical_capture"
    assert result["children"] == {}
    assert result["native_mixed_load"]["warmup"]["queries"]["offered"] == 3_000
    assert result["native_mixed_load"]["measure"]["queries"]["offered"] == 45_000
    assert result["native_mixed_load"]["measure"]["publisher"]["last_operation_sequence"] == 965
    assert result["native_mixed_load"]["retained_read"]["held_queries"] == 6
    assert result["native_resource_observation"]["epochs"][0]["observed_elapsed_ms"] == 965_000.0
    assert (
        result["native_mixed_load"]["warmup"]["publisher"]["current_gold_from_scheduled_p95_ms"]
        == 86.0
    )
    assert (
        result["native_mixed_load"]["warmup"]["publisher"]["current_gold_from_scheduled_p99_ms"]
        == 89.0
    )
    assert (
        result["native_mixed_load"]["measure"]["publisher"]["current_gold_from_scheduled_p95_ms"]
        == 884.0
    )
    assert (
        result["native_mixed_load"]["measure"]["publisher"]["current_gold_from_scheduled_p99_ms"]
        == 920.0
    )


def test_native_mixed_off_after_two_churn_cycles_binds_window_sequence(tmp_path: Path) -> None:
    summary, workload = _native_mixed_fixture(tmp_path, churn_cycles=2)
    result = _native_validate_mixed(summary, workload, churn_cycles=2)
    mixed = result["native_mixed_load"]
    assert [
        row["operation_key"]["sequence"]
        for row in result["summary"]["detail"]["publication_outcomes"]
    ] == [1, 2, 3, 4, 5, 6, 7]
    assert mixed["warmup"]["publisher"]["publication_records"][0]["operation_sequence"] == 8
    assert mixed["measure"]["publisher"]["publication_records"][0]["operation_sequence"] == 68
    assert mixed["measure"]["publisher"]["last_operation_sequence"] == 967

    mixed_input = summary["detail"]["mixed_load"]
    mixed_input["measure"]["publisher"]["publication_records"][0]["operation_sequence"] = 67
    with pytest.raises(ValueError):
        _native_validate_mixed(summary, workload, churn_cycles=2)


@pytest.mark.parametrize(
    "fault",
    (
        "missing",
        "old_scope",
        "wrong_view",
        "renew_gap",
        "short_hold",
        "wrong_query_count",
        "wrong_revision",
        "stale_expiry",
        "unknown_release",
        "not_last_pin",
        "incomplete_settlement",
        "late_settlement",
        "unsettled_floor",
        "forged_head",
        "bool_count",
    ),
)
def test_native_mixed_retained_read_requires_actual_bound_lifecycle(
    tmp_path: Path, fault: str
) -> None:
    summary, workload = _native_mixed_fixture(tmp_path)
    mixed = summary["detail"]["mixed_load"]
    held = mixed["retained_read"]
    if fault == "missing":
        del mixed["retained_read"]
    elif fault == "old_scope":
        mixed["scope"] = "Native UDS mixed offered load; no long pin/RSS/debt/release qualification"
    elif fault == "wrong_view":
        held["initial_view_sequence"] = 6
    elif fault == "renew_gap":
        held["renewals_ms"][1] = 301_000.0
    elif fault == "short_hold":
        held["first_release_attempt_ms"] = 599_999.0
    elif fault == "wrong_query_count":
        held["held_queries"] = 5
    elif fault == "wrong_revision":
        held["final_revision"] = 4
    elif fault == "stale_expiry":
        held["renewal_expiries_ms"][1] = held["renewal_expiries_ms"][0]
    elif fault == "unknown_release":
        held["last_status"] = "Unknown"
    elif fault == "not_last_pin":
        held["confirmed_last_pin"] = False
    elif fault == "incomplete_settlement":
        held["last_settlement_state"] = "Incomplete"
    elif fault == "late_settlement":
        held["settled_ms"] = 660_001.0
    elif fault == "unsettled_floor":
        held["settlement_exact_floor"] = 5
    elif fault == "forged_head":
        held["settlement_head_sequence"] = 966
    else:
        held["held_queries"] = True
    with pytest.raises(ValueError):
        _native_validate_mixed(summary, workload)


@pytest.mark.parametrize(
    "fault",
    (
        "empty_mixed",
        "missing_window",
        "bool_offered",
        "query_drop",
        "query_error",
        "wrong_class_ratio",
        "missing_latency_sample",
        "p95_target",
        "p99_target",
        "p99_order",
        "freshness_p95_target",
        "freshness_p99_target",
        "missing_publication",
        "publication_sequence",
        "publication_ack",
        "timing_order",
        "summary_p95",
        "wrong_profile",
        "child_proof",
        "resource_window",
        "duration",
    ),
)
def test_native_mixed_off_product_refuses_partial_or_forged_load(
    tmp_path: Path, fault: str
) -> None:
    summary, workload = _native_mixed_fixture(tmp_path)
    detail = summary["detail"]
    mixed = detail["mixed_load"]
    warmup = mixed["warmup"]
    measure = mixed["measure"]
    if fault == "empty_mixed":
        detail["mixed_load"] = {}
    elif fault == "missing_window":
        del mixed["measure"]
    elif fault == "bool_offered":
        warmup["queries"]["offered"] = True
    elif fault == "query_drop":
        measure["queries"]["dropped_queue_full"] = 1
    elif fault == "query_error":
        measure["queries"]["classes"]["unique"]["failed"] = 1
    elif fault == "wrong_class_ratio":
        measure["queries"]["classes"]["broad_top20"]["success"] = 8_999
    elif fault == "missing_latency_sample":
        measure["queries"]["classes"]["miss"]["latency_sample_count"] = 4_499
    elif fault == "p95_target":
        measure["queries"]["classes"]["unique"].update(p95_ms=101.0, p99_ms=200.0)
    elif fault == "p99_target":
        measure["queries"]["classes"]["unique"]["p99_ms"] = 501.0
    elif fault == "freshness_p95_target":
        for record in measure["publisher"]["publication_records"]:
            record["timing"]["current_gold_from_scheduled_ms"] = 1001.0
        measure["publisher"].update(
            current_gold_from_scheduled_p95_ms=1001.0,
            current_gold_from_scheduled_p99_ms=1001.0,
        )
    elif fault == "freshness_p99_target":
        for record in measure["publisher"]["publication_records"][-10:]:
            record["timing"]["current_gold_from_scheduled_ms"] = 5001.0
        measure["publisher"]["current_gold_from_scheduled_p99_ms"] = 5001.0
    elif fault == "p99_order":
        measure["queries"]["classes"]["unique"]["p99_ms"] = 1.0
    elif fault == "missing_publication":
        measure["publisher"]["publication_records"].pop()
    elif fault == "publication_sequence":
        measure["publisher"]["publication_records"][0]["operation_sequence"] = 65
    elif fault == "publication_ack":
        measure["publisher"]["publication_records"][0]["audit"][0]["submit_count"] = 0
    elif fault == "timing_order":
        measure["publisher"]["publication_records"][0]["timing"]["ack_from_scheduled_ms"] = 40.0
    elif fault == "summary_p95":
        measure["publisher"]["current_gold_from_scheduled_p95_ms"] = 885.0
    elif fault == "wrong_profile":
        workload["work_limits"]["reads"] = 67_108_865
        detail["foreground_workload"] = deepcopy(workload)
    elif fault == "child_proof":
        detail["child_trace_files"] = {"captures": []}
    elif fault == "resource_window":
        detail["native_resource_observation"]["epochs"][0]["observed_elapsed_ms"] = 900_000.0
    else:
        detail["total_ms"] = 900_000.0
    with pytest.raises(ValueError):
        _native_validate_mixed(summary, workload)


@pytest.mark.parametrize("epoch_index", (0, 1))
@pytest.mark.parametrize(
    ("axis", "ceiling"),
    (
        ("aggregate_process_rss", 2_147_483_648),
        ("allocated_native_scratch", 4_294_967_296),
        ("allocated_state_root", 68_719_476_736),
    ),
)
@pytest.mark.parametrize("excess", (0, 4096))
def test_native_mixed_observed_resource_ceiling(
    tmp_path: Path, epoch_index: int, axis: str, ceiling: int, excess: int
) -> None:
    summary, workload = _native_mixed_fixture(tmp_path)
    epoch = summary["detail"]["native_resource_observation"]["epochs"][epoch_index]
    observed = ceiling + excess
    fields = [axis]
    if axis == "allocated_native_scratch":
        fields.extend(("allocated_state_root", "allocated_fixture_root"))
    elif axis == "allocated_state_root":
        fields.append("allocated_fixture_root")
    for field in fields:
        for endpoint in ("first_bytes", "last_bytes", "sampled_max_bytes"):
            epoch[field][endpoint] = observed
    if excess:
        with pytest.raises(
            ValueError,
            match=rf"{epoch['epoch']}\.{axis} sampled bytes exceed declared ceiling",
        ):
            _native_validate_mixed(summary, workload)
    else:
        result = _native_validate_mixed(summary, workload)
        assert result["native_resource_observation"]["sampled_only"] is True
        assert result["native_resource_observation"]["continuous_peak"] is False


def test_native_mixed_unsupported_rss_remains_unobserved(tmp_path: Path) -> None:
    summary, workload = _native_mixed_fixture(tmp_path)
    summary["host"]["os"] = "macos"
    observation = _native_resource_fixture("macos")
    observation["epochs"][0]["observed_elapsed_ms"] = 965_000.0
    summary["detail"]["native_resource_observation"] = observation
    result = _native_validate_mixed(summary, workload)
    for epoch in result["native_resource_observation"]["epochs"]:
        assert epoch["aggregate_process_rss"]["sampled_max_bytes"] is None
    assert result["native_resource_observation"]["continuous_peak"] is False


@pytest.mark.parametrize("host_os", ("linux", "macos"))
def test_native_resource_sampling_preserves_real_and_unsupported_axes(
    tmp_path: Path, host_os: str
) -> None:
    summary, children, workload = _native_product_fixture(tmp_path, sampling=True, host_os=host_os)
    result = _native_validate(summary, children, workload, expected_resource_sampling=True)
    observation = result["native_resource_observation"]
    assert observation["continuous_peak"] is False
    assert [epoch["epoch"] for epoch in observation["epochs"]] == [
        "first_daemon",
        "reopened_daemon",
    ]
    process_io = observation["epochs"][0]["child_process_io"]
    assert process_io["method"] == ("proc_pid_io" if host_os == "linux" else "unsupported")
    if host_os == "macos":
        assert process_io["write_bytes"]["delta"] is None


@pytest.mark.parametrize(
    "fault",
    (
        "missing",
        "epoch",
        "child_no_sample",
        "interval",
        "continuous_peak",
        "worker_failure",
        "io_delta",
        "bool_count",
        "unavailable",
        "unsupported_forged",
        "external_capture_in_census",
        "one_sample_endpoints",
        "two_sample_maximum",
        "rss_zero_first",
        "io_window",
        "io_outside_epoch",
    ),
)
def test_native_resource_sampling_refuses_missing_or_forged_observation(
    tmp_path: Path, fault: str
) -> None:
    host_os = "macos" if fault == "unsupported_forged" else "linux"
    summary, children, workload = _native_product_fixture(tmp_path, sampling=True, host_os=host_os)
    detail = summary["detail"]
    observation = detail["native_resource_observation"]
    epoch = observation["epochs"][0]
    if fault == "missing":
        del detail["native_resource_observation"]
    elif fault == "epoch":
        epoch["epoch"] = "reopened_daemon"
    elif fault == "child_no_sample":
        epoch["child_rss"]["sample_count"] = 0
    elif fault == "interval":
        epoch["allocated_state_root"]["sample_interval_ms"] = 100
    elif fault == "continuous_peak":
        observation["continuous_peak"] = True
    elif fault == "worker_failure":
        epoch["worker_failure"] = "observer failed"
    elif fault == "io_delta":
        epoch["child_process_io"]["write_bytes"]["delta"] = 19
    elif fault == "bool_count":
        epoch["child_rss"]["sample_count"] = True
    elif fault == "unavailable":
        observation["unavailable"].append({"epoch": "reopened_daemon", "reason": "failed"})
    elif fault == "one_sample_endpoints":
        epoch["allocated_native_scratch"]["sample_count"] = 1
        epoch["allocated_native_scratch"]["last_bytes"] = 1
        epoch["allocated_native_scratch"]["sampled_max_bytes"] = 1
    elif fault == "two_sample_maximum":
        epoch["allocated_state_root"]["sampled_max_bytes"] = 8193
    elif fault == "rss_zero_first":
        epoch["child_rss"]["first_bytes"] = 0
    elif fault == "io_window":
        epoch["child_process_io"]["observed_window_ms"] = 1.01
    elif fault == "io_outside_epoch":
        epoch["observed_elapsed_ms"] = 0.5
    elif fault == "external_capture_in_census":
        observation["fixture_scope"] = (
            "owned native temporary fixture including state, scratch, config, stderr and capture files; other temporary roots excluded"
        )
    else:
        epoch["child_process_io"]["write_bytes"]["delta"] = 0
    with pytest.raises(ValueError):
        _native_validate(summary, children, workload, expected_resource_sampling=True)


def _native_two_cycle_fixture(tmp_path: Path) -> tuple[dict, dict, dict]:
    summary, children, workload = _native_product_fixture(tmp_path)
    detail = summary["detail"]
    phase_gold = {
        "wall_ms": 1.0,
        "observer_ms": 0.0,
        "completed": True,
        "root_bytes_before": None,
        "root_bytes_after": None,
        "root_net_bytes": None,
    }
    for phase in ("churn_cycles", "churn_old_view_search", "reopen_churn_search"):
        detail["phases"][phase] = dict(phase_gold)
    for cycle, sequence in ((1, 6), (2, 7)):
        detail["publication_outcomes"].append(
            {
                "phase": "churn_cycles",
                "cycle": cycle,
                "submit_count": 1,
                "completion": "immediate_ack",
                "submit_outcome": "terminal_published",
                "status_checks": 0,
                "last_status": None,
                "operation_key": _native_operation_key(sequence),
            }
        )
    detail["finite_churn"] = {
        "requested_cycles": 2,
        "completed_cycles": 2,
        "last_attempt": 2,
        "ack_wall_ms": 10.0,
        "query_wall_ms": 5.0,
        "max_cycle_wall_ms": 8.0,
        "scope": "finite single-owner CAS churn; not sustained steady-state or P95 qualification",
    }
    return summary, children, workload


def test_native_two_cycle_product_binds_fixed_churn_order(tmp_path: Path) -> None:
    summary, children, workload = _native_two_cycle_fixture(tmp_path)
    result = _native_validate(summary, children, workload, expected_churn_cycles=2)
    assert len(result["phases"]) == 26
    assert [
        row["operation_key"]["sequence"]
        for row in (result["summary"]["detail"]["publication_outcomes"])
    ] == [1, 2, 3, 4, 5, 6, 7]


def test_native_one_cycle_bool_count_does_not_alias_integer_one(tmp_path: Path) -> None:
    summary, children, workload = _native_two_cycle_fixture(tmp_path)
    detail = summary["detail"]
    detail["publication_outcomes"].pop()
    for field in ("requested_cycles", "completed_cycles", "last_attempt"):
        detail["finite_churn"][field] = 1
    assert (
        _native_validate(summary, children, workload, expected_churn_cycles=1)["summary"]["detail"][
            "finite_churn"
        ]["last_attempt"]
        == 1
    )
    detail["finite_churn"]["completed_cycles"] = True
    with pytest.raises(ValueError, match="churn completed_cycles"):
        _native_validate(summary, children, workload, expected_churn_cycles=1)


@pytest.mark.parametrize(
    "fault",
    (
        "cycle_bool",
        "cycle_gap",
        "completed_count",
        "requested_count",
        "publication_order",
        "missing_churn_phase",
        "duplicate_sequence",
        "last_attempt",
        "negative_timing",
        "scope",
    ),
)
def test_native_two_cycle_product_refuses_churn_drift(tmp_path: Path, fault: str) -> None:
    summary, children, workload = _native_two_cycle_fixture(tmp_path)
    detail = summary["detail"]
    rows = detail["publication_outcomes"]
    if fault == "cycle_bool":
        rows[5]["cycle"] = True
    elif fault == "cycle_gap":
        rows[6]["cycle"] = 3
    elif fault == "completed_count":
        detail["finite_churn"]["completed_cycles"] = 1
    elif fault == "requested_count":
        detail["finite_churn"]["requested_cycles"] = 1
    elif fault == "publication_order":
        rows[5], rows[6] = rows[6], rows[5]
    elif fault == "missing_churn_phase":
        del detail["phases"]["churn_old_view_search"]
    elif fault == "last_attempt":
        detail["finite_churn"]["last_attempt"] = 1
    elif fault == "negative_timing":
        detail["finite_churn"]["ack_wall_ms"] = -1.0
    elif fault == "scope":
        detail["finite_churn"]["scope"] = "sustained qualification"
    else:
        rows[6]["operation_key"]["sequence"] = 6
    with pytest.raises(ValueError):
        _native_validate(summary, children, workload, expected_churn_cycles=2)


@pytest.mark.parametrize("enospc_writes", (0, 1))
@pytest.mark.parametrize("timeline_schema_version", (2, 3, 4))
def test_native_product_binds_two_raw_child_captures_and_quiescent_io(
    tmp_path: Path, enospc_writes: int, timeline_schema_version: int
) -> None:
    summary, children, workload = _native_product_fixture(
        tmp_path,
        enospc_writes=enospc_writes,
        timeline_schema_version=timeline_schema_version,
    )
    result = _native_validate(summary, children, workload)
    assert result["backend"] == "index-native"
    assert result["children"]["first"]["object_io"]["written_bytes"] == 4096
    assert result["children"]["first"]["object_io"]["enospc_writes"] == enospc_writes
    assert result["children"]["first"]["sync"] == {
        "sync_calls": 3,
        "sync_failures": 0,
        "sync_elapsed_ns": 27,
    }


@pytest.mark.parametrize("schema_version", (2, 3, 4))
def test_native_child_accepts_legacy_and_current_timeline_versions(
    tmp_path: Path, schema_version: int
) -> None:
    raw, parsed = _native_child(
        tmp_path / "child.jsonl", "1" * 32, publication=True, schema_version=schema_version
    )
    assert _native_child_trace(parsed, raw, publication=True)["capture_id"] == "1" * 32


def test_native_product_rechecks_children_under_explicit_trace_cap(tmp_path: Path) -> None:
    summary, children, workload = _native_product_fixture(tmp_path)
    product = _native_validate(
        summary,
        children,
        workload,
        expected_trace_max_output_bytes=1_073_741_824,
    )
    assert product["trace_max_output_bytes"] == 1_073_741_824
    with pytest.raises(ValueError, match="output cap"):
        _native_validate(
            summary,
            children,
            workload,
            expected_trace_max_output_bytes=1_073_741_825,
        )


def test_native_child_rejects_unsupported_timeline_version(tmp_path: Path) -> None:
    raw, parsed = _native_child(
        tmp_path / "child.jsonl", "1" * 32, publication=True, schema_version=1
    )
    with pytest.raises(ValueError, match="partial, unhealthy or has invalid identity"):
        _native_child_trace(parsed, raw, publication=True)


@pytest.mark.parametrize(
    "fault",
    (
        "daemon",
        "trace_hash",
        "capture_id",
        "stderr_path",
        "counter",
        "child_missing",
        "stderr_tail",
        "phase_missing",
        "funding",
        "nonfinite",
        "epoch_bool",
        "enospc_without_failed_write",
        "failed_writes_exceed_calls",
        "enospc_missing",
        "ack_contradiction",
        "wrong_workspace",
        "wrong_sequence",
    ),
)
def test_native_product_refuses_cross_artifact_contradictions(tmp_path: Path, fault: str) -> None:
    summary, children, workload = _native_product_fixture(tmp_path)
    detail = summary["detail"]
    first = detail["child_trace_files"]["captures"][0]
    if fault == "daemon":
        detail["daemon_sha256"] = "0" * 64
    elif fault == "trace_hash":
        first["sha256"] = "0" * 64
    elif fault == "capture_id":
        first["capture_id"] = "0" * 32
    elif fault == "stderr_path":
        first["stderr_tail"]["path"] = "/wrong/path"
    elif fault == "counter":
        detail["child_object_io"]["epochs"]["drain"]["file_sync_calls"] = 2
    elif fault == "child_missing":
        del children["reopen"]
    elif fault == "stderr_tail":
        first["stderr_tail"]["tail_sha256"] = "0" * 64
    elif fault == "phase_missing":
        del detail["phases"]["full_event_ack"]
    elif fault == "funding":
        detail["foreground_workload"] = deepcopy(workload)
        detail["foreground_workload"]["work_limits"]["reads"] = 2
    elif fault == "epoch_bool":
        detail["child_object_io"]["epochs"]["drain"]["failed_writes"] = False
    elif fault == "enospc_without_failed_write":
        detail["child_object_io"]["epochs"]["drain"]["enospc_writes"] = 1
    elif fault == "failed_writes_exceed_calls":
        detail["child_object_io"]["epochs"]["drain"]["failed_writes"] = 6
    elif fault == "enospc_missing":
        del detail["child_object_io"]["epochs"]["drain"]["enospc_writes"]
    elif fault == "ack_contradiction":
        detail["publication_outcomes"][0]["status_checks"] = 1
    elif fault == "wrong_workspace":
        detail["publication_outcomes"][0]["operation_key"]["workspace"]["tenant"][0] = 9
    elif fault == "wrong_sequence":
        detail["publication_outcomes"][4]["operation_key"]["sequence"] = 4
    else:
        detail["phases"]["full_event_ack"]["wall_ms"] = float("inf")
    with pytest.raises(ValueError):
        _native_validate(summary, children, workload)


@pytest.mark.parametrize(
    ("old", "new"),
    (
        (b"enospc_writes=0", b"enospc_writes=1"),
        (b"failed_writes=0", b"failed_writes=6"),
    ),
)
def test_native_raw_stderr_refuses_impossible_write_failures(
    tmp_path: Path, old: bytes, new: bytes
) -> None:
    summary, children, workload = _native_product_fixture(tmp_path)
    first = children["first"]
    path = first["stderr_file"].path
    original = path.read_bytes()
    assert original.count(old) == 1
    changed = original.replace(old, new)
    path.write_bytes(changed)
    first["stderr_file"] = RawFile.capture(path)
    descriptor = summary["detail"]["child_trace_files"]["captures"][0]["stderr_tail"]
    descriptor["whole_file_bytes_at_drain"] = len(changed)
    descriptor["tail_bytes"] = len(changed)
    descriptor["tail_sha256"] = hashlib.sha256(changed).hexdigest()
    with pytest.raises(ValueError, match="stderr write failure counters"):
        _native_validate(summary, children, workload)


def test_native_stderr_lf_only_refuses_vertical_tab_fake_prefix(tmp_path: Path) -> None:
    summary, children, workload = _native_product_fixture(tmp_path)
    first = children["first"]
    path = first["stderr_file"].path
    original = path.read_bytes()
    spoofed = b"unrelated\x0b" + original
    path.write_bytes(spoofed)
    first["stderr_file"] = RawFile.capture(path)
    descriptor = summary["detail"]["child_trace_files"]["captures"][0]["stderr_tail"]
    descriptor["whole_file_bytes_at_drain"] = len(spoofed)
    descriptor["tail_bytes"] = len(spoofed)
    descriptor["tail_sha256"] = hashlib.sha256(spoofed).hexdigest()
    with pytest.raises(ValueError, match="summary missing"):
        _native_validate(summary, children, workload)


def _replay(
    summary: dict | None = None,
    manifest: dict | None = None,
    *,
    trace: bytes = TRACE,
    expected_tier: str = "large",
    expected_seed: int = 5,
    requested_client_timeout_ms: int | None = None,
    requested_history_max_bytes: int | None = None,
    requested_history_max_total_bytes: int | None = None,
    requested_disk_observation_mode: str | None = None,
    source_revision: str = SOURCE,
) -> dict:
    return replay(
        canonical(_summary() if summary is None else summary),
        trace,
        b"binary",
        canonical(_manifest() if manifest is None else manifest),
        source_revision=source_revision,
        expected_tier=expected_tier,
        expected_seed=expected_seed,
        requested_client_timeout_ms=requested_client_timeout_ms,
        requested_history_max_bytes=requested_history_max_bytes,
        requested_history_max_total_bytes=requested_history_max_total_bytes,
        requested_disk_observation_mode=requested_disk_observation_mode,
    )


def test_profile_binds_full_lifecycle_and_cost_domains() -> None:
    result = _replay()
    assert result["status"] == "diagnostic_unqualified"
    assert set(result["phases"]) == set(PHASES)
    assert result["file_count"] == 4096
    assert result["runtime_config"]["client_request_timeout_ms"] == 600_000
    assert result["runtime_config"]["disk_observation_mode"] == "boundary-only"
    phase = result["phases"]["full_seal"]
    assert phase["sync"]["atomic_file"]["elapsed_ns"] == 400
    assert phase["exact_live_token_scan"]["live_tokens"] == 7
    assert phase["bm25_live_build"][0]["new_census_docs"] == 2
    assert phase["bm25_boundary"]["encode"]["logical_bytes"] == 128
    assert result["phases"]["full_ingest"]["bm25_census_build"]["docs"] == 2
    assert result["phases"]["delta_ingest_seal"]["bm25_live_build"][0]["newly_dead_docs"] == 1
    assert phase["process_write_io"]["write_bytes"] == 4096
    assert phase["sync_call_wall_ratio"] == 0.00004
    assert phase["disk_observation"]["sampled_max_allocated_root_bytes"] is None
    assert (
        phase["disk_observation"]["sampled_high_water_status"]
        == "unavailable_periodic_sampling_disabled"
    )


def test_product_artifacts_validate_without_diagnostic_markers() -> None:
    product = validate_product_artifacts(
        canonical(_summary()),
        canonical(_manifest()),
        source_revision=SOURCE,
        expected_tier="large",
        expected_seed=5,
        requested_client_timeout_ms=None,
        requested_history_max_bytes=None,
    )
    assert product["tier"]["file_count"] == 4096
    assert set(product["phase_resources"]) == set(PHASES)
    assert product["phase_resources"]["full_seal"]["process_write_io"]["write_bytes"] == 4096
    with pytest.raises(ValueError, match="scale artifact is failed, partial"):
        validate_product_artifacts(
            canonical({**_summary(), "detail": {"passed": False}}),
            canonical(_manifest()),
            source_revision=SOURCE,
            expected_tier="large",
            expected_seed=5,
            requested_client_timeout_ms=None,
            requested_history_max_bytes=None,
        )


def test_profile_binds_periodic_disk_mode_and_observed_sample() -> None:
    summary = _summary()
    tier = summary["detail"]["measured_tiers"][0]
    tier["disk_observation_mode"] = "periodic"
    for phase in tier["phase_resources"].values():
        phase["disk_sample_interval_ms"] = 500
        phase["disk"] = {
            "observation_mode": "periodic",
            "sampled_max_allocated_root_bytes": 150,
            "sampled_high_water_status": "observed",
            "sampled": [{"offset_ms": 2.0, "snapshot": _observed_disk_snapshot(150)}],
            "periodic_probe_wall_ms": 1.0,
            "observed_max_gap_ms": 8.0,
            "sampled_max_is_true_peak": False,
            "start": _observed_disk_snapshot(100),
            "end": _observed_disk_snapshot(120),
        }
    profile = _replay(summary, requested_disk_observation_mode="periodic")
    assert profile["runtime_config"]["requested_disk_observation_mode"] == "periodic"
    assert profile["phases"]["full_seal"]["disk_observation"]["sample_count"] == 1
    with pytest.raises(ValueError, match="disk observation mode"):
        _replay(summary)


@pytest.mark.parametrize("forged_gap", [0.0, 9.0, 11.0])
def test_profile_refuses_forged_boundary_disk_gap(forged_gap: float) -> None:
    summary = _summary()
    summary["detail"]["measured_tiers"][0]["phase_resources"]["full_seal"]["disk"][
        "observed_max_gap_ms"
    ] = forged_gap
    with pytest.raises(ValueError, match="disk sample gap"):
        _replay(summary)


@pytest.mark.parametrize("stage", ["start", "end"])
def test_profile_requires_both_disk_boundary_snapshots(stage: str) -> None:
    summary = _summary()
    summary["detail"]["measured_tiers"][0]["phase_resources"]["full_seal"]["disk"].pop(stage)
    with pytest.raises(ValueError, match="disk boundary"):
        _replay(summary)


@pytest.mark.parametrize("stage", ["start", "end"])
@pytest.mark.parametrize(
    "mutation",
    [
        lambda snapshot: snapshot.pop("status"),
        lambda snapshot: snapshot.update(allocated_root_bytes=None),
        lambda snapshot: snapshot.update(allocated_root_bytes=False),
        lambda snapshot: snapshot.update(filesystem_free_bytes=-1),
        lambda snapshot: snapshot.update(filesystem_available_bytes=1 << 64),
        lambda snapshot: snapshot.update(unavailable_reason="probe_unavailable: raced rename"),
        lambda snapshot: snapshot.update(status="unavailable", unavailable_reason=""),
        lambda snapshot: snapshot.update(
            status="unavailable", unavailable_reason="probe_unavailable: raced rename"
        ),
    ],
)
def test_profile_refuses_forged_disk_boundary_census(stage: str, mutation) -> None:
    summary = _summary()
    mutation(summary["detail"]["measured_tiers"][0]["phase_resources"]["full_seal"]["disk"][stage])
    with pytest.raises(ValueError, match="disk boundary"):
        _replay(summary)


def test_profile_preserves_explicitly_unavailable_disk_boundary() -> None:
    summary = _summary()
    unavailable = {
        "status": "unavailable",
        "allocated_root_bytes": None,
        "filesystem_free_bytes": None,
        "filesystem_available_bytes": None,
        "unavailable_reason": "probe_unavailable: raced rename",
    }
    summary["detail"]["measured_tiers"][0]["phase_resources"]["full_seal"]["disk"]["start"] = (
        unavailable
    )
    disk = _replay(summary)["phases"]["full_seal"]["disk_observation"]
    assert disk["start"] == unavailable
    assert disk["end"] == _observed_disk_snapshot(120)
    assert disk["sampled_max_allocated_root_bytes"] is None


@pytest.mark.parametrize(
    "mutation",
    [
        lambda row: row.pop("disk_observation_mode"),
        lambda row: row["phase_resources"]["full_seal"]["disk"].update(observation_mode="periodic"),
        lambda row: row["phase_resources"]["full_seal"].update(disk_sample_interval_ms=500),
        lambda row: row["phase_resources"]["full_seal"]["disk"].update(
            sampled_max_allocated_root_bytes=150
        ),
        lambda row: row["phase_resources"]["full_seal"]["disk"].update(
            sampled_high_water_status="observed"
        ),
        lambda row: row["phase_resources"]["full_seal"]["disk"].update(
            periodic_probe_wall_ms=False
        ),
    ],
)
def test_profile_refuses_unbound_or_forged_boundary_disk_evidence(mutation) -> None:
    summary = _summary()
    mutation(summary["detail"]["measured_tiers"][0])
    with pytest.raises(ValueError):
        _replay(summary)


def test_profile_counts_file_authority_sync_barriers_exactly() -> None:
    trace = TRACE.replace(
        b"QI_CAUSAL_V1 kind=phase_end name=full_seal ok=1\n",
        b"QI_CAUSAL_V1 kind=sync label=file_authority_object ok=1 elapsed_ns=11\n"
        b"QI_CAUSAL_V1 kind=sync label=file_authority_directory ok=1 elapsed_ns=13\n"
        b"QI_CAUSAL_V1 kind=sync label=file_authority_directory ok=1 elapsed_ns=17\n"
        b"QI_CAUSAL_V1 kind=phase_end name=full_seal ok=1\n",
    )
    phase = _replay(trace=trace)["phases"]["full_seal"]
    assert phase["sync"]["file_authority_object"] == {
        "calls": 1,
        "failed_calls": 0,
        "elapsed_ns": 11,
    }
    assert phase["sync"]["file_authority_directory"] == {
        "calls": 2,
        "failed_calls": 0,
        "elapsed_ns": 30,
    }
    assert phase["sync_call_elapsed_ns_sum"] == 441


def test_profile_distinguishes_pool_posix_full_and_control_sync() -> None:
    trace = TRACE.replace(
        b"QI_CAUSAL_V1 kind=phase_end name=full_seal ok=1\n",
        b"QI_CAUSAL_V1 kind=sync label=pool_data_file_posix ok=1 elapsed_ns=11\n"
        b"QI_CAUSAL_V1 kind=sync label=pool_data_shard_posix ok=1 elapsed_ns=13\n"
        b"QI_CAUSAL_V1 kind=sync label=pool_adoption_full_barrier ok=1 elapsed_ns=17\n"
        b"QI_CAUSAL_V1 kind=sync label=pool_control_file_full ok=1 elapsed_ns=19\n"
        b"QI_CAUSAL_V1 kind=phase_end name=full_seal ok=1\n",
    )
    phase = _replay(trace=trace)["phases"]["full_seal"]
    for label, elapsed in (
        ("pool_data_file_posix", 11),
        ("pool_data_shard_posix", 13),
        ("pool_adoption_full_barrier", 17),
        ("pool_control_file_full", 19),
    ):
        assert phase["sync"][label] == {
            "calls": 1,
            "failed_calls": 0,
            "elapsed_ns": elapsed,
        }
    assert phase["sync_call_elapsed_ns_sum"] == 460


@pytest.mark.parametrize("label", ["file_authority_object", "file_authority_directory"])
def test_profile_refuses_failed_file_authority_sync(label: str) -> None:
    trace = TRACE.replace(
        b"QI_CAUSAL_V1 kind=phase_end name=full_seal ok=1\n",
        f"QI_CAUSAL_V1 kind=sync label={label} ok=0 elapsed_ns=11\n".encode()
        + b"QI_CAUSAL_V1 kind=phase_end name=full_seal ok=1\n",
    )
    with pytest.raises(ValueError, match="failed lexical sync"):
        _replay(trace=trace)


def test_profile_refuses_unknown_file_authority_sync_label() -> None:
    trace = TRACE.replace(b"label=atomic_file", b"label=file_authority_unknown")
    with pytest.raises(ValueError, match="invalid lexical sync marker"):
        _replay(trace=trace)


@pytest.mark.parametrize(
    "mutant",
    [
        SINGLE_TRACE.replace(b"name=full_seal ok=1", b"name=noop_seal ok=1"),
        SINGLE_TRACE + b"QI_CAUSAL_V1 kind=phase_start name=full_seal\n",
        SINGLE_TRACE.replace(b"label=atomic_file", b"label=unknown"),
        SINGLE_TRACE.replace(
            b"QI_CAUSAL_V1 kind=sync label=atomic_file ok=1 elapsed_ns=400\n", b""
        ),
        SINGLE_TRACE.replace(b"postings=4", b"postings=2"),
        SINGLE_TRACE.replace(b"ok=1 elapsed_ns=400", b"ok=0 elapsed_ns=400"),
    ],
)
def test_profile_rejects_unpaired_unknown_or_inconsistent_trace(mutant: bytes) -> None:
    with pytest.raises(ValueError):
        parse_trace(mutant, {"full_seal"})


@pytest.mark.parametrize(
    "key,value",
    [
        ("tier", "medium"),
        ("seed", 6),
        ("seed", True),
        ("seed", 5.0),
        ("file_count", 4095),
        ("file_count", 4096.0),
        ("source_repo_count", False),
        ("source_repo_count", 15),
        ("client_request_timeout_ms", 300_000),
        ("client_request_timeout_ms", 600_000.0),
        ("query_dispatch_budget_ms", 20_000),
        ("query_dispatch_budget_ms", 600_000.0),
        ("query_dispatch_budget_ms", None),
        ("query_dispatch_budget_ms", True),
        ("requested_client_request_timeout_ms", 600_000),
        ("requested_history_max_bytes", 16 * 1024 * 1024),
        ("requested_history_max_total_bytes", 256 * 1024 * 1024),
        ("history_policy_id", "explicit-pair-total-diagnostic-v1"),
        ("history_max_bytes", True),
        ("history_max_generations", 3),
        ("ingest_max_records", 100_001),
        ("ingest_max_text_bytes", 134_217_729),
        ("ingest_max_vector_bytes", 268_435_455),
        ("coverage_max_encoded_bytes", 67_108_865),
        ("coverage_max_decode_heap_bytes", 268_435_457),
        ("source_publication_part_bytes", 1_048_577),
        ("source_publication_max_bytes", 536_870_913),
        ("process_memory_ceiling_bytes", 2_147_483_648),
    ],
)
def test_profile_refuses_wrong_measured_input_or_policy(key: str, value: object) -> None:
    summary = _summary()
    summary["detail"]["measured_tiers"][0][key] = value
    with pytest.raises(ValueError):
        _replay(summary)


def test_profile_refuses_unrecorded_query_dispatch_budget() -> None:
    summary = _summary()
    del summary["detail"]["measured_tiers"][0]["query_dispatch_budget_ms"]
    with pytest.raises(ValueError, match="query_dispatch_budget_ms"):
        _replay(summary)


@pytest.mark.parametrize(
    "mutation",
    [
        lambda row: row.pop("retained_index_bytes"),
        lambda row: row["retained_index_bytes"]["by_seal"].pop("delta"),
        lambda row: row["retained_index_bytes"]["by_seal"].update(extra=1),
        lambda row: row["retained_index_bytes"]["by_seal"].update(full=True),
        lambda row: row["retained_index_bytes"]["by_seal"].update(full=1.5),
        lambda row: row["retained_index_bytes"]["by_seal"].update(full=0),
        lambda row: row["retained_index_bytes"]["by_seal"].update(full=(1 << 53) + 1),
        lambda row: row["retained_index_bytes"]["by_seal"].update(full=1_073_741_825),
        lambda row: row["retained_index_bytes"].update(method="sampled_st_blocks"),
    ],
)
def test_profile_refuses_unbound_retention_bytes(mutation) -> None:
    summary = _summary()
    mutation(summary["detail"]["measured_tiers"][0])
    with pytest.raises(ValueError):
        _replay(summary)


def test_profile_refuses_command_mismatch_and_accepts_exact_override() -> None:
    for kwargs in (
        {"expected_tier": "medium"},
        {"expected_seed": 6},
        {"requested_client_timeout_ms": 300_000},
        {"requested_history_max_bytes": 256 * 1024 * 1024},
        {"requested_history_max_total_bytes": 512 * 1024 * 1024},
    ):
        with pytest.raises(ValueError):
            _replay(**kwargs)
    summary = _summary()
    tier = summary["detail"]["measured_tiers"][0]
    tier["client_request_timeout_ms"] = 300_000
    tier["requested_client_request_timeout_ms"] = 300_000
    tier["history_max_bytes"] = 256 * 1024 * 1024
    tier["requested_history_max_bytes"] = 256 * 1024 * 1024
    tier["history_policy_id"] = "explicit-pair-default-total-v1"
    profile = _replay(
        summary,
        requested_client_timeout_ms=300_000,
        requested_history_max_bytes=256 * 1024 * 1024,
    )
    assert profile["runtime_config"]["history_max_bytes"] == 256 * 1024 * 1024


def test_profile_binds_explicit_pair_and_total_with_independent_mutants() -> None:
    summary = _summary()
    tier = summary["detail"]["measured_tiers"][0]
    tier.update(
        {
            "history_max_bytes": 300_000_000,
            "requested_history_max_bytes": 300_000_000,
            "history_max_total_bytes": 600_000_000,
            "requested_history_max_total_bytes": 600_000_000,
            "history_policy_id": "explicit-pair-total-diagnostic-v1",
        }
    )
    profile = _replay(
        summary,
        requested_history_max_bytes=300_000_000,
        requested_history_max_total_bytes=600_000_000,
    )
    assert profile["runtime_config"]["history_max_total_bytes"] == 600_000_000
    assert profile["runtime_config"]["history_policy_id"] == "explicit-pair-total-diagnostic-v1"
    for key, value in (
        ("history_max_total_bytes", 600_000_001),
        ("requested_history_max_total_bytes", 600_000_000.0),
        ("history_policy_id", "explicit-pair-default-total-v1"),
    ):
        mutant = deepcopy(summary)
        mutant["detail"]["measured_tiers"][0][key] = value
        with pytest.raises(ValueError, match=f"{key} does not match declared scale input"):
            _replay(
                mutant,
                requested_history_max_bytes=300_000_000,
                requested_history_max_total_bytes=600_000_000,
            )
    for pair, total in (
        (None, 600_000_000),
        (600_000_001, 600_000_000),
        (300_000_000, 0),
        (True, 600_000_000),
    ):
        with pytest.raises(ValueError):
            _replay(
                summary, requested_history_max_bytes=pair, requested_history_max_total_bytes=total
            )


def test_profile_refuses_missing_phase_trace_and_manifest_mutants() -> None:
    summary = _summary()
    del summary["detail"]["measured_tiers"][0]["phase_resources"]["delete_seal"]
    with pytest.raises(ValueError):
        _replay(summary)
    absent = TRACE.replace(_trace_phase("delete_seal"), b"")
    assert absent != TRACE
    with pytest.raises(ValueError):
        _replay(trace=absent)
    for key, value in (("total_files", 4095), ("repo_count", 16.0), ("tier", "medium")):
        manifest = _manifest()
        manifest["tiers"][2][key] = value
        with pytest.raises(ValueError):
            _replay(manifest=manifest)


@pytest.mark.parametrize(
    "mutant",
    [
        SINGLE_TRACE.replace(BM25_FULL, b""),
        SINGLE_TRACE.replace(BM25_ENCODE, b""),
        SINGLE_TRACE.replace(b"segment_fanout=1", b"segment_fanout=2"),
        SINGLE_TRACE.replace(b"retained_valid=1", b"retained_valid=0"),
        SINGLE_TRACE.replace(b"new_census_docs=2", b"new_census_docs=True"),
        SINGLE_TRACE.replace(b"logical_census_bytes=30", b"logical_census_bytes=30.0"),
        SINGLE_TRACE.replace(b"correction_keys=0", b"correction_keys=-1"),
        SINGLE_TRACE.replace(b"label=encode", b"label=unknown"),
        SINGLE_TRACE.replace(b"logical_bytes=128", b"logical_bytes=128 extra=1"),
        SINGLE_TRACE.replace(
            BM25_FULL, b"QI_CAUSAL_V1 kind=bm25_live_build ok=0 reason=counter_overflow\n"
        ),
        SINGLE_TRACE.replace(
            BM25_FULL, b"QI_CAUSAL_V1 kind=bm25_live_build ok=0 reason=retained_estimate_failed\n"
        ),
        SINGLE_TRACE.replace(
            BM25_ENCODE,
            b"QI_CAUSAL_V1 kind=bm25_boundary label=encode ok=0 reason=counter_overflow\n",
        ),
        SINGLE_TRACE.replace(b"index_files=1", b"index_files=1.0"),
        SINGLE_TRACE.replace(
            BM25_FULL,
            BM25_FULL + b"QI_CAUSAL_V1 kind=bm25_census_build ok=0 reason=counter_overflow\n",
        ),
        SINGLE_TRACE.replace(
            BM25_FULL, BM25_FULL + b"QI_CAUSAL_V1 kind=bm25_census_build ok=0 reason=unknown\n"
        ),
    ],
)
def test_profile_refuses_missing_or_malformed_bm25_observation(mutant: bytes) -> None:
    with pytest.raises(ValueError):
        parse_trace(mutant, {"full_seal"})


@pytest.mark.parametrize(
    ("changed_ns", "new_ns"),
    [
        (0, 1001),  # One child alone exceeds the 1000 ns parent.
        (400, 700),  # Both children fit separately, but their sum does not.
    ],
)
def test_profile_refuses_bm25_disjoint_child_clock_overrun(
    changed_ns: int,
    new_ns: int,
) -> None:
    parent_ns = 1000
    assert changed_ns + new_ns > parent_ns
    if changed_ns and new_ns:
        assert changed_ns <= parent_ns and new_ns <= parent_ns
    mutated = BM25_FULL.replace(b"changed_ns=0", f"changed_ns={changed_ns}".encode()).replace(
        b"new_ns=700", f"new_ns={new_ns}".encode()
    )
    assert mutated != BM25_FULL
    trace = SINGLE_TRACE.replace(BM25_FULL, mutated)
    assert trace != SINGLE_TRACE
    with pytest.raises(ValueError, match="BM25 child call clocks exceed parent"):
        parse_trace(trace, {"full_seal"})


def test_profile_refuses_aggregate_bm25_counter_overflow() -> None:
    maximum = str((1 << 64) - 1).encode()
    huge = BM25_CENSUS.replace(b"docs=2", b"docs=" + maximum)
    trace = SINGLE_TRACE.replace(BM25_FULL, huge + BM25_CENSUS + BM25_FULL)
    with pytest.raises(ValueError):
        parse_trace(trace, {"full_seal"})


def test_profile_refuses_missing_delta_bm25_build() -> None:
    with pytest.raises(ValueError):
        _replay(trace=TRACE.replace(BM25_DELTA, b"", 1))


def test_profile_refuses_source_and_io_type_alias() -> None:
    with pytest.raises(ValueError):
        _replay(source_revision="b" * 40)
    for invalid in (2.0, True):
        summary = _summary()
        summary["schema_version"] = invalid
        with pytest.raises(ValueError):
            _replay(summary)
    summary = _summary()
    summary["detail"]["measured_tiers"][0]["phase_resources"]["full_seal"]["process_write_io"][
        "write_bytes"
    ] = 4096.0
    with pytest.raises(ValueError):
        _replay(summary)


@pytest.mark.parametrize(
    "field",
    (
        "avg_file_lines",
        "hit_density_per_mille",
        "symbol_density_per_mille",
        "serving_owner_count",
        "qualification_owner",
    ),
)
@pytest.mark.parametrize("fault", ("missing", "changed", "wrong_type"))
def test_native_manifest_refuses_producer_contract_drift(
    tmp_path: Path, field: str, fault: str
) -> None:
    summary, children, workload = _native_product_fixture(tmp_path)
    manifest = _manifest()
    row = manifest["tiers"][0]
    if fault == "missing":
        del row[field]
    elif fault == "changed":
        row[field] = "foreign-owner" if field == "qualification_owner" else row[field] + 1
    else:
        row[field] = 1 if field == "qualification_owner" else True
    with pytest.raises(ValueError):
        validate_native_product_artifacts(
            canonical(summary),
            canonical(manifest),
            source_revision=SOURCE,
            expected_tier="small",
            expected_seed=7,
            expected_daemon_sha256="e" * 64,
            expected_foreground_workload=workload,
            expected_churn_cycles=0,
            child_records=children,
        )
