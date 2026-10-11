"""Closed canonical capture examples for complete, partial and invalid replay."""

from __future__ import annotations

import copy
import json

import pytest

from tools.benchmark.retrieval.causal_cost_timeline import (
    chrome_export,
    parse,
    reconcile_legacy,
    summarize,
)

CAPTURE = "0" * 32


def _line(value: dict) -> bytes:
    return json.dumps(value, separators=(",", ":")).encode() + b"\n"


def _capture(*, footer: bool = True, end: bool = True) -> bytes:
    rows = [
        {
            "record_type": "header",
            "schema_version": 1,
            "capture_id": CAPTURE,
            "stream_id": CAPTURE,
            "clock_domain": CAPTURE,
            "process_instance": CAPTURE,
            "started_ns": 0,
            "max_record_bytes": 4096,
            "max_counters_per_event": 1,
            "coverage": [],
        },
        {
            "record_type": "event",
            "output_seq": 1,
            "capture_id": CAPTURE,
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
            "capture_id": CAPTURE,
            "ts_ns": 20,
            "kind": {
                "type": "span_begin",
                "span_id": 2,
                "parent_span_id": 1,
                "stage": "query_candidate",
                "lane_id": 0,
            },
        },
        {
            "record_type": "event",
            "output_seq": 3,
            "capture_id": CAPTURE,
            "ts_ns": 25,
            "kind": {"type": "counter", "span_id": 2, "key": "candidates", "value": 3},
        },
    ]
    if end:
        rows.extend(
            [
                {
                    "record_type": "event",
                    "output_seq": 4,
                    "capture_id": CAPTURE,
                    "ts_ns": 35,
                    "kind": {"type": "span_end", "span_id": 2, "outcome": "ok"},
                },
                {
                    "record_type": "event",
                    "output_seq": 5,
                    "capture_id": CAPTURE,
                    "ts_ns": 40,
                    "kind": {"type": "span_end", "span_id": 1, "outcome": "ok"},
                },
            ]
        )
    if footer:
        events = len(rows) - 1
        rows.append(
            {
                "record_type": "footer",
                "schema_version": 1,
                "capture_id": CAPTURE,
                "closed_ns": 50,
                "status": "complete" if end else "partial",
                "close_reason": "normal",
                "last_output_seq": events,
                "producer_leases": 0,
                "events_attempted": events,
                "events_accepted": events,
                "events_dropped": 0,
                "events_written": events,
                "events_unwritten": 0,
                "open_spans": 0 if end else 2,
            }
        )
    return b"".join(map(_line, rows))


def _capture_with_work_aggregates() -> bytes:
    rows = [json.loads(line) for line in _capture().splitlines()]
    rows[0]["schema_version"] = 2
    footer = rows.pop()
    rows.extend(
        [
            {
                "record_type": "event",
                "output_seq": 6,
                "capture_id": CAPTURE,
                "ts_ns": 26,
                "kind": {
                    "type": "sync_aggregate",
                    "span_id": 2,
                    "label": "pool_sync_all",
                    "calls": 3,
                    "elapsed_sum_ns": 900,
                    "elapsed_max_ns": 500,
                    "failures": 1,
                },
            },
            {
                "record_type": "event",
                "output_seq": 7,
                "capture_id": CAPTURE,
                "ts_ns": 27,
                "kind": {
                    "type": "adoption_group_aggregate",
                    "span_id": 2,
                    "reason": "ref_limit",
                    "refs_bucket": "up_to_100_percent",
                    "bytes_bucket": "up_to_25_percent",
                    "groups": 2,
                    "failed_groups": 1,
                },
            },
        ]
    )
    footer.update(
        schema_version=2,
        last_output_seq=7,
        events_attempted=7,
        events_accepted=7,
        events_written=7,
    )
    rows.append(footer)
    return b"".join(map(_line, rows))


def _capture_with_object_kind_aggregate() -> bytes:
    rows = [json.loads(line) for line in _capture().splitlines()]
    rows[0]["schema_version"] = 2
    rows[3]["kind"]["key"] = "compacted_segments"
    rows.insert(
        -1,
        {
            "record_type": "event",
            "output_seq": 6,
            "capture_id": CAPTURE,
            "ts_ns": 26,
            "kind": {
                "type": "object_kind_aggregate",
                "span_id": 2,
                "kind": "file_postings",
                "new_objects": 3,
                "reused_objects": 5,
            },
        },
    )
    rows[-1].update(
        schema_version=2,
        last_output_seq=6,
        events_attempted=6,
        events_accepted=6,
        events_written=6,
    )
    return b"".join(map(_line, rows))


