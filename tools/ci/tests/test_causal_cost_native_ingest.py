"""Request-owned ingest attribution from the canonical parsed timeline shape."""

from __future__ import annotations

from tools.benchmark.retrieval.causal_cost_native_ingest import build_native_ingest_report

CAPTURE_ID = "a" * 32
READ_KEYS = (
    "object_port_get_calls",
    "object_port_get_succeeded",
    "object_port_get_wire_bytes",
    "object_port_retained_calls",
    "object_port_retained_succeeded",
    "object_port_retained_wire_bytes",
)
WORK_KEYS = ("work_reads", "work_writes", "work_bytes", "work_mutations")
EXTERNAL_CHILD_KEYS = (
    "managed_object_external_child_references",
    "managed_object_unique_external_children",
    "managed_object_guarded_repeated_external_children",
)
REUSED_EXTERNAL_CHILD_KEY = "managed_object_reused_external_child_validations"
CROSS_GROUP_CHILD_KEYS = (
    "managed_object_cross_group_external_child_overlap",
    "managed_object_cross_group_repeated_child_path_checks",
    "managed_object_child_path_checks",
)


def _span(
    span_id: int,
    parent: int | None,
    stage: str,
    lane: int,
    start: int,
    end: int | None,
    *,
    outcome: str | None = "ok",
    counters: dict[str, int] | None = None,
    identity: dict | None = None,
) -> dict:
    row = {
        "span_id": span_id,
        "parent_span_id": parent,
        "root_kind": stage if parent is None and stage in ("request", "operation") else None,
        "stage": stage,
        "lane_id": lane,
        "start_ns": start,
        "end_ns": end,
        "outcome": outcome if end is not None else None,
        "counters": {} if counters is None else counters,
        "bm25_aggregates": [],
    }
    if identity is not None:
        row["request_identity"] = identity
    return row


def _capture(*spans: dict, complete: bool = True) -> dict:
    return {
        "status": "complete" if complete else "partial",
        "schema_version": 5,
        "capture_id": CAPTURE_ID,
        "sha256": "b" * 64,
        "header": {"capture_id": CAPTURE_ID},
        "footer": {
            "status": "complete" if complete else "partial",
            "close_reason": "normal" if complete else "interrupted",
            "events_dropped": 0,
            "events_unwritten": 0,
            "open_spans": sum(row["end_ns"] is None for row in spans),
        },
        "spans": {row["span_id"]: row for row in spans},
        "health": [],
        "truncated_tail": False,
        "orphan_events": 0,
        "last_observed_ns": max((row["end_ns"] or row["start_ns"] for row in spans), default=0),
    }


def _identity(request_id: int) -> dict:
    return {"request_id": request_id, "plane": "native", "connection_id": 100 + request_id}


def test_complete_report_keeps_nearest_request_and_ignores_wrong_lane() -> None:
    capture = _capture(
        _span(1, None, "request", 0, 2, 200, identity=_identity(1)),
        _span(2, 1, "operation", 0, 5, 190),
        _span(3, 2, "native_source_apply", 3, 10, 70),
        _span(4, 2, "build_prepare", 3, 70, 180),
        _span(5, 4, "lexical_build", 2, 80, 130),
        _span(6, 2, "native_publish", 3, 180, 185),
        _span(10, None, "request", 0, 0, 150, identity=_identity(2)),
        _span(11, 10, "operation", 0, 10, 140),
        _span(12, 11, "native_source_apply", 3, 20, 50),
        _span(13, 11, "build_prepare", 3, 50, 120),
        _span(14, 11, "native_source_apply", 4, 60, 80),
        _span(20, None, "operation", 0, 0, 300),
        _span(21, 20, "native_source_apply", 3, 10, 290),
        _span(30, None, "request", 0, 0, 300, identity=_identity(3)),
        _span(31, 30, "native_source_apply", 4, 5, 280),
    )
    report = build_native_ingest_report(capture)
    assert report["status"] == "COMPLETE_DIAGNOSTIC"
    assert report["capture_id"] == CAPTURE_ID
    assert [row["request_span_id"] for row in report["requests"]] == [1, 10]
    first, second = report["requests"]
    assert first["request_identity"] == _identity(1)
    assert first["outcome"] == "ok"
    assert (first["start_ns"], first["end_ns"], first["wall_ns"]) == (2, 200, 198)
    assert first["stages"]["native_source_apply/3"]["count"] == 1
    assert first["stages"]["build_prepare/3"]["closed_count"] == 1
    assert first["stages"]["lexical_build/2"]["union_ns"] == 50
    assert second["request_identity"] == _identity(2)
    assert second["stages"]["native_source_apply/3"]["union_ns"] == 30
    assert "native_source_apply/4" not in second["stages"]


