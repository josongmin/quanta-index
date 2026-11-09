"""Closed canonical capture examples for complete, partial and invalid replay."""

from __future__ import annotations

import copy
import hashlib
import io
import json

import pytest
from jsonschema import Draft202012Validator

from tools.benchmark.retrieval.causal_cost_timeline import (
    MAX_CONFIGURED_CAPTURE_BYTES,
    MAX_LINE_BYTES,
    SCHEMA,
    _dispatch_v5_validators,
    _dispatch_validators,
    _schema_for_version,
    _selected_validator,
    chrome_export,
    parse,
    parse_stream,
    reconcile_legacy,
    summarize,
)

CAPTURE = "0" * 32
SQLITE_SYNC_MUTATION_KEYS = (
    "sqlite_sync_overwrite",
    "sqlite_sync_growth",
    "sqlite_sync_truncate",
    "sqlite_sync_create",
    "sqlite_sync_no_mutation",
    "sqlite_sync_unclassified",
)
SQLITE_WRITE_GOLD = (
    ("sqlite_write_calls", 17),
    ("sqlite_write_requested_bytes", 69_632),
    ("sqlite_write_failures", 2),
    ("sqlite_wal_write_calls", 11),
    ("sqlite_main_write_calls", 5),
    ("sqlite_other_write_calls", 1),
)
THREAD_CPU_GOLD = ("thread_cpu_ns", 7_654_321)
SQLITE_WRITE_BOUNDARY_GOLD = (
    ("sqlite_write_admission_calls", 19),
    ("sqlite_write_admission_refusals", 2),
    ("sqlite_write_admission_elapsed_ns", 2_100),
    ("sqlite_write_lower_envelope_elapsed_ns", 3_400),
    ("sqlite_write_post_callback_elapsed_ns", 900),
    ("sqlite_write_post_callback_refusals", 1),
)
SQLITE_CACHE_GOLD = (
    ("sqlite_cache_hits", 13),
    ("sqlite_cache_misses", 3),
    ("sqlite_cache_writes", 9),
    ("sqlite_cache_spills", 2),
)
SQLITE_WRITER_BEFORE_GOLD = (
    ("sqlite_page_size_bytes", 4096),
    ("sqlite_cache_size_magnitude", 2000),
    ("sqlite_cache_size_is_kib", 1),
    ("sqlite_cache_spill_pages", 2000),
    ("sqlite_wal_autocheckpoint_pages", 1000),
)
SQLITE_WRITER_AFTER_GOLD = (
    ("sqlite_page_size_bytes", 4096),
    ("sqlite_cache_size_magnitude", 8192),
    ("sqlite_cache_size_is_kib", 1),
    ("sqlite_cache_spill_pages", 8192),
    ("sqlite_wal_autocheckpoint_pages", 1000),
)


def _minimal_schema_value(spec: dict, definitions: dict) -> object:
    """Produce a positive fixture from required fields in the canonical schema."""
    if "$ref" in spec:
        return _minimal_schema_value(
            definitions[spec["$ref"].removeprefix("#/$defs/")], definitions
        )
    if "const" in spec:
        return spec["const"]
    if "enum" in spec:
        return spec["enum"][0]
    if "oneOf" in spec:
        return _minimal_schema_value(spec["oneOf"][0], definitions)
    kind = spec.get("type")
    if isinstance(kind, list):
        kind = kind[-1] if "null" in kind else kind[0]
    if kind == "null":
        return None
    if kind == "boolean":
        return False
    if kind == "integer":
        return spec.get("minimum", 0)
    if kind == "string":
        pattern = spec.get("pattern", "")
        if pattern == "^[0-9a-f]{32}$":
            return "0" * 32
        if pattern == "^[0-9a-f]{64}$":
            return "0" * 64
        return "x"
    if kind == "object":
        return {
            key: _minimal_schema_value(spec["properties"][key], definitions)
            for key in spec["required"]
        }
    raise AssertionError(f"unsupported fixture schema: {spec}")


@pytest.mark.parametrize("version,expected_kind_count", [(1, 7), (2, 13), (3, 15), (4, 16)])
def test_selected_schema_matches_original_for_every_kind_and_malformed_row(
    version: int, expected_kind_count: int
) -> None:
    schema = _schema_for_version(json.loads(SCHEMA.read_text()), version)
    original = Draft202012Validator(schema)
    records, kinds = _dispatch_validators(schema)
    assert set(records) == {"header", "event", "footer"}
    assert len(kinds) == expected_kind_count
    assert all(
        "oneOf" not in selected.schema["$defs"]["event"]["properties"]["kind"]
        for selected in kinds.values()
    )
    definitions = schema["$defs"]
    original_rows = [json.loads(line) for line in _capture().splitlines()]
    for row in (original_rows[0], original_rows[-1]):
        row["schema_version"] = version
        assert original.is_valid(row)
        assert _selected_validator(row, original, (records, kinds)).is_valid(row)
        for changed in (
            {**row, "unexpected": 1},
            {key: value for key, value in row.items() if key != "record_type"},
            {**row, "record_type": False},
            {**row, "schema_version": True},
        ):
            assert not original.is_valid(changed)
            assert not _selected_validator(changed, original, (records, kinds)).is_valid(changed)

    positive_rows: dict[str, dict] = {}
    for key in kinds:
        kind_spec = next(
            definitions[reference["$ref"].removeprefix("#/$defs/")]
            for reference in definitions["event"]["properties"]["kind"]["oneOf"]
            if definitions[reference["$ref"].removeprefix("#/$defs/")]["properties"]["type"][
                "const"
            ]
            == key
        )
        kind = _minimal_schema_value(kind_spec, definitions)
        assert isinstance(kind, dict)
        row = {
            "record_type": "event",
            "output_seq": 1,
            "capture_id": CAPTURE,
            "ts_ns": 1,
            "kind": kind,
        }
        assert original.is_valid(row), key
        assert _selected_validator(row, original, (records, kinds)).is_valid(row), key
        positive_rows[key] = row
        required_other = next(field for field in kind_spec["required"] if field != "type")
        malformed = (
            {**row, "unexpected": 1},
            {**row, "record_type": "unknown"},
            {**row, "output_seq": True},
            {**row, "kind": {**kind, "unexpected": 1}},
            {**row, "kind": {field: value for field, value in kind.items() if field != "type"}},
            {**row, "kind": {**kind, "type": False}},
            {**row, "kind": {**kind, "type": "unknown"}},
            {
                **row,
                "kind": {field: value for field, value in kind.items() if field != required_other},
            },
            {**row, "kind": {**kind, required_other: {}}},
        )
        for changed in malformed:
            assert not original.is_valid(changed), (version, key, changed)
            assert not _selected_validator(changed, original, (records, kinds)).is_valid(changed), (
                version,
                key,
                changed,
            )

    first_kind, second_kind = list(kinds)[:2]
    assert original.is_valid(positive_rows[second_kind])
    assert not kinds[first_kind].is_valid(positive_rows[second_kind])

    unknown = {
        "record_type": "event",
        "output_seq": 1,
        "capture_id": CAPTURE,
        "ts_ns": 1,
        "kind": {"type": "unknown"},
    }
    assert _selected_validator(unknown, original, (records, kinds)) is original
    altered = copy.deepcopy(schema)
    altered["allOf"] = [{"type": "object"}]
    assert _dispatch_validators(altered) == ({}, {})


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