def _bm25_rows() -> list[dict]:
    baseline = [json.loads(line) for line in _capture().splitlines()]
    header, footer = baseline[0], baseline[-1]
    header["schema_version"] = 2
    footer["schema_version"] = 2
    rows = [header]

    def event(ts_ns: int, kind: dict) -> None:
        rows.append(
            {
                "record_type": "event",
                "output_seq": len(rows),
                "capture_id": CAPTURE,
                "ts_ns": ts_ns,
                "kind": kind,
            }
        )

    event(
        10,
        {
            "type": "span_begin",
            "span_id": 1,
            "root_kind": "operation",
            "stage": "operation",
            "lane_id": 0,
        },
    )
    aggregates = (
        (
            2,
            "bm25_census_build",
            {
                "type": "bm25_census_aggregate",
                "calls": 1,
                "elapsed_ns": 100,
                "docs": 2,
                "encoded_bytes": 50,
            },
        ),
        (
            3,
            "bm25_boundary",
            {
                "type": "bm25_boundary_aggregate",
                "label": "base_read",
                "calls": 1,
                "elapsed_sum_ns": 40,
                "logical_bytes": 50,
                "index_files": 1,
            },
        ),
        (
            4,
            "bm25_live_build",
            {
                "type": "bm25_live_build_aggregate",
                "calls": 1,
                "elapsed_ns": 1000,
                "reused_segments": 1,
                "changed_segments": 1,
                "new_segments": 1,
                "mask_docs": 2,
                "new_census_docs": 1,
                "newly_dead_docs": 1,
                "logical_census_bytes": 50,
                "changed_ns": 400,
                "new_ns": 400,
                "death_ns": 100,
                "correction_keys": 2,
                "segment_fanout": 3,
                "retained_estimate_bytes": 4096,
            },
        ),
    )
    ts_ns = 11
    for span_id, stage, aggregate in aggregates:
        event(
            ts_ns,
            {
                "type": "span_begin",
                "span_id": span_id,
                "parent_span_id": 1,
                "stage": stage,
                "lane_id": 0,
            },
        )
        event(ts_ns + 1, {"span_id": span_id, **aggregate})
        event(ts_ns + 2, {"type": "span_end", "span_id": span_id, "outcome": "ok"})
        ts_ns += 3
    event(ts_ns, {"type": "span_end", "span_id": 1, "outcome": "ok"})
    count = len(rows) - 1
    footer.update(
        closed_ns=ts_ns + 1,
        last_output_seq=count,
        events_attempted=count,
        events_accepted=count,
        events_written=count,
    )
    rows.append(footer)
    return rows


def _bm25_kind(rows: list[dict], event_type: str) -> dict:
    return next(row["kind"] for row in rows if row.get("kind", {}).get("type") == event_type)


def _linked_capture(*, duplicate: bool = False) -> bytes:
    rows = [json.loads(line) for line in _capture().splitlines()]
    header, footer = rows[0], rows[-1]
    kinds = [
        (
            10,
            {
                "type": "span_begin",
                "span_id": 1,
                "root_kind": "operation",
                "stage": "operation",
                "lane_id": 0,
            },
        ),
        (
            12,
            {
                "type": "span_begin",
                "span_id": 2,
                "parent_span_id": 1,
                "stage": "client_attempt",
                "lane_id": 0,
            },
        ),
        (
            16,
            {
                "type": "span_begin",
                "span_id": 3,
                "root_kind": "request",
                "stage": "request",
                "lane_id": 0,
            },
        ),
        (
            17,
            {
                "type": "request_identity",
                "span_id": 3,
                "request_id": (1 << 53) + 7,
                "plane": "query",
                "connection_id": (1 << 53) + 9,
            },
        ),
        (18, {"type": "link", "span_id": 3, "target_capture_id": CAPTURE, "target_span_id": 2}),
        (30, {"type": "span_end", "span_id": 3, "outcome": "ok"}),
        (32, {"type": "span_end", "span_id": 2, "outcome": "ok"}),
        (35, {"type": "span_end", "span_id": 1, "outcome": "ok"}),
    ]
    if duplicate:
        kinds.insert(5, kinds[4])
    events = [
        {
            "record_type": "event",
            "output_seq": seq,
            "capture_id": CAPTURE,
            "ts_ns": ts,
            "kind": kind,
        }
        for seq, (ts, kind) in enumerate(kinds, 1)
    ]
    count = len(events)
    footer.update(
        last_output_seq=count,
        events_attempted=count,
        events_accepted=count,
        events_written=count,
        closed_ns=40,
    )
    return b"".join(map(_line, [header, *events, footer]))


def _phase_capture(*, sync_calls: int = 3, end: bool = True) -> bytes:
    rows = [json.loads(line) for line in _capture_with_work_aggregates().splitlines()]
    rows[1]["kind"].update(root_kind="capture", stage="harness_phase")
    rows[3]["kind"].update(key="candidates")
    rows[6]["kind"].update(calls=sync_calls, failures=0, elapsed_sum_ns=950)
    rows.insert(
        -1,
        {
            "record_type": "event",
            "output_seq": 8,
            "capture_id": CAPTURE,
            "ts_ns": 12,
            "kind": {"type": "phase_identity", "span_id": 1, "phase": "full_seal"},
        },
    )
    if not end:
        rows = [row for row in rows if row.get("kind", {}).get("type") != "span_end"]
        rows[-1]["status"] = "partial"
        rows[-1]["open_spans"] = 2
    for seq, row in enumerate(rows[1:-1], 1):
        row["output_seq"] = seq
    count = len(rows) - 2
    rows[-1].update(
        last_output_seq=count,
        events_attempted=count,
        events_accepted=count,
        events_written=count,
    )
    return b"".join(map(_line, rows))


