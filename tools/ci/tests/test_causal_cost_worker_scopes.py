"""Independent interval gold for explicitly incomplete worker diagnostics."""

from __future__ import annotations

import copy
import json

import pytest

from tools.benchmark.retrieval.causal_cost_diagnostic import build_index, main
from tools.benchmark.retrieval.causal_cost_worker_scopes import (
    build_closed_maintenance_report,
    build_closed_worker_report,
)

CAPTURE = "ab" * 16


def _span(key, parent, stage, lane, start, end, *, root=None, counters=None):
    return {
        "span_id": key,
        "parent_span_id": parent,
        "stage": stage,
        "lane_id": lane,
        "start_ns": start,
        "end_ns": end,
        "outcome": "ok" if end is not None else None,
        "root_kind": root,
        "counters": counters or {},
    }


def _partial():
    return {
        "status": "partial",
        "schema_version": 4,
        "capture_id": CAPTURE,
        "sha256": "01" * 32,
        "health": [],
        "truncated_tail": False,
        "orphan_events": 0,
        "footer": {"status": "partial", "events_unwritten": 3, "open_spans": 2},
        "spans": {
            1: _span(1, None, "operation", 0, 0, None, root="operation"),
            2: _span(2, 1, "operation", 1, 2, 80),
            3: _span(3, 2, "build_prepare", 0, 5, 70),
            4: _span(4, 3, "sqlite_work", 11, 20, 40, counters={"sync_calls": 0}),
            5: _span(5, 3, "sqlite_work", 11, 30, 60, counters={"thread_cpu_ns": 7}),
            6: _span(6, 1, "operation", 1, 85, None),
        },
    }


def test_partial_closed_scopes_keep_original_loss_and_unknown_counter_coverage():
    capture = _partial()
    original = copy.deepcopy(capture)
    result = build_closed_worker_report(capture)
    assert capture == original
    assert result["status"] == "OBSERVED_CLOSED_SCOPES_ONLY"
    assert result["capture"]["status"] == "partial"
    assert result["capture"]["footer"] == original["footer"]
    assert result["selection"] == {
        "observed_worker_roots": 2,
        "selected_closed_subtrees": 1,
        "excluded_open_subtrees": 1,
    }
    worker = result["workers"][0]
    assert (worker["span_id"], worker["start_ns"], worker["end_ns"], worker["duration_ns"]) == (
        2,
        2,
        80,
        78,
    )
    sync = next(group for group in worker["groups"] if group["lane_id"] == 11)
    assert (sync["observed_count"], sync["sum_ns"], sync["union_ns"], sync["max_ns"]) == (
        2,
        50,
        40,
        30,
    )
    assert sync["nearest_worker_phase"] == "build_prepare"
    assert sync["observed_counters"] == {"sync_calls": {"sum": 0, "observed_spans": 1}}
    assert sync["thread_cpu_samples"] == [{"span_id": 5, "wall_ns": 30, "thread_cpu_ns": 7}]
    assert "unrecorded" in result["scope"]["custody"]
    with pytest.raises(ValueError, match="complete canonical capture"):
        build_index(capture)


def test_open_descendant_excludes_whole_worker_without_synthetic_terminal():
    capture = _partial()
    capture["spans"][4].update(end_ns=None, outcome=None)
    result = build_closed_worker_report(capture)
    assert result["workers"] == []
    assert result["selection"]["excluded_open_subtrees"] == 2
    assert capture["spans"][4]["end_ns"] is None


@pytest.mark.parametrize("version", (4, 5))
def test_closed_report_accepts_shared_canonical_ir_from_both_artifact_versions(version):
    capture = _partial()
    capture["schema_version"] = version
    assert build_closed_worker_report(capture)["capture"]["schema_version"] == version