def test_stage_union_does_not_add_overlapping_siblings() -> None:
    report = build_native_ingest_report(
        _capture(
            _span(1, None, "request", 0, 0, 100, identity=_identity(1)),
            _span(2, 1, "native_source_apply", 3, 5, 10),
            _span(6, 1, "build_prepare", 3, 10, 95),
            _span(3, 6, "lexical_build", 2, 10, 50),
            _span(4, 6, "lexical_build", 2, 40, 70),
            _span(5, 6, "lexical_build", 2, 80, 90),
        )
    )
    group = report["requests"][0]["stages"]["lexical_build/2"]
    assert group["count"] == 3
    assert group["closed_count"] == 3
    assert group["union_ns"] == 70  # [10,70] and [80,90], not 40+30+10.


def test_object_reads_distinguish_missing_from_explicit_zero() -> None:
    zeros = dict.fromkeys(READ_KEYS, 0)
    report = build_native_ingest_report(
        _capture(
            _span(1, None, "request", 0, 0, 100, identity=_identity(1)),
            _span(2, 1, "native_source_apply", 3, 10, 40, counters=zeros),
            _span(3, 1, "build_prepare", 3, 40, 90),
        )
    )
    stages = report["requests"][0]["stages"]
    assert stages["native_source_apply/3"]["object_port_reads"] == zeros
    assert stages["build_prepare/3"]["object_port_reads"] is None


def test_object_reads_sum_only_their_own_stage_and_request() -> None:
    source_counts = dict(zip(READ_KEYS, (2, 2, 12, 1, 1, 8)))
    build_counts = dict(zip(READ_KEYS, (3, 3, 30, 0, 0, 0)))
    report = build_native_ingest_report(
        _capture(
            _span(1, None, "request", 0, 0, 100, identity=_identity(1)),
            _span(2, 1, "native_source_apply", 3, 10, 30, counters=source_counts),
            _span(3, 1, "build_prepare", 3, 30, 90, counters=build_counts),
            _span(10, None, "request", 0, 0, 80, identity=_identity(2)),
            _span(
                11,
                10,
                "native_source_apply",
                3,
                10,
                60,
                counters=dict.fromkeys(READ_KEYS, 0),
            ),
        )
    )
    first, second = report["requests"]
    assert first["stages"]["native_source_apply/3"]["object_port_reads"] == source_counts
    assert first["stages"]["build_prepare/3"]["object_port_reads"] == build_counts
    assert second["stages"]["native_source_apply/3"]["object_port_reads"] == dict.fromkeys(
        READ_KEYS, 0
    )


def test_work_usage_reports_direct_child_deltas_without_parent_addition() -> None:
    source = dict(zip(WORK_KEYS, (100, 10, 1_000, 20)))
    build = dict(zip(WORK_KEYS, (70, 7, 700, 14)))
    posting_a = dict(zip(WORK_KEYS, (9, 0, 42, 0)))
    posting_b = dict(zip(WORK_KEYS, (4, 2, 19, 3)))
    inventory = dict(zip(WORK_KEYS, (0, 1, 7, 1)))
    diff = dict(zip(WORK_KEYS, (2, 0, 11, 0)))
    report = build_native_ingest_report(
        _capture(
            _span(1, None, "request", 0, 0, 100, identity=_identity(1)),
            _span(2, 1, "native_source_apply", 3, 1, 10, counters=source),
            _span(3, 1, "build_prepare", 3, 10, 90, counters=build),
            _span(4, 3, "native_text_postings", 35, 20, 30, counters=posting_a),
            _span(5, 3, "native_text_postings", 35, 30, 40, counters=posting_b),
            _span(6, 3, "native_text_inventory_map_apply", 40, 40, 50, counters=inventory),
            _span(7, 3, "native_text_inventory_diff", 29, 50, 60, counters=diff),
        )
    )
    stages = report["requests"][0]["stages"]
    assert stages["native_source_apply/3"]["work_usage"] == source
    assert stages["build_prepare/3"]["work_usage"] == build
    assert stages["native_text_postings/35"]["work_usage"] == dict(zip(WORK_KEYS, (13, 2, 61, 3)))
    assert stages["native_text_inventory_map_apply/40"]["work_usage"] == inventory
    assert stages["native_text_inventory_diff/29"]["work_usage"] == diff
    assert "object_port_reads" not in stages["native_text_postings/35"]


