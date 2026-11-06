"""Independent fixed worker/refusal gold for the reusable diagnostic projection."""

from __future__ import annotations

import copy
import json

import pytest

from tools.benchmark.retrieval.causal_cost_diagnostic import (
    MAX_RECORD_BYTES,
    build_index,
    main,
    query_index,
)

CAPTURE_ID = "ab" * 16


def _span(
    span_id: int,
    parent: int | None,
    stage: str,
    lane: int,
    start: int,
    end: int,
    outcome: str = "ok",
    *,
    root: str | None = None,
    counters: dict[str, int] | None = None,
) -> dict:
    return {
        "span_id": span_id,
        "parent_span_id": parent,
        "stage": stage,
        "lane_id": lane,
        "start_ns": start,
        "end_ns": end,
        "outcome": outcome,
        "root_kind": root,
        "counters": counters or {},
    }


def _capture() -> dict:
    return {
        "status": "complete",
        "capture_id": CAPTURE_ID,
        "sha256": "01" * 32,
        "schema_version": 4,
        "health": [],
        "orphan_events": 0,
        "truncated_tail": False,
        "footer": {
            "capture_id": CAPTURE_ID,
            "status": "complete",
            "events_dropped": 0,
            "events_unwritten": 0,
            "open_spans": 0,
            "producer_leases": 0,
        },
        "spans": {
            1: _span(1, None, "operation", 0, 0, 100, root="operation"),
            2: _span(2, 1, "operation", 1, 2, 10, "error"),
            3: _span(3, 1, "operation", 1, 20, 80, "error"),
            4: _span(4, 3, "build_prepare", 2, 21, 79),
            5: _span(5, 4, "sqlite_work", 11, 25, 50),
            6: _span(6, 4, "sqlite_work", 11, 40, 70),
            7: _span(7, 4, "sqlite_work", 12, 60, 75),
            8: _span(
                8,
                4,
                "sqlite_work",
                22,
                22,
                78,
                counters={
                    "sqlite_write_calls": 5,
                    "sqlite_write_requested_bytes": 100,
                    "sqlite_write_failures": 0,
                    "sqlite_wal_write_calls": 3,
                    "sqlite_main_write_calls": 1,
                    "sqlite_other_write_calls": 1,
                },
            ),
        },
    }


def _capture_with_callback_timing() -> dict:
    capture = _capture()
    capture["spans"][8]["counters"].update(
        {
            "sqlite_write_admission_calls": 7,
            "sqlite_write_admission_refusals": 2,
            "sqlite_write_admission_elapsed_ns": 21,
            "sqlite_write_lower_envelope_elapsed_ns": 35,
            "sqlite_write_post_callback_elapsed_ns": 13,
            "sqlite_write_post_callback_refusals": 1,
        }
    )
    return capture


def _capture_with_cache_deltas() -> dict:
    capture = _capture()
    capture["spans"][8]["counters"].update(
        {
            "sqlite_write_admission_calls": 5,
            "sqlite_write_admission_refusals": 0,
            "sqlite_write_admission_elapsed_ns": 3,
            "sqlite_write_lower_envelope_elapsed_ns": 4,
            "sqlite_write_post_callback_elapsed_ns": 2,
            "sqlite_write_post_callback_refusals": 0,
        }
    )
    capture["spans"][9] = _span(
        9,
        4,
        "catalog_lock_hold",
        0,
        23,
        35,
        counters={
            "sqlite_cache_hits": 13,
            "sqlite_cache_misses": 3,
            "sqlite_cache_writes": 9,
            "sqlite_cache_spills": 2,
        },
    )
    capture["spans"][10] = _span(
        10,
        4,
        "catalog_lock_hold",
        0,
        45,
        60,
        counters={
            "sqlite_cache_hits": 5,
            "sqlite_cache_misses": 1,
            "sqlite_cache_writes": 4,
            "sqlite_cache_spills": 1,
        },
    )
    return capture