@pytest.mark.parametrize(
    "mutate",
    (
        lambda value: value.update(schema_version=True),
        lambda value: value.update(status="invalid"),
        lambda value: value.update(capture_id="foreign"),
        lambda value: value.update(sha256="bad"),
        lambda value: value.update(health=[{"code": "clock_regressed"}]),
        lambda value: value["spans"][4].update(end_ns=90),
        lambda value: value["spans"][4].update(lane_id=True),
        lambda value: value["spans"][4].update(counters={"sync_calls": -1}),
        lambda value: value["spans"][4].update(span_id=7),
    ),
)
def test_closed_report_rejects_invalid_identity_and_observed_intervals(mutate):
    capture = _partial()
    mutate(capture)
    with pytest.raises(ValueError):
        build_closed_worker_report(capture)


def test_closed_report_rejects_ancestry_cycle():
    capture = _partial()
    capture["spans"][4]["parent_span_id"] = 5
    capture["spans"][5]["parent_span_id"] = 4
    with pytest.raises(ValueError, match="cycles"):
        build_closed_worker_report(capture)


def test_closed_workers_cli_preserves_canonical_partial_and_exclusive_output(tmp_path):
    rows = [
        {
            "record_type": "header",
            "schema_version": 4,
            "capture_id": CAPTURE,
            "stream_id": CAPTURE,
            "clock_domain": CAPTURE,
            "process_instance": CAPTURE,
            "started_ns": 0,
            "max_record_bytes": 4096,
            "max_counters_per_event": 1,
            "coverage": [],
        }
    ]
    kinds = [
        (
            0,
            {
                "type": "span_begin",
                "span_id": 1,
                "stage": "operation",
                "lane_id": 0,
                "root_kind": "operation",
            },
        ),
        (
            1,
            {
                "type": "span_begin",
                "span_id": 2,
                "parent_span_id": 1,
                "stage": "operation",
                "lane_id": 1,
            },
        ),
        (8, {"type": "span_end", "span_id": 2, "outcome": "ok"}),
    ]
    rows.extend(
        {
            "record_type": "event",
            "output_seq": seq,
            "capture_id": CAPTURE,
            "ts_ns": ts,
            "kind": kind,
        }
        for seq, (ts, kind) in enumerate(kinds, 1)
    )
    rows.append(
        {
            "record_type": "footer",
            "schema_version": 4,
            "capture_id": CAPTURE,
            "status": "partial",
            "close_reason": "normal",
            "closed_ns": 10,
            "events_attempted": 4,
            "events_accepted": 4,
            "events_dropped": 0,
            "events_written": 3,
            "events_unwritten": 1,
            "last_output_seq": 3,
            "open_spans": 1,
            "producer_leases": 0,
        }
    )
    source, target = tmp_path / "capture.qtrace", tmp_path / "closed.json"
    source.write_text("\n".join(json.dumps(row) for row in rows) + "\n")
    args = ["closed-workers", "--input", str(source), "--out", str(target)]
    assert main(args) == 0
    report = json.loads(target.read_text())
    assert report["capture"]["status"] == "partial"
    assert report["capture"]["footer"]["events_unwritten"] == 1
    assert [row["span_id"] for row in report["workers"]] == [2]
    with pytest.raises(FileExistsError):
        main(args)


def _partial_with_maintenance():
    capture = _partial()
    capture["spans"].update(
        {
            7: _span(7, 1, "runtime_maintenance", 0, 10, 75),
            8: _span(8, 7, "catalog_lock_wait", 0, 12, 30),
            9: _span(9, 7, "catalog_lock_hold", 0, 30, 65),
            10: _span(10, 9, "sqlite_work", 26, 40, 60),
            11: _span(11, 9, "sqlite_work", 3, 45, 55),
            12: _span(12, 1, "runtime_maintenance", 0, 90, None),
            13: _span(13, 12, "catalog_lock_hold", 0, 95, None),
        }
    )
    capture["spans"][7]["outcome"] = "error"
    capture["spans"][10]["outcome"] = "cancelled"
    return capture