def _phase_capture_with_bm25() -> bytes:
    rows = [json.loads(line) for line in _phase_capture().splitlines()]
    footer = rows.pop()
    aggregates = (
        (3, "bm25_census_build", _bm25_kind(_bm25_rows(), "bm25_census_aggregate")),
        (4, "bm25_boundary", _bm25_kind(_bm25_rows(), "bm25_boundary_aggregate")),
        (5, "bm25_live_build", _bm25_kind(_bm25_rows(), "bm25_live_build_aggregate")),
    )
    for span_id, stage, aggregate in aggregates:
        started = 13 + (span_id - 3) * 3
        for ts_ns, kind in (
            (
                started,
                {
                    "type": "span_begin",
                    "span_id": span_id,
                    "parent_span_id": 1,
                    "stage": stage,
                    "lane_id": 0,
                },
            ),
            (started + 1, {**aggregate, "span_id": span_id}),
            (started + 2, {"type": "span_end", "span_id": span_id, "outcome": "ok"}),
        ):
            rows.append(
                {
                    "record_type": "event",
                    "capture_id": CAPTURE,
                    "ts_ns": ts_ns,
                    "kind": kind,
                }
            )
    for seq, row in enumerate(rows[1:], 1):
        row["output_seq"] = seq
    count = len(rows) - 1
    footer.update(
        last_output_seq=count,
        events_attempted=count,
        events_accepted=count,
        events_written=count,
    )
    return b"".join(map(_line, [*rows, footer]))


def _binding(capture: dict) -> dict:
    return {
        "source_revision": "a" * 40,
        "binary_sha256": "b" * 64,
        "summary_sha256": "c" * 64,
        "tier_manifest_sha256": "d" * 64,
        "timeline_sha256": capture["sha256"],
        "stderr_sha256": "e" * 64,
        "timeline_capture_id": capture["capture_id"],
    }


def _legacy_profile(capture: dict) -> dict:
    return {
        **_binding(capture),
        "trace_sha256": "e" * 64,
        "status": "diagnostic_unqualified",
        "phases": {
            "full_seal": {
                "status": "ok",
                "sync": {
                    "pool_sync_all": {"calls": 3, "failed_calls": 0, "elapsed_ns": 900},
                    "atomic_file": {"calls": 1, "failed_calls": 0, "elapsed_ns": 400},
                },
                "exact_live_token_scan": {"calls": 0},
                "bm25_census_build": {"calls": 0},
                "bm25_boundary": {"encode": {"calls": 1, "elapsed_ns": 70}},
                "bm25_live_build": [{"elapsed_ns": 1000, "new_segments": 1}],
                "process_write_io": {"write_bytes": 4096},
                "process_write_io_unavailable_reason": None,
            }
        },
    }


def test_complete_waterfall_has_exact_exclusive_work_and_hash() -> None:
    raw = _capture()
    capture = parse(raw)
    summary = summarize(capture)
    assert capture["status"] == "complete"
    assert [(span["duration_ns"], span["exclusive_ns"]) for span in summary["spans"]] == [
        (30, 15),
        (15, 15),
    ]
    assert summary["spans"][1]["counters"] == {"candidates": 3}
    viewer = chrome_export(capture)
    assert len(viewer["traceEvents"]) == 3
    assert [event["ph"] for event in viewer["traceEvents"]].count("X") == 2
    assert [event["ph"] for event in viewer["traceEvents"]].count("C") == 1
    assert viewer["quantaIndexSummary"]["stream_sha256"] == summary["stream_sha256"]


def test_closed_work_aggregates_are_metadata_not_fabricated_durations() -> None:
    parsed = parse(_capture_with_work_aggregates())
    assert parsed["status"] == "complete"
    assert parsed["schema_version"] == 2
    summary = summarize(parsed)
    assert summary["schema_version"] == 2
    assert summary["sync_aggregates"][0]["elapsed_sum_ns"] == 900
    assert summary["adoption_group_aggregates"][0]["failed_groups"] == 1
    viewer = chrome_export(parsed)
    aggregates = [event for event in viewer["traceEvents"] if event["cat"] == "work_aggregate"]
    assert [(event["name"], event["ph"]) for event in aggregates] == [
        ("sync:pool_sync_all", "i"),
        ("adoption_group:ref_limit", "i"),
    ]
    assert aggregates[0]["args"]["elapsed_sum_ns"] == "900"
    assert aggregates[1]["args"]["refs_bucket"] == "up_to_100_percent"
    assert viewer["quantaIndexSummary"]["sync_aggregates"][0]["span_id"] == "2"