def _record(*, span_id: int = 3, capture_id: str = CAPTURE_ID) -> dict:
    return {
        "event": "native_owned_work_refusal",
        "operation_key": {"workspace": {}, "epoch": 1, "sequence": 7},
        "budget_diagnostic": "original deadline",
        "execution": {
            "dispatch_ordinal": 2,
            "assignment_dispatches": 2,
            "assignment_deadline_ms": 1000,
            "claim": {"intent": {"workspace": {}}, "fence": 41, "lease_until_ms": 900},
            "trace": {"capture_id": capture_id, "span_id": span_id},
        },
        "phase": "granted_execution",
        "outcome": "Err(ResourceLimited)",
        "sample_ordinal": 1,
        "sample_limit": 8,
    }


def _raw(record: dict) -> str:
    return json.dumps(record, separators=(",", ":"))


def test_worker_index_unions_phase_lane_and_direct_requested_write_counts() -> None:
    index = build_index(_capture(), refusal_log=_raw(_record()))
    assert index["schema_version"] == 2
    assert index["status"] == "UNBOUND_DIAGNOSTIC_ONLY"
    assert index["source_binary_binding"] == "NOT_PROVIDED"
    assert index["capture"]["stream_sha256"] == "01" * 32
    assert index["worker_count"] == 2
    worker = index["workers"]["3"]
    wal = next(
        row for row in worker["groups"] if row["stage"] == "sqlite_work" and row["lane_id"] == 11
    )
    assert wal["nearest_worker_phase"] == "build_prepare"
    assert (wal["count"], wal["sum_ns"], wal["union_ns"], wal["max_ns"]) == (2, 55, 45, 30)
    write = next(row for row in worker["groups"] if row["lane_id"] == 22)
    assert write["direct_counters"]["sqlite_write_requested_bytes"] == 100
    assert write["direct_counters"]["sqlite_wal_write_calls"] == 3
    assert "sqlite_write_admission_elapsed_ns" not in write["direct_counters"]
    assert "NOT_OBSERVED" in index["scope"]["sqlite_write_callbacks"]
    assert index["refusals"]["status"] == "DIAGNOSTIC_MATCHED"
    assert index["refusals"]["matched"][0]["execution"]["claim"]["fence"] == 41
    selected = query_index(index, worker_span_id=3, stage="sqlite_work", lane=11)
    assert selected["matched_workers"] == 1
    assert selected["workers"]["3"]["groups"] == [wal]
    assert query_index(index, refusals_only=True)["matched_workers"] == 1


def test_worker_index_groups_work_seal_children_under_their_own_phase() -> None:
    capture = _capture()
    capture["spans"][4]["stage"] = "native_work_seal"
    index = build_index(capture)
    selected = query_index(index, worker_span_id=3, stage="sqlite_work", lane=11)
    assert selected["workers"]["3"]["groups"][0]["nearest_worker_phase"] == "native_work_seal"


@pytest.mark.parametrize("second_cpu", (None, 11))
def test_worker_index_keeps_cpu_per_span_only_when_group_observation_is_partial_or_full(
    second_cpu: int | None,
) -> None:
    capture = _capture()
    capture["spans"][9] = _span(
        9, 3, "native_source_apply", 3, 24, 40,
        counters={"thread_cpu_ns": 7, "bytes_read": 3},
    )
    second_counters = {"bytes_read": 5}
    if second_cpu is not None:
        second_counters["thread_cpu_ns"] = second_cpu
    capture["spans"][10] = _span(
        10, 3, "native_source_apply", 3, 30, 45, counters=second_counters,
    )
    original = copy.deepcopy(capture)

    index = build_index(capture)
    group = next(
        row for row in index["workers"]["3"]["groups"]
        if row["stage"] == "native_source_apply" and row["lane_id"] == 3
    )
    assert (group["count"], group["sum_ns"], group["union_ns"]) == (2, 31, 21)
    assert group["direct_counters"] == {"bytes_read": 8}
    assert "per-span" in index["scope"]["time"]
    assert capture == original
    assert capture["spans"][9]["counters"]["thread_cpu_ns"] == 7
    assert ("thread_cpu_ns" in capture["spans"][10]["counters"]) == (second_cpu is not None)
    assert query_index(index, worker_span_id=3, stage="native_source_apply")["workers"]["3"][
        "groups"
    ] == [group]

    stale = copy.deepcopy(index)
    stale_group = next(
        row for row in stale["workers"]["3"]["groups"]
        if row["stage"] == "native_source_apply" and row["lane_id"] == 3
    )
    stale_group["direct_counters"]["thread_cpu_ns"] = 7 + (second_cpu or 0)
    with pytest.raises(ValueError, match="per-span CPU"):
        query_index(stale)