def test_work_usage_distinguishes_zero_missing_and_open_spans() -> None:
    zeros = dict.fromkeys(WORK_KEYS, 0)
    report = build_native_ingest_report(
        _capture(
            _span(1, None, "request", 0, 0, 100, identity=_identity(1)),
            _span(2, 1, "build_prepare", 3, 1, 90, counters=zeros),
            _span(3, 2, "native_text_postings", 35, 10, 20, counters=zeros),
            _span(4, 2, "native_text_postings", 36, 20, 30, counters=zeros),
            _span(5, 2, "native_text_postings", 36, 30, 40),
            _span(6, 2, "native_text_postings", 37, 40, None, counters=zeros),
            complete=False,
        )
    )
    stages = report["requests"][0]["stages"]
    assert stages["native_text_postings/35"]["work_usage"] == zeros
    assert stages["native_text_postings/36"]["work_usage"] is None
    assert stages["native_text_postings/37"]["work_usage"] is None
    assert stages["build_prepare/3"]["work_usage"] == zeros


def test_partial_capture_keeps_open_request_last_without_inventing_duration() -> None:
    report = build_native_ingest_report(
        _capture(
            _span(1, None, "request", 0, 0, 50, identity=_identity(1)),
            _span(2, 1, "build_prepare", 3, 10, 40),
            _span(10, None, "request", 0, 0, None, identity=_identity(2)),
            _span(11, 10, "native_source_apply", 3, 20, None),
            complete=False,
        )
    )
    assert report["status"] == "PARTIAL_DIAGNOSTIC"
    assert [row["request_span_id"] for row in report["requests"]] == [1, 10]
    open_request = report["requests"][1]
    assert open_request["outcome"] is None
    assert (open_request["start_ns"], open_request["end_ns"]) == (0, None)
    assert open_request["wall_ns"] is None
    open_stage = open_request["stages"]["native_source_apply/3"]
    assert open_stage["count"] == 1
    assert open_stage["closed_count"] == 0
    assert open_stage["union_ns"] is None


def test_commit_attribution_uses_nearest_observed_phase_and_union() -> None:
    report = build_native_ingest_report(
        _capture(
            _span(1, None, "request", 0, 0, 100, identity=_identity(1)),
            _span(2, 1, "build_prepare", 3, 1, 80),
            _span(3, 2, "native_text_inventory_prepare", 2, 2, 75),
            _span(4, 3, "managed_object_confirm", 0, 10, 70),
            _span(5, 4, "sqlite_work", 3, 20, 30),
            _span(6, 4, "sqlite_work", 3, 25, 35),
            _span(7, 1, "native_publish", 3, 80, 99),
            _span(8, 7, "sqlite_work", 3, 90, 95),
        )
    )
    commits = report["requests"][0]["stages"]["sqlite_work/3"]
    assert commits["union_ns"] == 20
    assert commits["wall_union_ns_by_nearest_phase"] == {
        "native_text_inventory_prepare/2": 15,
        "native_publish/3": 5,
    }


def test_prepared_install_advance_is_attributed_to_its_posting_phase() -> None:
    report = build_native_ingest_report(
        _capture(
            _span(1, None, "request", 0, 0, 100, identity=_identity(1)),
            _span(2, 1, "build_prepare", 3, 1, 90),
            _span(3, 2, "native_text_postings", 37, 10, 70),
            _span(4, 3, "managed_object_advance", 3, 20, 45),
            _span(5, 4, "sqlite_work", 3, 30, 40),
        )
    )
    stages = report["requests"][0]["stages"]
    advance = stages["managed_object_advance/3"]
    assert advance["count"] == 1
    assert advance["wall_union_ns_by_nearest_phase"] == {"native_text_postings/37": 25}
    assert stages["sqlite_work/3"]["wall_union_ns_by_nearest_phase"] == {
        "native_text_postings/37": 10
    }


