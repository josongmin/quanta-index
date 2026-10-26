"""Replay legacy cost markers as an independent aggregate oracle.

The companion canonical JSONL parser owns span causality and waterfall export.
This module keeps strict sync/BM25 and phase-resource checks while the producer
has no equivalent canonical detail for those aggregates. Timers are userspace
call envelopes; /proc/self/io is kernel process accounting, not physical I/O.
"""

from __future__ import annotations

import argparse
import hashlib
import os
import stat
from copy import deepcopy
from pathlib import Path

from tools.benchmark.evidence import RawFile
from tools.benchmark.retrieval.causal_cost_timeline import (
    MAX_CONFIGURED_CAPTURE_BYTES,
)
from tools.benchmark.retrieval.causal_cost_timeline import (
    parse_stream as parse_timeline_stream,
)
from tools.benchmark.retrieval.conditional_proof import canonical, load, sha
from tools.benchmark.retrieval.finite_json import is_finite_json_number

PREFIX = "QI_CAUSAL_V1 "
SYNC_LABELS = frozenset(
    {
        "atomic_file",
        "atomic_parent",
        "atomic_at_file",
        "atomic_at_parent",
        "generation_directory",
        "coverage_orphan_directory",
        "history_manifest_file",
        "sealed_leftover_directory",
        "overlay_directory",
        "text_authority_directory",
        "history_layout_directory",
        "file_authority_object",
        "file_authority_directory",
        "pool_new_directory_full",
        "pool_new_parent_full",
        "pool_data_file_posix",
        "pool_data_shard_posix",
        "pool_object_file_full",
        "pool_object_shard_full",
        "pool_control_file_full",
        "pool_control_parent_full",
        "pool_reused_file_posix",
        "pool_reused_shard_posix",
        "pool_adoption_full_barrier",
        "pool_adoption_journal_full",
    }
)
KINDS = frozenset(
    {
        "phase_start",
        "phase_end",
        "sync",
        "exact_live_token_scan",
        "bm25_census_build",
        "bm25_boundary",
        "bm25_live_build",
    }
)
BM25_BOUNDARY_LABELS = frozenset({"base_read", "encode"})
BM25_BUILD_COUNTERS = frozenset(
    {
        "elapsed_ns",
        "reused_segments",
        "changed_segments",
        "new_segments",
        "mask_docs",
        "new_census_docs",
        "newly_dead_docs",
        "logical_census_bytes",
        "changed_ns",
        "new_ns",
        "death_ns",
        "correction_keys",
        "segment_fanout",
        "retained_valid",
        "retained_estimate_bytes",
    }
)
TIER_SHAPE = {
    "small": (1, 16),
    "medium": (4, 64),
    "large": (16, 256),
    "xlarge": (64, 512),
}
NATIVE_BASE_PHASES = frozenset(
    {
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
    }
)
NATIVE_CHURN_PHASES = frozenset(
    {
        "churn_cycles",
        "churn_old_view_search",
        "reopen_churn_search",
    }
)
NATIVE_IO_FIELDS = frozenset(
    {
        "write_calls",
        "written_bytes",
        "failed_writes",
        "enospc_writes",
        "file_sync_calls",
        "failed_file_syncs",
        "directory_sync_calls",
        "failed_directory_syncs",
        "barrier_calls",
        "failed_barriers",
        "reserve_calls",
        "reserve_objects",
        "single_object_reserves",
        "failed_reserves",
        "confirm_calls",
        "confirm_objects",
        "single_object_confirms",
        "failed_confirms",
        "write_elapsed_ns",
        "file_sync_elapsed_ns",
        "directory_sync_elapsed_ns",
        "barrier_elapsed_ns",
        "reserve_elapsed_ns",
        "confirm_elapsed_ns",
    }
)
NATIVE_U64_MAX = (1 << 64) - 1


def _native_hex(value: object, width: int, label: str) -> str:
    if (
        not isinstance(value, str)
        or len(value) != width
        or any(character not in "0123456789abcdef" for character in value)
    ):
        raise ValueError(f"native {label} is not lowercase hex")
    return value


def _native_u64(value: object, label: str) -> int:
    if type(value) is not int or not 0 <= value <= NATIVE_U64_MAX:
        raise ValueError(f"native {label} is not a u64")
    return value


def _native_digest(value: object, label: str) -> str:
    if not isinstance(value, str) or not value.startswith("sha256:"):
        raise ValueError(f"native {label} is not a SHA-256 digest")
    _native_hex(value.removeprefix("sha256:"), 64, label)
    return value


def _native_manifest(raw: bytes, tier: str) -> tuple[int, int]:
    manifest = load(raw)
    if (
        not isinstance(manifest, dict)
        or manifest.get("kind") != "quanta-index-scale-tier-manifest"
        or type(manifest.get("manifest_schema_version")) is not int
        or manifest["manifest_schema_version"] != 2
        or manifest.get("dimension") != "scale"
        or manifest.get("query_token") != "scale_needle_token"
        or not isinstance(manifest.get("tiers"), list)
        or len(manifest["tiers"]) != len(TIER_SHAPE)
        or tier not in TIER_SHAPE
    ):
        raise ValueError("native tier manifest contract is invalid")
    corpus_shape = {
        "small": (40, 60, 120),
        "medium": (80, 40, 100),
        "large": (120, 25, 80),
        "xlarge": (200, 15, 60),
    }
    for row, (name, (repos, files)) in zip(manifest["tiers"], TIER_SHAPE.items(), strict=True):
        if not isinstance(row, dict) or set(row) != {
            "tier",
            "repo_count",
            "files_per_repo",
            "total_files",
            "source_repo_count",
            "default_run",
            "selectable",
            "avg_file_lines",
            "hit_density_per_mille",
            "symbol_density_per_mille",
            "serving_owner_count",
            "qualification_owner",
        }:
            raise ValueError("native tier manifest row is malformed")
        for key, expected in zip(
            ("avg_file_lines", "hit_density_per_mille", "symbol_density_per_mille"),
            corpus_shape[name],
            strict=True,
        ):
            _exact_int(row[key], expected, f"native manifest {name}.{key}")
        _exact_int(row["serving_owner_count"], 1, f"native manifest {name}.serving_owner_count")
        if (
            row["qualification_owner"] != "canonical-quiet-host-run"
            or row["tier"] != name
            or any(
                type(row[key]) is not int
                for key in ("repo_count", "files_per_repo", "total_files", "source_repo_count")
            )
            or row["repo_count"] != repos
            or row["files_per_repo"] != files
            or row["total_files"] != repos * files
            or row["source_repo_count"] != repos
            or row["default_run"] is not (name == "small")
            or row["selectable"] is not True
        ):
            raise ValueError("native tier manifest shape differs from fixed generator")
    return TIER_SHAPE[tier]


def _native_fixed_operation(value: object, sequence: int) -> None:
    """Check the fixed Native scale producer's operation identity, not generic wire decoding."""
    if not isinstance(value, dict) or set(value) != {
        "workspace",
        "epoch",
        "producer",
        "stream",
        "stream_epoch",
        "sequence",
    }:
        raise ValueError("native publication operation key shape differs from fixed producer")
    workspace = value["workspace"]
    if not isinstance(workspace, dict) or set(workspace) != {
        "tenant",
        "collection",
        "worktree",
        "slot",
    }:
        raise ValueError("native publication workspace shape differs from fixed producer")
    for field, byte in (
        ("tenant", 1),
        ("collection", 2),
        ("worktree", 3),
        ("slot", 4),
        ("producer", 5),
        ("stream", 6),
    ):
        observed = workspace[field] if field in workspace else value[field]
        if (
            not isinstance(observed, list)
            or len(observed) != 16
            or any(type(item) is not int or item != byte for item in observed)
        ):
            raise ValueError(f"native publication {field} differs from fixed producer")
    for field, expected in (("epoch", 1), ("stream_epoch", 1), ("sequence", sequence)):
        if type(value[field]) is not int or value[field] != expected:
            raise ValueError(f"native publication {field} differs from fixed producer")


def _native_nonnegative_ms(value: object, label: str) -> None:
    if not is_finite_json_number(value) or value < 0:
        raise ValueError(f"native {label} is not finite nonnegative milliseconds")


def _native_publication_ack(row: object, sequence: int, phase: str, cycle: int | None) -> None:
    if not isinstance(row, dict) or row.get("phase") != phase:
        raise ValueError("native publication phase is malformed")
    if cycle is None:
        if row.get("cycle") is not None:
            raise ValueError("native base publication has a churn cycle")
    elif type(row.get("cycle")) is not int or row["cycle"] != cycle:
        raise ValueError("native publication churn cycle differs from fixed producer")
    if (
        type(row.get("submit_count")) is not int
        or row["submit_count"] != 1
        or row.get("completion") not in ("immediate_ack", "reconciled_status")
        or not isinstance(row.get("operation_key"), dict)
    ):
        raise ValueError("native publication lacks one original-key terminal ACK")
    checks = _native_u64(row.get("status_checks"), "publication status checks")
    if row["completion"] == "immediate_ack":
        if (
            row.get("submit_outcome") != "terminal_published"
            or checks != 0
            or row.get("last_status") is not None
        ):
            raise ValueError("native immediate ACK contradicts its submit/status record")
    elif (
        row.get("submit_outcome") not in ("accepted", "unknown_response", "unknown_error")
        or checks == 0
        or row.get("last_status") != "published"
    ):
        raise ValueError("native reconciled ACK contradicts its original-key status")
    _native_fixed_operation(row["operation_key"], sequence)