def test_write_callback_wall_is_aggregated_only_from_complete_observations() -> None:
    capture = _capture_with_callback_timing()
    capture["spans"][9] = _span(
        9,
        4,
        "sqlite_work",
        22,
        24,
        45,
        counters={
            "sqlite_write_calls": 1,
            "sqlite_write_requested_bytes": 7,
            "sqlite_write_failures": 0,
            "sqlite_wal_write_calls": 0,
            "sqlite_main_write_calls": 1,
            "sqlite_other_write_calls": 0,
            "sqlite_write_admission_calls": 1,
            "sqlite_write_admission_refusals": 0,
            "sqlite_write_admission_elapsed_ns": 4,
            "sqlite_write_lower_envelope_elapsed_ns": 9,
            "sqlite_write_post_callback_elapsed_ns": 2,
            "sqlite_write_post_callback_refusals": 0,
        },
    )
    index = build_index(capture)
    write = next(row for row in index["workers"]["3"]["groups"] if row["lane_id"] == 22)
    assert write["count"] == 2
    assert {
        key: write["direct_counters"][key]
        for key in (
            "sqlite_write_calls",
            "sqlite_write_admission_calls",
            "sqlite_write_admission_refusals",
            "sqlite_write_admission_elapsed_ns",
            "sqlite_write_lower_envelope_elapsed_ns",
            "sqlite_write_post_callback_elapsed_ns",
            "sqlite_write_post_callback_refusals",
        )
    } == {
        "sqlite_write_calls": 6,
        "sqlite_write_admission_calls": 8,
        "sqlite_write_admission_refusals": 2,
        "sqlite_write_admission_elapsed_ns": 25,
        "sqlite_write_lower_envelope_elapsed_ns": 44,
        "sqlite_write_post_callback_elapsed_ns": 15,
        "sqlite_write_post_callback_refusals": 1,
    }
    assert query_index(index, worker_span_id=3, lane=22)["workers"]["3"]["groups"] == [write]


def test_cache_deltas_are_direct_catalog_hold_counters_with_explicit_validity() -> None:
    index = build_index(_capture_with_cache_deltas())
    assert index["sqlite_cache_validity"] == "OBSERVED"
    selected = query_index(index, worker_span_id=3, stage="catalog_lock_hold", lane=0)
    assert selected["sqlite_cache_validity"] == "OBSERVED"
    assert selected["workers"]["3"]["groups"][0]["direct_counters"] == {
        "sqlite_cache_hits": 18,
        "sqlite_cache_misses": 4,
        "sqlite_cache_writes": 13,
        "sqlite_cache_spills": 3,
    }
    assert "NOT_OBSERVED" in index["scope"]["sqlite_cache"]

    old = _capture_with_cache_deltas()
    old["spans"][9]["counters"].clear()
    old["spans"][10]["counters"].clear()
    old_index = build_index(old)
    assert old_index["sqlite_cache_validity"] == "NOT_OBSERVED"
    assert not query_index(old_index, worker_span_id=3, lane=0)["workers"]["3"]["groups"][0][
        "direct_counters"
    ]