def test_closed_maintenance_reports_only_independent_closed_subtrees_and_reached_checkpoint():
    capture = _partial_with_maintenance()
    original = copy.deepcopy(capture)
    report = build_closed_maintenance_report(capture)
    assert capture == original
    assert report["capture"]["status"] == "partial"
    assert report["capture"]["footer"] == capture["footer"]
    assert report["selection"] == {
        "observed_maintenance_roots": 2,
        "selected_closed_subtrees": 1,
        "excluded_open_subtrees": 1,
    }
    assert "workers" not in report
    maintenance = report["maintenance"]
    assert len(maintenance) == 1
    assert (
        maintenance[0]["span_id"],
        maintenance[0]["duration_ns"],
        maintenance[0]["outcome"],
    ) == (7, 65, "error")
    assert maintenance[0]["reached_explicit_checkpoints"] == [
        {
            "span_id": 10,
            "start_ns": 40,
            "end_ns": 60,
            "duration_ns": 20,
            "outcome": "cancelled",
        }
    ]
    hold = next(
        group for group in maintenance[0]["groups"] if group["stage"] == "catalog_lock_hold"
    )
    assert (hold["observed_count"], hold["union_ns"], hold["sum_ns"]) == (1, 35, 35)
    checkpoint = next(
        group
        for group in maintenance[0]["groups"]
        if (group["stage"], group["lane_id"]) == ("sqlite_work", 26)
    )
    assert (checkpoint["union_ns"], checkpoint["sum_ns"]) == (20, 20)
    assert hold["union_ns"] + checkpoint["union_ns"] > hold["union_ns"]
    assert "overlap" in report["scope"]["checkpoint"]
    assert (
        "not its later busy-row validation or durable checkpoint completion"
        in report["scope"]["checkpoint"]
    )
    workers = build_closed_worker_report(capture)
    assert workers["selection"]["observed_worker_roots"] == 2
    assert all(
        group["stage"] != "runtime_maintenance"
        for worker in workers["workers"]
        for group in worker["groups"]
    )


def test_closed_maintenance_excludes_open_checkpoint_and_rejects_bad_ancestry():
    capture = _partial_with_maintenance()
    capture["spans"][10].update(end_ns=None, outcome=None)
    report = build_closed_maintenance_report(capture)
    assert report["maintenance"] == []
    assert report["selection"]["excluded_open_subtrees"] == 2

    cycle = _partial_with_maintenance()
    cycle["spans"][8]["parent_span_id"] = 9
    cycle["spans"][9]["parent_span_id"] = 8
    with pytest.raises(ValueError, match="cycles"):
        build_closed_maintenance_report(cycle)

    escaped = _partial_with_maintenance()
    escaped["spans"][10]["end_ns"] = 70
    with pytest.raises(ValueError, match="escapes"):
        build_closed_maintenance_report(escaped)


def test_closed_maintenance_cli_keeps_partial_footer_and_exclusive_output(tmp_path):
    rows = [
        {
            "record_type": "header",
            "schema_version": 4,
            "capture_id": CAPTURE,
            "stream_id": CAPTURE,
            "clock_domain": CAPTURE,
            "process_instance": CAPTURE,
            "started_ns": 0,
            "max_record_bytes": 4096,
            "max_counters_per_event": 1,
            "coverage": [],
        }
    ]
    kinds = [
        (
            0,
            {
                "type": "span_begin",
                "span_id": 1,
                "stage": "operation",
                "lane_id": 0,
                "root_kind": "operation",
            },
        ),
        (
            1,
            {
                "type": "span_begin",
                "span_id": 2,
                "parent_span_id": 1,
                "stage": "runtime_maintenance",
                "lane_id": 0,
            },
        ),
        (
            2,
            {
                "type": "span_begin",
                "span_id": 3,
                "parent_span_id": 2,
                "stage": "catalog_lock_hold",
                "lane_id": 0,
            },
        ),
        (
            3,
            {
                "type": "span_begin",
                "span_id": 4,
                "parent_span_id": 3,
                "stage": "sqlite_work",
                "lane_id": 26,
            },
        ),
        (5, {"type": "span_end", "span_id": 4, "outcome": "error", "error_code": 0}),
        (6, {"type": "span_end", "span_id": 3, "outcome": "ok"}),
        (7, {"type": "span_end", "span_id": 2, "outcome": "error", "error_code": 0}),
    ]
    rows.extend(
        {
            "record_type": "event",
            "output_seq": seq,
            "capture_id": CAPTURE,
            "ts_ns": ts,
            "kind": kind,
        }
        for seq, (ts, kind) in enumerate(kinds, 1)
    )
    rows.append(
        {
            "record_type": "footer",
            "schema_version": 4,
            "capture_id": CAPTURE,
            "status": "partial",
            "close_reason": "normal",
            "closed_ns": 10,
            "events_attempted": 8,
            "events_accepted": 8,
            "events_dropped": 0,
            "events_written": 7,
            "events_unwritten": 1,
            "last_output_seq": 7,
            "open_spans": 1,
            "producer_leases": 0,
        }
    )
    source, target = tmp_path / "capture.qtrace", tmp_path / "maintenance.json"
    source.write_text("\n".join(json.dumps(row) for row in rows) + "\n")
    args = ["closed-maintenance", "--input", str(source), "--out", str(target)]
    assert main(args) == 0
    report = json.loads(target.read_text())
    assert report["capture"]["status"] == "partial"
    assert report["capture"]["footer"]["events_unwritten"] == 1
    assert report["maintenance"][0]["reached_explicit_checkpoints"][0]["outcome"] == "error"
    with pytest.raises(FileExistsError):
        main(args)