def test_bm25_aggregates_are_bound_to_owner_spans_and_visible_in_summary() -> None:
    capture = parse(b"".join(map(_line, _bm25_rows())))
    assert capture["status"] == "complete"
    summary = summarize(capture)
    assert summary["bm25_census_aggregates"][0]["docs"] == 2
    assert summary["bm25_boundary_aggregates"][0]["label"] == "base_read"
    assert summary["bm25_live_build_aggregates"][0]["segment_fanout"] == 3
    assert [row["bm25_aggregates"][0]["type"] for row in summary["spans"][1:]] == [
        "bm25_census_aggregate",
        "bm25_boundary_aggregate",
        "bm25_live_build_aggregate",
    ]
    viewer = chrome_export(capture)
    assert sum(row["cat"] == "work_aggregate" for row in viewer["traceEvents"]) == 3
    assert viewer["quantaIndexSummary"]["bm25_live_build_aggregates"][0]["elapsed_ns"] == "1000"


@pytest.mark.parametrize(
    ("event_type", "field"),
    (
        ("bm25_census_aggregate", "docs"),
        ("bm25_boundary_aggregate", "label"),
        ("bm25_live_build_aggregate", "segment_fanout"),
    ),
)
def test_bm25_aggregates_require_exact_fields(event_type: str, field: str) -> None:
    rows = _bm25_rows()
    _bm25_kind(rows, event_type).pop(field)
    with pytest.raises(ValueError, match="invalid causal timeline row"):
        parse(b"".join(map(_line, rows)))


@pytest.mark.parametrize(
    ("event_type", "field"),
    (
        ("bm25_census_aggregate", "docs"),
        ("bm25_boundary_aggregate", "calls"),
        ("bm25_live_build_aggregate", "new_segments"),
    ),
)
def test_bm25_aggregates_reject_u64_overflow(event_type: str, field: str) -> None:
    rows = _bm25_rows()
    _bm25_kind(rows, event_type)[field] = 1 << 64
    with pytest.raises(ValueError, match="invalid causal timeline row"):
        parse(b"".join(map(_line, rows)))


@pytest.mark.parametrize(
    ("event_type", "field", "value", "message"),
    (
        ("bm25_census_aggregate", "docs", 0, "bytes without documents"),
        ("bm25_boundary_aggregate", "calls", 0, "metrics without calls"),
        ("bm25_live_build_aggregate", "segment_fanout", 2, "fanout mismatch"),
        ("bm25_live_build_aggregate", "newly_dead_docs", 3, "exceed mask docs"),
        ("bm25_live_build_aggregate", "death_ns", 401, "exceeds changed timer"),
        ("bm25_live_build_aggregate", "new_ns", 601, "exceed elapsed time"),
    ),
)
def test_bm25_aggregates_reject_inconsistent_metrics(
    event_type: str, field: str, value: int, message: str
) -> None:
    rows = _bm25_rows()
    _bm25_kind(rows, event_type)[field] = value
    with pytest.raises(ValueError, match=message):
        parse(b"".join(map(_line, rows)))


@pytest.mark.parametrize(
    ("event_type", "span_id"),
    (
        ("bm25_census_aggregate", 2),
        ("bm25_boundary_aggregate", 3),
        ("bm25_live_build_aggregate", 4),
    ),
)
def test_bm25_aggregates_reject_wrong_owner_stage(event_type: str, span_id: int) -> None:
    rows = _bm25_rows()
    owner = next(
        row["kind"]
        for row in rows
        if row.get("kind", {}).get("type") == "span_begin" and row["kind"]["span_id"] == span_id
    )
    owner["stage"] = "lexical_seal"
    with pytest.raises(ValueError, match=f"{event_type} targets non-"):
        parse(b"".join(map(_line, rows)))


def test_bm25_aggregates_reject_open_label_and_schema_v1() -> None:
    rows = _bm25_rows()
    _bm25_kind(rows, "bm25_boundary_aggregate")["label"] = "other"
    with pytest.raises(ValueError, match="invalid causal timeline row"):
        parse(b"".join(map(_line, rows)))

    rows = _bm25_rows()
    rows[0]["schema_version"] = 1
    rows[-1]["schema_version"] = 1
    with pytest.raises(ValueError, match="invalid causal timeline row"):
        parse(b"".join(map(_line, rows)))


def test_bm25_aggregates_reject_derived_u64_overflow() -> None:
    rows = _bm25_rows()
    live = _bm25_kind(rows, "bm25_live_build_aggregate")
    live["reused_segments"] = (1 << 64) - 1
    with pytest.raises(ValueError, match="fanout mismatch"):
        parse(b"".join(map(_line, rows)))

    rows = _bm25_rows()
    live = _bm25_kind(rows, "bm25_live_build_aggregate")
    live["changed_ns"] = (1 << 64) - 1
    live["death_ns"] = 0
    with pytest.raises(ValueError, match="child timers exceed elapsed time"):
        parse(b"".join(map(_line, rows)))