def test_cache_deltas_refuse_partial_mixed_wrong_owner_and_corrupt_cache() -> None:
    for mutate in (
        lambda capture: capture["spans"][9]["counters"].pop("sqlite_cache_hits"),
        lambda capture: capture["spans"][9]["counters"].update(sqlite_cache_spills=-1),
        lambda capture: capture["spans"][9].update(lane_id=1),
        lambda capture: capture["spans"][10]["counters"].clear(),
    ):
        capture = _capture_with_cache_deltas()
        mutate(capture)
        with pytest.raises(ValueError, match="SQLite cache|sqlite_cache"):
            build_index(capture)

    baseline = build_index(_capture_with_cache_deltas())
    for mutate in (
        lambda index: index.update(sqlite_cache_validity="NOT_OBSERVED"),
        lambda index: next(
            group
            for group in index["workers"]["3"]["groups"]
            if group["stage"] == "catalog_lock_hold"
        )["direct_counters"].pop("sqlite_cache_misses"),
        lambda index: next(
            group
            for group in index["workers"]["3"]["groups"]
            if group["stage"] == "catalog_lock_hold"
        )["direct_counters"].update(sqlite_cache_hits=-1),
        lambda index: next(
            group for group in index["workers"]["3"]["groups"] if group["lane_id"] == 22
        )["direct_counters"].update(sqlite_write_failures=1),
    ):
        cached = copy.deepcopy(baseline)
        mutate(cached)
        with pytest.raises(ValueError, match="SQLite cache|cached worker counters"):
            query_index(cached)


@pytest.mark.parametrize(
    "error_key",
    (
        "sqlite_write_failures",
        "sqlite_write_admission_refusals",
        "sqlite_write_post_callback_refusals",
    ),
)
def test_page_write_error_makes_cache_deltas_unusable_without_erasing_raw_values(
    error_key: str,
) -> None:
    capture = _capture_with_cache_deltas()
    capture["spans"][8]["counters"][error_key] = 1
    if error_key == "sqlite_write_admission_refusals":
        capture["spans"][8]["counters"]["sqlite_write_admission_calls"] += 1
    index = build_index(capture)
    assert index["sqlite_cache_validity"] == "PAGER_WRITE_UNDEFINED_AFTER_WRITE_ERROR"
    selected = query_index(index, worker_span_id=3, stage="catalog_lock_hold", lane=0)
    assert selected["sqlite_cache_validity"] == "PAGER_WRITE_UNDEFINED_AFTER_WRITE_ERROR"
    assert selected["workers"]["3"]["groups"][0]["direct_counters"]["sqlite_cache_writes"] == 13
    invalid_cached = copy.deepcopy(index)
    invalid_cached["sqlite_cache_validity"] = "OBSERVED"
    with pytest.raises(ValueError, match="ignores write failure"):
        query_index(invalid_cached)

    outside_worker = _capture_with_cache_deltas()
    outside_worker["spans"][11] = _span(
        11,
        1,
        "sqlite_work",
        22,
        81,
        90,
        counters={
            "sqlite_write_calls": 1,
            "sqlite_write_requested_bytes": 4,
            "sqlite_write_failures": int(error_key == "sqlite_write_failures"),
            "sqlite_wal_write_calls": 0,
            "sqlite_main_write_calls": 1,
            "sqlite_other_write_calls": 0,
            "sqlite_write_admission_calls": 1 + int(error_key == "sqlite_write_admission_refusals"),
            "sqlite_write_admission_refusals": int(error_key == "sqlite_write_admission_refusals"),
            "sqlite_write_admission_elapsed_ns": 1,
            "sqlite_write_lower_envelope_elapsed_ns": 1,
            "sqlite_write_post_callback_elapsed_ns": 1,
            "sqlite_write_post_callback_refusals": int(
                error_key == "sqlite_write_post_callback_refusals"
            ),
        },
    )
    outside_index = build_index(outside_worker)
    assert outside_index["sqlite_cache_validity"] == "PAGER_WRITE_UNDEFINED_AFTER_WRITE_ERROR"
    outside_selected = query_index(
        outside_index, worker_span_id=3, stage="catalog_lock_hold", lane=0
    )
    assert (
        outside_selected["workers"]["3"]["groups"][0]["direct_counters"]["sqlite_cache_writes"]
        == 13
    )