def _native_mixed_queries(value: object, name: str, seconds: int) -> None:
    if not isinstance(value, dict) or set(value) != {
        "window_seconds",
        "offered",
        "dropped_queue_full",
        "dropped_scheduler_late",
        "drain_secs",
        "max_queue_ms",
        "classes",
        "first_query_error",
        "latency_scope",
        "qualification",
    }:
        raise ValueError(f"native mixed {name} query evidence is incomplete")
    if (
        _native_u64(value["window_seconds"], "mixed window") != seconds
        or _native_u64(value["offered"], "mixed offered") != seconds * 50
        or _native_u64(value["dropped_queue_full"], "mixed queue drops") != 0
        or _native_u64(value["dropped_scheduler_late"], "mixed late drops") != 0
        or value["first_query_error"] is not None
        or value["qualification"] != "observation_only"
        or value["latency_scope"]
        != (
            "scheduled arrival to decoded and validated reply, includes failed completions; "
            "drops have no latency and are separate failures"
        )
        or not is_finite_json_number(value["drain_secs"])
        or value["drain_secs"] < seconds
    ):
        raise ValueError(f"native mixed {name} offered window or failure gate differs")
    _native_nonnegative_ms(value["max_queue_ms"], "mixed max queue")
    classes = value["classes"]
    ratios = {"unique": 7, "broad_top20": 2, "miss": 1}
    if not isinstance(classes, dict) or set(classes) != set(ratios):
        raise ValueError("native mixed query classes differ")
    for label, ratio in ratios.items():
        row = classes[label]
        if not isinstance(row, dict) or set(row) != {
            "success",
            "failed",
            "latency_sample_count",
            "p50_ms",
            "p95_ms",
            "p99_ms",
        }:
            raise ValueError("native mixed query class evidence is incomplete")
        expected = seconds * 5 * ratio
        if (
            _native_u64(row["success"], "mixed success") != expected
            or _native_u64(row["failed"], "mixed failed") != 0
            or _native_u64(row["latency_sample_count"], "mixed latency count") != expected
        ):
            raise ValueError("native mixed query completions do not conserve the offered classes")
        for field in ("p50_ms", "p95_ms", "p99_ms"):
            _native_nonnegative_ms(row[field], f"mixed {label}.{field}")
        if not row["p50_ms"] <= row["p95_ms"] <= row["p99_ms"]:
            raise ValueError("native mixed query percentile order differs")
        if name == "measure" and (row["p95_ms"] > 100 or row["p99_ms"] > 500):
            raise ValueError("native mixed warm lexical latency target exceeded")