def test_object_kind_aggregate_is_bounded_metadata_and_visible_in_waterfall() -> None:
    capture = parse(_capture_with_object_kind_aggregate())
    assert capture["status"] == "complete"
    summary = summarize(capture)
    assert summary["object_kind_aggregates"] == [
        {
            "span_id": 2,
            "ts_ns": 26,
            "kind": "file_postings",
            "new_objects": 3,
            "reused_objects": 5,
        }
    ]
    assert summary["spans"][1]["duration_ns"] == 15
    assert summary["spans"][1]["counters"] == {"compacted_segments": 3}
    viewer = chrome_export(capture)
    aggregates = [event for event in viewer["traceEvents"] if event["cat"] == "work_aggregate"]
    assert [(event["name"], event["ph"]) for event in aggregates] == [
        ("object_kind:file_postings", "i")
    ]
    assert aggregates[0]["args"] == {
        "span_id": "2",
        "new_objects": "3",
        "reused_objects": "5",
    }
    assert viewer["quantaIndexSummary"]["object_kind_aggregates"][0]["span_id"] == "2"


def test_v1_rejects_object_kind_aggregate() -> None:
    rows = [json.loads(line) for line in _capture_with_object_kind_aggregate().splitlines()]
    rows[0]["schema_version"] = 1
    rows[3]["kind"]["key"] = "candidates"
    rows[-1]["schema_version"] = 1
    with pytest.raises(ValueError, match="invalid causal timeline row"):
        parse(b"".join(map(_line, rows)))


def test_v1_rejects_v2_compacted_segments_counter() -> None:
    rows = [json.loads(line) for line in _capture_with_object_kind_aggregate().splitlines()]
    del rows[-2]
    rows[0]["schema_version"] = 1
    rows[-1].update(
        schema_version=1,
        last_output_seq=5,
        events_attempted=5,
        events_accepted=5,
        events_written=5,
    )
    with pytest.raises(ValueError, match="invalid causal timeline row"):
        parse(b"".join(map(_line, rows)))


@pytest.mark.parametrize(
    "kind",
    [
        "file_body",
        "file_postings",
        "native_body",
        "native_trigrams",
        "native_positions",
        "segment",
        "record",
        "map_page",
        "blob_block",
        "blob_node",
        "attempt_batch",
    ],
)
def test_object_kind_aggregate_accepts_each_closed_kind(kind: str) -> None:
    rows = [json.loads(line) for line in _capture_with_object_kind_aggregate().splitlines()]
    rows[-2]["kind"]["kind"] = kind
    assert parse(b"".join(map(_line, rows)))["object_kind_aggregates"][0]["kind"] == kind


@pytest.mark.parametrize(
    ("field", "value"),
    [
        ("kind", "unknown_object"),
        ("new_objects", -1),
        ("new_objects", 1 << 64),
        ("reused_objects", 1 << 64),
        ("reused_objects", True),
        ("extra", 1),
    ],
)
def test_object_kind_aggregate_rejects_invalid_fields(field: str, value: str | int | bool) -> None:
    rows = [json.loads(line) for line in _capture_with_object_kind_aggregate().splitlines()]
    rows[-2]["kind"][field] = value
    with pytest.raises(ValueError):
        parse(b"".join(map(_line, rows)))


def test_object_kind_aggregate_requires_all_fields() -> None:
    rows = [json.loads(line) for line in _capture_with_object_kind_aggregate().splitlines()]
    del rows[-2]["kind"]["reused_objects"]
    with pytest.raises(ValueError, match="invalid causal timeline row"):
        parse(b"".join(map(_line, rows)))


def test_v1_rejects_v2_work_records_and_mixed_header_footer_versions() -> None:
    rows = [json.loads(line) for line in _capture_with_work_aggregates().splitlines()]
    rows[0]["schema_version"] = 1
    rows[-1]["schema_version"] = 1
    with pytest.raises(ValueError, match="invalid causal timeline row"):
        parse(b"".join(map(_line, rows)))
    rows[0]["schema_version"] = 2
    with pytest.raises(ValueError, match="invalid causal timeline row"):
        parse(b"".join(map(_line, rows)))


@pytest.mark.parametrize(
    ("event_index", "field", "value"),
    [
        (6, "label", "arbitrary_sync_label"),
        (7, "reason", "threshold_unknown"),
        (7, "refs_bucket", "nearly_full"),
        (7, "bytes_bucket", "more_than_cap"),
        (6, "failures", 4),
        (6, "elapsed_max_ns", 901),
        (7, "failed_groups", 3),
    ],
)
def test_work_aggregate_rejects_invalid_closed_fields(
    event_index: int, field: str, value: str | int
) -> None:
    rows = [json.loads(line) for line in _capture_with_work_aggregates().splitlines()]
    rows[event_index]["kind"][field] = value
    with pytest.raises(ValueError):
        parse(b"".join(map(_line, rows)))