class _CountingSpans(dict):
    def __init__(self, rows):
        super().__init__(rows)
        self.parent_lookups = 0

    def __getitem__(self, key):
        self.parent_lookups += 1
        return super().__getitem__(key)

    def get(self, key, default=None):
        self.parent_lookups += 1
        return super().get(key, default)


@pytest.mark.parametrize(
    ("build", "root_stage", "root_lane", "rows_key", "phase_key"),
    (
        (build_closed_worker_report, "operation", 1, "workers", "nearest_worker_phase"),
        (
            build_closed_maintenance_report,
            "runtime_maintenance",
            0,
            "maintenance",
            "nearest_phase",
        ),
    ),
)
def test_deep_closed_scope_has_fixed_phase_and_linear_parent_traversal(
    build, root_stage, root_lane, rows_key, phase_key
):
    depth = 400
    rows = {
        1: _span(1, None, "operation", 0, 0, None, root="operation"),
        2: _span(2, 1, root_stage, root_lane, 1, 900),
    }
    rows.update((key, _span(key, key - 1, "encode", 0, 2, 800)) for key in range(3, depth + 3))
    expected_group = {
        "stage": "encode",
        "lane_id": 0,
        phase_key: "outside_phase",
        "observed_count": depth,
        "sum_ns": depth * 798,
        "union_ns": 798,
        "max_ns": 798,
        "observed_counters": {},
        "thread_cpu_samples": [],
    }
    reports = []
    for ordered in (rows.items(), reversed(tuple(rows.items()))):
        capture = _partial()
        capture["spans"] = _CountingSpans(ordered)
        report = build(capture)
        assert capture["spans"].parent_lookups < 12 * (depth + 2)
        selected = report[rows_key]
        assert len(selected) == 1
        assert selected[0]["span_id"] == 2
        assert selected[0]["groups"] == [expected_group]
        reports.append(report)
    assert reports[0] == reports[1]


@pytest.mark.parametrize(
    ("build", "root_stage", "root_lane", "rows_key", "phase_key"),
    (
        (build_closed_worker_report, "operation", 1, "workers", "nearest_worker_phase"),
        (
            build_closed_maintenance_report,
            "runtime_maintenance",
            0,
            "maintenance",
            "nearest_phase",
        ),
    ),
)
def test_named_phase_belongs_to_descendants_not_its_own_group(
    build, root_stage, root_lane, rows_key, phase_key
):
    capture = _partial()
    capture["spans"] = {
        1: _span(1, None, "operation", 0, 0, None, root="operation"),
        2: _span(2, 1, root_stage, root_lane, 1, 100),
        3: _span(3, 2, "build_prepare", 0, 2, 90),
        4: _span(4, 3, "finalize", 0, 3, 80),
        5: _span(5, 4, "encode", 0, 4, 70),
    }
    groups = {row["stage"]: row for row in build(capture)[rows_key][0]["groups"]}
    assert groups["build_prepare"][phase_key] == "outside_phase"
    assert groups["finalize"][phase_key] == "build_prepare"
    assert groups["encode"][phase_key] == "finalize"