def test_missing_callback_observation_cannot_prove_cache_write_validity() -> None:
    from tools.benchmark.retrieval.causal_cost_diagnostic import WRITE_CALLBACK_KEYS

    capture = _capture_with_cache_deltas()
    for key in WRITE_CALLBACK_KEYS:
        capture["spans"][8]["counters"].pop(key)
    index = build_index(capture)
    assert index["sqlite_cache_validity"] == "PAGER_WRITE_OUTCOME_NOT_OBSERVED"
    selected = query_index(index, worker_span_id=3, stage="catalog_lock_hold", lane=0)
    assert selected["workers"]["3"]["groups"][0]["direct_counters"]["sqlite_cache_writes"] == 13
    index["sqlite_cache_validity"] = "OBSERVED"
    with pytest.raises(ValueError, match="missing callback timing"):
        query_index(index)


def test_write_callback_timing_rejects_partial_or_contradictory_capture() -> None:
    for mutate in (
        lambda counters: counters.pop("sqlite_write_admission_elapsed_ns"),
        lambda counters: counters.update(sqlite_write_admission_elapsed_ns=-1),
        lambda counters: counters.update(sqlite_write_admission_calls=6),
        lambda counters: counters.update(sqlite_write_post_callback_refusals=6),
    ):
        capture = _capture_with_callback_timing()
        mutate(capture["spans"][8]["counters"])
        with pytest.raises(ValueError, match="SQLite callback timing"):
            build_index(capture)

    mixed = _capture_with_callback_timing()
    mixed["spans"][9] = _span(
        9,
        4,
        "sqlite_work",
        22,
        24,
        45,
        counters={
            "sqlite_write_calls": 1,
            "sqlite_write_requested_bytes": 7,
            "sqlite_write_failures": 0,
            "sqlite_wal_write_calls": 0,
            "sqlite_main_write_calls": 1,
            "sqlite_other_write_calls": 0,
        },
    )
    with pytest.raises(ValueError, match="partial within one group"):
        build_index(mixed)


def test_zero_reached_write_retains_refused_admission_wall_without_fabricating_lower() -> None:
    capture = _capture_with_callback_timing()
    counters = capture["spans"][8]["counters"]
    counters.update(
        sqlite_write_calls=0,
        sqlite_write_requested_bytes=0,
        sqlite_write_failures=0,
        sqlite_wal_write_calls=0,
        sqlite_main_write_calls=0,
        sqlite_other_write_calls=0,
        sqlite_write_admission_calls=2,
        sqlite_write_admission_refusals=2,
        sqlite_write_admission_elapsed_ns=9,
        sqlite_write_lower_envelope_elapsed_ns=0,
        sqlite_write_post_callback_elapsed_ns=0,
        sqlite_write_post_callback_refusals=0,
    )
    built = build_index(capture)
    write = next(row for row in built["workers"]["3"]["groups"] if row["lane_id"] == 22)
    assert write["direct_counters"]["sqlite_write_admission_elapsed_ns"] == 9
    assert write["direct_counters"]["sqlite_write_lower_envelope_elapsed_ns"] == 0
    for key in ("sqlite_write_lower_envelope_elapsed_ns", "sqlite_write_post_callback_elapsed_ns"):
        refused = copy.deepcopy(capture)
        refused["spans"][8]["counters"][key] = 1
        with pytest.raises(ValueError, match="SQLite callback timing"):
            build_index(refused)
    no_calls = copy.deepcopy(capture)
    no_calls["spans"][8]["counters"].update(
        sqlite_write_admission_calls=0,
        sqlite_write_admission_refusals=0,
        sqlite_write_admission_elapsed_ns=0,
    )
    assert build_index(no_calls)["workers"]["3"]["groups"]
    no_calls["spans"][8]["counters"]["sqlite_write_admission_elapsed_ns"] = 1
    with pytest.raises(ValueError, match="SQLite callback timing"):
        build_index(no_calls)