def test_missing_footer_and_open_spans_remain_partial() -> None:
    assert parse(_capture(footer=False))["status"] == "partial"
    partial = parse(_capture(end=False))
    assert partial["status"] == "partial"
    viewer = chrome_export(partial)
    assert all(event["ph"] in ("X", "C") for event in viewer["traceEvents"])
    assert any(event["ph"] == "C" for event in viewer["traceEvents"])


def test_unclosed_spans_preserve_last_observed_clock_without_terminal_event() -> None:
    partial = parse(_capture(end=False))
    assert partial["last_observed_ns"] == 50
    summary = summarize(partial)
    assert [(row["end_ns"], row["last_observed_ns"]) for row in summary["spans"]] == [
        (None, 50),
        (None, 50),
    ]
    viewer = chrome_export(partial)
    extents = [row for row in viewer["traceEvents"] if row["cat"] == "unclosed_observed_extent"]
    assert [
        (row["ph"], row["dur"], row["args"]["display_only_observed_extent"]) for row in extents
    ] == [
        ("X", 0.04, True),
        ("X", 0.03, True),
    ]
    assert not any(row["cat"] == "terminal" for row in viewer["traceEvents"])

    interrupted = parse(_capture(footer=False, end=False))
    assert interrupted["last_observed_ns"] == 25
    assert [
        row["dur"] for row in chrome_export(interrupted)["traceEvents"] if row["ph"] == "X"
    ] == [
        0.015,
        0.005,
    ]


def test_legacy_reconciliation_compares_only_common_phase_sync_and_keeps_residuals() -> None:
    capture = parse(_phase_capture())
    binding = _binding(capture)
    legacy = _legacy_profile(capture)
    comparison = reconcile_legacy(capture, legacy, execution_binding=binding)
    assert comparison["status"] == "partial_unqualified"
    assert comparison["phase_parity"] == "verified"
    assert comparison["sync_common_parity"] == "verified"
    assert comparison["bm25_common_parity"] == "unsupported"
    assert [
        (row["phase"], row["label"], row["counts_status"], row["elapsed_status"])
        for row in comparison["sync_comparisons"]
    ] == [("full_seal", "pool_sync_all", "verified", "unsupported_different_timer_envelopes")]
    residual = comparison["legacy_residual"]
    assert [(row["source"], row["kind"], row.get("label")) for row in residual] == [
        ("stderr QI_CAUSAL_V1 markers", "sync", "atomic_file"),
        ("stderr QI_CAUSAL_V1 markers", "sync_elapsed_ns", "pool_sync_all"),
        ("summary.json phase_resources", "process_write_io", None),
        ("stderr QI_CAUSAL_V1 markers", "bm25_boundary", "encode"),
        ("stderr QI_CAUSAL_V1 markers", "bm25_live_build", None),
    ]
    viewer = chrome_export(capture, legacy, execution_binding=binding)
    assert viewer["quantaIndexSummary"]["legacy_reconciliation"] == comparison
    assert not any(row["cat"] == "legacy" for row in viewer["traceEvents"])


def test_legacy_reconciliation_compares_typed_bm25_sizes_but_keeps_timing_residual() -> None:
    capture = parse(_phase_capture_with_bm25())
    legacy = _legacy_profile(capture)
    phase = legacy["phases"]["full_seal"]
    phase["bm25_census_build"] = {
        "calls": 1,
        "elapsed_ns": 999,
        "docs": 2,
        "encoded_bytes": 50,
    }
    phase["bm25_boundary"] = {
        "base_read": {
            "calls": 1,
            "elapsed_ns": 999,
            "logical_bytes": 50,
            "index_files": 1,
        }
    }
    live = _bm25_kind(_bm25_rows(), "bm25_live_build_aggregate")
    phase["bm25_live_build"] = [
        {
            **{key: value for key, value in live.items() if key not in ("type", "calls")},
            "elapsed_ns": 999,
            "retained_valid": 1,
        }
    ]
    phase["exact_live_token_scan"] = {"calls": 1, "elapsed_ns": 100, "terms": 2}

    comparison = reconcile_legacy(capture, legacy, execution_binding=_binding(capture))
    assert comparison["status"] == "partial_unqualified"
    assert comparison["bm25_common_parity"] == "verified"
    assert [
        (row["kind"], row["fields_status"], row["elapsed_status"])
        for row in comparison["bm25_comparisons"]
    ] == [
        ("bm25_boundary", "verified", "unsupported_timing_comparison"),
        ("bm25_census_build", "verified", "unsupported_timing_comparison"),
        ("bm25_live_build", "verified", "unsupported_timing_comparison"),
    ]
    assert comparison["canonical_bm25_without_legacy_phase_label"] == []
    assert [
        (row["kind"], row["reason"])
        for row in comparison["legacy_residual"]
        if row["kind"].startswith("bm25") or row["kind"] == "exact_live_token_scan"
    ] == [
        ("exact_live_token_scan", "no_semantically_equivalent_canonical_metric"),
        ("bm25_boundary", "timing_comparison_unproven"),
        ("bm25_census_build", "timing_comparison_unproven"),
        ("bm25_live_build", "timing_comparison_unproven"),
    ]

    phase["bm25_boundary"]["base_read"]["logical_bytes"] = 51
    mismatch = reconcile_legacy(capture, legacy, execution_binding=_binding(capture))
    assert mismatch["bm25_common_parity"] == "mismatch"
    boundary = next(row for row in mismatch["bm25_comparisons"] if row["kind"] == "bm25_boundary")
    assert boundary["fields_status"] == "mismatch"
    assert boundary["mismatch_fields"] == ["logical_bytes"]
    assert any(
        row["kind"] == "bm25_boundary" and row["reason"] == "canonical_field_mismatch"
        for row in mismatch["legacy_residual"]
    )

    phase["bm25_boundary"]["base_read"].update(calls=2, logical_bytes=50)
    count_mismatch = reconcile_legacy(capture, legacy, execution_binding=_binding(capture))
    boundary = next(
        row for row in count_mismatch["bm25_comparisons"] if row["kind"] == "bm25_boundary"
    )
    assert boundary["calls_status"] == "mismatch"
    assert boundary["sizes_status"] == "unsupported_call_count_mismatch"
    assert boundary["mismatch_fields"] == ["calls"]


