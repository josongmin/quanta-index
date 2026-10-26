"""Independent typed trace gold for canonical phase custody and metric refusal."""

from __future__ import annotations

import copy
import json

import pytest

from tools.benchmark.retrieval.causal_cost_canonical import canonical_profile
from tools.benchmark.retrieval.causal_cost_timeline import owner_phase, parse


def fixture(phase_name="full_ingest_seal", request_end=80) -> tuple[dict, dict, dict]:
    identity = "0" * 32
    header = {
        "record_type": "header",
        "schema_version": 2,
        "capture_id": identity,
        "stream_id": identity,
        "clock_domain": identity,
        "process_instance": identity,
        "started_ns": 0,
        "max_record_bytes": 4096,
        "max_counters_per_event": 1,
        "coverage": ["harness", "lexical"],
    }
    events = []

    def event(ts, kind):
        events.append(
            {
                "record_type": "event",
                "output_seq": len(events) + 1,
                "capture_id": identity,
                "ts_ns": ts,
                "kind": kind,
            }
        )

    # The daemon request is a separate root. Its actual link, not time overlap,
    # makes the lexical owner belong to the full-ingest phase.
    for span_id, stage, parent, start, end in (
        (1, "harness_phase", None, 0, 100),
        (2, "client_attempt", 1, 10, 90),
        (3, "request", None, 20, request_end),
        (4, "lexical_build", 3, 30, 70),
        (5, "bm25_census_build", 4, 35, 40),
        (6, "bm25_boundary", 4, 41, 45),
        (7, "bm25_live_build", 4, 46, 55),
    ):
        begin = {"type": "span_begin", "span_id": span_id, "stage": stage, "lane_id": 1}
        if parent is None:
            begin["root_kind"] = "request" if stage == "request" else "operation"
        else:
            begin["parent_span_id"] = parent
        event(start, begin)
        event(end, {"type": "span_end", "span_id": span_id, "outcome": "ok"})
    event(1, {"type": "phase_identity", "span_id": 1, "phase": phase_name})
    event(
        21,
        {
            "type": "request_identity",
            "span_id": 3,
            "request_id": 4,
            "plane": "control",
            "connection_id": 1,
        },
    )
    event(22, {"type": "link", "span_id": 3, "target_capture_id": identity, "target_span_id": 2})
    event(
        65,
        {
            "type": "sync_aggregate",
            "span_id": 4,
            "label": "pool_sync_all",
            "calls": 3,
            "failures": 0,
            "elapsed_sum_ns": 9,
            "elapsed_max_ns": 4,
        },
    )
    event(
        39,
        {
            "type": "bm25_census_aggregate",
            "span_id": 5,
            "calls": 1,
            "elapsed_ns": 3,
            "docs": 2,
            "encoded_bytes": 8,
        },
    )
    event(
        44,
        {
            "type": "bm25_boundary_aggregate",
            "span_id": 6,
            "label": "encode",
            "calls": 1,
            "elapsed_sum_ns": 2,
            "logical_bytes": 8,
            "index_files": 1,
        },
    )
    event(
        54,
        {
            "type": "bm25_live_build_aggregate",
            "span_id": 7,
            "calls": 1,
            "elapsed_ns": 6,
            "reused_segments": 0,
            "changed_segments": 0,
            "new_segments": 1,
            "mask_docs": 0,
            "new_census_docs": 2,
            "newly_dead_docs": 0,
            "logical_census_bytes": 8,
            "changed_ns": 0,
            "new_ns": 5,
            "death_ns": 0,
            "correction_keys": 0,
            "segment_fanout": 1,
            "retained_estimate_bytes": 16,
        },
    )
    footer = {
        "record_type": "footer",
        "schema_version": 2,
        "capture_id": identity,
        "closed_ns": 101,
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
    capture = parse(b"".join(json.dumps(row).encode() + b"\n" for row in [header, *events, footer]))
    product = {
        "provenance": {
            "git_head": "a" * 40,
            "corpus_digest": "sha256:" + "b" * 64,
            "config_digest": "sha256:" + "c" * 64,
        },
        "tier": {"tier": "small", "seed": 7},
        "runtime_config": {},
        "phase_resources": {
            phase_name: {
                "observation_span_ms": 0.0002,
                "process_write_io": None,
                "process_write_io_unavailable_reason": "unsupported OS",
            }
        },
    }
    binding = {
        "source_revision": "a" * 40,
        "binary_sha256": "d" * 64,
        "summary_sha256": "e" * 64,
        "tier_manifest_sha256": "f" * 64,
        "timeline_sha256": capture["sha256"],
        "timeline_capture_id": capture["capture_id"],
        "product_status": "PRODUCT_ARTIFACT_VERIFIED",
        "build_binding": "UNPROVEN",
    }
    return capture, product, binding


def test_profile_uses_verified_flow_and_preserves_unavailable_io_and_nested_clocks():
    capture, product, binding = fixture()
    profile = canonical_profile(capture, product, execution_binding=binding)
    phase = profile["phases"]["full_ingest_seal"]
    assert owner_phase(capture, 4) == "full_ingest_seal"
    assert phase["sync"]["pool_sync_all"] == {
        "calls": 3,
        "failures": 0,
        "elapsed_sum_ns": 9,
        "elapsed_max_ns": 4,
    }
    assert phase["duration_ns"] == 100
    assert phase["resources"]["process_write_io"] is None
    assert phase["resources"]["process_write_io_unavailable_reason"] == "unsupported OS"
    assert next(row for row in phase["owner_spans"] if row["span_id"] == 4)["exclusive_ns"] == 22
    assert profile["build_binding"] == "UNPROVEN"
    assert profile["status"] == "diagnostic_unqualified"
    assert "wall_share" not in phase


def test_native_phase_wire_is_parseable_without_legacy_profile_promotion():
    capture, product, binding = fixture(phase_name="native_full_event_ack")
    assert owner_phase(capture, 4) == "native_full_event_ack"
    with pytest.raises(ValueError, match="no measured ingest seal"):
        canonical_profile(capture, product, execution_binding=binding)


def _native_fixture() -> tuple[dict, dict, dict]:
    capture, product, binding = fixture(phase_name="native_full_event_ack")
    product.update(
        {
            "backend": "index-native",
            "daemon_sha256": "1" * 64,
            "corpus_digest": product["provenance"]["corpus_digest"],
            "config_digest": product["provenance"]["config_digest"],
            "foreground_workload": {
                "mode": "fixed",
                "work_limits": {
                    "reads": 1,
                    "writes": 1,
                    "bytes": 1,
                    "mutations": 1,
                },
            },
            "phases": {"full_event_ack": {"completed": True, "wall_ms": 1.0, "observer_ms": 0.0}},
            "children": {"first": {"capture_id": "1" * 32}, "reopen": {"capture_id": "2" * 32}},
        }
    )
    binding.update({"native_daemon_sha256": "1" * 64, "native_daemon_sha256_after": "1" * 64})
    return capture, product, binding


def test_native_profile_retains_validated_resource_observations_without_aliasing():
    capture, product, binding = _native_fixture()
    observation = {
        "mode": "opt_in_diagnostic_v2",
        "continuous_peak": False,
        "sampled_only": True,
        "epochs": [{"epoch": "first_daemon", "child_rss": {"sampled_max_bytes": 8192}}],
    }
    product["native_resource_observation"] = observation
    result = canonical_profile(capture, product, execution_binding=binding)
    assert result["native_resource_observation"] == observation
    observation["epochs"][0]["child_rss"]["sampled_max_bytes"] = 16384
    assert (
        result["native_resource_observation"]["epochs"][0]["child_rss"]["sampled_max_bytes"] == 8192
    )
    assert result["scope"]["performance_qualification"] == "NOT_RUN"


def test_native_canonical_keeps_parent_phases_and_child_clocks_separate():
    capture, product, binding = _native_fixture()
    result = canonical_profile(capture, product, execution_binding=binding)
    assert result["phases"]["full_event_ack"]["duration_ns"] == 100
    assert result["children"]["first"]["capture_id"] == "1" * 32
    assert result["scope"]["legacy_equivalence"] == "NOT_APPLICABLE_NATIVE"
    assert result["scope"]["performance_qualification"] == "NOT_RUN"


@pytest.mark.parametrize("fault", ("daemon_after", "daemon_product", "phase", "child_id", "loss"))
def test_native_canonical_refuses_binding_or_phase_contradiction(fault):
    capture, product, binding = _native_fixture()
    if fault == "daemon_after":
        binding["native_daemon_sha256_after"] = "2" * 64
    elif fault == "daemon_product":
        product["daemon_sha256"] = "2" * 64
    elif fault == "phase":
        product["phases"]["other"] = {"completed": True}
    elif fault == "child_id":
        product["children"]["first"]["capture_id"] = capture["capture_id"]
    else:
        capture["footer"]["events_dropped"] = 1
    with pytest.raises(ValueError):
        canonical_profile(capture, product, execution_binding=binding)


@pytest.mark.parametrize(
    "family",
    [
        "sync_aggregates",
        "bm25_census_aggregates",
        "bm25_boundary_aggregates",
        "bm25_live_build_aggregates",
    ],
)
def test_missing_family_never_becomes_zero(family):
    capture, product, binding = fixture()
    capture[family] = []
    with pytest.raises(ValueError, match="lacks"):
        canonical_profile(capture, product, execution_binding=binding)


@pytest.mark.parametrize(
    "fault", ["link", "interval", "binding", "phase", "loss", "duplicate", "overflow"]
)
def test_profile_refuses_contradiction(fault):
    capture, product, binding = fixture()
    if fault == "link":
        capture["verified_flows"] = []
    elif fault == "interval":
        capture["spans"][4]["end_ns"] = 95
    elif fault == "binding":
        binding["timeline_sha256"] = "f" * 64
    elif fault == "phase":
        capture["spans"][1]["phase"] = "delta_ingest_seal"
    elif fault == "loss":
        capture["footer"]["events_dropped"] = 1
    elif fault == "duplicate":
        capture["sync_aggregates"].append(copy.deepcopy(capture["sync_aggregates"][0]))
    else:
        capture["sync_aggregates"][0]["calls"] = 1 << 64
    with pytest.raises(ValueError):
        canonical_profile(capture, product, execution_binding=binding)


def test_separate_full_seal_requires_its_own_typed_metrics():
    capture, product, binding = fixture("full_seal")
    profile = canonical_profile(capture, product, execution_binding=binding)
    assert profile["phases"]["full_seal"]["sync"]["pool_sync_all"]["calls"] == 3
    capture["bm25_live_build_aggregates"] = []
    with pytest.raises(ValueError, match="full_seal lacks"):
        canonical_profile(capture, product, execution_binding=binding)


def test_server_post_write_root_tail_does_not_invalidate_completed_computation():
    capture, product, binding = fixture(request_end=101)
    profile = canonical_profile(capture, product, execution_binding=binding)
    assert owner_phase(capture, 3) == "full_ingest_seal"
    assert profile["phases"]["full_ingest_seal"]["sync"]["pool_sync_all"]["calls"] == 3