def test_backfill_counts_only_descendant_writes_and_nonadditive_commit_overlap() -> None:
    capture = _capture()
    capture["spans"].update(
        {
            9: _span(9, 4, "sqlite_work", 25, 24, 76),
            10: _span(
                10,
                9,
                "sqlite_work",
                22,
                30,
                50,
                counters={
                    "sqlite_write_calls": 2,
                    "sqlite_write_requested_bytes": 200,
                    "sqlite_write_failures": 1,
                    "sqlite_wal_write_calls": 2,
                    "sqlite_main_write_calls": 0,
                    "sqlite_other_write_calls": 0,
                },
            ),
            11: _span(11, 4, "sqlite_work", 3, 21, 70),
            12: _span(12, 4, "sqlite_work", 25, 60, 77),
            13: _span(
                13,
                12,
                "sqlite_work",
                22,
                62,
                66,
                counters={
                    "sqlite_write_calls": 1,
                    "sqlite_write_requested_bytes": 40,
                    "sqlite_write_failures": 0,
                    "sqlite_wal_write_calls": 0,
                    "sqlite_main_write_calls": 1,
                    "sqlite_other_write_calls": 0,
                },
            ),
        }
    )
    index = build_index(capture, refusal_log=_raw(_record()))
    backfill = index["workers"]["3"]["backfill"]
    assert backfill == {
        "window_count": 2,
        "window_outcomes": {"ok": 2, "error": 0, "cancelled": 0, "aborted": 0},
        "window_union_ns": 53,
        "descendant_write_counters": {
            "sqlite_write_calls": 3,
            "sqlite_write_requested_bytes": 240,
            "sqlite_write_failures": 1,
            "sqlite_wal_write_calls": 2,
            "sqlite_main_write_calls": 1,
            "sqlite_other_write_calls": 0,
        },
        "commit_union_ns": 49,
        "commit_overlap_ns": 46,
        "commit_minus_backfill_ns": 3,
    }
    # The original lane22 sibling overlaps the backfill clock interval, but
    # its requested 100 bytes have no lane25 ancestor.
    assert backfill["descendant_write_counters"]["sqlite_write_requested_bytes"] == 240
    assert query_index(index, worker_span_id=3, lane=25)["workers"]["3"]["backfill"] == backfill
    assert query_index(index, worker_span_id=3, lane=3)["workers"]["3"]["backfill"] == backfill


def test_backfill_rejects_invalid_ancestry_or_lane_before_aggregation() -> None:
    capture = _capture()
    capture["spans"][9] = _span(9, 4, "sqlite_work", 25, 24, 76)
    capture["spans"][10] = _span(10, 9, "sqlite_work", 22, 20, 40)
    with pytest.raises(ValueError, match="escapes parent interval"):
        build_index(capture)
    capture["spans"][10] = _span(10, 9, "sqlite_work", 65536, 30, 40)
    with pytest.raises(ValueError, match="invalid stage or lane"):
        build_index(capture)


def test_aborted_backfill_is_observed_time_without_done_claim() -> None:
    capture = _capture()
    capture["spans"][9] = _span(9, 4, "sqlite_work", 25, 30, 50, "aborted")
    backfill = build_index(capture)["workers"]["3"]["backfill"]
    assert backfill["window_count"] == 1
    assert backfill["window_union_ns"] == 20
    assert backfill["window_outcomes"] == {
        "ok": 0,
        "error": 0,
        "cancelled": 0,
        "aborted": 1,
    }