def test_legacy_reconciliation_is_partial_for_open_phase_and_detects_common_mismatch() -> None:
    partial_capture = parse(_phase_capture(end=False))
    partial = reconcile_legacy(
        partial_capture,
        _legacy_profile(partial_capture),
        execution_binding=_binding(partial_capture),
    )
    assert partial["phase_parity"] == "unsupported"
    assert partial["sync_common_parity"] == "unsupported"
    mismatch_capture = parse(_phase_capture(sync_calls=2))
    mismatch = reconcile_legacy(
        mismatch_capture,
        _legacy_profile(mismatch_capture),
        execution_binding=_binding(mismatch_capture),
    )
    assert mismatch["phase_parity"] == "verified"
    assert mismatch["sync_common_parity"] == "mismatch"
    assert mismatch["sync_comparisons"][0]["counts_status"] == "mismatch"
    assert (
        next(row for row in mismatch["legacy_residual"] if row.get("label") == "pool_sync_all")[
            "reason"
        ]
        == "canonical_count_mismatch"
    )

    rows = [json.loads(line) for line in _phase_capture().splitlines()]
    phase_end = next(
        row
        for row in rows
        if row.get("kind", {}).get("type") == "span_end" and row["kind"]["span_id"] == 1
    )
    phase_end["kind"]["outcome"] = "error"
    outcome_capture = parse(b"".join(map(_line, rows)))
    outcome = reconcile_legacy(
        outcome_capture,
        _legacy_profile(outcome_capture),
        execution_binding=_binding(outcome_capture),
    )
    assert outcome["phase_parity"] == "mismatch"
    assert outcome["status"] == "partial_unqualified"


@pytest.mark.parametrize("field", tuple(_binding(parse(_phase_capture()))))
@pytest.mark.parametrize("which", ("execution", "profile"))
def test_legacy_reconciliation_refuses_missing_or_mismatched_binding(
    field: str, which: str
) -> None:
    capture = parse(_phase_capture())
    binding = _binding(capture)
    profile = _legacy_profile(capture)
    target = binding if which == "execution" else profile
    target.pop(field)
    with pytest.raises(ValueError, match="binding"):
        reconcile_legacy(capture, profile, execution_binding=binding)

    binding = _binding(capture)
    profile = _legacy_profile(capture)
    target = binding if which == "execution" else profile
    target[field] = "f" * len(target[field])
    with pytest.raises(ValueError, match="binding"):
        reconcile_legacy(capture, profile, execution_binding=binding)


def test_legacy_reconciliation_refuses_unbound_or_wrong_capture_and_stderr() -> None:
    capture = parse(_phase_capture())
    binding = _binding(capture)
    profile = _legacy_profile(capture)
    with pytest.raises(ValueError, match="explicit execution binding"):
        summarize(capture, profile)
    wrong_capture = copy.deepcopy(capture)
    wrong_capture["capture_id"] = "1" * 32
    with pytest.raises(ValueError, match="capture or stderr binding"):
        reconcile_legacy(wrong_capture, profile, execution_binding=binding)
    profile["trace_sha256"] = "0" * 64
    with pytest.raises(ValueError, match="capture or stderr binding"):
        reconcile_legacy(capture, profile, execution_binding=binding)