def test_managed_install_external_children_report_exact_closed_groups_without_cross_group_unique_sum() -> None:
    first = dict(zip(EXTERNAL_CHILD_KEYS, (5, 3, 1))) | {REUSED_EXTERNAL_CHILD_KEY: 1}
    second = dict(zip(EXTERNAL_CHILD_KEYS, (4, 2, 2))) | {REUSED_EXTERNAL_CHILD_KEY: 2}
    report = build_native_ingest_report(
        _capture(
            _span(1, None, "request", 0, 0, 150, identity=_identity(1)),
            _span(2, 1, "build_prepare", 3, 1, 145),
            _span(3, 2, "native_text_postings", 39, 10, 120),
            _span(4, 3, "managed_object_install", 3, 20, 40, outcome="error", counters=first),
            _span(5, 3, "managed_object_install", 3, 45, 60, counters=second),
            _span(6, 3, "managed_object_install", 3, 65, 80),
            _span(7, 3, "managed_object_install", 3, 85, None, counters=first),
            _span(8, 3, "managed_object_install", 0, 90, 100, counters=first),
            complete=False,
        )
    )
    group = report["requests"][0]["stages"]["managed_object_install/3"]
    assert group["count"] == 4 and group["closed_count"] == 3
    assert group["external_child_groups"] == [
        {
            "span_id": 4,
            "nearest_phase": "native_text_postings/39",
            "outcome": "error",
            "observation": "COMPLETE",
            "counts": {key: first[key] for key in EXTERNAL_CHILD_KEYS},
            "observed_keys": [],
            "reused_external_child_validations": 1,
            "cross_group_child_checks": {
                "observation": "MISSING", "counts": None, "observed_keys": [],
            },
        },
        {
            "span_id": 5,
            "nearest_phase": "native_text_postings/39",
            "outcome": "ok",
            "observation": "COMPLETE",
            "counts": {key: second[key] for key in EXTERNAL_CHILD_KEYS},
            "observed_keys": [],
            "reused_external_child_validations": 2,
            "cross_group_child_checks": {
                "observation": "MISSING", "counts": None, "observed_keys": [],
            },
        },
        {
            "span_id": 6,
            "nearest_phase": "native_text_postings/39",
            "outcome": "ok",
            "observation": "MISSING",
            "counts": None,
            "observed_keys": [],
            "reused_external_child_validations": None,
            "cross_group_child_checks": {
                "observation": "MISSING", "counts": None, "observed_keys": [],
            },
        },
    ]
    assert "external_child_groups" not in report["requests"][0]["stages"]["managed_object_install/0"]
    assert "unique" not in group  # Per-group uniqueness cannot become a run-wide sum.


def test_managed_install_cross_group_checks_preserve_actual_prefix_and_missing_state() -> None:
    complete = dict(zip(CROSS_GROUP_CHILD_KEYS, (1, 3, 4)))
    zero = dict.fromkeys(CROSS_GROUP_CHILD_KEYS, 0)
    partial = {CROSS_GROUP_CHILD_KEYS[0]: 2}
    report = build_native_ingest_report(
        _capture(
            _span(1, None, "request", 0, 0, 100, identity=_identity(1)),
            _span(2, 1, "build_prepare", 3, 1, 90),
            _span(3, 2, "managed_object_install", 3, 10, 20, outcome="error", counters=complete),
            _span(4, 2, "managed_object_install", 3, 30, 40, counters=partial),
            _span(5, 2, "managed_object_install", 3, 50, 60),
            _span(6, 2, "managed_object_install", 3, 70, 80, counters=zero),
            complete=False,
        )
    )
    rows = report["requests"][0]["stages"]["managed_object_install/3"]["external_child_groups"]
    assert [row["cross_group_child_checks"] for row in rows] == [
        {"observation": "COMPLETE", "counts": complete, "observed_keys": []},
        {"observation": "PARTIAL", "counts": None, "observed_keys": [CROSS_GROUP_CHILD_KEYS[0]]},
        {"observation": "MISSING", "counts": None, "observed_keys": []},
        {"observation": "COMPLETE", "counts": zero, "observed_keys": []},
    ]
    assert [row["outcome"] for row in rows] == ["error", "ok", "ok", "ok"]


def test_managed_install_external_children_distinguish_partial_missing_and_zero() -> None:
    partial = {EXTERNAL_CHILD_KEYS[0]: 2}
    zeros = dict.fromkeys(EXTERNAL_CHILD_KEYS, 0)
    report = build_native_ingest_report(
        _capture(
            _span(1, None, "request", 0, 0, 100, identity=_identity(1)),
            _span(2, 1, "build_prepare", 3, 1, 90),
            _span(3, 2, "managed_object_install", 3, 10, 20, counters=partial),
            _span(4, 2, "managed_object_install", 3, 30, 40),
            _span(5, 2, "managed_object_install", 3, 50, 60, counters=zeros),
            complete=False,
        )
    )
    groups = report["requests"][0]["stages"]["managed_object_install/3"]["external_child_groups"]
    assert [(row["span_id"], row["observation"], row["counts"]) for row in groups] == [
        (3, "PARTIAL", None),
        (4, "MISSING", None),
        (5, "COMPLETE", zeros),
    ]
    assert groups[0]["observed_keys"] == [EXTERNAL_CHILD_KEYS[0]]