def test_zero_refusal_and_unassigned_or_foreign_are_not_verified_joins() -> None:
    assert build_index(_capture(), refusal_log="")["refusals"]["status"] == (
        "NOT_APPLICABLE_NO_SAMPLED_REFUSAL"
    )
    assert build_index(_capture())["refusals"]["status"] == "NOT_RUN_NO_REFUSAL_LOG"
    pregrant = {
        **_record(),
        "phase": "before_grant",
        "execution": None,
        "budget_diagnostic": None,
        "outcome": "Err(Cancelled)",
    }
    null_trace = copy.deepcopy(_record())
    null_trace["execution"]["trace"] = None
    foreign = _record(capture_id="cd" * 16)
    raw = "\n".join((_raw(pregrant), _raw(null_trace), _raw(foreign)))
    result = build_index(_capture(), refusal_log=raw)["refusals"]
    assert result["status"] == "UNJOINED_SAMPLED_REFUSAL"
    assert len(result["unassigned"]) == 2
    assert len(result["foreign"]) == 1
    assert result["matched"] == []
    assert build_index(_capture(), refusal_log=_raw(foreign))["refusals"]["status"] == (
        "NOT_APPLICABLE_NO_CURRENT_CAPTURE_REFUSAL"
    )
    partial = build_index(_capture(), refusal_log="\n".join((_raw(_record()), _raw(pregrant))))
    assert partial["refusals"]["status"] == "DIAGNOSTIC_PARTIAL_JOIN"


def test_escaped_libtest_carrier_and_separate_capture_epochs() -> None:
    record = _record()
    escaped = "Error: " + json.dumps("failed; bounded daemon diagnostics:\n" + _raw(record) + "\n")
    assert (
        build_index(_capture(), refusal_log=escaped)["refusals"]["matched"][0]["worker_span_id"]
        == 3
    )
    foreign = _record(capture_id="cd" * 16)
    raw = "\n".join(
        [_raw({**foreign, "sample_ordinal": ordinal}) for ordinal in range(1, 9)] + [_raw(record)]
    )
    result = build_index(_capture(), refusal_log=raw)["refusals"]
    assert result["raw_candidates_seen"] == 9
    assert len(result["matched"]) == 1 and len(result["foreign"]) == 8


@pytest.mark.parametrize("span_id", [1, 5, 99])
def test_refusal_must_reference_exact_worker_child(span_id: int) -> None:
    with pytest.raises(ValueError, match="root or nonworker"):
        build_index(_capture(), refusal_log=_raw(_record(span_id=span_id)))


def test_capture_health_ancestry_and_sqlite_partition_fail_closed() -> None:
    for mutate in (
        lambda capture: capture.update(status="partial"),
        lambda capture: capture["footer"].update(events_dropped=1),
        lambda capture: capture["spans"][5].update(end_ns=90),
        lambda capture: capture["spans"][8]["counters"].update(sqlite_main_write_calls=2),
        lambda capture: capture["spans"][8]["counters"].pop("sqlite_write_requested_bytes"),
    ):
        capture = _capture()
        mutate(capture)
        with pytest.raises(ValueError):
            build_index(capture)


def test_malformed_and_oversized_sampled_record_fail_closed() -> None:
    record = _raw(_record())
    bad = (
        record.replace(
            '"event":"native_owned_work_refusal"',
            '"event":"native_owned_work_refusal","event":"native_owned_work_refusal"',
        ),
        record.replace('"outcome":"Err(ResourceLimited)"', '"outcome":"Err(Cancelled)"'),
        record.replace('"dispatch_ordinal":2', '"dispatch_ordinal":1'),
        record + " " * MAX_RECORD_BYTES,
        "prefix " + record,
    )
    for raw in bad:
        with pytest.raises(ValueError):
            build_index(_capture(), refusal_log=raw)