def test_partial_capture_rejects_observed_parent_cycle() -> None:
    rows = [json.loads(line) for line in _capture(footer=False, end=False).splitlines()]
    rows[1]["kind"]["parent_span_id"] = 2
    rows[1]["kind"].pop("root_kind")
    raw = b"".join(map(_line, rows))
    with pytest.raises(ValueError, match="parent cycle"):
        parse(raw)


def test_truncated_final_row_is_explicit_partial() -> None:
    raw = _capture(footer=False) + b'{"record_type":"event","output_seq":4'
    partial = parse(raw)
    assert partial["status"] == "partial"
    assert partial["truncated_tail"] is True
    assert partial["event_count"] == 5


def test_truncated_data_after_footer_is_invalid() -> None:
    with pytest.raises(ValueError, match="data follows footer"):
        parse(_capture() + b'{"record_type":"event","output_seq":6')


def test_malformed_middle_row_and_sequence_gap_are_invalid() -> None:
    raw = _capture()
    with pytest.raises(ValueError, match="row 3"):
        parse(raw.replace(b'"output_seq":2', b'"output_seq":2,"output_seq":2'))
    with pytest.raises(ValueError, match="sequence gap"):
        parse(raw.replace(b'"output_seq":2', b'"output_seq":7'))


def test_footer_accounting_and_regressed_clock_are_invalid() -> None:
    raw = _capture()
    with pytest.raises(ValueError, match="attempted accounting"):
        parse(raw.replace(b'"events_attempted":5', b'"events_attempted":4'))
    with pytest.raises(ValueError, match="clock regressed"):
        parse(raw.replace(b'"ts_ns":35', b'"ts_ns":15'))


def test_counter_overflow_is_invalid() -> None:
    raw = _capture()
    with pytest.raises(ValueError, match="exceeds u64"):
        parse(raw.replace(b'"value":3', b'"value":18446744073709551616'))


def test_invalid_footer_and_event_before_clock_origin_are_rejected() -> None:
    raw = _capture()
    with pytest.raises(ValueError, match="declared.*invalid"):
        parse(raw.replace(b'"status":"complete"', b'"status":"invalid"'))
    with pytest.raises(ValueError, match="precedes capture clock origin"):
        parse(raw.replace(b'"started_ns":0', b'"started_ns":11'))


@pytest.mark.parametrize(
    "code",
    [
        "span_id_exhausted",
        "clock_regressed",
        "counter_overflow",
        "queue_overflow",
        "record_too_large",
        "producer_after_close",
        "foreign_capture_span",
        "missing_parent_span",
        "invalid_attempt_token",
        "duplicate_attempt_token",
        "unlinked_attempt",
    ],
)
def test_complete_footer_rejects_every_health_code_and_preserves_partial_prefix(code: str) -> None:
    rows = [json.loads(line) for line in _capture().splitlines()]
    footer = rows.pop()
    rows.append(
        {
            "record_type": "event",
            "output_seq": 6,
            "capture_id": CAPTURE,
            "ts_ns": 45,
            "kind": {"type": "health", "code": code},
        }
    )
    prefix = b"".join(map(_line, rows))
    partial = parse(prefix)
    assert partial["status"] == "partial"
    assert [row["code"] for row in partial["health"]] == [code]
    footer.update(
        last_output_seq=6,
        events_attempted=6,
        events_accepted=6,
        events_written=6,
    )
    with pytest.raises(ValueError, match="complete capture contradicts.*health"):
        parse(prefix + _line(footer))


def test_unique_attempt_link_flows_client_to_server_and_ids_are_strings() -> None:
    parsed = parse(_linked_capture())
    assert len(parsed["verified_flows"]) == 1
    viewer = chrome_export(parsed)
    flows = [event for event in viewer["traceEvents"] if event["ph"] in ("s", "f")]
    assert [(event["ph"], event["ts"]) for event in flows] == [("s", 0.012), ("f", 0.016)]
    slices = [event for event in viewer["traceEvents"] if event["ph"] == "X"]
    assert next(row["tid"] for row in slices if row["name"] == "request") != next(
        row["tid"] for row in slices if row["name"] == "client_attempt"
    )
    request = next(
        row for row in viewer["quantaIndexSummary"]["spans"] if row["stage"] == "request"
    )
    assert request["request_identity"]["request_id"] == str((1 << 53) + 7)


def test_duplicate_attempt_link_is_ambiguous_without_flow() -> None:
    parsed = parse(_linked_capture(duplicate=True))
    assert parsed["ambiguous_links"] == 2
    assert not parsed["verified_flows"]
    assert not any(event["ph"] in ("s", "f") for event in chrome_export(parsed)["traceEvents"])


def test_counter_events_and_error_terminal_have_viewer_events() -> None:
    raw = _capture().replace(b'"outcome":"ok"', b'"outcome":"error"', 1)
    viewer = chrome_export(parse(raw))
    assert any(
        event["ph"] == "C" and event["name"] == "candidates" for event in viewer["traceEvents"]
    )
    assert any(event["ph"] == "i" and event["cat"] == "terminal" for event in viewer["traceEvents"])