def _boundary_rows(rows: list[dict]) -> bytes:
    """Keep the fixed footer accounting while changing only boundary order."""
    count = len(rows) - 2
    for sequence, row in enumerate(rows[1:-1], 1):
        row["output_seq"] = sequence
    rows[-1].update(
        last_output_seq=count,
        events_attempted=count,
        events_accepted=count,
        events_written=count,
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


def _ann_region_rows() -> list[dict]:
    rows = [json.loads(line) for line in _capture().splitlines()]
    rows[0]["schema_version"] = 3
    rows[2]["kind"]["stage"] = "semantic_seal"
    rows[3] = {
        "record_type": "event",
        "capture_id": CAPTURE,
        "ts_ns": 25,
        "kind": {
            "type": "ann_region_observation",
            "span_id": 2,
            "phase": "installed",
            "fileset_ref": "ab" * 32,
            "descriptor_ref": None,
            "ordinal": 7,
            "rows": 256,
            "index_kind": "ivf_hnsw_sq",
            "files": 2,
            "chunks": 3,
        },
    }
    rows[4]["ts_ns"] = 30
    rows.insert(
        5,
        {
            "record_type": "event",
            "capture_id": CAPTURE,
            "ts_ns": 32,
            "kind": {
                "type": "span_begin",
                "span_id": 3,
                "parent_span_id": 1,
                "stage": "query_candidate",
                "lane_id": 0,
            },
        },
    )
    rows.insert(
        6,
        {
            "record_type": "event",
            "capture_id": CAPTURE,
            "ts_ns": 34,
            "kind": {
                **rows[3]["kind"],
                "span_id": 3,
                "phase": "selected_query",
                "descriptor_ref": "cd" * 32,
            },
        },
    )
    rows.insert(
        7,
        {
            "record_type": "event",
            "capture_id": CAPTURE,
            "ts_ns": 36,
            "kind": {"type": "span_end", "span_id": 3, "outcome": "ok"},
        },
    )
    rows[8]["ts_ns"] = 40
    rows[-1]["schema_version"] = 3
    rows[-1]["closed_ns"] = 50
    _resequence_ann_region_rows(rows)
    return rows


def _resequence_ann_region_rows(rows: list[dict]) -> None:
    for sequence, event in enumerate(rows[1:-1], 1):
        event["output_seq"] = sequence
    count = len(rows) - 2
    rows[-1].update(
        last_output_seq=count,
        events_attempted=count,
        events_accepted=count,
        events_written=count,
    )


def _mark_partial_footer(rows: list[dict], *, dropped: int) -> None:
    _resequence_ann_region_rows(rows)
    rows[-1]["status"] = "partial"
    rows[-1]["events_dropped"] = dropped
    rows[-1]["events_attempted"] = rows[-1]["events_accepted"] + dropped


def _sqlite_write_rows() -> list[dict]:
    rows = [json.loads(line) for line in _capture().splitlines()]
    rows[0]["schema_version"] = 4
    rows[-1]["schema_version"] = 4
    rows[2]["kind"].update(stage="sqlite_work", lane_id=11)
    rows[3]["kind"].update(key=SQLITE_WRITE_GOLD[0][0], value=SQLITE_WRITE_GOLD[0][1])
    for offset, (key, value) in enumerate(SQLITE_WRITE_GOLD[1:], 1):
        rows.insert(
            3 + offset,
            {
                "record_type": "event",
                "capture_id": CAPTURE,
                "ts_ns": 25 + offset,
                "kind": {"type": "counter", "span_id": 2, "key": key, "value": value},
            },
        )
    _resequence_ann_region_rows(rows)
    return rows


def _sqlite_write_boundary_rows(gold: tuple[tuple[str, int], ...]) -> list[dict]:
    rows = [json.loads(line) for line in _capture().splitlines()]
    rows[0]["schema_version"] = 4
    rows[-1]["schema_version"] = 4
    rows[2]["kind"].update(stage="sqlite_work", lane_id=22)
    rows[3]["kind"].update(key=gold[0][0], value=gold[0][1])
    for offset, (key, value) in enumerate(gold[1:], 1):
        rows.insert(
            3 + offset,
            {
                "record_type": "event",
                "capture_id": CAPTURE,
                "ts_ns": 25 + offset,
                "kind": {"type": "counter", "span_id": 2, "key": key, "value": value},
            },
        )
    _resequence_ann_region_rows(rows)
    return rows


def _sqlite_cache_rows(gold: tuple[tuple[str, int], ...] = SQLITE_CACHE_GOLD) -> list[dict]:
    rows = [json.loads(line) for line in _capture().splitlines()]
    rows[0]["schema_version"] = 4
    rows[-1]["schema_version"] = 4
    rows[2]["kind"].update(parent_span_id=3, stage="catalog_lock_hold", lane_id=0)
    rows[3]["kind"].update(key=gold[0][0], value=gold[0][1])
    for offset, (key, value) in enumerate(gold[1:], 1):
        rows.insert(
            3 + offset,
            {
                "record_type": "event",
                "capture_id": CAPTURE,
                "ts_ns": 25 + offset,
                "kind": {"type": "counter", "span_id": 2, "key": key, "value": value},
            },
        )
    rows.insert(
        2,
        {
            "record_type": "event",
            "capture_id": CAPTURE,
            "ts_ns": 15,
            "kind": {
                "type": "span_begin",
                "span_id": 3,
                "parent_span_id": 1,
                "stage": "operation",
                "lane_id": 1,
            },
        },
    )
    rows.insert(
        -2,
        {
            "record_type": "event",
            "capture_id": CAPTURE,
            "ts_ns": 38,
            "kind": {"type": "span_end", "span_id": 3, "outcome": "ok"},
        },
    )
    _resequence_ann_region_rows(rows)
    return rows


def _sqlite_writer_settings_rows() -> list[dict]:
    rows = [json.loads(line) for line in _capture().splitlines()]
    rows[0]["schema_version"] = 4
    rows[-1]["schema_version"] = 4
    rows[2]["kind"].update(stage="sqlite_work", lane_id=27)
    rows[3]["kind"].update(
        key=SQLITE_WRITER_BEFORE_GOLD[0][0], value=SQLITE_WRITER_BEFORE_GOLD[0][1]
    )
    for key, value in SQLITE_WRITER_BEFORE_GOLD[1:]:
        rows.insert(
            -3,
            {
                "record_type": "event",
                "capture_id": CAPTURE,
                "ts_ns": 26,
                "kind": {"type": "counter", "span_id": 2, "key": key, "value": value},
            },
        )
    rows.insert(
        -2,
        {
            "record_type": "event",
            "capture_id": CAPTURE,
            "ts_ns": 36,
            "kind": {
                "type": "span_begin",
                "span_id": 3,
                "parent_span_id": 1,
                "stage": "sqlite_work",
                "lane_id": 28,
            },
        },
    )
    for key, value in SQLITE_WRITER_AFTER_GOLD:
        rows.insert(
            -2,
            {
                "record_type": "event",
                "capture_id": CAPTURE,
                "ts_ns": 37,
                "kind": {"type": "counter", "span_id": 3, "key": key, "value": value},
            },
        )
    rows.insert(
        -2,
        {
            "record_type": "event",
            "capture_id": CAPTURE,
            "ts_ns": 38,
            "kind": {"type": "span_end", "span_id": 3, "outcome": "ok"},
        },
    )
    _resequence_ann_region_rows(rows)
    return rows


def _owned_renewal_rows() -> list[dict]:
    rows = [json.loads(line) for line in _capture().splitlines()]
    rows[0]["schema_version"] = 4
    rows[-1]["schema_version"] = 4
    rows[2]["kind"].update(stage="sqlite_work", lane_id=29)
    rows.pop(3)  # No unrelated counter in the renewal owner.
    rows.insert(
        3,
        {
            "record_type": "event",
            "capture_id": CAPTURE,
            "ts_ns": 25,
            "kind": {
                "type": "owned_renewal_refusal",
                "span_id": 2,
                "reason": "lease_expired",
                "now_ms": 500,
                "lease_until_ms": 500,
                "deadline_ms": None,
            },
        },
    )
    rows[4]["kind"]["outcome"] = "error"
    return rows


def test_v4_owned_renewal_refusal_preserves_exact_point_and_error_owner() -> None:
    capture = parse(_boundary_rows(_owned_renewal_rows()))
    expected = {
        "span_id": 2,
        "ts_ns": 25,
        "reason": "lease_expired",
        "now_ms": 500,
        "lease_until_ms": 500,
        "deadline_ms": None,
    }
    assert capture["status"] == "complete"
    assert capture["owned_renewal_refusals"] == [expected]
    assert summarize(capture)["owned_renewal_refusals"] == [expected]
    instant = [
        event
        for event in chrome_export(capture)["traceEvents"]
        if event.get("cat") == "owned_renewal_refusal"
    ]
    assert len(instant) == 1
    assert instant[0]["name"] == "owned_renewal_refusal:lease_expired"
    assert instant[0]["ph"] == "i"
    assert instant[0]["args"] == {
        "span_id": "2",
        "reason": "lease_expired",
        "now_ms": "500",
        "lease_until_ms": "500",
        "deadline_ms": None,
    }


@pytest.mark.parametrize(
    "reason",
    [
        "missing_claim",
        "not_claimed",
        "claim_identity",
        "claim_fence",
        "lease_expired",
        "missing_assignment",
        "inactive_assignment",
        "undispatched_assignment",
        "execution_expired",
        "assignment_claim",
        "grant_mismatch",
    ],
)
def test_owned_renewal_refusal_accepts_only_closed_reason_set(reason: str) -> None:
    rows = _owned_renewal_rows()
    rows[3]["kind"].update(reason=reason, deadline_ms=500)
    assert parse(_boundary_rows(rows))["owned_renewal_refusals"][0]["reason"] == reason


@pytest.mark.parametrize("version", [1, 2, 3])
def test_legacy_capture_rejects_owned_renewal_refusal(version: int) -> None:
    rows = _owned_renewal_rows()
    rows[0]["schema_version"] = version
    rows[-1]["schema_version"] = version
    with pytest.raises(ValueError, match="invalid causal timeline row"):
        parse(_boundary_rows(rows))


@pytest.mark.parametrize(
    "field,value",
    [
        ("reason", "unknown"),
        ("now_ms", -1),
        ("now_ms", 1 << 64),
        ("lease_until_ms", -1),
        ("lease_until_ms", 1 << 64),
        ("deadline_ms", -1),
        ("deadline_ms", 1 << 64),
    ],
)
def test_owned_renewal_refusal_rejects_malformed_fields(field: str, value: object) -> None:
    rows = _owned_renewal_rows()
    rows[3]["kind"][field] = value
    with pytest.raises(ValueError, match="invalid causal timeline row"):
        parse(_boundary_rows(rows))


def test_owned_renewal_refusal_requires_all_payload_fields() -> None:
    rows = _owned_renewal_rows()
    del rows[3]["kind"]["deadline_ms"]
    with pytest.raises(ValueError, match="invalid causal timeline row"):
        parse(_boundary_rows(rows))


@pytest.mark.parametrize(
    "reason,lease_until_ms,deadline_ms,expected",
    [
        ("lease_expired", None, None, "lease expiry"),
        ("lease_expired", 501, None, "lease expiry"),
        ("execution_expired", None, None, "execution expiry"),
        ("execution_expired", None, 501, "execution expiry"),
    ],
)
def test_owned_renewal_refusal_rejects_unexpired_boundary(
    reason: str, lease_until_ms: int | None, deadline_ms: int | None, expected: str
) -> None:
    rows = _owned_renewal_rows()
    rows[3]["kind"].update(
        reason=reason, lease_until_ms=lease_until_ms, deadline_ms=deadline_ms
    )
    with pytest.raises(ValueError, match=expected):
        parse(_boundary_rows(rows))


@pytest.mark.parametrize(
    "failure",
    ["foreign_owner", "wrong_lane", "wrong_stage", "duplicate", "outside", "final_ok"],
)
def test_owned_renewal_refusal_rejects_invalid_owner_contract(failure: str) -> None:
    rows = _owned_renewal_rows()
    if failure == "foreign_owner":
        rows[3]["kind"]["span_id"] = 99
    elif failure == "wrong_lane":
        rows[2]["kind"]["lane_id"] = 30
    elif failure == "wrong_stage":
        rows[2]["kind"]["stage"] = "operation"
    elif failure == "duplicate":
        rows.insert(4, copy.deepcopy(rows[3]))
    elif failure == "outside":
        rows[3]["ts_ns"] = 36
    else:
        rows[4]["kind"]["outcome"] = "ok"
    with pytest.raises(ValueError):
        parse(_boundary_rows(rows))


def _sqlite_sync_rows() -> list[dict]:
    rows = [json.loads(line) for line in _capture().splitlines()]
    rows[0]["schema_version"] = 3
    rows[2]["kind"].update(stage="sqlite_work", lane_id=11)
    rows.insert(
        4,
        {
            "record_type": "event",
            "capture_id": CAPTURE,
            "ts_ns": 30,
            "kind": {
                "type": "sqlite_sync_observation",
                "span_id": 2,
                "observation": {
                    "observed": True,
                    "same_native_fd": True,
                    "ambiguous": False,
                    "full_requested": True,
                    "full_default_syscall": True,
                    "full_result": 0,
                    "full_errno": 0,
                    "posix_result": None,
                    "posix_errno": None,
                },
            },
        },
    )
    rows[-1]["schema_version"] = 3
    _resequence_ann_region_rows(rows)
    return rows


def _sqlite_sync_mutation_rows(key: str) -> list[dict]:
    rows = [json.loads(line) for line in _capture().splitlines()]
    rows[0]["schema_version"] = 4
    rows[-1]["schema_version"] = 4
    rows[2]["kind"].update(stage="sqlite_work", lane_id=11)
    rows[3]["kind"].update(key=key, value=1)
    return rows


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


def test_ann_region_identity_is_bound_to_seal_and_query_spans() -> None:
    capture = parse(b"".join(map(_line, _ann_region_rows())))
    assert capture["schema_version"] == 3
    observations = capture["ann_region_observations"]
    assert [(row["phase"], row["span_id"], row["descriptor_ref"]) for row in observations] == [
        ("installed", 2, None),
        ("selected_query", 3, "cd" * 32),
    ]
    summary = summarize(capture)
    assert summary["ann_region_observations"] == observations
    assert summary["spans"][1]["counters"] == {}
    viewer = chrome_export(capture)
    instants = [row for row in viewer["traceEvents"] if row["cat"] == "ann_region_identity"]
    assert [(row["name"], row["ph"]) for row in instants] == [
        ("ann_region:installed", "i"),
        ("ann_region:selected_query", "i"),
    ]
    assert instants[0]["args"]["fileset_ref"] == "ab" * 32
    assert instants[1]["args"]["ordinal"] == "7"
    assert viewer["quantaIndexSummary"]["ann_region_observations"][1]["rows"] == "256"


@pytest.mark.parametrize(
    ("field", "value"),
    (
        ("phase", "unknown"),
        ("fileset_ref", "A" * 64),
        ("descriptor_ref", "cd" * 32),
        ("ordinal", 1 << 64),
        ("rows", 0),
        ("rows", True),
        ("rows", 8193),
        ("index_kind", "untrained"),
        ("files", 257),
        ("chunks", 289),
    ),
)
def test_ann_region_identity_rejects_invalid_closed_fields(field: str, value: object) -> None:
    rows = _ann_region_rows()
    rows[3]["kind"][field] = value
    with pytest.raises(ValueError, match="invalid causal timeline row"):
        parse(b"".join(map(_line, rows)))


def test_ann_region_selected_query_requires_descriptor_reference() -> None:
    rows = _ann_region_rows()
    rows[6]["kind"]["descriptor_ref"] = None
    with pytest.raises(ValueError, match="invalid causal timeline row"):
        parse(b"".join(map(_line, rows)))


def test_ann_region_identity_rejects_wrong_stage_interval_and_immutability() -> None:
    for mutation, reason in (
        (lambda rows: rows[2]["kind"].update(stage="query_candidate"), "wrong owner stage"),
        (lambda rows: rows[3].update(ts_ns=31), "outside owner interval"),
        (lambda rows: rows[6]["kind"].update(rows=255), "observations disagree"),
        (lambda rows: rows[6]["kind"].update(ordinal=8), "observations disagree"),
        (lambda rows: rows[3]["kind"].update(chunks=1), "fewer chunks than files"),
        (lambda rows: rows[4]["kind"].update(outcome="error"), "failed owner"),
    ):
        rows = _ann_region_rows()
        mutation(rows)
        with pytest.raises(ValueError, match=reason):
            parse(b"".join(map(_line, rows)))


def test_ann_region_identity_rejects_duplicate_or_orphan() -> None:
    rows = _ann_region_rows()
    rows.insert(4, {**copy.deepcopy(rows[3]), "ts_ns": 26})
    _resequence_ann_region_rows(rows)
    with pytest.raises(ValueError, match="duplicate ANN region observation"):
        parse(b"".join(map(_line, rows)))
    rows = _ann_region_rows()
    extra = copy.deepcopy(rows[3])
    extra["ts_ns"] = 26
    extra["kind"]["fileset_ref"] = "ef" * 32
    rows.insert(4, extra)
    _resequence_ann_region_rows(rows)
    with pytest.raises(ValueError, match="multiple ANN region observations in one owner span"):
        parse(b"".join(map(_line, rows)))
    rows = _ann_region_rows()
    rows[6]["kind"]["span_id"] = 99
    with pytest.raises(ValueError, match="orphan work events"):
        parse(b"".join(map(_line, rows)))


@pytest.mark.parametrize("legacy_version", (1, 2))
def test_legacy_timeline_rejects_ann_region_records(legacy_version: int) -> None:
    rows = _ann_region_rows()
    rows[0]["schema_version"] = legacy_version
    rows[-1]["schema_version"] = legacy_version
    with pytest.raises(ValueError, match="invalid causal timeline row"):
        parse(b"".join(map(_line, rows)))


@pytest.mark.parametrize("version", (3, 4))
def test_sqlite_sync_observation_is_bound_once_without_changing_span_timing(version: int) -> None:
    rows = _sqlite_sync_rows()
    rows[0]["schema_version"] = version
    rows[-1]["schema_version"] = version
    capture = parse(b"".join(map(_line, rows)))
    observation = capture["sqlite_sync_observations"][0]
    assert observation["observation"]["full_result"] == 0
    assert capture["spans"][2]["sqlite_sync_observation"] == observation["observation"]
    summary = summarize(capture)
    owner = next(row for row in summary["spans"] if row["span_id"] == 2)
    assert (owner["duration_ns"], owner["exclusive_ns"]) == (15, 15)
    assert owner["sqlite_sync_observation"] == observation["observation"]
    viewer = chrome_export(capture)
    instants = [row for row in viewer["traceEvents"] if row["cat"] == "sqlite_sync"]
    assert len(instants) == 1
    assert instants[0]["args"]["full_result"] == 0


def test_v4_sqlite_write_counters_preserve_fixed_values() -> None:
    rows = _sqlite_write_rows()
    captured = parse(b"".join(map(_line, rows)))
    assert captured["schema_version"] == 4
    assert captured["status"] == "complete"
    assert [(counter["key"], counter["value"]) for counter in captured["counters"]] == list(
        SQLITE_WRITE_GOLD
    )
    owner = next(row for row in summarize(captured)["spans"] if row["span_id"] == 2)
    assert owner["counters"] == {
        "sqlite_write_calls": 17,
        "sqlite_write_requested_bytes": 69_632,
        "sqlite_write_failures": 2,
        "sqlite_wal_write_calls": 11,
        "sqlite_main_write_calls": 5,
        "sqlite_other_write_calls": 1,
    }
    viewer_counters = [
        event for event in chrome_export(captured)["traceEvents"] if event["ph"] == "C"
    ]
    assert [(event["name"], event["args"][event["name"]]) for event in viewer_counters] == list(
        SQLITE_WRITE_GOLD
    )


@pytest.mark.parametrize("version", (1, 2, 3))
@pytest.mark.parametrize("key", tuple(entry[0] for entry in SQLITE_WRITE_GOLD))
def test_legacy_timeline_rejects_v4_sqlite_write_counter(key: str, version: int) -> None:
    rows = _sqlite_write_rows()
    rows[0]["schema_version"] = version
    rows[-1]["schema_version"] = version
    rows[3]["kind"]["key"] = key
    del rows[4:9]
    _resequence_ann_region_rows(rows)
    with pytest.raises(ValueError, match="invalid causal timeline row"):
        parse(b"".join(map(_line, rows)))


def test_v4_timeline_without_sqlite_write_counters_remains_complete() -> None:
    rows = [json.loads(line) for line in _capture().splitlines()]
    rows[0]["schema_version"] = 4
    rows[-1]["schema_version"] = 4
    captured = parse(b"".join(map(_line, rows)))
    assert captured["status"] == "complete"
    assert captured["counters"] == [{"span_id": 2, "key": "candidates", "value": 3, "ts_ns": 25}]


@pytest.mark.parametrize("stage", ("native_source_apply", "native_work_seal", "compaction"))
def test_v4_thread_cpu_counter_replays_as_one_exact_span_and_perfetto_counter(stage: str) -> None:
    rows = [json.loads(line) for line in _capture().splitlines()]
    rows[0]["schema_version"] = 4
    rows[-1]["schema_version"] = 4
    rows[2]["kind"]["stage"] = stage
    rows[3]["kind"].update(key=THREAD_CPU_GOLD[0], value=THREAD_CPU_GOLD[1])
    captured = parse(b"".join(map(_line, rows)))
    assert captured["status"] == "complete"
    assert captured["counters"] == [
        {"span_id": 2, "key": THREAD_CPU_GOLD[0], "value": THREAD_CPU_GOLD[1], "ts_ns": 25}
    ]
    owner = next(row for row in summarize(captured)["spans"] if row["span_id"] == 2)
    assert owner["counters"] == dict([THREAD_CPU_GOLD])
    viewer = [event for event in chrome_export(captured)["traceEvents"] if event["ph"] == "C"]
    assert [(event["name"], event["args"][event["name"]]) for event in viewer] == [
        THREAD_CPU_GOLD
    ]


def test_v4_thread_cpu_counter_rejects_second_interval_on_same_span() -> None:
    rows = [json.loads(line) for line in _capture().splitlines()]
    rows[0]["schema_version"] = 4
    rows[-1]["schema_version"] = 4
    rows[2]["kind"]["stage"] = "native_source_apply"
    rows[3]["kind"].update(key=THREAD_CPU_GOLD[0], value=THREAD_CPU_GOLD[1])
    rows.insert(
        4,
        {
            "record_type": "event",
            "capture_id": CAPTURE,
            "ts_ns": 26,
            "kind": {"type": "counter", "span_id": 2, "key": THREAD_CPU_GOLD[0], "value": 1},
        },
    )
    _resequence_ann_region_rows(rows)
    with pytest.raises(ValueError, match="thread CPU counter is duplicated"):
        parse(b"".join(map(_line, rows)))


@pytest.mark.parametrize(
    "stage", ("build_prepare", "semantic_prepare", "query_candidate", "sqlite_work")
)
def test_v4_thread_cpu_counter_rejects_unmeasured_stage(stage: str) -> None:
    rows = [json.loads(line) for line in _capture().splitlines()]
    rows[0]["schema_version"] = 4
    rows[-1]["schema_version"] = 4
    rows[2]["kind"]["stage"] = stage
    rows[3]["kind"].update(key=THREAD_CPU_GOLD[0], value=THREAD_CPU_GOLD[1])
    with pytest.raises(ValueError, match="thread CPU counter targets wrong stage"):
        parse(b"".join(map(_line, rows)))


def test_v4_thread_cpu_counter_rejects_orphan_even_in_partial_capture() -> None:
    rows = [json.loads(line) for line in _capture().splitlines()]
    rows[0]["schema_version"] = 4
    rows[-1]["schema_version"] = 4
    rows[3]["kind"].update(span_id=99, key=THREAD_CPU_GOLD[0], value=THREAD_CPU_GOLD[1])
    rows[-1]["status"] = "partial"
    with pytest.raises(ValueError, match="thread CPU counter has no existing owner span"):
        parse(b"".join(map(_line, rows)))


def test_v4_absent_thread_cpu_counter_is_not_synthesized_as_zero() -> None:
    rows = [json.loads(line) for line in _capture().splitlines()]
    rows[0]["schema_version"] = 4
    rows[-1]["schema_version"] = 4
    captured = parse(b"".join(map(_line, rows)))
    assert all(counter["key"] != THREAD_CPU_GOLD[0] for counter in captured["counters"])
    assert all(THREAD_CPU_GOLD[0] not in span["counters"] for span in summarize(captured)["spans"])
    assert all(
        event.get("name") != THREAD_CPU_GOLD[0]
        for event in chrome_export(captured)["traceEvents"]
    )


@pytest.mark.parametrize("version", (1, 2, 3))
def test_legacy_timeline_rejects_thread_cpu_counter(version: int) -> None:
    rows = [json.loads(line) for line in _capture().splitlines()]
    rows[0]["schema_version"] = version
    rows[-1]["schema_version"] = version
    rows[3]["kind"].update(key=THREAD_CPU_GOLD[0], value=THREAD_CPU_GOLD[1])
    with pytest.raises(ValueError, match="invalid causal timeline row"):
        parse(b"".join(map(_line, rows)))


def test_v4_sqlite_write_boundary_counters_keep_exact_values_and_viewer_names() -> None:
    captured = parse(b"".join(map(_line, _sqlite_write_boundary_rows(SQLITE_WRITE_BOUNDARY_GOLD))))
    assert captured["status"] == "complete"
    assert [(row["key"], row["value"]) for row in captured["counters"]] == list(
        SQLITE_WRITE_BOUNDARY_GOLD
    )
    owner = next(row for row in summarize(captured)["spans"] if row["span_id"] == 2)
    assert owner["counters"] == dict(SQLITE_WRITE_BOUNDARY_GOLD)
    viewer = [event for event in chrome_export(captured)["traceEvents"] if event["ph"] == "C"]
    assert [(event["name"], event["args"][event["name"]]) for event in viewer] == list(
        SQLITE_WRITE_BOUNDARY_GOLD
    )


def test_v4_sqlite_write_boundary_capture_reaches_worker_index() -> None:
    from tools.benchmark.retrieval.causal_cost_diagnostic import build_index, query_index

    gold = SQLITE_WRITE_GOLD + SQLITE_WRITE_BOUNDARY_GOLD
    rows = _sqlite_write_boundary_rows(gold)
    rows[2]["kind"]["parent_span_id"] = 3
    rows.insert(
        2,
        {
            "record_type": "event",
            "capture_id": CAPTURE,
            "ts_ns": 15,
            "kind": {
                "type": "span_begin",
                "span_id": 3,
                "parent_span_id": 1,
                "stage": "operation",
                "lane_id": 1,
            },
        },
    )
    write_end = next(
        row
        for row in rows
        if row.get("kind", {}).get("type") == "span_end" and row["kind"]["span_id"] == 2
    )
    write_end["ts_ns"] = 45
    operation_end_index = next(
        index
        for index, row in enumerate(rows)
        if row.get("kind", {}).get("type") == "span_end" and row["kind"]["span_id"] == 1
    )
    rows[operation_end_index]["ts_ns"] = 50
    rows.insert(
        operation_end_index,
        {
            "record_type": "event",
            "capture_id": CAPTURE,
            "ts_ns": 48,
            "kind": {"type": "span_end", "span_id": 3, "outcome": "ok"},
        },
    )
    rows[-1]["closed_ns"] = 60
    _resequence_ann_region_rows(rows)

    capture = parse(b"".join(map(_line, rows)))
    assert capture["status"] == "complete"
    index = build_index(capture)
    selected = query_index(index, worker_span_id=3, stage="sqlite_work", lane=22)
    assert selected["matched_workers"] == 1
    assert selected["workers"]["3"]["groups"][0]["direct_counters"] == dict(gold)


@pytest.mark.parametrize("version", (1, 2, 3))
@pytest.mark.parametrize("key", tuple(key for key, _ in SQLITE_WRITE_BOUNDARY_GOLD))
def test_legacy_timeline_rejects_write_boundary_counter(key: str, version: int) -> None:
    rows = _sqlite_write_boundary_rows(((key, 1),))
    rows[0]["schema_version"] = version
    rows[-1]["schema_version"] = version
    with pytest.raises(ValueError, match="invalid causal timeline row"):
        parse(b"".join(map(_line, rows)))


def test_v4_without_write_boundary_counters_does_not_invent_zeroes() -> None:
    rows = [json.loads(line) for line in _capture().splitlines()]
    rows[0]["schema_version"] = 4
    rows[-1]["schema_version"] = 4
    captured = parse(b"".join(map(_line, rows)))
    assert all(key not in captured["spans"][2]["counters"] for key, _ in SQLITE_WRITE_BOUNDARY_GOLD)


def test_v4_sqlite_cache_deltas_reach_worker_index_with_fixed_gold() -> None:
    from tools.benchmark.retrieval.causal_cost_diagnostic import build_index, query_index

    captured = parse(b"".join(map(_line, _sqlite_cache_rows())))
    assert captured["status"] == "complete"
    assert [(row["key"], row["value"]) for row in captured["counters"]] == list(SQLITE_CACHE_GOLD)
    indexed = build_index(captured)
    assert indexed["sqlite_cache_validity"] == "PAGER_WRITE_OUTCOME_NOT_OBSERVED"
    selected = query_index(indexed, worker_span_id=3, stage="catalog_lock_hold", lane=0)
    assert selected["matched_workers"] == 1
    assert selected["sqlite_cache_validity"] == "PAGER_WRITE_OUTCOME_NOT_OBSERVED"
    assert selected["workers"]["3"]["groups"][0]["direct_counters"] == dict(SQLITE_CACHE_GOLD)
    viewer = [event for event in chrome_export(captured)["traceEvents"] if event["ph"] == "C"]
    assert [(event["name"], event["args"][event["name"]]) for event in viewer] == list(
        SQLITE_CACHE_GOLD
    )


@pytest.mark.parametrize("version", (1, 2, 3))
@pytest.mark.parametrize("key", tuple(key for key, _ in SQLITE_CACHE_GOLD))
def test_legacy_timeline_rejects_v4_sqlite_cache_counter(key: str, version: int) -> None:
    rows = _sqlite_cache_rows(((key, 1),))
    rows[0]["schema_version"] = version
    rows[-1]["schema_version"] = version
    with pytest.raises(ValueError, match="invalid causal timeline row"):
        parse(b"".join(map(_line, rows)))


def test_sqlite_cache_counter_requires_complete_catalog_hold_set() -> None:
    for mutate in (
        lambda rows: rows.pop(4),
        lambda rows: rows[3]["kind"].update(lane_id=1),
        lambda rows: rows[5]["kind"].update(value=-1),
    ):
        rows = _sqlite_cache_rows()
        mutate(rows)
        _resequence_ann_region_rows(rows)
        with pytest.raises(ValueError, match="SQLite cache|invalid causal timeline row"):
            parse(b"".join(map(_line, rows)))


def test_old_v4_catalog_hold_has_unobserved_cache_not_zeroes() -> None:
    from tools.benchmark.retrieval.causal_cost_diagnostic import build_index

    rows = _sqlite_cache_rows()
    rows = [
        row
        for row in rows
        if not (
            row.get("kind", {}).get("type") == "counter"
            and row["kind"]["key"] in dict(SQLITE_CACHE_GOLD)
        )
    ]
    _resequence_ann_region_rows(rows)
    captured = parse(b"".join(map(_line, rows)))
    assert not captured["spans"][2]["counters"]
    indexed = build_index(captured)
    assert indexed["sqlite_cache_validity"] == "NOT_OBSERVED"
    assert not indexed["workers"]["3"]["groups"][0]["direct_counters"]


def test_v4_catalog_writer_settings_keep_signed_unit_and_outside_worker_scope() -> None:
    from tools.benchmark.retrieval.causal_cost_diagnostic import build_index

    captured = parse(b"".join(map(_line, _sqlite_writer_settings_rows())))
    assert captured["status"] == "complete"
    assert captured["spans"][2]["counters"] == dict(SQLITE_WRITER_BEFORE_GOLD)
    assert captured["spans"][3]["counters"] == dict(SQLITE_WRITER_AFTER_GOLD)
    assert captured["spans"][2]["parent_span_id"] == captured["spans"][3]["parent_span_id"] == 1
    indexed = build_index(captured)
    assert indexed["worker_count"] == 0
    assert indexed["sqlite_cache_validity"] == "NOT_OBSERVED"

    positive_pages = _sqlite_writer_settings_rows()
    for row in positive_pages:
        kind = row.get("kind", {})
        if kind.get("type") == "counter" and kind["span_id"] == 3:
            if kind["key"] == "sqlite_cache_size_magnitude":
                kind["value"] = 8192
            elif kind["key"] == "sqlite_cache_size_is_kib":
                kind["value"] = 0
    pages_capture = parse(b"".join(map(_line, positive_pages)))
    assert pages_capture["spans"][3]["counters"]["sqlite_cache_size_magnitude"] == 8192
    assert pages_capture["spans"][3]["counters"]["sqlite_cache_size_is_kib"] == 0


@pytest.mark.parametrize("version", (1, 2, 3))
def test_legacy_timeline_rejects_v4_catalog_writer_settings(version: int) -> None:
    rows = _sqlite_writer_settings_rows()
    rows[0]["schema_version"] = version
    rows[-1]["schema_version"] = version
    with pytest.raises(ValueError, match="invalid causal timeline row"):
        parse(b"".join(map(_line, rows)))


def test_catalog_writer_settings_refuse_partial_duplicate_wrong_owner_and_unit() -> None:
    def remove_before_counter(rows: list[dict]) -> None:
        rows[:] = [
            row
            for row in rows
            if not (
                row.get("kind", {}).get("type") == "counter"
                and row["kind"]["span_id"] == 2
                and row["kind"]["key"] == "sqlite_cache_spill_pages"
            )
        ]

    def duplicate_before_counter(rows: list[dict]) -> None:
        duplicate = next(
            row
            for row in rows
            if row.get("kind", {}).get("type") == "counter" and row["kind"]["span_id"] == 2
        )
        rows.insert(-3, copy.deepcopy(duplicate))

    def remove_after(rows: list[dict]) -> None:
        rows[:] = [row for row in rows if row.get("kind", {}).get("span_id") != 3]

    def wrong_unit(rows: list[dict]) -> None:
        next(
            row
            for row in rows
            if row.get("kind", {}).get("key") == "sqlite_cache_size_is_kib"
            and row["kind"]["span_id"] == 3
        )["kind"]["value"] = 2

    for mutate in (
        remove_before_counter,
        duplicate_before_counter,
        remove_after,
        wrong_unit,
        lambda rows: rows[2]["kind"].update(parent_span_id=99),
        lambda rows: rows[2]["kind"].update(lane_id=29),
    ):
        rows = _sqlite_writer_settings_rows()
        mutate(rows)
        _resequence_ann_region_rows(rows)
        with pytest.raises(ValueError, match="SQLite writer settings|missing parent"):
            parse(b"".join(map(_line, rows)))


@pytest.mark.parametrize(
    ("missing_span", "missing_key"),
    ((2, "sqlite_cache_spill_pages"), (3, "sqlite_wal_autocheckpoint_pages")),
)
def test_partial_writer_settings_preserve_missing_counter(
    missing_span: int, missing_key: str
) -> None:
    rows = _sqlite_writer_settings_rows()
    rows = [
        row
        for row in rows
        if not (
            row.get("kind", {}).get("type") == "counter"
            and row["kind"]["span_id"] == missing_span
            and row["kind"]["key"] == missing_key
        )
    ]
    _mark_partial_footer(rows, dropped=1)
    captured = parse(b"".join(map(_line, rows)))
    assert captured["status"] == "partial"
    assert missing_key not in captured["spans"][missing_span]["counters"]
    assert captured["footer"]["events_dropped"] == 1


def test_partial_writer_settings_preserve_missing_after_pair() -> None:
    rows = _sqlite_writer_settings_rows()
    rows = [row for row in rows if row.get("kind", {}).get("span_id") != 3]
    _mark_partial_footer(rows, dropped=7)
    missing_pair = parse(b"".join(map(_line, rows)))
    assert missing_pair["status"] == "partial"
    assert 2 in missing_pair["spans"] and 3 not in missing_pair["spans"]


def test_partial_writer_settings_preserve_truncated_prefix() -> None:
    prefix = _sqlite_writer_settings_rows()[:4]
    truncated = parse(b"".join(map(_line, prefix)))
    assert truncated["status"] == "partial"
    assert truncated["footer"] is None
    assert truncated["spans"][2]["counters"] == {"sqlite_page_size_bytes": 4096}


def test_partial_pager_delta_preserves_observed_prefix_and_wrong_owner_still_refuses() -> None:
    rows = _sqlite_cache_rows()
    rows = [
        row
        for row in rows
        if not (
            row.get("kind", {}).get("type") == "counter"
            and row["kind"]["key"] == "sqlite_cache_spills"
        )
    ]
    _mark_partial_footer(rows, dropped=1)
    captured = parse(b"".join(map(_line, rows)))
    assert captured["status"] == "partial"
    assert captured["spans"][2]["counters"] == dict(SQLITE_CACHE_GOLD[:-1])

    rows[3]["kind"]["lane_id"] = 1
    with pytest.raises(ValueError, match="SQLite cache counters target wrong owner"):
        parse(b"".join(map(_line, rows)))


def test_partial_catalog_writer_settings_health_stays_partial() -> None:
    rows = _sqlite_writer_settings_rows()
    rows = [
        row
        for row in rows
        if not (
            row.get("kind", {}).get("type") == "counter"
            and row["kind"]["span_id"] == 3
            and row["kind"]["key"] == "sqlite_cache_spill_pages"
        )
    ]
    rows.insert(
        -2,
        {
            "record_type": "event",
            "capture_id": CAPTURE,
            "ts_ns": 39,
            "kind": {"type": "health", "code": "counter_unavailable"},
        },
    )
    _mark_partial_footer(rows, dropped=1)
    captured = parse(b"".join(map(_line, rows)))
    assert captured["status"] == "partial"
    assert [row["code"] for row in captured["health"]] == ["counter_unavailable"]


@pytest.mark.parametrize(
    ("key", "lane"),
    [(key, 11 + ordinal % 3) for ordinal, key in enumerate(SQLITE_SYNC_MUTATION_KEYS)],
)
def test_v4_sqlite_sync_mutation_class_roundtrip(key: str, lane: int) -> None:
    rows = _sqlite_sync_mutation_rows(key)
    rows[2]["kind"]["lane_id"] = lane
    captured = parse(b"".join(map(_line, rows)))
    assert captured["schema_version"] == 4
    assert captured["status"] == "complete"
    assert captured["counters"] == [{"span_id": 2, "key": key, "value": 1, "ts_ns": 25}]
    owner = next(row for row in summarize(captured)["spans"] if row["span_id"] == 2)
    assert owner["counters"] == {key: 1}
    assert any(
        event["ph"] == "C" and event["name"] == key
        for event in chrome_export(captured)["traceEvents"]
    )


@pytest.mark.parametrize("version", (1, 2, 3))
@pytest.mark.parametrize("key", SQLITE_SYNC_MUTATION_KEYS)
def test_legacy_timeline_rejects_v4_sqlite_sync_mutation_class(key: str, version: int) -> None:
    rows = _sqlite_sync_mutation_rows(key)
    rows[0]["schema_version"] = version
    rows[-1]["schema_version"] = version
    with pytest.raises(ValueError, match="invalid causal timeline row"):
        parse(b"".join(map(_line, rows)))


@pytest.mark.parametrize(
    ("stage", "lane"),
    (("query_candidate", 11), ("sqlite_work", 0), ("sqlite_work", 10), ("sqlite_work", 14)),
)
def test_v4_sqlite_sync_mutation_class_rejects_wrong_owner(stage: str, lane: int) -> None:
    rows = _sqlite_sync_mutation_rows("sqlite_sync_overwrite")
    rows[2]["kind"].update(stage=stage, lane_id=lane)
    with pytest.raises(ValueError, match="wrong owner stage or lane"):
        parse(b"".join(map(_line, rows)))


@pytest.mark.parametrize("value", (0, 2))
def test_v4_sqlite_sync_mutation_class_requires_unit_value(value: int) -> None:
    rows = _sqlite_sync_mutation_rows("sqlite_sync_growth")
    rows[3]["kind"]["value"] = value
    with pytest.raises(ValueError, match="value must be one"):
        parse(b"".join(map(_line, rows)))


@pytest.mark.parametrize("second_key", ("sqlite_sync_create", "sqlite_sync_overwrite"))
def test_v4_sqlite_sync_mutation_class_rejects_second_counter(second_key: str) -> None:
    rows = _sqlite_sync_mutation_rows("sqlite_sync_overwrite")
    second = copy.deepcopy(rows[3])
    second["ts_ns"] = 26
    second["kind"]["key"] = second_key
    rows.insert(4, second)
    _resequence_ann_region_rows(rows)
    with pytest.raises(ValueError, match="duplicate SQLite sync mutation classification"):
        parse(b"".join(map(_line, rows)))


def test_v4_sqlite_sync_mutation_class_rejects_foreign_span() -> None:
    rows = _sqlite_sync_mutation_rows("sqlite_sync_unclassified")
    rows[3]["kind"]["span_id"] = 99
    with pytest.raises(ValueError, match="no existing owner span"):
        parse(b"".join(map(_line, rows)))


@pytest.mark.parametrize("legacy_version", (1, 2))
def test_legacy_timeline_rejects_sqlite_sync_observation(legacy_version: int) -> None:
    rows = _sqlite_sync_rows()
    rows[0]["schema_version"] = legacy_version
    rows[-1]["schema_version"] = legacy_version
    with pytest.raises(ValueError, match="invalid causal timeline row"):
        parse(b"".join(map(_line, rows)))


@pytest.mark.parametrize(
    ("field", "value", "reason"),
    (
        ("observed", False, "unobserved SQLite sync"),
        ("full_requested", False, "result without request"),
        ("full_result", None, "default full syscall without an attempt"),
        ("full_result", 1, "outside zero or minus one"),
        ("full_errno", 5, "successful syscall has nonzero errno"),
        ("posix_errno", 5, "result and errno presence disagree"),
    ),
)
def test_sqlite_sync_observation_rejects_contradictory_options(
    field: str, value: object, reason: str
) -> None:
    rows = _sqlite_sync_rows()
    rows[4]["kind"]["observation"][field] = value
    with pytest.raises(ValueError, match=reason):
        parse(b"".join(map(_line, rows)))


@pytest.mark.parametrize(
    ("full_result", "full_errno", "posix_result", "posix_errno", "reason"),
    (
        (0, 0, 0, 0, "full sync succeeded but POSIX fallback was attempted"),
        (-1, 45, None, None, "full sync failed without POSIX fallback"),
    ),
)
def test_sqlite_sync_observation_rejects_impossible_fallback_order(
    full_result: int,
    full_errno: int,
    posix_result: int | None,
    posix_errno: int | None,
    reason: str,
) -> None:
    rows = _sqlite_sync_rows()
    rows[4]["kind"]["observation"].update(
        full_result=full_result,
        full_errno=full_errno,
        posix_result=posix_result,
        posix_errno=posix_errno,
    )
    with pytest.raises(ValueError, match=reason):
        parse(b"".join(map(_line, rows)))


def test_sqlite_sync_observation_rejects_malformed_or_wrong_owner() -> None:
    for mutation, reason in (
        (
            lambda rows: rows[4]["kind"]["observation"].pop("observed"),
            "invalid causal timeline row",
        ),
        (
            lambda rows: rows[4]["kind"]["observation"].update(extra=True),
            "invalid causal timeline row",
        ),
        (lambda rows: rows[4]["kind"].update(span_id=99), "foreign span"),
        (lambda rows: rows[2]["kind"].update(stage="operation"), "wrong owner stage or lane"),
        (lambda rows: rows[2]["kind"].update(lane_id=10), "wrong owner stage or lane"),
        (lambda rows: rows[4].update(ts_ns=36), "outside owner interval"),
    ):
        rows = _sqlite_sync_rows()
        mutation(rows)
        with pytest.raises(ValueError, match=reason):
            parse(b"".join(map(_line, rows)))
    rows = _sqlite_sync_rows()
    rows.insert(5, copy.deepcopy(rows[4]))
    _resequence_ann_region_rows(rows)
    with pytest.raises(ValueError, match="duplicate SQLite sync observation"):
        parse(b"".join(map(_line, rows)))


def test_sqlite_sync_no_attempt_and_absence_do_not_become_success() -> None:
    rows = _sqlite_sync_rows()
    no_attempt = rows[4]["kind"]["observation"]
    no_attempt.update(
        observed=True,
        same_native_fd=False,
        full_requested=False,
        full_default_syscall=False,
        full_result=None,
        full_errno=None,
        posix_result=None,
        posix_errno=None,
    )
    capture = parse(b"".join(map(_line, rows)))
    assert capture["spans"][2]["sqlite_sync_observation"]["full_result"] is None
    rows.pop(4)
    _resequence_ann_region_rows(rows)
    absent = summarize(parse(b"".join(map(_line, rows))))
    owner = next(row for row in absent["spans"] if row["span_id"] == 2)
    assert owner["sqlite_sync_observation"] is None
    assert absent["sqlite_sync_observations"] == []
    assert "sqlite_sync_observation" not in summarize(parse(_capture()))["spans"][1]
    rows = _sqlite_sync_rows()
    rows[4]["kind"]["observation"].update(
        observed=False,
        same_native_fd=False,
        ambiguous=True,
        full_requested=False,
        full_default_syscall=False,
        full_result=None,
        full_errno=None,
        posix_result=None,
        posix_errno=None,
    )
    assert parse(b"".join(map(_line, rows)))["spans"][2]["sqlite_sync_observation"]["ambiguous"]


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


def test_early_end_folds_into_same_public_span_result() -> None:
    expected = parse(_capture())
    rows = [json.loads(line) for line in _capture().splitlines()]
    child_end = rows.pop(4)
    rows.insert(2, child_end)
    actual = parse(_boundary_rows(rows))
    assert actual["status"] == "complete"
    assert actual["spans"] == expected["spans"]
    assert actual["counters"] == expected["counters"]
    assert actual["event_count"] == expected["event_count"]
    assert actual["footer"] == expected["footer"]


@pytest.mark.parametrize("early", [False, True])
def test_duplicate_end_after_fold_or_begun_span_is_rejected(early: bool) -> None:
    rows = [json.loads(line) for line in _capture().splitlines()]
    child_end = rows.pop(4)
    if early:
        rows.insert(2, child_end)
        rows.insert(4, copy.deepcopy(child_end))
    else:
        rows.insert(4, child_end)
        rows.insert(5, copy.deepcopy(child_end))
    with pytest.raises(ValueError, match="duplicate span end: 2"):
        parse(_boundary_rows(rows))


@pytest.mark.parametrize("early", [False, True])
def test_regressed_terminal_clock_keeps_refusal_after_fold(early: bool) -> None:
    rows = [json.loads(line) for line in _capture().splitlines()]
    child_end = rows.pop(4)
    child_end["ts_ns"] = 15
    rows.insert(2 if early else 4, child_end)
    with pytest.raises(ValueError, match="span clock regressed: 2"):
        parse(_boundary_rows(rows))


def test_unmatched_early_end_complete_refuses_partial_retains() -> None:
    rows = [json.loads(line) for line in _capture().splitlines()]
    del rows[2:4]  # child begin and counter; child end has no begin
    with pytest.raises(ValueError, match="complete capture has end without begin: 2"):
        parse(_boundary_rows(copy.deepcopy(rows)))
    rows[-1]["status"] = "partial"
    partial = parse(_boundary_rows(rows))
    assert partial["status"] == "partial"
    assert set(partial["spans"]) == {1}
    assert partial["spans"][1]["end_ns"] == 40


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


@pytest.mark.parametrize("version", [1, 2, 3, 4])
def test_stream_and_bytes_parser_have_one_versioned_result(version: int) -> None:
    rows = [json.loads(line) for line in _capture().splitlines()]
    rows[0]["schema_version"] = version
    rows[-1]["schema_version"] = version
    raw = b"".join(map(_line, rows))
    captured = parse_stream(io.BytesIO(raw))
    assert captured == parse(raw)
    assert captured["status"] == "complete"
    assert captured["schema_version"] == version
    assert captured["event_count"] == 5
    assert captured["sha256"] == hashlib.sha256(raw).hexdigest()
    assert set(captured["spans"]) == {1, 2}
    assert captured["spans"][2]["parent_span_id"] == 1
    assert captured["spans"][2]["start_ns"] == 20
    assert captured["spans"][2]["end_ns"] == 35
    assert captured["spans"][2]["counters"] == {"candidates": 3}
    partial = parse_stream(io.BytesIO(raw[:-1]))
    assert partial == parse(raw[:-1])
    assert partial["status"] == "partial"
    assert partial["truncated_tail"] is True
    assert partial["event_count"] == 5
    assert partial["sha256"] == hashlib.sha256(raw[:-1]).hexdigest()


def test_stream_parser_uses_only_bounded_lazy_binary_lines() -> None:
    class LineOnlyStream(io.BytesIO):
        requests: list[int]

        def __init__(self, raw: bytes) -> None:
            super().__init__(raw)
            self.requests = []

        def read(self, size: int = -1) -> bytes:
            raise AssertionError("whole-stream read is forbidden")

        def readline(self, size: int = -1) -> bytes:
            assert 0 < size <= MAX_LINE_BYTES + 2
            self.requests.append(size)
            return super().readline(size)

    raw = _capture()
    stream = LineOnlyStream(raw)
    assert parse_stream(stream)["sha256"] == hashlib.sha256(raw).hexdigest()
    assert len(stream.requests) > 2


@pytest.mark.parametrize("limit", [False, True, 0, -1, 1.5, MAX_CONFIGURED_CAPTURE_BYTES + 1])
def test_stream_parser_rejects_invalid_configured_capture_bound(limit: object) -> None:
    with pytest.raises(ValueError, match="capture byte bound"):
        parse_stream(io.BytesIO(_capture()), max_capture_bytes=limit)


def test_stream_parser_enforces_exact_capture_and_line_caps() -> None:
    raw = _capture()
    assert parse_stream(io.BytesIO(raw), max_capture_bytes=len(raw))["status"] == "complete"
    assert (
        parse_stream(io.BytesIO(raw), max_capture_bytes=MAX_CONFIGURED_CAPTURE_BYTES)["status"]
        == "complete"
    )
    with pytest.raises(ValueError, match="offline replay byte bound"):
        parse_stream(io.BytesIO(raw), max_capture_bytes=len(raw) - 1)
    with pytest.raises(ValueError, match="header exceeds maximum line size"):
        parse_stream(io.BytesIO(b" " * (MAX_LINE_BYTES + 1) + b"\n"))
    with pytest.raises(ValueError, match="declared record cap"):
        parse_stream(io.BytesIO(raw + b"x" * 4097 + b"\n"))


@pytest.mark.parametrize(
    "raw",
    [
        _capture(footer=False) + b'{"record_type":"event","output_seq":4',
        _capture() + b'{"record_type":"event","output_seq":6',
        _capture().replace(b'"output_seq":2', b'"output_seq":7'),
    ],
)
def test_stream_and_bytes_parser_match_partial_and_failure_semantics(raw: bytes) -> None:
    try:
        expected = parse(raw)
    except ValueError as error:
        with pytest.raises(type(error)) as streamed:
            parse_stream(io.BytesIO(raw))
        assert str(streamed.value) == str(error)
    else:
        assert parse_stream(io.BytesIO(raw)) == expected


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
    outgoing = next(event for event in viewer["traceEvents"] if event.get("flow_out"))
    incoming = next(event for event in viewer["traceEvents"] if event.get("flow_in"))
    assert (outgoing["ts"], outgoing["dur"], incoming["ts"]) == (0.012, 0, 0.016)
    assert outgoing["name"] == "client_attempt_start"
    assert incoming["name"] == "request"
    assert outgoing["bind_id"] == incoming["bind_id"] == "0000000000000001"
    assert int(outgoing["bind_id"], 16) <= (1 << 64) - 1
    assert outgoing["args"]["display_only_flow_endpoint"] is True
    assert incoming["args"]["explicit_link"]["source_span_id"] == "3"
    assert incoming["args"]["explicit_link"]["target_span_id"] == "2"
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
    assert not any(
        event.get("flow_out") or event.get("flow_in")
        for event in chrome_export(parsed)["traceEvents"]
    )


def test_equal_start_flow_endpoint_precedes_the_request_without_changing_duration() -> None:
    rows = [json.loads(line) for line in _linked_capture().splitlines()]
    for row in rows:
        if row.get("kind", {}).get("type") == "span_begin" and row["kind"]["span_id"] == 3:
            row["ts_ns"] = 12
    viewer = chrome_export(parse(b"".join(map(_line, rows))))
    outgoing = next(event for event in viewer["traceEvents"] if event.get("flow_out"))
    incoming = next(event for event in viewer["traceEvents"] if event.get("flow_in"))
    assert outgoing["ts"] == incoming["ts"] == 0.012
    assert outgoing["dur"] == 0
    assert incoming["dur"] == 0.018
    assert viewer["traceEvents"].index(outgoing) < viewer["traceEvents"].index(incoming)


@pytest.mark.parametrize(
    "key",
    [
        "candidates",
        "managed_object_inputs",
        "work_reads",
        "work_writes",
        "work_bytes",
        "work_mutations",
    ],
)
def test_counter_events_and_error_terminal_have_viewer_events(key: str) -> None:
    raw = _capture().replace(b'"outcome":"ok"', b'"outcome":"error"', 1)
    raw = raw.replace(b'"key":"candidates"', f'"key":"{key}"'.encode(), 1)
    captured = parse(raw)
    assert summarize(captured)["spans"][1]["counters"] == {key: 3}
    viewer = chrome_export(captured)
    assert any(event["ph"] == "C" and event["name"] == key for event in viewer["traceEvents"])
    assert any(event["ph"] == "i" and event["cat"] == "terminal" for event in viewer["traceEvents"])


@pytest.mark.parametrize(
    "stage",
    [
        "native_source_apply",
        "native_source_owner_prepare",
        "native_source_path_validate",
        "native_source_path_install",
        "native_source_map_apply",
    ],
)
@pytest.mark.parametrize("outcome", ["ok", "error"])
@pytest.mark.parametrize("version", [1, 2, 3, 4])
def test_native_source_phase_schema_replays_success_and_refusal(
    stage: str, outcome: str, version: int
) -> None:
    rows = [json.loads(line) for line in _capture().splitlines()]
    rows[0]["schema_version"] = version
    rows[-1]["schema_version"] = version
    rows[2]["kind"]["stage"] = stage
    rows[4]["kind"]["outcome"] = outcome
    if outcome == "error":
        rows[4]["kind"]["error_code"] = 1
    captured = parse(b"".join(map(_line, rows)))
    assert captured["spans"][2]["stage"] == stage
    assert captured["spans"][2]["outcome"] == outcome
    assert summarize(captured)["status"] == "complete"
    assert any(event.get("name") == stage for event in chrome_export(captured)["traceEvents"])


def test_catalog_lock_wait_and_hold_are_siblings_under_one_parent() -> None:
    original = [json.loads(line) for line in _capture().splitlines()]
    original[0]["schema_version"] = 3
    original[-1]["schema_version"] = 3
    wait = original[2]
    wait["kind"]["stage"] = "catalog_lock_wait"
    hold = copy.deepcopy(wait)
    hold["kind"].update(span_id=3, stage="catalog_lock_hold")
    hold["ts_ns"] = 26
    rows = [
        original[0],
        original[1],
        wait,
        {
            "record_type": "event",
            "capture_id": CAPTURE,
            "ts_ns": 25,
            "kind": {"type": "span_end", "span_id": 2, "outcome": "ok"},
        },
        hold,
        {
            "record_type": "event",
            "capture_id": CAPTURE,
            "ts_ns": 35,
            "kind": {"type": "span_end", "span_id": 3, "outcome": "ok"},
        },
        original[5],
        original[-1],
    ]
    for sequence, row in enumerate(rows[1:-1], 1):
        row["output_seq"] = sequence
    rows[-1].update(
        last_output_seq=6,
        events_attempted=6,
        events_accepted=6,
        events_written=6,
    )

    captured = parse(b"".join(map(_line, rows)))
    assert captured["status"] == "complete"
    summary = {row["span_id"]: row for row in summarize(captured)["spans"]}
    assert [summary[span_id]["parent_span_id"] for span_id in (2, 3)] == [1, 1]
    assert [summary[span_id]["stage"] for span_id in (2, 3)] == [
        "catalog_lock_wait",
        "catalog_lock_hold",
    ]
    assert [summary[span_id]["duration_ns"] for span_id in (2, 3)] == [5, 9]
    names = {event["name"] for event in chrome_export(captured)["traceEvents"]}
    assert {"catalog_lock_wait", "catalog_lock_hold"} <= names


@pytest.mark.parametrize("stage", ["catalog_lock_wait", "catalog_lock_hold"])
@pytest.mark.parametrize("outcome", ["cancelled", "error", "aborted"])
def test_catalog_lock_stage_preserves_terminal_outcome(stage: str, outcome: str) -> None:
    rows = [json.loads(line) for line in _capture().splitlines()]
    rows[0]["schema_version"] = 3
    rows[-1]["schema_version"] = 3
    rows[2]["kind"]["stage"] = stage
    rows[4]["kind"]["outcome"] = outcome
    if outcome != "aborted":
        rows[4]["kind"]["error_code"] = 1

    captured = parse(b"".join(map(_line, rows)))
    assert captured["status"] == "complete"
    assert captured["spans"][2]["stage"] == stage
    assert summarize(captured)["spans"][1]["outcome"] == outcome


@pytest.mark.parametrize(
    "stage",
    [
        "native_text_inventory_prepare",
        "native_text_inventory_map_apply",
        "native_text_inventory_diff",
        "native_text_inventory_read",
        "native_text_occurrence_read",
        "native_text_posting_edit",
        "native_text_postings",
        "native_text_root",
        "native_coverage_scan",
        "native_coverage_map_apply",
        "native_searchable_prepare",
        "native_coverage_save",
        "native_member_assemble",
        "native_work_seal",
        "managed_object_reserve",
        "managed_object_confirm",
    ],
)
@pytest.mark.parametrize("version", [1, 2, 3, 4])
def test_native_attribution_stage_schema_replays_complete_capture(stage: str, version: int) -> None:
    rows = [json.loads(line) for line in _capture().splitlines()]
    rows[0]["schema_version"] = version
    rows[-1]["schema_version"] = version
    rows[2]["kind"]["stage"] = stage
    captured = parse(b"".join(map(_line, rows)))
    assert captured["spans"][2]["stage"] == stage
    assert summarize(captured)["status"] == "complete"
    assert any(event.get("name") == stage for event in chrome_export(captured)["traceEvents"])


def _v5_rows() -> list[dict | list]:
    legacy = [json.loads(line) for line in _capture().splitlines()]
    header, footer = legacy[0], legacy[-1]
    header["schema_version"] = 5
    footer["schema_version"] = 5
    return [
        header,
        ["e", 1, CAPTURE, 10, ["b", 1, None, "operation", 0, "operation"]],
        ["e", 2, CAPTURE, 20, ["b", 2, 1, "query_candidate", 0, None]],
        ["e", 3, CAPTURE, 25, ["c", 2, "candidates", 3]],
        ["e", 4, CAPTURE, 35, ["e", 2, "ok", None]],
        ["e", 5, CAPTURE, 40, ["e", 1, "ok", None]],
        footer,
    ]


def _v5_bytes(rows: list[dict | list]) -> bytes:
    return b"".join(json.dumps(row, separators=(",", ":")).encode() + b"\n" for row in rows)


def _v5_map_root_rows(*, outcome: str = "ok") -> list[dict | list]:
    rows = _v5_rows()
    rows[2][4] = ["b", 2, 1, "map_apply", 0, None]
    rows[3:4] = [
        ["e", 0, CAPTURE, 25, ["c", 2, key, value]]
        for key, value in (
            ("map_root_reuse_candidates", 2),
            ("map_root_reuses", 1),
            ("map_root_builds", 1),
        )
    ]
    rows[-3][4] = ["e", 2, outcome, 1 if outcome == "error" else None]
    count = len(rows) - 2
    for sequence, event in enumerate(rows[1:-1], 1):
        event[1] = sequence
    rows[-1].update(
        last_output_seq=count, events_attempted=count,
        events_accepted=count, events_written=count,
    )
    return rows


def _v5_map_cpu_rows(lane: int) -> list[dict | list]:
    rows = _v5_map_root_rows()
    if lane != 0:
        if lane == 2:
            rows.insert(3, ["e", 0, CAPTURE, 21, ["b", 3, 2, "map_apply", 1, None]])
            rows.insert(4, ["e", 0, CAPTURE, 22, ["b", 4, 3, "map_apply", lane, None]])
            span_id = 4
        else:
            rows.insert(3, ["e", 0, CAPTURE, 22, ["b", 3, 2, "map_apply", lane, None]])
            span_id = 3
    else:
        span_id = 2
    rows.insert(-3, ["e", 0, CAPTURE, 27, ["c", span_id, "thread_cpu_ns", 7]])
    if lane != 0:
        rows.insert(-3, ["e", 0, CAPTURE, 30, ["e", span_id, "ok", None]])
        if lane == 2:
            rows.insert(-3, ["e", 0, CAPTURE, 32, ["e", 3, "ok", None]])
    count = len(rows) - 2
    for sequence, event in enumerate(rows[1:-1], 1):
        event[1] = sequence
    rows[-1].update(
        last_output_seq=count,
        events_attempted=count,
        events_accepted=count,
        events_written=count,
    )
    return rows


@pytest.mark.parametrize("lane", range(6))
def test_v5_map_apply_cpu_accepts_only_emitted_phase_lanes(lane: int) -> None:
    captured = parse(_v5_bytes(_v5_map_cpu_rows(lane)))
    assert captured["status"] == "complete"
    span_id = 4 if lane == 2 else 3 if lane else 2
    span = captured["spans"][span_id]
    assert span["stage"] == "map_apply"
    assert span["lane_id"] == lane
    assert span["counters"]["thread_cpu_ns"] == 7
    assert span["parent_span_id"] == (3 if lane == 2 else 2 if lane else 1)


@pytest.mark.parametrize("fault", ("unknown_lane", "wrong_parent", "wrong_nested_parent"))
def test_v5_map_apply_cpu_rejects_unknown_lane_or_parent(fault: str) -> None:
    lane = 2 if fault == "wrong_nested_parent" else 1
    rows = _v5_map_cpu_rows(lane)
    if fault == "unknown_lane":
        rows[3][4][4] = 6
    elif fault == "wrong_parent":
        rows[3][4][2] = 1
    else:
        rows[4][4][2] = 2
    with pytest.raises(ValueError, match="thread CPU counter targets wrong stage"):
        parse(_v5_bytes(rows))


def test_v4_map_apply_cpu_is_not_admitted_as_a_legacy_stage() -> None:
    rows = _v5_map_cpu_rows(0)
    rows[0]["schema_version"] = rows[-1]["schema_version"] = 4
    with pytest.raises(ValueError):
        parse(_v5_bytes(rows))


@pytest.mark.parametrize("outcome", ("ok", "error"))
def test_v5_map_root_reuse_counters_keep_completed_and_failed_batch(outcome: str) -> None:
    captured = parse(_v5_bytes(_v5_map_root_rows(outcome=outcome)))
    assert captured["status"] == "complete"
    assert captured["spans"][2]["outcome"] == outcome
    assert captured["spans"][2]["counters"] == {
        "map_root_reuse_candidates": 2,
        "map_root_reuses": 1,
        "map_root_builds": 1,
    }
    summary = next(row for row in summarize(captured)["spans"] if row["span_id"] == 2)
    assert summary["counters"] == captured["spans"][2]["counters"]


@pytest.mark.parametrize(
    ("fault", "error"),
    (
        ("missing", "family is partial"),
        ("absent", "family is partial"),
        ("duplicate", "duplicated"),
        ("wrong_stage", "wrong version, stage or lane"),
        ("wrong_lane", "wrong version, stage or lane"),
        ("orphan", "no existing owner"),
        ("after_end", "follows span end"),
        ("reuses_exceed_candidates", "reuses exceed candidates"),
    ),
)
def test_v5_map_root_reuse_rejects_invalid_family(fault: str, error: str) -> None:
    rows = _v5_map_root_rows()
    if fault == "missing":
        del rows[3]
    elif fault == "absent":
        del rows[3:6]
    elif fault == "duplicate":
        rows.insert(4, copy.deepcopy(rows[3]))
    elif fault == "wrong_stage":
        rows[2][4][3] = "native_text_inventory_map_apply"
    elif fault == "wrong_lane":
        rows[2][4][4] = 1
    elif fault == "orphan":
        rows[3][4][1] = 99
    elif fault == "after_end":
        rows[3], rows[-3] = rows[-3], rows[3]
        rows[-3][3] = rows[3][3]
    else:
        rows[4][4][3] = 3
    count = len(rows) - 2
    for sequence, event in enumerate(rows[1:-1], 1):
        event[1] = sequence
    rows[-1].update(
        last_output_seq=count, events_attempted=count,
        events_accepted=count, events_written=count,
    )
    with pytest.raises(ValueError, match=error):
        parse(_v5_bytes(rows))


def test_v5_map_root_partial_capture_keeps_observed_prefix() -> None:
    rows = _v5_map_root_rows()
    del rows[5]
    rows[-1]["status"] = "partial"
    count = len(rows) - 2
    for sequence, event in enumerate(rows[1:-1], 1):
        event[1] = sequence
    rows[-1].update(
        last_output_seq=count, events_attempted=count,
        events_accepted=count, events_written=count,
    )
    captured = parse(_v5_bytes(rows))
    assert captured["status"] == "partial"
    assert captured["spans"][2]["counters"] == {
        "map_root_reuse_candidates": 2,
        "map_root_reuses": 1,
    }


def test_v4_rejects_v5_map_root_stage_and_counters() -> None:
    schema = _schema_for_version(json.loads(SCHEMA.read_text()), 4)
    assert "map_apply" not in schema["$defs"]["spanBegin"]["properties"]["stage"]["enum"]
    assert not set(("map_root_reuse_candidates", "map_root_reuses", "map_root_builds")) & set(
        schema["$defs"]["counterKey"]["enum"]
    )


TEXT_OCCURRENCE_DIFF_GOLD = (
    ("text_occurrence_prior_validation_attempts", 2),
    ("text_occurrence_next_validation_attempts", 3),
    ("text_occurrence_ram_reuses", 1),
    ("text_occurrence_spill_reuses", 1),
    ("text_occurrence_equal_reuses", 1),
    ("text_occurrence_validation_failures", 1),
)


def _v5_diff_rows(*, outcome: str = "ok") -> list[dict | list]:
    rows = _v5_rows()
    rows[2][4] = ["b", 2, 1, "native_text_inventory_diff", 2, None]
    rows[3:4] = [
        ["e", 0, CAPTURE, 25, ["c", 2, key, value]]
        for key, value in TEXT_OCCURRENCE_DIFF_GOLD
    ]
    rows[-3][4] = ["e", 2, outcome, 1 if outcome == "error" else None]
    count = len(rows) - 2
    for sequence, event in enumerate(rows[1:-1], 1):
        event[1] = sequence
    rows[-1].update(
        last_output_seq=count, events_attempted=count,
        events_accepted=count, events_written=count,
    )
    return rows


@pytest.mark.parametrize("outcome", ("ok", "error"))
def test_v5_text_occurrence_diff_family_replays_and_preserves_failed_span(outcome: str) -> None:
    captured = parse(_v5_bytes(_v5_diff_rows(outcome=outcome)))
    assert captured["status"] == "complete"
    assert captured["spans"][2]["outcome"] == outcome
    assert captured["spans"][2]["counters"] == dict(TEXT_OCCURRENCE_DIFF_GOLD)


@pytest.mark.parametrize(
    ("fault", "error"),
    (
        ("missing", "family is partial"),
        ("duplicate", "duplicated"),
        ("wrong_stage", "wrong version, stage or lane"),
        ("wrong_lane", "wrong version, stage or lane"),
        ("wrong_version", None),
        ("after_end", "follows span end"),
        ("too_many_failures", "failures exceed attempts"),
        ("attempt_overflow", "attempts exceed u64"),
        ("orphan", "no existing owner"),
    ),
)
def test_v5_text_occurrence_diff_rejects_invalid_owner_or_family(
    fault: str, error: str | None
) -> None:
    rows = _v5_diff_rows()
    if fault == "missing":
        del rows[3]
    elif fault == "duplicate":
        rows.insert(4, copy.deepcopy(rows[3]))
    elif fault == "wrong_stage":
        rows[2][4][3] = "native_text_inventory_read"
    elif fault == "wrong_lane":
        rows[2][4][4] = 1
    elif fault == "wrong_version":
        rows[0]["schema_version"] = rows[-1]["schema_version"] = 4
        # Legacy rows are object-encoded; the old schema excludes this family.
        for index, row in enumerate(rows[1:-1], 1):
            kind = row[4]
            if kind[0] == "b":
                payload = {"type": "span_begin", "span_id": kind[1], "stage": kind[3], "lane_id": kind[4]}
                payload.update({"root_kind": kind[5]} if kind[2] is None else {"parent_span_id": kind[2]})
            elif kind[0] == "c":
                payload = {"type": "counter", "span_id": kind[1], "key": kind[2], "value": kind[3]}
            else:
                payload = {"type": "span_end", "span_id": kind[1], "outcome": kind[2]}
            rows[index] = {"record_type": "event", "output_seq": row[1], "capture_id": CAPTURE, "ts_ns": row[3], "kind": payload}
    elif fault == "after_end":
        rows[3], rows[-3] = rows[-3], rows[3]
        rows[-3][3] = rows[3][3]
    elif fault == "too_many_failures":
        rows[8][4][3] = 6
    elif fault == "attempt_overflow":
        rows[3][4][3] = (1 << 64) - 1
    else:
        rows[3][4][1] = 99
    count = len(rows) - 2
    for sequence, row in enumerate(rows[1:-1], 1):
        row["output_seq" if isinstance(row, dict) else 1] = sequence
    rows[-1].update(last_output_seq=count, events_attempted=count, events_accepted=count, events_written=count)
    with pytest.raises(ValueError, match=error):
        parse(_v5_bytes(rows))


def test_v5_text_occurrence_diff_partial_capture_keeps_unobserved_family_partial() -> None:
    rows = _v5_diff_rows()
    del rows[3]
    rows[-1]["status"] = "partial"
    count = len(rows) - 2
    for sequence, row in enumerate(rows[1:-1], 1):
        row[1] = sequence
    rows[-1].update(last_output_seq=count, events_attempted=count, events_accepted=count, events_written=count)
    captured = parse(_v5_bytes(rows))
    assert captured["status"] == "partial"
    assert "text_occurrence_prior_validation_attempts" not in captured["spans"][2]["counters"]


def _v5_object_install_rows() -> list[dict | list]:
    rows = _v5_rows()
    rows[2][4] = ["b", 2, 1, "managed_object_install", 0, None]
    rows[3] = [
        "e", 3, CAPTURE, 25,
        {
            "type": "object_install_aggregate", "span_id": 2,
            "phase": "file_sync", "calls": 2, "elapsed_sum_ns": 6,
            "thread_cpu_ns": 9, "failures": 0,
        },
    ]
    rows.insert(
        4,
        [
            "e", 4, CAPTURE, 26,
            {
                "type": "object_install_aggregate", "span_id": 2,
                "phase": "settlement", "calls": 1, "elapsed_sum_ns": 4,
                "thread_cpu_ns": None, "failures": 1,
            },
        ],
    )
    rows.insert(5, ["e", 5, CAPTURE, 27, ["c", 2, "thread_cpu_ns", 12]])
    rows[6][1] = 6
    rows[7][1] = 7
    count = len(rows) - 2
    rows[-1].update(
        last_output_seq=count,
        events_attempted=count,
        events_accepted=count,
        events_written=count,
    )
    return rows


def test_v5_object_install_phase_replay_summary_and_perfetto() -> None:
    captured = parse(_v5_bytes(_v5_object_install_rows()))
    assert captured["status"] == "complete"
    assert captured["event_count"] == 7
    assert [row["phase"] for row in captured["object_install_aggregates"]] == [
        "file_sync", "settlement"
    ]
    packet = next(row for row in summarize(captured)["spans"] if row["span_id"] == 2)
    assert packet["duration_ns"] == 15
    assert packet["object_install"]["observed_elapsed_sum_ns"] == 10
    assert packet["object_install"]["unattributed_ns"] == 5
    assert packet["object_install"]["observed_phases"][1]["thread_cpu_ns"] is None
    exported = chrome_export(captured)
    phase_event = next(
        event for event in exported["traceEvents"]
        if event["name"] == "object_install:file_sync"
    )
    assert phase_event["ph"] == "i"
    assert phase_event["args"]["elapsed_sum_ns"] == "6"
    assert phase_event["args"]["thread_cpu_ns"] == "9"
    assert any(
        event["name"] == "object_install:file_sync:thread_cpu_ns"
        and event["ph"] == "C"
        for event in exported["traceEvents"]
    )
    assert not any(
        event["name"] == "object_install:settlement:thread_cpu_ns"
        for event in exported["traceEvents"]
    )
    exported_packet = next(
        row for row in exported["quantaIndexSummary"]["spans"]
        if row["span_id"] == "2"
    )
    assert exported_packet["object_install"]["unattributed_ns"] == "5"
    assert exported["quantaIndexSummary"]["object_install_aggregates"][1][
        "thread_cpu_ns"
    ] is None


@pytest.mark.parametrize(
    "phase",
    (
        "namespace_and_create",
        "namespace_validation",
        "writer_create",
        "writer_ticket_prepare",
        "writer_open",
        "writer_created_witness",
    ),
)
def test_v5_object_install_namespace_phases_replay(phase: str) -> None:
    rows = _v5_object_install_rows()
    rows[3][4]["phase"] = phase
    captured = parse(_v5_bytes(rows))
    observed = summarize(captured)["object_install_aggregates"]
    assert observed[0]["phase"] == phase


@pytest.mark.parametrize(
    ("field", "value", "error"),
    (
        ("phase", "unknown", "invalid causal timeline row"),
        ("span_id", 1, "wrong owner"),
        ("calls", 0, "invalid causal timeline row"),
        ("failures", 3, "failures exceed calls"),
        ("elapsed_sum_ns", 16, "exceed packet wall"),
    ),
)
def test_v5_object_install_aggregate_rejects_invalid_contract(
    field: str, value: object, error: str
) -> None:
    rows = _v5_object_install_rows()
    rows[3][4][field] = value
    with pytest.raises(ValueError, match=error):
        parse(_v5_bytes(rows))


def test_v5_object_install_rejects_duplicate_phase_and_missing_cpu_field() -> None:
    rows = _v5_object_install_rows()
    rows[4][4]["phase"] = "file_sync"
    with pytest.raises(ValueError, match="duplicate object install phase"):
        parse(_v5_bytes(rows))
    rows = _v5_object_install_rows()
    del rows[4][4]["thread_cpu_ns"]
    with pytest.raises(ValueError, match="invalid causal timeline row"):
        parse(_v5_bytes(rows))


def test_v4_rejects_v5_object_install_advance_stages_and_aggregate() -> None:
    rows = [json.loads(line) for line in _capture().splitlines()]
    for row in (rows[0], rows[-1]):
        row["schema_version"] = 4
    for stage in ("managed_object_install", "managed_object_advance"):
        rows[2]["kind"]["stage"] = stage
        with pytest.raises(ValueError, match="invalid causal timeline row"):
            parse(_v5_bytes(rows))
    rows[2]["kind"]["stage"] = "query_candidate"
    rows[3]["kind"] = _v5_object_install_rows()[3][4]
    with pytest.raises(ValueError, match="invalid causal timeline row"):
        parse(_v5_bytes(rows))


def test_v5_fixed_wire_matches_v4_canonical_semantics() -> None:
    raw_v5 = _v5_bytes(_v5_rows())
    captured = parse(raw_v5)
    legacy = [json.loads(line) for line in _capture().splitlines()]
    legacy[0]["schema_version"] = 4
    legacy[-1]["schema_version"] = 4
    prior = parse(_v5_bytes(legacy))
    assert captured["schema_version"] == 5
    assert captured["status"] == prior["status"] == "complete"
    assert captured["spans"] == prior["spans"]
    assert captured["counters"] == prior["counters"]
    assert captured["capture_id"] == prior["capture_id"] == CAPTURE
    assert captured["event_count"] == prior["event_count"] == 5
    assert captured["sha256"] == hashlib.sha256(raw_v5).hexdigest()


def test_v5_compaction_thread_cpu_keeps_exact_wire_and_rejects_async_stage() -> None:
    rows = _v5_rows()
    rows[2][4] = ["b", 2, 1, "compaction", 3, None]
    rows[3][4] = ["c", 2, "thread_cpu_ns", 7]
    captured = parse(_v5_bytes(rows))
    assert captured["status"] == "complete"
    assert captured["spans"][2]["parent_span_id"] == 1
    assert captured["counters"] == [
        {"span_id": 2, "key": "thread_cpu_ns", "value": 7, "ts_ns": 25}
    ]
    rows[2][4] = ["b", 2, 1, "semantic_prepare", 3, None]
    with pytest.raises(ValueError, match="thread CPU"):
        parse(_v5_bytes(rows))


@pytest.mark.parametrize(
    ("stage", "lane"),
    (
        ("managed_object_reserve", 0),
        ("managed_object_confirm", 0),
        ("managed_object_advance", 0),
        ("sqlite_work", 3),
        ("sqlite_work", 8),
        ("sqlite_work", 11),
        ("sqlite_work", 21),
        ("sqlite_work", 26),
    ),
)
def test_v5_managed_and_sqlite_thread_cpu_replays_exact_counter(
    stage: str, lane: int
) -> None:
    rows = _v5_rows()
    rows[2][4] = ["b", 2, 1, stage, lane, None]
    rows[3][4] = ["c", 2, THREAD_CPU_GOLD[0], THREAD_CPU_GOLD[1]]
    captured = parse(_v5_bytes(rows))
    assert captured["status"] == "complete"
    assert captured["counters"] == [
        {"span_id": 2, "key": THREAD_CPU_GOLD[0], "value": THREAD_CPU_GOLD[1], "ts_ns": 25}
    ]
    owner = next(span for span in summarize(captured)["spans"] if span["span_id"] == 2)
    assert owner["stage"] == stage
    assert owner["lane_id"] == lane
    assert owner["counters"] == dict([THREAD_CPU_GOLD])
    viewer = [event for event in chrome_export(captured)["traceEvents"] if event["ph"] == "C"]
    assert [(event["name"], event["args"][event["name"]]) for event in viewer] == [
        THREAD_CPU_GOLD
    ]


@pytest.mark.parametrize("lane", (9, 10, 22, 27, 28))
def test_v5_sqlite_thread_cpu_rejects_unselected_lane(lane: int) -> None:
    rows = _v5_rows()
    rows[2][4] = ["b", 2, 1, "sqlite_work", lane, None]
    rows[3][4] = ["c", 2, THREAD_CPU_GOLD[0], THREAD_CPU_GOLD[1]]
    with pytest.raises(ValueError, match="thread CPU counter targets wrong stage"):
        parse(_v5_bytes(rows))


@pytest.mark.parametrize("stage", ("managed_object_reserve", "managed_object_confirm"))
def test_v4_managed_thread_cpu_remains_unselected(stage: str) -> None:
    rows = [json.loads(line) for line in _capture().splitlines()]
    rows[0]["schema_version"] = 4
    rows[-1]["schema_version"] = 4
    rows[2]["kind"]["stage"] = stage
    rows[3]["kind"].update(key=THREAD_CPU_GOLD[0], value=THREAD_CPU_GOLD[1])
    with pytest.raises(ValueError, match="thread CPU counter targets wrong stage"):
        parse(b"".join(map(_line, rows)))


@pytest.mark.parametrize(
    "stage",
    (
        "native_text_inventory_prepare",
        "native_text_inventory_map_apply",
        "native_text_inventory_diff",
        "native_text_inventory_read",
        "native_text_occurrence_read",
        "native_text_posting_edit",
    ),
)
def test_v5_inventory_stage_closes_with_thread_cpu_and_exact_parent(stage: str) -> None:
    rows = _v5_rows()
    rows[2][4] = ["b", 2, 1, stage, 2, None]
    rows[3][4] = ["c", 2, "thread_cpu_ns", 7]
    captured = parse(_v5_bytes(rows))
    assert captured["status"] == "complete"
    span = captured["spans"][2]
    assert (
        span["parent_span_id"], span["stage"], span["lane_id"],
        span["start_ns"], span["end_ns"], span["outcome"], span["counters"]
    ) == (1, stage, 2, 20, 35, "ok", {"thread_cpu_ns": 7})
    assert captured["counters"] == [
        {"span_id": 2, "key": "thread_cpu_ns", "value": 7, "ts_ns": 25}
    ]
    assert any(
        event["name"] == stage and event["ph"] == "X"
        for event in chrome_export(captured)["traceEvents"]
    )


@pytest.mark.parametrize(
    "wire_kind",
    [
        {"type": "span_begin", "span_id": 2, "stage": "query_candidate", "lane_id": 0},
        ["b", 2, 1, "query_candidate", 0],
        ["b", 2, 1, "unknown_stage", 0, None],
        ["b", 2, 1, "native_text_inventory", 2, None],
        ["b", 2, 1, "query_candidate", True, None],
        ["b", 2, 0, "query_candidate", 0, None],
        ["e", 2, "ok"],
        ["c", 2, "candidates", -1],
        ["c", 2, "unknown_counter", 3],
        ["x", 2, "candidates", 3],
    ],
)
def test_v5_rejects_malformed_common_kind(wire_kind: object) -> None:
    rows = _v5_rows()
    rows[2][4] = wire_kind
    with pytest.raises(ValueError):
        parse(_v5_bytes(rows))


@pytest.mark.parametrize(
    "outcome,error_code",
    [("ok", 0), ("aborted", 1), ("error", None), ("cancelled", None),
     ("error", 1 << 32)],
)
def test_v5_span_end_rejects_outcome_code_mismatch_and_u32_overflow(
    outcome: str, error_code: int | None
) -> None:
    rows = _v5_rows()
    rows[4][4] = ["e", 2, outcome, error_code]
    with pytest.raises(ValueError, match="invalid causal timeline row"):
        parse(_v5_bytes(rows))


@pytest.mark.parametrize(
    "outcome,error_code",
    [("ok", None), ("aborted", None), ("error", 0), ("cancelled", (1 << 32) - 1)],
)
def test_v5_full_schema_and_fast_dispatch_agree_on_span_end_abi(
    outcome: str, error_code: int | None
) -> None:
    schema = _schema_for_version(json.loads(SCHEMA.read_text()), 5)
    full = Draft202012Validator(schema)
    dispatch = _dispatch_v5_validators(schema)
    rows = _v5_rows()
    rows[4][4] = ["e", 2, outcome, error_code]
    event = rows[4]
    assert full.is_valid(event)
    assert _selected_validator(event, full, dispatch).is_valid(event)
    for bad in (["e", 2, "ok", 1], ["e", 2, "aborted", 1],
                ["e", 2, "error", None], ["e", 2, "cancelled", None],
                ["e", 2, "error", 1 << 32]):
        changed = ["e", 4, CAPTURE, 35, bad]
        assert not full.is_valid(changed)
        assert not _selected_validator(changed, full, dispatch).is_valid(changed)
    assert parse(_v5_bytes(rows))["spans"][2]["outcome"] == outcome


def test_v4_persisted_object_span_end_keeps_legacy_acceptance() -> None:
    rows = [json.loads(line) for line in _capture().splitlines()]
    rows[0]["schema_version"] = 4
    rows[-1]["schema_version"] = 4
    rows[4]["kind"].update(outcome="error")
    assert parse(_v5_bytes(rows))["spans"][2]["outcome"] == "error"
    rows[4]["kind"].update(outcome="ok", error_code=1)
    assert parse(_v5_bytes(rows))["spans"][2]["outcome"] == "ok"


@pytest.mark.parametrize(
    "event",
    [
        ["e", 2, CAPTURE, 20],
        ["e", 2, CAPTURE, 20, ["b", 2, 1, "query_candidate", 0, None], 0],
        ["x", 2, CAPTURE, 20, ["b", 2, 1, "query_candidate", 0, None]],
        ["e", True, CAPTURE, 20, ["b", 2, 1, "query_candidate", 0, None]],
        ["e", 2, "f" * 32, 20, ["b", 2, 1, "query_candidate", 0, None]],
        ["e", 2, CAPTURE, -1, ["b", 2, 1, "query_candidate", 0, None]],
        {"record_type": "event", "output_seq": 2, "capture_id": CAPTURE, "ts_ns": 20,
         "kind": {"type": "span_begin", "span_id": 2, "parent_span_id": 1,
                  "stage": "query_candidate", "lane_id": 0}},
    ],
)
def test_v5_rejects_malformed_or_legacy_outer_event(event: object) -> None:
    rows = _v5_rows()
    rows[2] = event
    with pytest.raises(ValueError):
        parse(_v5_bytes(rows))


def test_v5_rejects_duplicate_rare_kind_field_and_v4_tuple_mixture() -> None:
    rows = _v5_rows()
    rows[2] = ["e", 2, CAPTURE, 20, {"type": "phase_identity", "span_id": 1,
                                      "phase": "full_seal"}]
    raw = _v5_bytes(rows).replace(b'"span_id":1,', b'"span_id":1,"span_id":1,')
    with pytest.raises(ValueError, match="duplicate JSON key"):
        parse(raw)
    rows = _v5_rows()
    rows[0]["schema_version"] = 4
    rows[-1]["schema_version"] = 4
    with pytest.raises(ValueError):
        parse(_v5_bytes(rows))


def test_v5_partial_footer_and_original_byte_cap() -> None:
    rows = _v5_rows()
    partial = parse(_v5_bytes(rows[:-1]))
    assert partial["status"] == "partial"
    assert partial["footer"] is None
    with pytest.raises(ValueError, match="byte bound"):
        parse(_v5_bytes(rows), max_capture_bytes=100)
    rows[0]["max_record_bytes"] = 256
    rows[2] = ["e", 2, CAPTURE, 20, {"type": "health", "code": "queue_overflow",
                                     "detail": "x" * 250}]
    with pytest.raises(ValueError, match="declared record cap"):
        parse(_v5_bytes(rows))


def test_v5_preserves_full_capture_id_without_numeric_conversion() -> None:
    rows = _v5_rows()
    capture_id = "fedcba98765432100123456789abcdef"
    for row in rows:
        if isinstance(row, dict):
            row["capture_id"] = capture_id
            if row["record_type"] == "header":
                for field in ("stream_id", "clock_domain", "process_instance"):
                    row[field] = capture_id
        else:
            row[2] = capture_id
    assert parse(_v5_bytes(rows))["capture_id"] == capture_id


@pytest.mark.parametrize(
    "row_index,path",
    [
        (1, (1,)),       # output sequence
        (1, (3,)),       # timestamp
        (1, (4, 1)),    # begin span ID
        (2, (4, 2)),    # parent span ID
        (2, (4, 4)),    # lane ID
        (4, (4, 1)),    # end span ID
        (4, (4, 3)),    # end error code
        (3, (4, 1)),    # counter span ID
        (3, (4, 3)),    # counter value
    ],
)
@pytest.mark.parametrize("bad", [1.0, True])
def test_v5_common_numeric_fields_require_exact_json_integer(
    row_index: int, path: tuple[int, ...], bad: object
) -> None:
    rows = _v5_rows()
    if row_index == 4 and path == (4, 3):
        rows[4][4][2] = "error"
    slot = rows[row_index]
    for index in path[:-1]:
        slot = slot[index]
    slot[path[-1]] = bad
    with pytest.raises(ValueError):
        parse(_v5_bytes(rows))


def test_v5_exponent_numeric_alias_refused_after_schema_dispatch() -> None:
    rows = _v5_rows()
    rows[1][1] = 1.0
    raw = _v5_bytes(rows).replace(b'"e",1.0,', b'"e",1e0,', 1)
    schema = _schema_for_version(json.loads(SCHEMA.read_text()), 5)
    full = Draft202012Validator(schema)
    event = json.loads(raw.splitlines()[1])
    fast = _selected_validator(event, full, _dispatch_v5_validators(schema))
    assert full.is_valid(event) == fast.is_valid(event)
    with pytest.raises(ValueError, match="exact JSON integer"):
        parse(raw)


def test_v5_rare_owned_renewal_keeps_object_payload_and_v4_semantics() -> None:
    legacy = _owned_renewal_rows()
    v4 = parse(_boundary_rows(copy.deepcopy(legacy)))
    header, footer = legacy[0], legacy[-1]
    header["schema_version"] = 5
    footer["schema_version"] = 5
    footer.update(last_output_seq=5, events_attempted=5, events_accepted=5, events_written=5)
    rows: list[dict | list] = [
        header,
        ["e", 1, CAPTURE, 10, ["b", 1, None, "operation", 0, "operation"]],
        ["e", 2, CAPTURE, 20, ["b", 2, 1, "sqlite_work", 29, None]],
        ["e", 3, CAPTURE, 25, {
            "type": "owned_renewal_refusal", "span_id": 2, "reason": "lease_expired",
            "now_ms": 500, "lease_until_ms": 500, "deadline_ms": None,
        }],
        ["e", 4, CAPTURE, 35, ["e", 2, "error", 1]],
        ["e", 5, CAPTURE, 40, ["e", 1, "ok", None]],
        footer,
    ]
    v5 = parse(_v5_bytes(rows))
    assert v5["status"] == "complete"
    assert v5["owned_renewal_refusals"] == v4["owned_renewal_refusals"]
    assert v5["spans"] == v4["spans"]