def test_managed_install_actual_reuse_reports_failed_prefix_without_planned_triple() -> None:
    report = build_native_ingest_report(
        _capture(
            _span(1, None, "request", 0, 0, 100, identity=_identity(1)),
            _span(2, 1, "build_prepare", 3, 1, 90),
            _span(3, 2, "managed_object_install", 3, 10, 20, outcome="error",
                  counters={REUSED_EXTERNAL_CHILD_KEY: 1}),
            _span(4, 2, "managed_object_install", 3, 30, 40,
                  counters={REUSED_EXTERNAL_CHILD_KEY: 0}),
            _span(5, 2, "managed_object_install", 3, 50, 60),
        )
    )
    groups = report["requests"][0]["stages"]["managed_object_install/3"]["external_child_groups"]
    assert [(row["observation"], row["reused_external_child_validations"], row["outcome"])
            for row in groups] == [
        ("MISSING", 1, "error"),
        ("MISSING", 0, "ok"),
        ("MISSING", None, "ok"),
    ]


def test_member_build_phases_keep_direct_work_and_nearest_sql_owner() -> None:
    build = dict(zip(WORK_KEYS, (100, 90, 1000, 80)))
    member = dict(zip(WORK_KEYS, (50, 40, 500, 30)))
    check = dict(zip(WORK_KEYS, (10, 5, 100, 4)))
    stage = dict(zip(WORK_KEYS, (20, 15, 200, 10)))
    maps = dict(zip(WORK_KEYS, (15, 14, 150, 12)))
    report = build_native_ingest_report(
        _capture(
            _span(1, None, "request", 0, 0, 100, identity=_identity(1)),
            _span(2, 1, "build_prepare", 3, 1, 95, counters=build),
            _span(3, 2, "build_prepare", 41, 5, 80, counters=member),
            _span(4, 3, "build_prepare", 49, 10, 20, counters=check),
            _span(5, 3, "build_prepare", 50, 20, 50, counters=stage),
            _span(6, 3, "build_prepare", 51, 50, 70, counters=maps),
            _span(7, 5, "managed_object_confirm", 3, 25, 45),
            _span(8, 7, "sqlite_work", 3, 30, 40),
            _span(9, 2, "build_prepare", 43, 80, 90),
            _span(10, 2, "build_prepare", 52, 90, 91),
            # A member span alone cannot establish Kernel request custody.
            _span(20, None, "request", 0, 0, 110, identity=_identity(2)),
            _span(21, 20, "build_prepare", 41, 1, 105, counters=member),
        )
    )
    assert report["matched_requests"] == 1
    stages = report["requests"][0]["stages"]
    assert stages["build_prepare/3"]["work_usage"] == build
    assert stages["build_prepare/41"]["work_usage"] == member
    assert stages["build_prepare/49"]["work_usage"] == check
    assert stages["build_prepare/50"]["work_usage"] == stage
    assert stages["build_prepare/51"]["work_usage"] == maps
    assert stages["build_prepare/43"]["work_usage"] is None
    assert "build_prepare/52" not in stages
    assert stages["managed_object_confirm/3"]["wall_union_ns_by_nearest_phase"] == {
        "build_prepare/50": 20
    }
    assert stages["sqlite_work/3"]["wall_union_ns_by_nearest_phase"] == {"build_prepare/50": 10}


def test_literal_build_phases_keep_missing_work_unobserved() -> None:
    zeros = dict.fromkeys(WORK_KEYS, 0)
    report = build_native_ingest_report(
        _capture(
            _span(1, None, "request", 0, 0, 100, identity=_identity(1)),
            _span(2, 1, "build_prepare", 3, 1, 95),
            _span(3, 2, "build_prepare", 40, 5, 80, counters=zeros),
            _span(4, 3, "build_prepare", 44, 10, 20, counters=zeros),
            _span(5, 3, "build_prepare", 45, 20, 30, counters=zeros),
            _span(6, 3, "build_prepare", 46, 30, 40, counters=zeros),
            _span(7, 3, "build_prepare", 47, 40, None, counters=zeros),
            _span(8, 3, "build_prepare", 48, 50, 60),
            _span(9, 2, "build_prepare", 42, 80, 90, counters=zeros),
            complete=False,
        )
    )
    stages = report["requests"][0]["stages"]
    for lane in (40, 42, 44, 45, 46):
        assert stages[f"build_prepare/{lane}"]["work_usage"] == zeros
    assert stages["build_prepare/47"]["work_usage"] is None
    assert stages["build_prepare/47"]["union_ns"] is None
    assert stages["build_prepare/48"]["work_usage"] is None