def _native_mixed_publisher(value: object, seconds: int, sequence: int) -> None:
    if not isinstance(value, dict) or set(value) != {
        "scheduled",
        "attempted",
        "acked",
        "published",
        "missed_late",
        "last_operation_sequence",
        "last_error",
        "publication_records",
        "current_gold_from_scheduled_p95_ms",
        "current_gold_from_scheduled_p99_ms",
        "old_view_witness_p95_ms",
        "old_view_witness_p99_ms",
    }:
        raise ValueError("native mixed publisher evidence is incomplete")
    for field in ("scheduled", "attempted", "acked", "published"):
        if _native_u64(value[field], f"mixed {field}") != seconds:
            raise ValueError("native mixed publication attempt/ACK denominator differs")
    if (
        _native_u64(value["missed_late"], "mixed missed publications") != 0
        or value["last_error"] is not None
        or _native_u64(value["last_operation_sequence"], "mixed last operation")
        != sequence + seconds - 1
    ):
        raise ValueError("native mixed publisher failed or lost its final operation")
    records = value["publication_records"]
    if not isinstance(records, list) or len(records) != seconds:
        raise ValueError("native mixed publication records are incomplete")
    timings = {"current_gold_from_scheduled": [], "old_view_witness": []}
    for offset, row in enumerate(records):
        if not isinstance(row, dict) or set(row) != {
            "operation_sequence",
            "status",
            "audit",
            "timing",
        }:
            raise ValueError("native mixed publication record is malformed")
        key = sequence + offset
        if (
            _native_u64(row["operation_sequence"], "mixed operation") != key
            or row["status"] != "published"
            or not isinstance(row["audit"], list)
            or len(row["audit"]) != 1
        ):
            raise ValueError("native mixed original publication operation differs")
        _native_publication_ack(row["audit"][0], key, "mixed_load", None)
        timing = row["timing"]
        if not isinstance(timing, dict) or set(timing) != {
            "input_ready_from_scheduled_ms",
            "ack_from_scheduled_ms",
            "current_gold_from_scheduled_ms",
            "old_view_witness_ms",
        }:
            raise ValueError("native mixed publication timing is incomplete")
        for field, duration in timing.items():
            _native_nonnegative_ms(duration, f"mixed {field}")
        if not (
            timing["input_ready_from_scheduled_ms"]
            <= timing["ack_from_scheduled_ms"]
            <= timing["current_gold_from_scheduled_ms"]
        ):
            raise ValueError("native mixed publication timing order differs")
        for field, samples in timings.items():
            samples.append(timing[f"{field}_ms"])
    for field, samples in timings.items():
        samples.sort()
        for percentile in (95, 99):
            observed = value[f"{field}_p{percentile}_ms"]
            _native_nonnegative_ms(observed, f"mixed {field} percentile")
            if observed != samples[(len(samples) * percentile + 99) // 100 - 1]:
                raise ValueError("native mixed publisher percentile differs from its records")


def _native_mixed_load(value: object, sequence: int) -> None:
    if (
        not isinstance(value, dict)
        or set(value) != {"scope", "warmup", "measure", "retained_read"}
        or value["scope"]
        != (
            "Native UDS mixed offered load with product retained-read witness; "
            "sampled resource observations are not continuous peaks"
        )
    ):
        raise ValueError("native mixed-load evidence is absent or has a different scope")
    held = value["retained_read"]
    if not isinstance(held, dict) or set(held) != {
        "scope",
        "lease_millis",
        "renew_period_ms",
        "max_renew_gap_ms",
        "initial_view_sequence",
        "initial_revision",
        "final_revision",
        "initial_expiry_ms",
        "renewals_ms",
        "renewal_expiries_ms",
        "held_queries",
        "last_held_query_ms",
        "first_release_attempt_ms",
        "confirmed_release_ms",
        "confirmed_last_pin",
        "settled_ms",
        "settlement_polls",
        "last_settlement_state",
        "settlement_head_sequence",
        "settlement_exact_floor",
        "last_status",
    }:
        raise ValueError("native mixed retained read evidence is missing")
    if (
        held["scope"]
        != (
            "product ReadSession Exact; monotonic 600-second held ticket and bounded "
            "settlement, not continuous resource peak"
        )
        or _native_u64(held["initial_view_sequence"], "mixed held initial view") != sequence - 1
        or held["confirmed_last_pin"] is not True
        or held["last_status"] != "Released"
        or held["last_settlement_state"] != "SettledAtSnapshot"
        or _native_u64(held["settlement_polls"], "mixed held settlement polls") == 0
        or _native_u64(held["lease_millis"], "mixed held lease") != 300_000
        or _native_u64(held["renew_period_ms"], "mixed held renewal period") != 120_000
        or _native_u64(held["max_renew_gap_ms"], "mixed held renewal gap") != 180_000
    ):
        raise ValueError("native mixed retained read identity or release differs")
    renewals = held["renewals_ms"]
    expiries = held["renewal_expiries_ms"]
    if (
        not isinstance(renewals, list)
        or not renewals
        or not isinstance(expiries, list)
        or len(expiries) != len(renewals)
    ):
        raise ValueError("native mixed retained read has no renewals")
    initial_revision = _native_u64(held["initial_revision"], "mixed held initial revision")
    final_revision = _native_u64(held["final_revision"], "mixed held final revision")
    if initial_revision == 0 or final_revision != initial_revision + len(renewals):
        raise ValueError("native mixed retained read revision chain differs")
    previous_expiry = _native_u64(held["initial_expiry_ms"], "mixed held initial expiry")
    if previous_expiry == 0:
        raise ValueError("native mixed retained read initial expiry is zero")
    for index, observed in enumerate(expiries):
        expiry = _native_u64(observed, f"mixed held renewal expiry {index}")
        if expiry <= previous_expiry:
            raise ValueError("native mixed retained read expiry did not advance")
        previous_expiry = expiry
    for field in (
        "first_release_attempt_ms",
        "confirmed_release_ms",
        "settled_ms",
        "last_held_query_ms",
    ):
        _native_nonnegative_ms(held[field], f"mixed held {field}")
    first_release = held["first_release_attempt_ms"]
    confirmed = held["confirmed_release_ms"]
    settled = held["settled_ms"]
    last_query = held["last_held_query_ms"]
    clocks = [0.0]
    for index, sample in enumerate(renewals):
        _native_nonnegative_ms(sample, f"mixed held renewal {index}")
        clocks.append(sample)
    clocks.append(first_release)
    if (
        any(
            after <= before or after - before > 180_000 for before, after in zip(clocks, clocks[1:])
        )
        or first_release < 600_000
        or not last_query <= first_release <= last_query + 2_000
        or not first_release <= confirmed <= first_release + 2_000
        or not confirmed <= settled <= first_release + 60_000
        or _native_u64(held["held_queries"], "mixed held query count") != len(renewals) + 2
    ):
        raise ValueError("native mixed retained read duration, renewal, or settlement differs")
    floor = _native_u64(held["settlement_exact_floor"], "mixed held settlement floor")
    head = _native_u64(held["settlement_head_sequence"], "mixed held settlement head")
    if not sequence - 1 < floor <= head <= sequence + 959:
        raise ValueError("native mixed retained read settlement floor/head differs")
    for name, seconds in (("warmup", 60), ("measure", 900)):
        window = value[name]
        if (
            not isinstance(window, dict)
            or set(window) != {"name", "queries", "publisher"}
            or window["name"] != name
        ):
            raise ValueError("native mixed-load window is missing or partial")
        _native_mixed_queries(window["queries"], name, seconds)
        _native_mixed_publisher(window["publisher"], seconds, sequence)
        sequence += seconds
    publisher = value["measure"]["publisher"]
    if (
        publisher["current_gold_from_scheduled_p95_ms"] > 1000
        or publisher["current_gold_from_scheduled_p99_ms"] > 5000
    ):
        raise ValueError("native mixed publication freshness target exceeded")


def _native_sample(
    value: object,
    label: str,
    method: str,
    interval: int,
    *,
    positive: bool | None,
    positive_endpoints: bool = False,
) -> None:
    if (
        not isinstance(value, dict)
        or set(value)
        != {
            "method",
            "sample_interval_ms",
            "first_bytes",
            "last_bytes",
            "sampled_max_bytes",
            "sample_count",
            "missing_count",
            "last_missing_reason",
            "max_observed_gap_ms",
            "observer_wall_ms",
        }
        or value["method"] != method
        or _native_u64(value["sample_interval_ms"], f"{label} interval") != interval
    ):
        raise ValueError(f"native {label} sampling method or fields differ")
    count = _native_u64(value["sample_count"], f"{label} sample count")
    missing = _native_u64(value["missing_count"], f"{label} missing count")
    reason = value["last_missing_reason"]
    if (missing == 0 and reason is not None) or (
        missing > 0 and (not isinstance(reason, str) or not reason)
    ):
        raise ValueError(f"native {label} missing-sample reason differs")
    for field in ("max_observed_gap_ms", "observer_wall_ms"):
        _native_nonnegative_ms(value[field], f"{label}.{field}")
    if positive is None:
        if (
            count != 0
            or any(
                value[field] is not None
                for field in ("first_bytes", "last_bytes", "sampled_max_bytes")
            )
            or missing == 0
        ):
            raise ValueError(f"native {label} unsupported method forged a byte sample")
        return
    if count == 0:
        raise ValueError(f"native {label} has no actual sample")
    first, last, maximum = (
        _native_u64(value[field], f"{label}.{field}")
        for field in ("first_bytes", "last_bytes", "sampled_max_bytes")
    )
    if (
        maximum < max(first, last)
        or (count <= 2 and maximum != max(first, last))
        or (count == 1 and first != last)
        or (positive and maximum == 0)
        or (positive_endpoints and min(first, last, maximum) == 0)
    ):
        raise ValueError(f"native {label} sampled maximum or positive bytes differ")


def _native_process_io(value: object, label: str, *, linux: bool, epoch_elapsed_ms: float) -> None:
    if (
        not isinstance(value, dict)
        or set(value)
        != {
            "method",
            "scope",
            "sample_interval_ms",
            "read_bytes",
            "write_bytes",
            "cancelled_write_bytes",
            "first_sample_offset_ms",
            "last_sample_offset_ms",
            "observed_window_ms",
            "sample_count",
            "missing_count",
            "last_missing_reason",
            "max_observed_gap_ms",
            "observer_wall_ms",
            "regression_error",
        }
        or value["method"] != ("proc_pid_io" if linux else "unsupported")
        or value["scope"]
        != (
            "unreaped_child_after_socket_ready_until_shutdown_signal; "
            "kernel_process_accounting_not_device_media_bytes"
        )
        or _native_u64(value["sample_interval_ms"], f"{label} interval") != 100
    ):
        raise ValueError(f"native {label} process IO method or fields differ")
    count = _native_u64(value["sample_count"], f"{label} sample count")
    missing = _native_u64(value["missing_count"], f"{label} missing count")
    reason = value["last_missing_reason"]
    if (
        value["regression_error"] is not None
        or (missing == 0 and reason is not None)
        or (missing > 0 and (not isinstance(reason, str) or not reason))
    ):
        raise ValueError(f"native {label} process IO missing/regression status differs")
    for field in ("max_observed_gap_ms", "observer_wall_ms"):
        _native_nonnegative_ms(value[field], f"{label}.{field}")
    if (linux and count < 2) or (not linux and (count != 0 or missing == 0)):
        raise ValueError(f"native {label} process IO sample availability differs")
    for field in ("read_bytes", "write_bytes", "cancelled_write_bytes"):
        counters = value[field]
        if not isinstance(counters, dict) or set(counters) != {"first", "last", "delta"}:
            raise ValueError(f"native {label}.{field} counter tuple absent")
        if linux:
            first = _native_u64(counters["first"], f"{label}.{field}.first")
            last = _native_u64(counters["last"], f"{label}.{field}.last")
            if (
                last < first
                or _native_u64(counters["delta"], f"{label}.{field}.delta") != last - first
            ):
                raise ValueError(f"native {label}.{field} delta differs from observed endpoints")
        elif any(counter is not None for counter in counters.values()):
            raise ValueError(f"native {label}.{field} unsupported axis claimed bytes")
    timing = ("first_sample_offset_ms", "last_sample_offset_ms", "observed_window_ms")
    if linux:
        for field in timing:
            _native_nonnegative_ms(value[field], f"{label}.{field}")
        first_offset = value["first_sample_offset_ms"]
        last_offset = value["last_sample_offset_ms"]
        if (
            last_offset < first_offset
            or last_offset > epoch_elapsed_ms + 1e-6
            or abs(value["observed_window_ms"] - (last_offset - first_offset)) > 1e-6
        ):
            raise ValueError(f"native {label} process IO sample offsets regressed")
    elif any(value[field] is not None for field in timing):
        raise ValueError(f"native {label} unsupported process IO claimed sample timing")


def _native_resource_observation(value: object, host_os: object) -> dict:
    if (
        host_os not in ("linux", "macos")
        or not isinstance(value, dict)
        or set(value)
        != {
            "mode",
            "scope",
            "sampling",
            "observer_cost_scope",
            "stop_limit",
            "continuous_peak",
            "sampled_only",
            "aggregate_rss_scope",
            "aggregate_rss_census",
            "aggregate_rss_max_processes",
            "disk_observer_cost_scope",
            "fixture_scope",
            "scratch_scope",
            "epochs",
            "unavailable",
        }
    ):
        raise ValueError("native resource observation is absent, partial or unsupported")
    if (
        value["mode"] != "opt_in_diagnostic_v2"
        or value["scope"]
        != "child_daemon_after_socket_ready_until_shutdown_signal; startup and drain completion excluded"
        or value["sampling"]
        != "periodic and boundary observations; sampled maximum is not a continuous peak"
        or value["continuous_peak"] is not False
        or value["sampled_only"] is not True
        or _native_u64(value["aggregate_rss_max_processes"], "aggregate RSS cap") != 4096
        or value["unavailable"] != []
    ):
        raise ValueError("native resource observation scope or worker availability differs")
    fixed_scopes = {
        "observer_cost_scope": "probe and traversal wall time; scheduling and aggregation excluded",
        "stop_limit": "cooperative between filesystem entries; an individual metadata syscall may block",
        "aggregate_rss_scope": "harness and daemon plus observed descendants, including previously seen reparented children; per-process RSS sum may double-count shared pages",
        "aggregate_rss_census": "non-atomic, bounded; processes entirely between observations may be missed; numeric PID reuse is excluded by start-time identity",
        "disk_observer_cost_scope": "single fixture traversal charged to allocated_fixture_root; state and scratch are subsets with zero additional traversal cost",
        "fixture_scope": "owned native temporary fixture including state, scratch, config and in-fixture diagnostics; external retained JSONL/stderr and other temporary roots excluded",
        "scratch_scope": "state_root/native-scratch-v1 allocated blocks; linked objects and other temporary roots excluded",
    }
    for field, expected in fixed_scopes.items():
        if value[field] != expected:
            raise ValueError(f"native resource {field} differs from fixed observation contract")
    epochs = value["epochs"]
    if not isinstance(epochs, list) or len(epochs) != 2:
        raise ValueError("native first/reopened daemon resource epochs absent")
    for epoch, expected_name in zip(epochs, ("first_daemon", "reopened_daemon"), strict=True):
        if (
            not isinstance(epoch, dict)
            or set(epoch)
            != {
                "epoch",
                "observed_elapsed_ms",
                "child_rss",
                "aggregate_process_rss",
                "allocated_fixture_root",
                "allocated_state_root",
                "allocated_native_scratch",
                "child_process_io",
                "worker_failure",
            }
            or epoch["epoch"] != expected_name
            or epoch["worker_failure"] is not None
        ):
            raise ValueError(f"native {expected_name} resource epoch failed or incomplete")
        _native_nonnegative_ms(epoch["observed_elapsed_ms"], f"{expected_name} elapsed")
        _native_sample(
            epoch["child_rss"],
            f"{expected_name}.rss",
            "proc_pid_status_vmrss" if host_os == "linux" else "ps_o_rss_kib",
            100,
            positive=True,
            positive_endpoints=True,
        )
        _native_sample(
            epoch["aggregate_process_rss"],
            f"{expected_name}.aggregate_rss",
            "proc_stat_identity_and_status_vmrss_sum" if host_os == "linux" else "unsupported",
            100,
            positive=True if host_os == "linux" else None,
            positive_endpoints=host_os == "linux",
        )
        for field, positive in (
            ("allocated_fixture_root", True),
            ("allocated_state_root", True),
            ("allocated_native_scratch", False),
        ):
            _native_sample(
                epoch[field],
                f"{expected_name}.{field}",
                "unique_inode_st_blocks_x_512",
                500,
                positive=positive,
            )
        _native_process_io(
            epoch["child_process_io"],
            f"{expected_name}.process_io",
            linux=host_os == "linux",
            epoch_elapsed_ms=epoch["observed_elapsed_ms"],
        )
    return value


def _native_linux_resource_observation(value: object) -> dict:
    """Validate the outer owner record without promoting sampled values to peaks."""
    if not isinstance(value, dict) or set(value) != {
        "owner",
        "scope",
        "cgroup_parent",
        "cgroup_parent_dev",
        "cgroup_parent_ino",
        "cgroup_path",
        "root_pid",
        "root_start_ticks",
        "sample_interval_ms",
        "sample_count",
        "aggregate_rss_sampled_max_bytes",
        "aggregate_rss_method",
        "cgroup_memory_peak_bytes",
        "cgroup_memory_method",
        "cgroup_cpu_usage_ns",
        "ownership_complete",
        "migration_caveat",
        "continuous_rss_peak",
        "allocated_disk_peak",
    }:
        raise ValueError("native outer Linux resource observation is malformed")
    if (
        value["owner"] != "linux-process-owner-v1"
        or value["scope"] != "scale_harness_and_owned_descendants_until_cgroup_empty"
        or value["aggregate_rss_method"]
        != "sampled_smaps_rollup_rss_sum_shared_pages_may_repeat"
        or value["cgroup_memory_method"]
        != "cgroup_v2_memory.peak_kernel_accounted_memory_not_rss"
        or value["migration_caveat"]
        != "unobserved_cgroup_migration_requires_external_prevention"
        or value["ownership_complete"] is not True
        or value["continuous_rss_peak"] is not False
        or value["allocated_disk_peak"] is not None
    ):
        raise ValueError("native outer Linux resource custody or method differs")
    for field in (
        "cgroup_parent_dev",
        "cgroup_parent_ino",
        "root_pid",
        "root_start_ticks",
        "sample_interval_ms",
        "sample_count",
        "aggregate_rss_sampled_max_bytes",
        "cgroup_memory_peak_bytes",
        "cgroup_cpu_usage_ns",
    ):
        _native_u64(value[field], f"outer Linux {field}")
    for field in (
        "cgroup_parent_ino",
        "root_pid",
        "root_start_ticks",
        "sample_interval_ms",
        "sample_count",
    ):
        if value[field] == 0:
            raise ValueError(f"native outer Linux {field} must be positive")
    parent_text = value["cgroup_parent"]
    child_text = value["cgroup_path"]
    if not isinstance(parent_text, str) or not isinstance(child_text, str):
        raise ValueError("native outer Linux cgroup paths are invalid")
    parent = Path(parent_text)
    child = Path(child_text)
    if (
        not parent.is_absolute()
        or not child.is_absolute()
        or str(parent) != parent_text
        or str(child) != child_text
        or ".." in parent.parts
        or ".." in child.parts
        or parent == child
        or child.parent != parent
    ):
        raise ValueError("native outer Linux cgroup must be a direct child of its parent")
    return value


def _native_child_trace(
    trace: dict,
    raw: RawFile,
    *,
    publication: bool,
    trace_max_output_bytes: int | None = None,
) -> dict:
    if not isinstance(raw, RawFile) or not isinstance(trace, dict):
        raise ValueError("native child trace custody is malformed")
    bound = trace_max_output_bytes if trace_max_output_bytes is not None else 67_108_864
    if not 0 < raw.size <= bound or trace.get("sha256") != raw.sha256.removeprefix("sha256:"):
        raise ValueError("native child trace raw bytes or digest differ")
    actual = raw.consume_seekable(
        lambda handle: parse_timeline_stream(handle, max_capture_bytes=bound)
    )
    if actual != trace:
        raise ValueError("native child parsed capture differs from committed raw bytes")
    header = trace.get("header")
    footer = trace.get("footer")
    capture_id = _native_hex(trace.get("capture_id"), 32, "child capture ID")
    if (
        not isinstance(header, dict)
        or not isinstance(footer, dict)
        or trace.get("status") != "complete"
        or type(trace.get("schema_version")) is not int
        or trace["schema_version"] not in (2, 3, 4)
        or trace.get("health")
        or trace.get("orphan_events") != 0
        or trace.get("truncated_tail") is not False
        or header.get("max_record_bytes") != 4096
        or any(
            header.get(key) != capture_id
            for key in ("capture_id", "stream_id", "clock_domain", "process_instance")
        )
        or footer.get("status") != "complete"
        or footer.get("close_reason") != "normal"
        or any(
            footer.get(key) != 0
            for key in ("events_dropped", "events_unwritten", "open_spans", "producer_leases")
        )
        or any(
            footer.get(key) != trace.get("event_count")
            for key in ("events_attempted", "events_accepted", "events_written", "last_output_seq")
        )
    ):
        raise ValueError("native child trace is partial, unhealthy or has invalid identity")
    spans = trace.get("spans")
    if not isinstance(spans, dict):
        raise ValueError("native child spans absent")
    roots = [
        span
        for span in spans.values()
        if span.get("stage") == "operation" and span.get("parent_span_id") is None
    ]
    identified = {
        span_id
        for span_id, span in spans.items()
        if span.get("stage") == "request"
        and span.get("root_kind") == "request"
        and span.get("parent_span_id") is None
        and isinstance(span.get("request_identity"), dict)
    }
    if (
        len(roots) != 1
        or roots[0].get("outcome") != "ok"
        or not identified
        or not any(
            span.get("stage") == "runtime_boot" and span.get("outcome") == "ok"
            for span in spans.values()
        )
    ):
        raise ValueError("native child boot, operation or identified request absent")
    if publication:
        for required in ("build_prepare", "native_publish"):
            witnesses = [span for span in spans.values() if span.get("stage") == required]
            if not any(
                span.get("outcome") == "ok" and _native_beneath_identified(spans, span, identified)
                for span in witnesses
            ):
                raise ValueError(f"native child {required} witness absent")
    sync = {}
    for row in trace.get("counters", []):
        if row.get("span_id") != roots[0]["span_id"] or row.get("key") not in (
            "sync_calls",
            "sync_failures",
            "sync_elapsed_ns",
        ):
            continue
        key = row["key"]
        if key in sync:
            raise ValueError("native child duplicate IO counter")
        sync[key] = _native_u64(row.get("value"), key)
    if set(sync) != {"sync_calls", "sync_failures", "sync_elapsed_ns"}:
        raise ValueError("native child quiescent IO counters absent")
    if sync["sync_failures"] != 0 or (publication and sync["sync_calls"] == 0):
        raise ValueError("native child install IO failed or first publication has no calls")
    return {"capture_id": capture_id, "sync": sync}


def _native_beneath_identified(spans: dict, span: dict, identified: set) -> bool:
    current = span
    seen = set()
    while current is not None:
        span_id = current.get("span_id")
        if span_id in identified:
            return True
        if span_id in seen:
            return False
        seen.add(span_id)
        current = spans.get(current.get("parent_span_id"))
    return False


def _native_check_write_failures(observation: dict, label: str) -> None:
    if (
        not observation["enospc_writes"]
        <= observation["failed_writes"]
        <= observation["write_calls"]
    ):
        raise ValueError(f"native {label} write failure counters contradict reached writes")


def _native_stderr_summary(raw: RawFile, descriptor: dict, capture_id: str) -> dict:
    if not isinstance(raw, RawFile) or not isinstance(descriptor, dict):
        raise ValueError("native stderr custody is malformed")
    if descriptor.get("path") != str(raw.path):
        raise ValueError("native stderr descriptor path differs from raw file")
    if _native_u64(descriptor.get("whole_file_bytes_at_drain"), "stderr size") != raw.size:
        raise ValueError("native stderr size differs from drain")
    offset = raw.size - min(raw.size, 16 * 1024)
    count = raw.size - offset
    if (
        _native_u64(descriptor.get("tail_offset"), "stderr tail offset") != offset
        or _native_u64(descriptor.get("tail_bytes"), "stderr tail bytes") != count
    ):
        raise ValueError("native stderr bounded tail differs from drain")
    _native_hex(descriptor.get("tail_sha256"), 64, "stderr tail SHA")

    def read_tail(handle):
        metadata = os.fstat(handle.fileno())
        if (
            not stat.S_ISREG(metadata.st_mode)
            or metadata.st_mode & 0o777 != 0o600
            or metadata.st_nlink != 1
            or _native_u64(descriptor.get("file_device"), "stderr device") != metadata.st_dev
            or _native_u64(descriptor.get("file_inode"), "stderr inode") != metadata.st_ino
            or metadata.st_size != raw.size
        ):
            raise ValueError("native stderr file epoch or private mode differs")
        handle.seek(offset)
        tail = handle.read(count)
        if len(tail) != count:
            raise ValueError("native stderr bounded tail truncated")
        return tail

    tail = raw.consume_seekable(read_tail)
    if hashlib.sha256(tail).hexdigest() != descriptor["tail_sha256"]:
        raise ValueError("native stderr bounded tail hash differs")
    selected = []
    for index, chunk in enumerate(tail.split(b"\n")[:-1]):
        if offset and index == 0:
            continue
        if not chunk.startswith(b"native-object-io"):
            continue
        try:
            line = chunk.decode("ascii")
        except UnicodeDecodeError as error:
            raise ValueError("native object IO summary is not ASCII") from error
        if f"capture_id={capture_id}" not in line.split(" "):
            continue
        fields = {}
        for item in line.removeprefix("native-object-io ").split(" "):
            key, separator, value = item.partition("=")
            if not separator or not key or not value or key in fields:
                raise ValueError("native object IO field is missing or repeated")
            fields[key] = value
        if set(fields) != NATIVE_IO_FIELDS | {"capture_id", "status"}:
            raise ValueError("native object IO fields differ from fixed vocabulary")
        if fields["capture_id"] != capture_id or fields["status"] != "quiescent":
            raise ValueError("native object IO capture/status differs")
        parsed = {"capture_id": capture_id, "status": "quiescent"}
        for key in NATIVE_IO_FIELDS:
            value = fields[key]
            if not value.isascii() or not value.isdecimal() or len(value) > 20:
                raise ValueError(f"native object IO {key} is not unsigned decimal")
            parsed[key] = _native_u64(int(value), key)
        _native_check_write_failures(parsed, "stderr")
        selected.append(parsed)
    if len(selected) != 1:
        raise ValueError("native object IO summary missing or repeated in bounded tail")
    return selected[0]


SMALL_PHASES = frozenset(
    {
        "full_ingest_seal",
        "full_activate",
        "delta_ingest_seal",
        "delta_activate",
        "noop_seal",
        "noop_activate",
    }
)
LARGE_PHASES = frozenset(
    {
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
    }
)
DEFAULT_TIMEOUT_MS = 600_000
DEFAULT_HISTORY_MAX_BYTES = 1024 * 1024 * 1024
HISTORY_MAX_TOTAL_BYTES = 2 * 1024 * 1024 * 1024
HISTORY_MAX_REVISION_PAIRS = 128


def _exact_int(value: object, expected: int, label: str) -> None:
    if type(value) is not int or value != expected:
        raise ValueError(f"{label} does not match declared scale input")


def _disk_allocation(snapshot: object, label: str) -> int | None:
    counters = ("allocated_root_bytes", "filesystem_free_bytes", "filesystem_available_bytes")
    if not isinstance(snapshot, dict) or set(snapshot) != {
        "status",
        "unavailable_reason",
        *counters,
    }:
        raise ValueError(f"{label} snapshot is missing or malformed")
    reason = snapshot["unavailable_reason"]
    if snapshot["status"] == "observed":
        if reason is not None or any(
            type(snapshot[key]) is not int or not 0 <= snapshot[key] <= (1 << 64) - 1
            for key in counters
        ):
            raise ValueError(f"{label} observed counters are invalid")
        return snapshot["allocated_root_bytes"]
    if (
        snapshot["status"] != "unavailable"
        or not isinstance(reason, str)
        or not reason.strip()
        or any(snapshot[key] is not None for key in counters)
    ):
        raise ValueError(f"{label} unavailable state is invalid")
    return None


def _declared_inputs(
    manifest_raw: bytes,
    tier: dict,
    *,
    expected_tier: str,
    expected_seed: int,
    requested_client_timeout_ms: int | None,
    requested_history_max_bytes: int | None,
    requested_history_max_total_bytes: int | None,
    requested_disk_observation_mode: str | None,
) -> dict:
    if (
        expected_tier not in TIER_SHAPE
        or type(expected_seed) is not int
        or not 0 <= expected_seed < 1 << 64
    ):
        raise ValueError("invalid declared scale tier or seed")
    if requested_client_timeout_ms is not None and (
        type(requested_client_timeout_ms) is not int
        or not 1 <= requested_client_timeout_ms <= 600_000
    ):
        raise ValueError("invalid declared client timeout")
    if requested_history_max_bytes is not None and (
        type(requested_history_max_bytes) is not int
        or not 1 <= requested_history_max_bytes <= (1 << 64) - 1
    ):
        raise ValueError("invalid declared history limit")
    if requested_history_max_total_bytes is not None and (
        type(requested_history_max_total_bytes) is not int
        or not 1 <= requested_history_max_total_bytes <= (1 << 64) - 1
    ):
        raise ValueError("invalid declared total history limit")
    if requested_history_max_total_bytes is not None and requested_history_max_bytes is None:
        raise ValueError("total history override requires an explicit pair bound")
    if requested_disk_observation_mode is not None and requested_disk_observation_mode not in (
        "boundary-only",
        "periodic",
    ):
        raise ValueError("invalid declared disk observation mode")
    disk_observation_mode = requested_disk_observation_mode or "boundary-only"
    effective_total = requested_history_max_total_bytes or HISTORY_MAX_TOTAL_BYTES
    effective_pair = requested_history_max_bytes or DEFAULT_HISTORY_MAX_BYTES
    if effective_pair > effective_total:
        raise ValueError("pair history bound exceeds total history bound")
    policy_id = (
        "explicit-pair-total-diagnostic-v1"
        if requested_history_max_total_bytes is not None
        else "explicit-pair-default-total-v1"
        if requested_history_max_bytes is not None
        else "scale-supported-v1"
    )
    manifest = load(manifest_raw)
    if (
        not isinstance(manifest, dict)
        or manifest.get("kind") != "quanta-index-scale-tier-manifest"
        or type(manifest.get("manifest_schema_version")) is not int
        or manifest["manifest_schema_version"] != 2
        or manifest.get("dimension") != "scale"
        or manifest.get("query_token") != "scale_needle_token"
        or not isinstance(manifest.get("tiers"), list)
        or len(manifest["tiers"]) != len(TIER_SHAPE)
    ):
        raise ValueError("scale tier manifest contract is invalid")
    for row, (name, (repos, files_per_repo)) in zip(
        manifest["tiers"], TIER_SHAPE.items(), strict=True
    ):
        if not isinstance(row, dict) or row.get("tier") != name:
            raise ValueError("scale tier manifest order or name changed")
        for key, expected in (
            ("repo_count", repos),
            ("files_per_repo", files_per_repo),
            ("total_files", repos * files_per_repo),
            ("source_repo_count", repos),
        ):
            _exact_int(row.get(key), expected, f"manifest {name}.{key}")
        if row.get("default_run") is not (name == "small") or row.get("selectable") is not True:
            raise ValueError("scale tier manifest selection policy changed")

    repos, files_per_repo = TIER_SHAPE[expected_tier]
    if tier.get("tier") != expected_tier:
        raise ValueError("measured tier differs from declared command")
    _exact_int(tier.get("seed"), expected_seed, "measured seed")
    _exact_int(tier.get("file_count"), repos * files_per_repo, "measured file count")
    _exact_int(tier.get("source_repo_count"), repos, "measured source repo count")
    if tier.get("disk_observation_mode") != disk_observation_mode:
        raise ValueError("measured disk observation mode differs from declared command")
    policy = {
        "query_dispatch_budget_ms": 600_000,
        "requested_client_request_timeout_ms": requested_client_timeout_ms,
        "client_request_timeout_ms": requested_client_timeout_ms or DEFAULT_TIMEOUT_MS,
        "requested_history_max_bytes": requested_history_max_bytes,
        "history_max_bytes": effective_pair,
        "requested_history_max_total_bytes": requested_history_max_total_bytes,
        "history_policy_id": policy_id,
        "history_max_generations": 2,
        "history_max_revision_pairs": HISTORY_MAX_REVISION_PAIRS,
        "ingest_max_records": 100_000,
        "ingest_max_text_bytes": 134_217_728,
        "ingest_max_vector_bytes": 268_435_456,
        "coverage_max_encoded_bytes": 67_108_864,
        "coverage_max_decode_heap_bytes": 268_435_456,
        "source_publication_part_bytes": 1_048_576,
        "source_publication_max_bytes": 536_870_912,
        "process_memory_ceiling_bytes": 4_294_967_296,
        "history_max_total_bytes": effective_total,
    }
    for key, expected in policy.items():
        if expected is None:
            if key not in tier or tier[key] is not None:
                raise ValueError(f"{key} does not match declared scale input")
        elif isinstance(expected, str):
            if type(tier.get(key)) is not str or tier[key] != expected:
                raise ValueError(f"{key} does not match declared scale input")
        else:
            _exact_int(tier.get(key), expected, key)
    policy["requested_disk_observation_mode"] = requested_disk_observation_mode
    policy["disk_observation_mode"] = disk_observation_mode
    return policy


def _fields(line: str) -> dict[str, str]:
    result: dict[str, str] = {}
    for part in line[len(PREFIX) :].split():
        key, separator, value = part.partition("=")
        if not separator or not key or not value or key in result:
            raise ValueError("malformed or duplicated causal marker field")
        result[key] = value
    if result.get("kind") not in KINDS:
        raise ValueError("unknown causal marker kind")
    return result


def _natural(row: dict[str, str], name: str) -> int:
    value = row[name]
    if not value.isascii() or not value.isdecimal() or len(value) > 20:
        raise ValueError(f"invalid {name} in causal marker")
    parsed = int(value)
    if parsed > (1 << 64) - 1:
        raise ValueError(f"{name} exceeds u64 in causal marker")
    return parsed


def _check_fields(row: dict[str, str], expected: set[str]) -> None:
    if set(row) != expected:
        raise ValueError(f"wrong causal marker fields for {row['kind']}")


def _add_u64(counter: dict[str, int], key: str, amount: int) -> None:
    next_value = counter[key] + amount
    if next_value > (1 << 64) - 1:
        raise ValueError(f"aggregated {key} exceeds u64 in causal marker")
    counter[key] = next_value


def parse_trace(raw: bytes, expected_phases: set[str]) -> dict:
    text = raw.decode("utf-8")
    phases: dict[str, dict] = {}
    unattributed = {"sync_calls": 0, "exact_live_token_scans": 0, "bm25_events": 0}
    active: str | None = None
    for line in text.splitlines():
        if not line.startswith(PREFIX):
            continue
        row = _fields(line)
        kind = row["kind"]
        if kind == "phase_start":
            _check_fields(row, {"kind", "name"})
            name = row["name"]
            if name not in expected_phases or name in phases or active is not None:
                raise ValueError("unexpected, repeated or nested scale phase")
            phases[name] = {
                "status": "running",
                "sync": {},
                "exact_live_token_scan": {
                    "calls": 0,
                    "elapsed_ns": 0,
                    "terms": 0,
                    "postings": 0,
                    "live_postings": 0,
                    "live_tokens": 0,
                },
                "bm25_census_build": {"calls": 0, "elapsed_ns": 0, "docs": 0, "encoded_bytes": 0},
                "bm25_boundary": {},
                "bm25_live_build": [],
            }
            active = name
        elif kind == "phase_end":
            _check_fields(row, {"kind", "name", "ok"})
            if row["name"] != active or row["ok"] not in ("0", "1"):
                raise ValueError("unpaired or malformed scale phase end")
            phases[active]["status"] = "ok" if row["ok"] == "1" else "failed"
            active = None
        elif kind == "sync":
            _check_fields(row, {"kind", "label", "ok", "elapsed_ns"})
            if row["label"] not in SYNC_LABELS or row["ok"] not in ("0", "1"):
                raise ValueError("invalid lexical sync marker")
            if row["ok"] == "0":
                raise ValueError("failed lexical sync in a successful scale artifact")
            elapsed = _natural(row, "elapsed_ns")
            if active is None:
                unattributed["sync_calls"] += 1
                continue
            counter = phases[active]["sync"].setdefault(
                row["label"], {"calls": 0, "failed_calls": 0, "elapsed_ns": 0}
            )
            counter["calls"] += 1
            counter["failed_calls"] += row["ok"] == "0"
            counter["elapsed_ns"] += elapsed
        elif kind == "bm25_census_build":
            if row.get("ok") == "0":
                _check_fields(row, {"kind", "ok", "reason"})
                if row["reason"] != "counter_overflow":
                    raise ValueError("invalid BM25 census failure reason")
                raise ValueError("BM25 census observation overflowed")
            _check_fields(row, {"kind", "ok", "elapsed_ns", "docs", "encoded_bytes"})
            if row["ok"] != "1":
                raise ValueError("failed BM25 census build marker")
            counts = {key: _natural(row, key) for key in ("elapsed_ns", "docs", "encoded_bytes")}
            if counts["docs"] == 0 and counts["encoded_bytes"] != 0:
                raise ValueError("BM25 census bytes without documents")
            if active is None:
                unattributed["bm25_events"] += 1
                continue
            counter = phases[active]["bm25_census_build"]
            _add_u64(counter, "calls", 1)
            for key, value in counts.items():
                _add_u64(counter, key, value)
        elif kind == "bm25_boundary":
            if row.get("ok") == "0":
                _check_fields(row, {"kind", "label", "ok", "reason"})
                if row["label"] not in BM25_BOUNDARY_LABELS or row["reason"] != "counter_overflow":
                    raise ValueError("invalid BM25 boundary failure")
                raise ValueError("BM25 boundary observation overflowed")
            _check_fields(
                row, {"kind", "label", "ok", "elapsed_ns", "logical_bytes", "index_files"}
            )
            if row["ok"] != "1" or row["label"] not in BM25_BOUNDARY_LABELS:
                raise ValueError("invalid BM25 boundary marker")
            counts = {
                key: _natural(row, key) for key in ("elapsed_ns", "logical_bytes", "index_files")
            }
            if active is None:
                unattributed["bm25_events"] += 1
                continue
            counter = phases[active]["bm25_boundary"].setdefault(
                row["label"], {"calls": 0, "elapsed_ns": 0, "logical_bytes": 0, "index_files": 0}
            )
            _add_u64(counter, "calls", 1)
            for key, value in counts.items():
                _add_u64(counter, key, value)
        elif kind == "bm25_live_build":
            if row.get("ok") == "0":
                _check_fields(row, {"kind", "ok", "reason"})
                if row["reason"] not in {"counter_overflow", "retained_estimate_failed"}:
                    raise ValueError("invalid BM25 live build failure reason")
                raise ValueError("BM25 live build observation overflowed")
            _check_fields(row, {"kind", "ok"} | BM25_BUILD_COUNTERS)
            if row["ok"] != "1":
                raise ValueError("failed BM25 live build marker")
            counts = {key: _natural(row, key) for key in BM25_BUILD_COUNTERS}
            if (
                counts["retained_valid"] != 1
                or counts["reused_segments"] + counts["changed_segments"] + counts["new_segments"]
                != counts["segment_fanout"]
                or counts["newly_dead_docs"] > counts["mask_docs"]
                or counts["death_ns"] > counts["changed_ns"]
            ):
                raise ValueError("inconsistent BM25 live build marker")
            # These sibling scopes run synchronously and do not overlap. The
            # death timer is nested within changed_ns and is not added here.
            if counts["changed_ns"] + counts["new_ns"] > counts["elapsed_ns"]:
                raise ValueError("BM25 child call clocks exceed parent")
            if active is None:
                unattributed["bm25_events"] += 1
                continue
            phases[active]["bm25_live_build"].append(counts)
        else:
            _check_fields(
                row,
                {
                    "kind",
                    "ok",
                    "elapsed_ns",
                    "terms",
                    "postings",
                    "live_postings",
                    "live_tokens",
                },
            )
            if row["ok"] != "1":
                raise ValueError("failed exact live token scan marker")
            counts = {key: _natural(row, key) for key in row if key not in ("kind", "ok")}
            if counts["live_postings"] > counts["postings"]:
                raise ValueError("live posting count exceeds scanned postings")
            if active is None:
                unattributed["exact_live_token_scans"] += 1
                continue
            counter = phases[active]["exact_live_token_scan"]
            counter["calls"] += 1
            for key, value in counts.items():
                counter[key] += value
    if active is not None or set(phases) != expected_phases:
        raise ValueError("incomplete scale phase marker coverage")
    if any(phase["status"] != "ok" for phase in phases.values()):
        raise ValueError("failed scale phase marker")
    if not any(phase["sync"] for phase in phases.values()):
        raise ValueError("scale trace has no lexical durability call markers")
    seal_phases = {"full_ingest_seal", "full_seal", "delta_ingest_seal", "noop_seal", "delete_seal"}
    for name, phase in phases.items():
        if name in seal_phases and (
            len(phase["bm25_live_build"]) != 1
            or phase["bm25_boundary"].get("encode", {}).get("calls") != 1
        ):
            raise ValueError(f"{name} lacks one complete BM25 seal observation")
    return {"phases": phases, "unattributed": unattributed}


def validate_native_product_artifacts(
    summary_raw: bytes,
    manifest_raw: bytes,
    *,
    source_revision: str,
    expected_tier: str,
    expected_seed: int,
    expected_daemon_sha256: str,
    expected_foreground_workload: dict,
    expected_churn_cycles: int,
    expected_mixed_load: bool = False,
    expected_trace_mode: str | None = None,
    expected_trace_max_output_bytes: int | None = None,
    expected_resource_sampling: bool = False,
    child_records: dict,
    linux_resource_observation: dict | None = None,
) -> dict:
    """Validate one Native product and any separately clocked raw children.

    Corpus/config digests are bound recorded values, not an independent
    regeneration of the source fixture. `child_records` is prepared from the
    same run and owns immutable RawFile commitments plus parsed trace values.
    """
    _native_hex(source_revision, 40, "source revision")
    _native_hex(expected_daemon_sha256, 64, "expected daemon SHA")
    if linux_resource_observation is not None:
        _native_linux_resource_observation(linux_resource_observation)
    if (
        type(expected_seed) is not int
        or not 0 <= expected_seed <= NATIVE_U64_MAX
        or type(expected_churn_cycles) is not int
        or not 0 <= expected_churn_cycles <= 256
        or type(expected_mixed_load) is not bool
        or type(expected_resource_sampling) is not bool
    ):
        raise ValueError("native declared seed, churn or mixed profile is invalid")
    trace_mode = expected_trace_mode or ("off" if expected_mixed_load else "record")
    if (
        expected_trace_mode not in (None, "off", "record")
        or trace_mode not in ("off", "record")
        or (expected_mixed_load and trace_mode != "off")
        or (
            expected_trace_max_output_bytes is not None
            and (
                type(expected_trace_max_output_bytes) is not int
                or not 6144 <= expected_trace_max_output_bytes <= MAX_CONFIGURED_CAPTURE_BYTES
                or trace_mode != "record"
            )
        )
    ):
        raise ValueError("native declared trace mode or output cap differs from contract")
    off = trace_mode == "off"
    repos, files = _native_manifest(manifest_raw, expected_tier)
    summary = load(summary_raw)
    if (
        not isinstance(summary, dict)
        or type(summary.get("schema_version")) is not int
        or summary["schema_version"] != 2
        or summary.get("dimension") != "scale-index-native"
        or summary.get("mode") != "cold"
        or type(summary.get("concurrency")) is not int
        or summary["concurrency"] != 1
        or not isinstance(summary.get("provenance"), dict)
        or not isinstance(summary.get("detail"), dict)
    ):
        raise ValueError("native BenchArtifactV1 envelope differs")
    provenance = summary["provenance"]
    detail = summary["detail"]
    if provenance.get("git_head") != source_revision:
        raise ValueError("native product source revision differs")
    corpus_digest = _native_digest(provenance.get("corpus_digest"), "corpus digest")
    config_digest = _native_digest(provenance.get("config_digest"), "config digest")
    _native_hex(detail.get("native_config_sha256"), 64, "config SHA")
    if (
        detail.get("backend") != "index-native-configured-daemon-uds"
        or detail.get("scope") != "index-only-source-event-to-ACK-to-searchability"
        or detail.get("tier") != expected_tier
        or type(detail.get("seed")) is not int
        or detail["seed"] != expected_seed
        or type(detail.get("file_count")) is not int
        or detail["file_count"] != repos * files
        or _native_u64(detail.get("source_bytes"), "source bytes") == 0
        or detail.get("daemon_sha256") != expected_daemon_sha256
        or detail.get("child_trace_verified") is not (not off)
    ):
        raise ValueError("native product tier, source or daemon differs from declared inputs")
    expected_limits = (
        expected_foreground_workload.get("work_limits")
        if isinstance(expected_foreground_workload, dict)
        else None
    )
    if (
        not isinstance(expected_limits, dict)
        or set(expected_foreground_workload) != {"mode", "work_limits"}
        or expected_foreground_workload["mode"] not in ("fixed", "explicit")
        or set(expected_limits) != {"reads", "writes", "bytes", "mutations"}
        or any(_native_u64(value, key) == 0 for key, value in expected_limits.items())
        or detail.get("foreground_workload") != expected_foreground_workload
    ):
        raise ValueError("native foreground funding differs from declared input")
    observed_workload = detail["foreground_workload"]
    if (
        not isinstance(observed_workload, dict)
        or set(observed_workload) != {"mode", "work_limits"}
        or type(observed_workload["mode"]) is not str
        or not isinstance(observed_workload["work_limits"], dict)
        or set(observed_workload["work_limits"]) != {"reads", "writes", "bytes", "mutations"}
        or any(
            _native_u64(value, key) == 0 for key, value in observed_workload["work_limits"].items()
        )
    ):
        raise ValueError("native product foreground funding has invalid axes")
    mixed = detail.get("mixed_load")
    if off and (
        "child_trace_files" not in detail
        or detail["child_trace_files"] is not None
        or "child_object_io" not in detail
        or detail["child_object_io"] is not None
        or not isinstance(child_records, dict)
        or child_records
    ):
        raise ValueError("native Off must have no child capture or object IO claim")
    if expected_mixed_load:
        if (
            expected_tier != "large"
            or expected_seed != 42
            or expected_foreground_workload
            != {
                "mode": "explicit",
                "work_limits": {
                    "reads": 67_108_864,
                    "writes": 67_108_864,
                    "bytes": 549_755_813_888,
                    "mutations": 67_108_864,
                },
            }
            or not expected_resource_sampling
            or detail.get("child_trace_files") is not None
            or detail.get("child_object_io") is not None
            or not isinstance(child_records, dict)
            or child_records
        ):
            raise ValueError("native mixed-load requires the fixed sampled Large42 Off profile")
        _native_mixed_load(mixed, 6 + expected_churn_cycles)
    elif mixed is not None:
        raise ValueError("native mixed-load profile differs from declared input")
    churn = detail.get("finite_churn")
    if expected_churn_cycles == 0:
        if churn is not None:
            raise ValueError("native unexpected churn evidence")
    else:
        if not isinstance(churn, dict) or set(churn) != {
            "requested_cycles",
            "completed_cycles",
            "last_attempt",
            "ack_wall_ms",
            "query_wall_ms",
            "max_cycle_wall_ms",
            "scope",
        }:
            raise ValueError("native churn evidence is incomplete")
        for field in ("requested_cycles", "completed_cycles", "last_attempt"):
            if _native_u64(churn[field], f"churn {field}") != expected_churn_cycles:
                raise ValueError(f"native churn {field} differs from declared cycles")
        for field in ("ack_wall_ms", "query_wall_ms", "max_cycle_wall_ms"):
            _native_nonnegative_ms(churn[field], f"churn {field}")
        if churn["scope"] != (
            "finite single-owner CAS churn; not sustained steady-state or P95 qualification"
        ):
            raise ValueError("native churn scope differs from fixed producer")
    resource_observation = detail.get("native_resource_observation")
    if expected_resource_sampling:
        host = summary.get("host")
        if not isinstance(host, dict):
            raise ValueError("native sampled product host absent")
        resource_observation = _native_resource_observation(resource_observation, host.get("os"))
    elif resource_observation is not None:
        raise ValueError("native unexpected resource sampling evidence")
    phases = detail.get("phases")
    expected_phases = NATIVE_BASE_PHASES | (
        NATIVE_CHURN_PHASES if expected_churn_cycles else frozenset()
    )
    if expected_mixed_load and not expected_churn_cycles:
        expected_phases |= {"reopen_churn_search"}
    if not isinstance(phases, dict) or set(phases) != expected_phases:
        raise ValueError("native phase set is missing or unexpected")
    for name, phase in phases.items():
        if not isinstance(phase, dict) or phase.get("completed") is not True:
            raise ValueError(f"native phase {name} is not complete")
        for field in ("wall_ms", "observer_ms"):
            value = phase.get(field)
            if not is_finite_json_number(value) or value < 0:
                raise ValueError(f"native phase {name} has invalid {field}")
        for field in ("root_bytes_before", "root_bytes_after"):
            if phase.get(field) is not None:
                _native_u64(phase[field], f"{name}.{field}")
        before, after = phase.get("root_bytes_before"), phase.get("root_bytes_after")
        expected_net = after - before if before is not None and after is not None else None
        if phase.get("root_net_bytes") != expected_net or (
            expected_net is not None and type(phase["root_net_bytes"]) is not int
        ):
            raise ValueError(f"native phase {name} root delta differs from boundaries")
    if not is_finite_json_number(detail.get("total_ms")) or detail["total_ms"] <= 0:
        raise ValueError("native total wall time is not finite and positive")
    publications = detail.get("publication_outcomes")
    expected_publications = (
        "profile_configure",
        "full_event_ack",
        "delta_event_ack",
        "noop_event_ack",
        "delete_event_ack",
    )
    if not isinstance(publications, list) or len(publications) != (
        len(expected_publications) + expected_churn_cycles
    ):
        raise ValueError("native publication run list is incomplete")
    for index, row in enumerate(publications):
        expected_phase = (
            expected_publications[index] if index < len(expected_publications) else "churn_cycles"
        )
        expected_cycle = (
            index - len(expected_publications) + 1 if expected_phase == "churn_cycles" else None
        )
        _native_publication_ack(row, index + 1, expected_phase, expected_cycle)
    product = {
        "backend": "index-native",
        "summary": summary,
        "provenance": provenance,
        "tier": {"tier": expected_tier, "seed": expected_seed},
        "phases": phases,
        "foreground_workload": expected_foreground_workload,
        "native_resource_observation": deepcopy(resource_observation),
        "daemon_sha256": expected_daemon_sha256,
        "corpus_digest": corpus_digest,
        "config_digest": config_digest,
        "children": {},
        "native_mixed_load": deepcopy(mixed),
        "child_capture_status": (
            "off_no_canonical_capture" if off else "verified_independent_child_captures"
        ),
        "native_trace_mode": trace_mode,
        "source_scope": "recorded source corpus/config digests; no independent corpus byte regeneration",
    }
    if expected_trace_max_output_bytes is not None:
        product["trace_max_output_bytes"] = expected_trace_max_output_bytes
    if linux_resource_observation is not None:
        product["linux_resource_observation"] = deepcopy(linux_resource_observation)
    if expected_mixed_load:
        required_ms = (
            sum(mixed[name]["queries"]["drain_secs"] for name in ("warmup", "measure")) * 1000
        )
        if (
            detail["total_ms"] < required_ms
            or resource_observation["epochs"][0]["observed_elapsed_ms"] < required_ms
        ):
            raise ValueError("native mixed-load duration is outside its process observation")
        # Observed exceedances refute the declared profile. Samples below these
        # ceilings still cannot qualify continuous peaks or unobserved roots.
        sampled_ceilings = (
            ("aggregate_process_rss", 2 * 1024**3),
            ("allocated_native_scratch", 4 * 1024**3),
            ("allocated_state_root", 64 * 1024**3),
        )
        for epoch in resource_observation["epochs"]:
            for field, ceiling in sampled_ceilings:
                observed = epoch[field]["sampled_max_bytes"]
                if observed is not None and observed > ceiling:
                    raise ValueError(
                        f"native mixed-load {epoch['epoch']}.{field} sampled bytes "
                        f"exceed declared ceiling {ceiling}"
                    )
        if (
            linux_resource_observation is not None
            and linux_resource_observation["aggregate_rss_sampled_max_bytes"] > 2 * 1024**3
        ):
            raise ValueError("native mixed-load outer Linux sampled aggregate RSS exceeds 2 GiB")
        return product
    if off:
        return product
    if not isinstance(child_records, dict) or set(child_records) != {"first", "reopen"}:
        raise ValueError("native requires first and reopen raw child records")
    custody = detail.get("child_trace_files")
    if not isinstance(custody, dict) or not isinstance(custody.get("captures"), list):
        raise ValueError("native child capture custody absent")
    entries = custody["captures"]
    if len(entries) != 2 or [entry.get("role") for entry in entries if isinstance(entry, dict)] != [
        "first",
        "reopen",
    ]:
        raise ValueError("native child capture roles are missing or repeated")
    directory = custody.get("directory")
    if not isinstance(directory, str) or not Path(directory).is_absolute():
        raise ValueError("native child capture directory is invalid")
    object_io = detail.get("child_object_io")
    epochs = object_io.get("epochs") if isinstance(object_io, dict) else None
    if not isinstance(epochs, dict) or set(epochs) != {"drain", "final_drain"}:
        raise ValueError("native child object IO epochs are missing")
    for epoch, observation in epochs.items():
        if (
            not isinstance(observation, dict)
            or set(observation) != NATIVE_IO_FIELDS | {"capture_id", "status"}
            or observation.get("status") != "quiescent"
        ):
            raise ValueError(f"native {epoch} object IO observation is malformed")
        _native_hex(observation.get("capture_id"), 32, f"{epoch} capture ID")
        for field in NATIVE_IO_FIELDS:
            _native_u64(observation[field], f"{epoch}.{field}")
        _native_check_write_failures(observation, epoch)
    verified_children = {}
    for role, entry in zip(("first", "reopen"), entries, strict=True):
        record = child_records[role]
        if not isinstance(record, dict) or set(record) != {"trace_file", "trace", "stderr_file"}:
            raise ValueError(f"native {role} raw record malformed")
        trace_file = record["trace_file"]
        stderr_file = record["stderr_file"]
        if not isinstance(trace_file, RawFile) or not isinstance(stderr_file, RawFile):
            raise ValueError(f"native {role} raw custody is not a RawFile")
        if (
            trace_file.path != Path(directory) / f"{role}.jsonl"
            or stderr_file.path != Path(directory) / f"{role}.stderr"
            or entry.get("file") != f"{role}.jsonl"
            or entry.get("path") != str(trace_file.path)
            or entry.get("publication_witness_required") is not (role == "first")
            or _native_u64(entry.get("bytes"), f"{role} trace bytes") != trace_file.size
            or entry.get("sha256") != trace_file.sha256.removeprefix("sha256:")
        ):
            raise ValueError(f"native {role} child raw path, digest or publication differs")
        trace_mode = trace_file.path.stat()
        if (
            not stat.S_ISREG(trace_mode.st_mode)
            or trace_mode.st_mode & 0o777 != 0o600
            or trace_mode.st_nlink != 1
        ):
            raise ValueError(f"native {role} child raw file is not private and singular")
        child = _native_child_trace(
            record["trace"],
            trace_file,
            publication=role == "first",
            trace_max_output_bytes=expected_trace_max_output_bytes,
        )
        if child["capture_id"] != entry.get("capture_id"):
            raise ValueError(f"native {role} child capture ID differs")
        stderr = _native_stderr_summary(stderr_file, entry.get("stderr_tail"), child["capture_id"])
        if epochs["drain" if role == "first" else "final_drain"] != stderr:
            raise ValueError(f"native {role} object IO observation differs from raw stderr")
        for key, fields in (
            ("sync_calls", ("file_sync_calls", "directory_sync_calls", "barrier_calls")),
            ("sync_failures", ("failed_file_syncs", "failed_directory_syncs", "failed_barriers")),
            (
                "sync_elapsed_ns",
                ("file_sync_elapsed_ns", "directory_sync_elapsed_ns", "barrier_elapsed_ns"),
            ),
        ):
            total = sum(stderr[field] for field in fields)
            if total > NATIVE_U64_MAX or total != child["sync"][key]:
                raise ValueError(f"native {role} {key} differs between child and stderr")
        verified_children[role] = {
            "capture_id": child["capture_id"],
            "clock_domain": record["trace"]["clock_domain"],
            "trace_sha256": trace_file.sha256,
            "trace_bytes": trace_file.size,
            "stderr_sha256": stderr_file.sha256,
            "stderr_bytes": stderr_file.size,
            "sync": child["sync"],
            "object_io": stderr,
        }
    if verified_children["first"]["capture_id"] == verified_children["reopen"]["capture_id"]:
        raise ValueError("native reopened child reused first capture identity")
    product["children"] = verified_children
    return product


def validate_product_artifacts(
    summary_raw: bytes,
    manifest_raw: bytes,
    *,
    source_revision: str,
    expected_tier: str,
    expected_seed: int,
    requested_client_timeout_ms: int | None,
    requested_history_max_bytes: int | None,
    requested_history_max_total_bytes: int | None = None,
    requested_disk_observation_mode: str | None = None,
) -> dict:
    """Validate bound scale artifacts without requiring diagnostic trace markers."""
    summary = load(summary_raw)
    if (
        not isinstance(summary, dict)
        or type(summary.get("schema_version")) is not int
        or summary["schema_version"] != 2
    ):
        raise ValueError("expected one BenchArtifactV1 scale summary")
    detail = summary.get("detail")
    provenance = summary.get("provenance")
    if (
        not isinstance(detail, dict)
        or not isinstance(provenance, dict)
        or detail.get("passed") is not True
        or not isinstance(detail.get("measured_tiers"), list)
        or len(detail["measured_tiers"]) != 1
        or provenance.get("git_head") != source_revision
    ):
        raise ValueError("scale artifact is failed, partial or from another source")
    for digest_key in ("corpus_digest", "config_digest"):
        digest = provenance.get(digest_key)
        if (
            not isinstance(digest, str)
            or not digest.startswith("sha256:")
            or len(digest) != 71
            or any(character not in "0123456789abcdef" for character in digest[7:])
        ):
            raise ValueError(f"scale artifact lacks a bound {digest_key}")
    tier = detail["measured_tiers"][0]
    if not isinstance(tier, dict):
        raise ValueError("scale artifact measured tier is malformed")
    resources = tier.get("phase_resources")
    if (
        tier.get("status") != "measured"
        or not isinstance(resources, dict)
        or not resources
        or any(not isinstance(value, dict) for value in resources.values())
    ):
        raise ValueError("scale artifact lacks measured phase resources")
    policy = _declared_inputs(
        manifest_raw,
        tier,
        expected_tier=expected_tier,
        expected_seed=expected_seed,
        requested_client_timeout_ms=requested_client_timeout_ms,
        requested_history_max_bytes=requested_history_max_bytes,
        requested_history_max_total_bytes=requested_history_max_total_bytes,
        requested_disk_observation_mode=requested_disk_observation_mode,
    )
    retention = tier.get("retained_index_bytes")
    expected_seals = {"full", "delta", "noop"}
    if expected_tier != "small":
        expected_seals.add("delete")
    if (
        not isinstance(retention, dict)
        or set(retention) != {"method", "scope", "by_seal"}
        or retention.get("method") != "product_retention_receipt_gauge_v1"
        or retention.get("scope")
        != "one serving repo/revision pair; exact unique-inode regular-file bytes admitted by each seal; total equals pair; unavailable after daemon restart"
        or not isinstance(retention.get("by_seal"), dict)
        or set(retention["by_seal"]) != expected_seals
        or any(
            type(value) is not int
            or not 1 <= value <= (1 << 53)
            or value > policy["history_max_bytes"]
            or value > policy["history_max_total_bytes"]
            for value in retention["by_seal"].values()
        )
    ):
        raise ValueError("scale artifact lacks exact product retention bytes for each seal")
    expected_phases = SMALL_PHASES if expected_tier == "small" else LARGE_PHASES
    if set(resources) != expected_phases:
        raise ValueError("scale artifact has incomplete or unexpected lifecycle phases")
    validated_resources = {}
    for name, observation in resources.items():
        span_ms = observation.get("observation_span_ms")
        if not is_finite_json_number(span_ms) or span_ms <= 0:
            raise ValueError(f"{name} has no positive observed wall span")
        mode = policy["disk_observation_mode"]
        disk = observation.get("disk")
        interval = observation.get("disk_sample_interval_ms", object())
        if not isinstance(disk, dict) or disk.get("observation_mode") != mode:
            raise ValueError(f"{name} disk observation mode differs from declared command")
        if disk.get("sampled_max_is_true_peak") is not False:
            raise ValueError(f"{name} disk sample cannot prove a true peak")
        boundary_bytes = [
            _disk_allocation(disk.get(stage), f"{name} disk boundary {stage}")
            for stage in ("start", "end")
        ]
        if mode == "boundary-only":
            if (
                interval is not None
                or disk.get("sampled_max_allocated_root_bytes", object()) is not None
                or disk.get("sampled_high_water_status") != "unavailable_periodic_sampling_disabled"
                or disk.get("sampled") != []
                or not is_finite_json_number(disk.get("periodic_probe_wall_ms"))
                or disk.get("periodic_probe_wall_ms") != 0
            ):
                raise ValueError(f"{name} boundary disk evidence claims periodic sampling")
        else:
            peak = disk.get("sampled_max_allocated_root_bytes", object())
            if (
                type(interval) is not int
                or interval != 500
                or not isinstance(disk.get("sampled"), list)
                or not is_finite_json_number(disk.get("periodic_probe_wall_ms"))
                or disk["periodic_probe_wall_ms"] < 0
                or (peak is not None and (type(peak) is not int or peak < 0))
                or disk.get("sampled_high_water_status")
                != ("observed" if peak is not None else "unavailable_no_interior_allocation_sample")
            ):
                raise ValueError(f"{name} periodic disk evidence is inconsistent")
        previous_offset = 0.0
        maximum_gap = 0.0
        interior_high = None
        for sample in disk["sampled"]:
            if not isinstance(sample, dict):
                raise ValueError(f"{name} disk sample is malformed")
            offset = sample.get("offset_ms")
            snapshot = sample.get("snapshot")
            if (
                not is_finite_json_number(offset)
                or not previous_offset < offset < span_ms
                or not isinstance(snapshot, dict)
            ):
                raise ValueError(f"{name} disk sample offset is invalid")
            maximum_gap = max(maximum_gap, offset - previous_offset)
            previous_offset = offset
            allocated = _disk_allocation(snapshot, f"{name} disk sample")
            if allocated is not None:
                interior_high = (
                    allocated if interior_high is None else max(interior_high, allocated)
                )
        maximum_gap = max(maximum_gap, span_ms - previous_offset)
        observed_gap = disk.get("observed_max_gap_ms")
        if not is_finite_json_number(observed_gap) or abs(observed_gap - maximum_gap) > 0.000_001:
            raise ValueError(f"{name} disk sample gap differs from observed offsets")
        if mode == "periodic":
            if interior_high is None:
                derived_high = None
            else:
                derived_high = max(interior_high, *(value or 0 for value in boundary_bytes))
            if disk["sampled_max_allocated_root_bytes"] != derived_high:
                raise ValueError(f"{name} disk sampled maximum differs from observed samples")
        disk_observation = {
            "mode": mode,
            "start": disk["start"],
            "end": disk["end"],
            "sample_interval_ms": interval,
            "sampled_max_allocated_root_bytes": disk["sampled_max_allocated_root_bytes"],
            "sampled_high_water_status": disk["sampled_high_water_status"],
            "sample_count": len(disk["sampled"]),
            "periodic_probe_wall_ms": disk["periodic_probe_wall_ms"],
            "observed_max_gap_ms": observed_gap,
        }
        io = observation.get("process_write_io")
        reason = observation.get("process_write_io_unavailable_reason")
        if (io is None) == (reason is None):
            raise ValueError(f"{name} process write I/O state is ambiguous")
        if reason is not None and (not isinstance(reason, str) or not reason):
            raise ValueError(f"{name} process write I/O reason is invalid")
        if io is not None and (
            not isinstance(io, dict)
            or set(io) != {"write_bytes", "cancelled_write_bytes", "syscw", "wchar"}
            or any(type(value) is not int for value in io.values())
            or any(not 0 <= io[key] <= (1 << 64) - 1 for key in ("write_bytes", "syscw", "wchar"))
            or not -(1 << 63) <= io["cancelled_write_bytes"] <= (1 << 63) - 1
        ):
            raise ValueError(f"{name} process write I/O counters are invalid")
        cpu = {}
        for cpu_key in ("cpu_process_user_ms", "cpu_process_system_ms"):
            cpu_value = observation.get(cpu_key)
            if not is_finite_json_number(cpu_value) or cpu_value < 0:
                raise ValueError(f"{name} {cpu_key} is invalid")
            cpu[cpu_key] = cpu_value
        validated_resources[name] = {
            "observation_span_ms": span_ms,
            "disk_observation": disk_observation,
            "process_write_io": io,
            "process_write_io_unavailable_reason": reason,
            **cpu,
        }
    return {
        "summary": summary,
        "provenance": provenance,
        "tier": tier,
        "runtime_config": policy,
        "retained_index_bytes": retention,
        "phase_resources": validated_resources,
    }


def replay(
    summary_raw: bytes,
    trace_raw: bytes,
    binary_raw: bytes,
    manifest_raw: bytes,
    *,
    source_revision: str,
    expected_tier: str,
    expected_seed: int,
    requested_client_timeout_ms: int | None,
    requested_history_max_bytes: int | None,
    requested_history_max_total_bytes: int | None = None,
    requested_disk_observation_mode: str | None = None,
) -> dict:
    product = validate_product_artifacts(
        summary_raw,
        manifest_raw,
        source_revision=source_revision,
        expected_tier=expected_tier,
        expected_seed=expected_seed,
        requested_client_timeout_ms=requested_client_timeout_ms,
        requested_history_max_bytes=requested_history_max_bytes,
        requested_history_max_total_bytes=requested_history_max_total_bytes,
        requested_disk_observation_mode=requested_disk_observation_mode,
    )
    tier = product["tier"]
    provenance = product["provenance"]
    policy = product["runtime_config"]
    retention = product["retained_index_bytes"]
    trace = parse_trace(trace_raw, set(product["phase_resources"]))
    for name, resource in product["phase_resources"].items():
        phase = trace["phases"][name]
        phase.update(resource)
        sync_ns = sum(counter["elapsed_ns"] for counter in phase["sync"].values())
        scan_ns = phase["exact_live_token_scan"]["elapsed_ns"]
        phase["sync_call_elapsed_ns_sum"] = sync_ns
        phase["exact_scan_elapsed_ns_sum"] = scan_ns
        phase["sync_call_wall_ratio"] = sync_ns / (resource["observation_span_ms"] * 1_000_000)
        phase["exact_scan_wall_ratio"] = scan_ns / (resource["observation_span_ms"] * 1_000_000)
    return {
        "schema_version": 1,
        "status": "diagnostic_unqualified",
        "source_revision": source_revision,
        "binary_sha256": sha(binary_raw),
        "summary_sha256": sha(summary_raw),
        "trace_sha256": sha(trace_raw),
        "corpus_digest": provenance.get("corpus_digest"),
        "config_digest": provenance.get("config_digest"),
        "tier": tier["tier"],
        "seed": tier["seed"],
        "file_count": tier["file_count"],
        "runtime_config": policy,
        "retained_index_bytes": retention,
        "scope": {
            "sync": "all explicit sync_all calls in quanta-index-lexical; dependency and semantic sync excluded",
            "posting": "Tantivy deleted-segment pure-string exact-live-token-count pass only",
            "bm25": "census build and seal call envelopes; logical bytes count encoded payloads already handled by the caller, not physical read/write bytes; changed/death timers are nested",
            "io": "Linux /proc/self/io process counters; not device-completed bytes, fsync latency or phase-exclusive I/O",
            "wall": "opt-in instrumentation perturbs timing; unprofiled matched source run needed for effect",
            "ratios": "sums of call envelopes divided by phase observation span; concurrent work may overlap, so shares are diagnostic and not additive",
        },
        **trace,
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--summary", type=Path, required=True)
    parser.add_argument("--manifest", type=Path, required=True)
    parser.add_argument("--trace", type=Path, required=True)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--source-revision", required=True)
    parser.add_argument("--tier", choices=tuple(TIER_SHAPE), required=True)
    parser.add_argument("--seed", type=int, required=True)
    parser.add_argument("--client-timeout-ms", type=int)
    parser.add_argument("--history-max-bytes", type=int)
    parser.add_argument("--history-max-total-bytes", type=int)
    parser.add_argument("--disk-observation", choices=("boundary-only", "periodic"))
    parser.add_argument("--out", type=Path, required=True)
    args = parser.parse_args()
    try:
        if len(args.source_revision) != 40 or any(
            c not in "0123456789abcdef" for c in args.source_revision
        ):
            raise ValueError("source revision must be a full lowercase commit SHA")
        result = replay(
            args.summary.read_bytes(),
            args.trace.read_bytes(),
            args.binary.read_bytes(),
            args.manifest.read_bytes(),
            source_revision=args.source_revision,
            expected_tier=args.tier,
            expected_seed=args.seed,
            requested_client_timeout_ms=args.client_timeout_ms,
            requested_history_max_bytes=args.history_max_bytes,
            requested_history_max_total_bytes=args.history_max_total_bytes,
            requested_disk_observation_mode=args.disk_observation,
        )
        with args.out.open("xb") as stream:
            stream.write(canonical(result) + b"\n")
    except (ValueError, OSError, KeyError, TypeError) as error:
        parser.exit(2, f"causal cost replay refused: {error}\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
