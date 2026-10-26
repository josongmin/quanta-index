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
    assert index["refusals"]["status"] == "DIAGNOSTIC_MATCHED"
    assert index["refusals"]["matched"][0]["execution"]["claim"]["fence"] == 41
    selected = query_index(index, worker_span_id=3, stage="sqlite_work", lane=11)
    assert selected["matched_workers"] == 1
    assert selected["workers"]["3"]["groups"] == [wal]
    assert query_index(index, refusals_only=True)["matched_workers"] == 1


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
        lambda index: index["capture"].update(stream_sha256="bad"),
        lambda index: index["capture"]["footer"].update(events_dropped=1),
        lambda index: index.update(source_binary_binding="VERIFIED"),
        lambda index: index.update(worker_count=3),
        lambda index: index["workers"]["3"].update(duration_ns=1),
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


@pytest.mark.parametrize("malformed", ['{"schema_version":1,"schema_version":1}', '{"x":NaN}'])
def test_cached_query_cli_rejects_duplicate_or_nonfinite_json(tmp_path, malformed: str) -> None:
    path = tmp_path / "malformed-index.json"
    path.write_text(malformed, encoding="utf-8")
    with pytest.raises(ValueError):
        main(["query", "--index", str(path)])