def test_cached_query_cli_uses_index_without_reparsing_qtrace(
    tmp_path, monkeypatch, capsys
) -> None:
    from tools.benchmark.retrieval import causal_cost_diagnostic as diagnostic

    index_path = tmp_path / "worker-index.json"
    index_path.write_text(json.dumps(build_index(_capture())), encoding="utf-8")
    monkeypatch.setattr(
        diagnostic, "parse_stream", lambda *args, **kwargs: pytest.fail("qtrace reparsed")
    )
    assert main(["query", "--index", str(index_path), "--worker-span-id", "3"]) == 0
    assert json.loads(capsys.readouterr().out)["matched_workers"] == 1


def test_corrupt_cached_index_is_rejected_before_query() -> None:
    for mutate in (
        lambda index: index.update(schema_version=1),
        lambda index: index["capture"].update(stream_sha256="bad"),
        lambda index: index["capture"]["footer"].update(events_dropped=1),
        lambda index: index.update(source_binary_binding="VERIFIED"),
        lambda index: index.update(worker_count=3),
        lambda index: index["workers"]["3"].update(duration_ns=1),
        lambda index: index["workers"]["3"]["backfill"].update(window_count=1),
        lambda index: index["workers"]["3"]["backfill"]["window_outcomes"].update(ok=1),
        lambda index: index["workers"]["3"]["backfill"].update(window_union_ns=1),
        lambda index: index["workers"]["3"]["backfill"].update(commit_overlap_ns=1),
        lambda index: index["workers"]["3"]["backfill"]["descendant_write_counters"].update(
            sqlite_write_calls=1, sqlite_wal_write_calls=1
        ),
        lambda index: index["workers"]["3"]["groups"][0].update(lane_id=65536),
        lambda index: index["workers"]["3"]["groups"][0].update(union_ns=999),
        lambda index: next(row for row in index["workers"]["3"]["groups"] if row["lane_id"] == 22)[
            "direct_counters"
        ].update(sqlite_main_write_calls=2),
        lambda index: index["refusals"]["matched"][0].update(worker_span_id=2),
        lambda index: index["refusals"].update(status="NOT_APPLICABLE_NO_SAMPLED_REFUSAL"),
    ):
        index = build_index(_capture(), refusal_log=_raw(_record()))
        mutate(index)
        with pytest.raises(ValueError):
            query_index(index)


def test_cached_write_callback_timing_rejects_partial_negative_or_mixed_projection() -> None:
    baseline = build_index(_capture_with_callback_timing())
    assert query_index(baseline, worker_span_id=3, lane=22)["matched_workers"] == 1

    def counters(index: dict) -> dict:
        return next(
            row["direct_counters"]
            for row in index["workers"]["3"]["groups"]
            if row["lane_id"] == 22
        )

    for mutate in (
        lambda row: row.pop("sqlite_write_lower_envelope_elapsed_ns"),
        lambda row: row.update(sqlite_write_admission_calls=-1),
        lambda row: row.update(sqlite_write_admission_refusals=3),
        lambda row: row.update(sqlite_write_post_callback_refusals=6),
        # Simulate an old segment's reached call merged without its new counters.
        lambda row: row.update(sqlite_write_calls=6, sqlite_main_write_calls=2),
    ):
        index = copy.deepcopy(baseline)
        mutate(counters(index))
        with pytest.raises(ValueError, match="SQLite callback timing|cached worker counters"):
            query_index(index)


@pytest.mark.parametrize("malformed", ['{"schema_version":1,"schema_version":1}', '{"x":NaN}'])
def test_cached_query_cli_rejects_duplicate_or_nonfinite_json(tmp_path, malformed: str) -> None:
    path = tmp_path / "malformed-index.json"
    path.write_text(malformed, encoding="utf-8")
    with pytest.raises(ValueError):
        main(["query", "--index", str(path)])
