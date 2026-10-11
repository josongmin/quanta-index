"""Validate one canonical causal JSONL capture and export a bounded waterfall.

Writer order is checked separately from monotonic timestamps. A missing footer,
open span, or explicit recorder loss is partial evidence; malformed complete
rows and contradictory footer accounting are invalid evidence.
"""

from __future__ import annotations

import argparse
import copy
import hashlib
import heapq
import json
from collections import Counter
from pathlib import Path
from typing import Any

from jsonschema import Draft202012Validator, ValidationError

SCHEMA = Path(__file__).with_name("causal_timeline.schema.json")
MAX_LINE_BYTES = 1_048_576
MAX_CAPTURE_BYTES = 128 * 1024 * 1024
JS_SAFE_INTEGER = (1 << 53) - 1
U64_MAX = (1 << 64) - 1
BM25_AGGREGATE_STAGES = {
    "bm25_census_aggregate": "bm25_census_build",
    "bm25_boundary_aggregate": "bm25_boundary",
    "bm25_live_build_aggregate": "bm25_live_build",
}
EXECUTION_BINDING_WIDTHS = {
    "source_revision": 40,
    "binary_sha256": 64,
    "summary_sha256": 64,
    "tier_manifest_sha256": 64,
    "timeline_sha256": 64,
    "stderr_sha256": 64,
    "timeline_capture_id": 32,
}


class CausalSyntaxError(ValueError):
    """A final unterminated row may be an interrupted JSON write."""


def _unique_pairs(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    result: dict[str, Any] = {}
    for key, value in pairs:
        if key in result:
            raise ValueError(f"duplicate JSON key: {key}")
        result[key] = value
    return result


def _reject_constant(value: str) -> None:
    raise ValueError(f"non-finite JSON constant: {value}")


def _decode_record(raw: bytes, validator: Draft202012Validator, line_no: int) -> dict[str, Any]:
    try:
        record = json.loads(
            raw.decode("utf-8"),
            object_pairs_hook=_unique_pairs,
            parse_constant=_reject_constant,
        )
        validator.validate(record)
    except (UnicodeError, json.JSONDecodeError) as error:
        raise CausalSyntaxError(f"invalid causal timeline row {line_no}: {error}") from error
    except (ValidationError, ValueError) as error:
        raise ValueError(f"invalid causal timeline row {line_no}: {error}") from error
    if not isinstance(record, dict):
        raise ValueError(f"invalid causal timeline row {line_no}: object required")
    return record


def _schema_for_version(schema: dict[str, Any], version: int) -> dict[str, Any]:
    selected = copy.deepcopy(schema)
    for record_type in ("header", "footer"):
        selected["$defs"][record_type]["properties"]["schema_version"]["const"] = version
    if version == 1:
        selected["$defs"]["counterKey"]["enum"].remove("compacted_segments")
        selected["$defs"]["event"]["properties"]["kind"]["oneOf"] = [
            reference
            for reference in selected["$defs"]["event"]["properties"]["kind"]["oneOf"]
            if reference["$ref"]
            not in (
                "#/$defs/syncAggregate",
                "#/$defs/objectKindAggregate",
                "#/$defs/adoptionGroupAggregate",
                "#/$defs/bm25CensusAggregate",
                "#/$defs/bm25BoundaryAggregate",
                "#/$defs/bm25LiveBuildAggregate",
            )
        ]
    return selected


def parse(raw: bytes) -> dict[str, Any]:
    """Return validated spans, links, counters, coverage and a precise status."""
    if not raw:
        raise ValueError("empty causal timeline")
    if len(raw) > MAX_CAPTURE_BYTES:
        raise ValueError("causal timeline exceeds offline replay byte bound")
    schema = json.loads(SCHEMA.read_text())
    lines = raw.split(b"\n")
    terminated = lines[-1] == b""
    if terminated:
        lines.pop()
    if not lines or not lines[0]:
        raise ValueError("causal timeline header missing")
    if len(lines[0]) > MAX_LINE_BYTES:
        raise ValueError("causal timeline header exceeds maximum line size")
    try:
        header_probe = json.loads(
            lines[0].decode("utf-8"),
            object_pairs_hook=_unique_pairs,
            parse_constant=_reject_constant,
        )
    except (UnicodeError, json.JSONDecodeError, ValueError) as error:
        raise ValueError(f"invalid causal timeline header: {error}") from error
    version = header_probe.get("schema_version") if isinstance(header_probe, dict) else None
    if type(version) is not int or version not in (1, 2):
        raise ValueError("unsupported causal timeline schema version")
    validator = Draft202012Validator(_schema_for_version(schema, version))
    header = _decode_record(lines[0], validator, 1)
    if header["record_type"] != "header":
        raise ValueError("causal timeline must start with header")
    cap = header["max_record_bytes"]
    records: list[dict[str, Any]] = []
    truncated_tail = False
    for index, line in enumerate(lines[1:], 2):
        if not line:
            raise ValueError(f"blank causal timeline row {index}")
        if len(line) > cap:
            raise ValueError(f"causal timeline row {index} exceeds declared record cap")
        try:
            record = _decode_record(line, validator, index)
        except CausalSyntaxError:
            if index == len(lines) and not terminated:
                if records and records[-1]["record_type"] == "footer":
                    raise ValueError("causal data follows footer") from None
                truncated_tail = True
                break
            raise
        records.append(record)
    if not terminated and not truncated_tail:
        # A fully parseable last record without newline is still a partial write.
        truncated_tail = True
    footer = records[-1] if records and records[-1]["record_type"] == "footer" else None
    events = records[:-1] if footer else records
    if any(row["record_type"] != "event" for row in events):
        raise ValueError("header/footer occurs inside causal event stream")
    capture_id = header["capture_id"]
    spans: dict[int, dict[str, Any]] = {}
    pending_ends: dict[int, dict[str, Any]] = {}
    links: list[dict[str, Any]] = []
    counters: list[dict[str, Any]] = []
    sync_aggregates: list[dict[str, Any]] = []
    object_kind_aggregates: list[dict[str, Any]] = []
    adoption_group_aggregates: list[dict[str, Any]] = []
    bm25_census_aggregates: list[dict[str, Any]] = []
    bm25_boundary_aggregates: list[dict[str, Any]] = []
    bm25_live_build_aggregates: list[dict[str, Any]] = []
    identities: dict[int, dict[str, Any]] = {}
    identity_times: dict[int, int] = {}
    health: list[dict[str, Any]] = []
    phase_names: dict[int, str] = {}
    phase_times: dict[int, int] = {}
    orphan_events = 0
    last_ts = header["started_ns"]
    for seq, event in enumerate(events, 1):
        if event["output_seq"] != seq:
            raise ValueError(f"causal output sequence gap at row {seq}")
        if event["capture_id"] != capture_id:
            raise ValueError("causal event capture ID changed")
        kind = event["kind"]
        event_type = kind["type"]
        if event["ts_ns"] < header["started_ns"]:
            raise ValueError("causal event precedes capture clock origin")
        last_ts = max(last_ts, event["ts_ns"])
        if event_type == "span_begin":
            span_id = kind["span_id"]
            if span_id in spans:
                raise ValueError(f"duplicate span begin: {span_id}")
            spans[span_id] = {
                "span_id": span_id,
                "parent_span_id": kind.get("parent_span_id"),
                "root_kind": kind.get("root_kind"),
                "stage": kind["stage"],
                "lane_id": kind["lane_id"],
                "start_ns": event["ts_ns"],
                "end_ns": None,
                "outcome": None,
                "counters": {},
                "bm25_aggregates": [],
            }
        elif event_type == "span_end":
            span_id = kind["span_id"]
            if span_id in pending_ends:
                raise ValueError(f"duplicate span end: {span_id}")
            pending_ends[span_id] = {"end_ns": event["ts_ns"], "outcome": kind["outcome"]}
        elif event_type == "counter":
            counters.append(
                {
                    "span_id": kind["span_id"],
                    "key": kind["key"],
                    "value": kind["value"],
                    "ts_ns": event["ts_ns"],
                }
            )
            span = spans.get(kind["span_id"])
            if span is None:
                orphan_events += 1
                continue
            key = kind["key"]
            previous = span["counters"].get(key, 0)
            total = previous + kind["value"]
            if total > (1 << 64) - 1:
                raise ValueError("causal counter aggregate exceeds u64")
            span["counters"][key] = total
        elif event_type == "sync_aggregate":
            if any(
                kind[key] > (1 << 64) - 1
                for key in ("calls", "elapsed_sum_ns", "elapsed_max_ns", "failures")
            ):
                raise ValueError("causal sync aggregate exceeds u64")
            if kind["failures"] > kind["calls"] or kind["elapsed_max_ns"] > kind["elapsed_sum_ns"]:
                raise ValueError("causal sync aggregate counts contradict one another")
            if kind["calls"] == 0 and (kind["elapsed_sum_ns"] or kind["elapsed_max_ns"]):
                raise ValueError("causal sync aggregate has elapsed time without calls")
            sync_aggregates.append(
                {
                    "span_id": kind["span_id"],
                    "ts_ns": event["ts_ns"],
                    **{
                        key: kind[key]
                        for key in (
                            "label",
                            "calls",
                            "elapsed_sum_ns",
                            "elapsed_max_ns",
                            "failures",
                        )
                    },
                }
            )
        elif event_type == "object_kind_aggregate":
            if kind["new_objects"] > (1 << 64) - 1 or kind["reused_objects"] > (1 << 64) - 1:
                raise ValueError("causal object kind aggregate exceeds u64")
            object_kind_aggregates.append(
                {
                    "span_id": kind["span_id"],
                    "ts_ns": event["ts_ns"],
                    "kind": kind["kind"],
                    "new_objects": kind["new_objects"],
                    "reused_objects": kind["reused_objects"],
                }
            )
        elif event_type == "adoption_group_aggregate":
            if any(kind[key] > (1 << 64) - 1 for key in ("groups", "failed_groups")):
                raise ValueError("causal adoption group aggregate exceeds u64")
            if kind["failed_groups"] > kind["groups"]:
                raise ValueError("causal adoption group failures exceed attempted groups")
            adoption_group_aggregates.append(
                {
                    "span_id": kind["span_id"],
                    "ts_ns": event["ts_ns"],
                    **{
                        key: kind[key]
                        for key in (
                            "reason",
                            "refs_bucket",
                            "bytes_bucket",
                            "groups",
                            "failed_groups",
                        )
                    },
                }
            )
        elif event_type == "bm25_census_aggregate":
            if kind["calls"] == 0 and any(
                kind[key] for key in ("elapsed_ns", "docs", "encoded_bytes")
            ):
                raise ValueError("BM25 census metrics without calls")
            if kind["docs"] == 0 and kind["encoded_bytes"] != 0:
                raise ValueError("BM25 census bytes without documents")
            bm25_census_aggregates.append(
                {
                    "span_id": kind["span_id"],
                    "ts_ns": event["ts_ns"],
                    **{key: kind[key] for key in ("calls", "elapsed_ns", "docs", "encoded_bytes")},
                }
            )
        elif event_type == "bm25_boundary_aggregate":
            if kind["calls"] == 0 and any(
                kind[key] for key in ("elapsed_sum_ns", "logical_bytes", "index_files")
            ):
                raise ValueError("BM25 boundary metrics without calls")
            bm25_boundary_aggregates.append(
                {
                    "span_id": kind["span_id"],
                    "ts_ns": event["ts_ns"],
                    **{
                        key: kind[key]
                        for key in (
                            "label",
                            "calls",
                            "elapsed_sum_ns",
                            "logical_bytes",
                            "index_files",
                        )
                    },
                }
            )
        elif event_type == "bm25_live_build_aggregate":
            metrics = (
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
                "retained_estimate_bytes",
            )
            if kind["calls"] == 0 and any(kind[key] for key in metrics):
                raise ValueError("BM25 live build metrics without calls")
            fanout = kind["reused_segments"] + kind["changed_segments"] + kind["new_segments"]
            if fanout > U64_MAX or fanout != kind["segment_fanout"]:
                raise ValueError("BM25 live build segment fanout mismatch")
            if kind["newly_dead_docs"] > kind["mask_docs"]:
                raise ValueError("BM25 live build newly dead docs exceed mask docs")
            if kind["death_ns"] > kind["changed_ns"]:
                raise ValueError("BM25 live build death timer exceeds changed timer")
            child_ns = kind["changed_ns"] + kind["new_ns"]
            if child_ns > U64_MAX or child_ns > kind["elapsed_ns"]:
                raise ValueError("BM25 live build child timers exceed elapsed time")
            bm25_live_build_aggregates.append(
                {
                    "span_id": kind["span_id"],
                    "ts_ns": event["ts_ns"],
                    "calls": kind["calls"],
                    **{key: kind[key] for key in metrics},
                }
            )
        elif event_type == "link":
            links.append(
                {
                    "source_span_id": kind["span_id"],
                    "target_capture_id": kind["target_capture_id"],
                    "target_span_id": kind["target_span_id"],
                    "ts_ns": event["ts_ns"],
                }
            )
        elif event_type == "phase_identity":
            span_id = kind["span_id"]
            if span_id in phase_names:
                raise ValueError(f"duplicate phase identity: {span_id}")
            phase_names[span_id] = kind["phase"]
            phase_times[span_id] = event["ts_ns"]
        elif event_type == "request_identity":
            span_id = kind["span_id"]
            if span_id in identities:
                raise ValueError(f"duplicate request identity: {span_id}")
            identities[span_id] = {
                key: kind[key] for key in ("request_id", "plane", "connection_id")
            }
            identity_times[span_id] = event["ts_ns"]
        elif event_type == "health":
            health.append({"ts_ns": event["ts_ns"], **kind})
    for span_id, end in pending_ends.items():
        span = spans.get(span_id)
        if span is None:
            if footer and footer["status"] == "complete":
                raise ValueError(f"complete capture has end without begin: {span_id}")
            continue
        if end["end_ns"] < span["start_ns"]:
            raise ValueError(f"span clock regressed: {span_id}")
        span.update(end)
    for span_id, span in spans.items():
        parent = span["parent_span_id"]
        if parent is None:
            if span["root_kind"] is None:
                raise ValueError(f"root span lacks root kind: {span_id}")
        elif span["root_kind"] is not None:
            raise ValueError(f"child span claims root kind: {span_id}")
        elif parent == span_id:
            raise ValueError(f"self-parent span: {span_id}")
        elif parent in spans and span["end_ns"] is not None and spans[parent]["end_ns"] is not None:
            if (
                span["start_ns"] < spans[parent]["start_ns"]
                or span["end_ns"] > spans[parent]["end_ns"]
            ):
                raise ValueError(f"child span outside parent: {span_id}")
        if span_id in phase_names:
            if span["stage"] != "harness_phase":
                raise ValueError("phase identity targets non-phase span")
            span["phase"] = phase_names[span_id]
    # Missing parents can be a partial capture, but a cycle made entirely of
    # observed spans is contradictory even when the footer is absent.
    parent_state: dict[int, int] = {}
    for span_id in spans:
        trail: list[int] = []
        current: int | None = span_id
        while current is not None and current in spans:
            state = parent_state.get(current)
            if state == 1:
                raise ValueError("causal parent cycle")
            if state == 2:
                break
            parent_state[current] = 1
            trail.append(current)
            current = spans[current]["parent_span_id"]
        for visited in trail:
            parent_state[visited] = 2
    for row in [
        *counters,
        *sync_aggregates,
        *object_kind_aggregates,
        *adoption_group_aggregates,
        *bm25_census_aggregates,
        *bm25_boundary_aggregates,
        *bm25_live_build_aggregates,
        *links,
        *(
            {"span_id": span_id, "ts_ns": ts}
            for span_id, ts in (*phase_times.items(), *identity_times.items())
        ),
    ]:
        span_id = row.get("span_id", row.get("source_span_id"))
        span = spans.get(span_id)
        if span is not None and (
            row["ts_ns"] < span["start_ns"]
            or (span["end_ns"] is not None and row["ts_ns"] > span["end_ns"])
        ):
            raise ValueError(f"causal span metadata outside owner interval: {span_id}")
    orphan_events += sum(
        row["span_id"] not in spans
        for row in (
            *sync_aggregates,
            *object_kind_aggregates,
            *adoption_group_aggregates,
            *bm25_census_aggregates,
            *bm25_boundary_aggregates,
            *bm25_live_build_aggregates,
        )
    )
    for event_type, aggregates in (
        ("bm25_census_aggregate", bm25_census_aggregates),
        ("bm25_boundary_aggregate", bm25_boundary_aggregates),
        ("bm25_live_build_aggregate", bm25_live_build_aggregates),
    ):
        for row in aggregates:
            owner = spans.get(row["span_id"])
            if owner is None:
                continue
            if owner["stage"] != BM25_AGGREGATE_STAGES[event_type]:
                raise ValueError(
                    f"{event_type} targets non-{BM25_AGGREGATE_STAGES[event_type]} span"
                )
            owner["bm25_aggregates"].append({"type": event_type, **row})
    for span_id, identity in identities.items():
        span = spans.get(span_id)
        if span is None:
            if footer and footer["status"] == "complete":
                raise ValueError(f"complete capture has orphan request identity: {span_id}")
            continue
        if span["stage"] != "request" or span["root_kind"] != "request":
            raise ValueError(f"request identity targets non-request root: {span_id}")
        span["request_identity"] = identity
    if footer:
        if footer["status"] == "invalid":
            raise ValueError("producer declared causal capture invalid")
        if footer["capture_id"] != capture_id or footer["last_output_seq"] != len(events):
            raise ValueError("causal footer capture or sequence mismatch")
        if footer["events_written"] != len(events):
            raise ValueError("causal footer written count mismatch")
        if footer["events_attempted"] != footer["events_accepted"] + footer["events_dropped"]:
            raise ValueError("causal footer attempted accounting mismatch")
        if footer["events_accepted"] != footer["events_written"] + footer["events_unwritten"]:
            raise ValueError("causal footer accepted accounting mismatch")
        if footer["closed_ns"] < last_ts:
            raise ValueError("causal footer clock precedes event")
        if footer["open_spans"] != sum(span["end_ns"] is None for span in spans.values()):
            raise ValueError("causal footer open-span accounting mismatch")
        last_ts = footer["closed_ns"]
    complete = bool(footer and footer["status"] == "complete" and not truncated_tail)
    if complete:
        if (
            footer["producer_leases"]
            or footer["events_dropped"]
            or footer["events_unwritten"]
            or footer["open_spans"]
        ):
            raise ValueError("complete causal footer claims loss or live producers")
        if pending_ends.keys() != spans.keys():
            raise ValueError("complete capture has unmatched span boundary")
        if orphan_events:
            raise ValueError("complete capture has orphan work events")
        if any(
            span["parent_span_id"] is not None and span["parent_span_id"] not in spans
            for span in spans.values()
        ):
            raise ValueError("complete capture has missing parent")
        if any(span["stage"] == "harness_phase" and "phase" not in span for span in spans.values()):
            raise ValueError("complete capture has unnamed harness phase")
        if any(span_id not in spans for span_id in phase_names):
            raise ValueError("complete capture has orphan phase identity")
        if any(
            span["stage"] == "request" and span_id not in identities
            for span_id, span in spans.items()
        ):
            raise ValueError("complete capture has request root without identity")
        if any(link["source_span_id"] not in spans for link in links):
            raise ValueError("complete capture has orphan link source")
        if any(
            link["target_capture_id"] == capture_id and link["target_span_id"] not in spans
            for link in links
        ):
            raise ValueError("complete capture has orphan local link target")
        if health:
            raise ValueError("complete capture contradicts loss or invalidity health")
    candidate_links = [
        link
        for link in links
        if link["target_capture_id"] == capture_id
        and link["source_span_id"] in spans
        and link["target_span_id"] in spans
        and spans[link["source_span_id"]]["stage"] == "request"
        and spans[link["target_span_id"]]["stage"] == "client_attempt"
        and link["source_span_id"] in identities
    ]
    source_counts = Counter(link["source_span_id"] for link in candidate_links)
    target_counts = Counter(link["target_span_id"] for link in candidate_links)
    verified_flows = [
        link
        for link in candidate_links
        if source_counts[link["source_span_id"]] == 1
        and target_counts[link["target_span_id"]] == 1
        and spans[link["target_span_id"]]["start_ns"]
        <= spans[link["source_span_id"]]["start_ns"]
        <= link["ts_ns"]
        and (
            spans[link["target_span_id"]]["end_ns"] is None
            or spans[link["source_span_id"]]["start_ns"] <= spans[link["target_span_id"]]["end_ns"]
        )
    ]
    return {
        "status": "complete" if complete else "partial",
        "schema_version": version,
        "capture_id": capture_id,
        "clock_domain": header["clock_domain"],
        "stream_id": header["stream_id"],
        "sha256": hashlib.sha256(raw).hexdigest(),
        "header": header,
        "footer": footer,
        "spans": spans,
        "links": links,
        "verified_flows": verified_flows,
        "ambiguous_links": len(candidate_links) - len(verified_flows),
        "counters": counters,
        "sync_aggregates": sync_aggregates,
        "object_kind_aggregates": object_kind_aggregates,
        "adoption_group_aggregates": adoption_group_aggregates,
        "bm25_census_aggregates": bm25_census_aggregates,
        "bm25_boundary_aggregates": bm25_boundary_aggregates,
        "bm25_live_build_aggregates": bm25_live_build_aggregates,
        "health": health,
        "request_identities": identities,
        "event_count": len(events),
        "last_observed_ns": last_ts,
        "truncated_tail": truncated_tail,
        "orphan_events": orphan_events,
    }


def _union_ns(intervals: list[tuple[int, int]]) -> int:
    total = 0
    cursor = None
    for start, end in sorted(intervals):
        if cursor is None or start >= cursor:
            total += end - start
            cursor = end
        elif end > cursor:
            total += end - cursor
            cursor = end
    return total


def summarize(
    capture: dict[str, Any],
    legacy_profile: dict[str, Any] | None = None,
    *,
    execution_binding: dict[str, Any] | None = None,
) -> dict[str, Any]:
    spans = capture["spans"]
    rows = []
    children_by_parent: dict[int, list[tuple[int, int]]] = {}
    for child in spans.values():
        if child["parent_span_id"] is not None and child["end_ns"] is not None:
            children_by_parent.setdefault(child["parent_span_id"], []).append(
                (child["start_ns"], child["end_ns"])
            )
    for span_id, span in sorted(spans.items()):
        end = span["end_ns"]
        duration = None if end is None else end - span["start_ns"]
        exclusive = (
            None if duration is None else duration - _union_ns(children_by_parent.get(span_id, []))
        )
        if exclusive is not None and exclusive < 0:
            raise ValueError(f"children exceed parent span: {span_id}")
        rows.append(
            {
                "span_id": span_id,
                "parent_span_id": span["parent_span_id"],
                "stage": span["stage"],
                "phase": span.get("phase"),
                "lane_id": span["lane_id"],
                "start_ns": span["start_ns"],
                "end_ns": end,
                "last_observed_ns": capture["last_observed_ns"] if end is None else None,
                "duration_ns": duration,
                "exclusive_ns": exclusive,
                "outcome": span["outcome"],
                "counters": span["counters"],
                "bm25_aggregates": [aggregate.copy() for aggregate in span["bm25_aggregates"]],
                "request_identity": span.get("request_identity", {}).copy() or None,
            }
        )
    summary = {
        "schema_version": capture["schema_version"],
        "status": capture["status"],
        "capture_id": capture["capture_id"],
        "stream_sha256": capture["sha256"],
        "event_count": capture["event_count"],
        "last_observed_ns": capture["last_observed_ns"],
        "truncated_tail": capture["truncated_tail"],
        "orphan_events": capture["orphan_events"],
        "spans": rows,
        "sync_aggregates": [row.copy() for row in capture["sync_aggregates"]],
        "object_kind_aggregates": [row.copy() for row in capture["object_kind_aggregates"]],
        "adoption_group_aggregates": [row.copy() for row in capture["adoption_group_aggregates"]],
        "bm25_census_aggregates": [row.copy() for row in capture["bm25_census_aggregates"]],
        "bm25_boundary_aggregates": [row.copy() for row in capture["bm25_boundary_aggregates"]],
        "bm25_live_build_aggregates": [row.copy() for row in capture["bm25_live_build_aggregates"]],
        "links": [link.copy() for link in capture["links"]],
        "verified_flow_count": len(capture["verified_flows"]),
        "ambiguous_link_count": capture["ambiguous_links"],
        "health": capture["health"],
        "loss": None
        if capture["footer"] is None
        else {
            key: capture["footer"][key]
            for key in ("events_dropped", "events_unwritten", "open_spans")
        },
    }
    if legacy_profile is not None:
        summary["legacy_reconciliation"] = reconcile_legacy(
            capture, legacy_profile, execution_binding=execution_binding
        )
    return summary


def _verify_legacy_binding(
    capture: dict[str, Any],
    legacy_profile: dict[str, Any],
    execution_binding: dict[str, Any] | None,
) -> None:
    if not isinstance(execution_binding, dict):
        raise ValueError("legacy reconciliation requires an explicit execution binding")
    for field, width in EXECUTION_BINDING_WIDTHS.items():
        expected = execution_binding.get(field)
        observed = legacy_profile.get(field)
        if (
            not isinstance(expected, str)
            or len(expected) != width
            or any(character not in "0123456789abcdef" for character in expected)
            or observed != expected
        ):
            raise ValueError(f"legacy reconciliation {field} binding is missing or mismatched")
    if (
        execution_binding["timeline_sha256"] != capture["sha256"]
        or execution_binding["timeline_capture_id"] != capture["capture_id"]
        or legacy_profile.get("trace_sha256") != execution_binding["stderr_sha256"]
    ):
        raise ValueError("legacy reconciliation capture or stderr binding mismatch")


def reconcile_legacy(
    capture: dict[str, Any],
    legacy_profile: dict[str, Any],
    *,
    execution_binding: dict[str, Any] | None = None,
) -> dict[str, Any]:
    """Compare bound phase, sync and BM25 fields with equivalent owner scopes.

    Legacy markers carry no monotonic timestamps or parent IDs. Residual values
    therefore stay as source-labelled metadata, never Chrome timeline events.
    """
    _verify_legacy_binding(capture, legacy_profile, execution_binding)
    phases = legacy_profile.get("phases")
    if legacy_profile.get("status") != "diagnostic_unqualified" or not isinstance(phases, dict):
        raise ValueError("legacy profile is not a validated diagnostic replay")
    spans = capture["spans"]
    observed_phases: dict[str, dict[str, Any]] = {}
    duplicate_phase = False
    unnamed_phase = False
    for span in spans.values():
        if span["stage"] != "harness_phase":
            continue
        name = span.get("phase")
        if name is None:
            unnamed_phase = True
        elif name in observed_phases:
            duplicate_phase = True
        else:
            observed_phases[name] = span
    legacy_outcomes = {name: phase["status"] for name, phase in phases.items()}
    observed_outcomes = {name: span["outcome"] for name, span in observed_phases.items()}
    if (
        duplicate_phase
        or set(observed_phases) - set(phases)
        or any(
            observed_outcomes[name] is not None and observed_outcomes[name] != legacy_outcomes[name]
            for name in set(observed_phases) & set(phases)
        )
    ):
        phase_parity = "mismatch"
    elif (
        capture["status"] != "complete"
        or unnamed_phase
        or set(observed_phases) != set(phases)
        or any(outcome is None for outcome in observed_outcomes.values())
    ):
        phase_parity = "unsupported"
    else:
        phase_parity = "verified"

    def owner_phase(span_id: int) -> str | None:
        current = span_id
        while current in spans:
            span = spans[current]
            if span["stage"] == "harness_phase":
                return span.get("phase")
            current = span["parent_span_id"]
            if current is None:
                break
        return None

    canonical_sync: dict[tuple[str, str], dict[str, int]] = {}
    for aggregate in capture["sync_aggregates"]:
        phase = owner_phase(aggregate["span_id"])
        if phase is None:
            continue
        key = (phase, aggregate["label"])
        value = canonical_sync.setdefault(key, {"calls": 0, "elapsed_sum_ns": 0, "failures": 0})
        for field in value:
            total = value[field] + aggregate[field]
            if total > (1 << 64) - 1:
                raise ValueError("canonical sync phase aggregate exceeds u64")
            value[field] = total

    legacy_sync = {
        (name, label): {
            "calls": counters["calls"],
            "elapsed_sum_ns": counters["elapsed_ns"],
            "failures": counters["failed_calls"],
        }
        for name, phase in phases.items()
        for label, counters in phase["sync"].items()
    }
    common = sorted(legacy_sync.keys() & canonical_sync.keys())

    # timed_sync_marker() wraps observed_sync(): call/failure counts cover the
    # same operation, while legacy elapsed also includes canonical aggregation
    # bookkeeping. Neither timer includes stderr formatting/output time.
    def matching_counts(key: tuple[str, str]) -> bool:
        return all(
            legacy_sync[key][field] == canonical_sync[key][field] for field in ("calls", "failures")
        )

    comparisons = [
        {
            "phase": phase,
            "label": label,
            "legacy": legacy_sync[(phase, label)],
            "canonical": canonical_sync[(phase, label)],
            "counts_status": "verified" if matching_counts((phase, label)) else "mismatch",
            "elapsed_status": "unsupported_different_timer_envelopes",
        }
        for phase, label in common
    ]
    if any(row["counts_status"] == "mismatch" for row in comparisons):
        sync_common_parity = "mismatch"
    elif common and capture["status"] == "complete" and phase_parity == "verified":
        sync_common_parity = "verified"
    else:
        sync_common_parity = "unsupported"

    bm25_fields = {
        "bm25_census_build": ("calls", "docs", "encoded_bytes"),
        "bm25_boundary": ("calls", "logical_bytes", "index_files"),
        "bm25_live_build": (
            "calls",
            "reused_segments",
            "changed_segments",
            "new_segments",
            "mask_docs",
            "new_census_docs",
            "newly_dead_docs",
            "logical_census_bytes",
            "correction_keys",
            "segment_fanout",
            "retained_estimate_bytes",
        ),
    }
    bm25_timer_fields = {
        "bm25_census_build": ("elapsed_ns",),
        "bm25_boundary": ("elapsed_ns",),
        "bm25_live_build": ("elapsed_ns", "changed_ns", "new_ns", "death_ns"),
    }
    bm25_sources = (
        ("bm25_census_build", capture["bm25_census_aggregates"]),
        ("bm25_boundary", capture["bm25_boundary_aggregates"]),
        ("bm25_live_build", capture["bm25_live_build_aggregates"]),
    )
    canonical_bm25: dict[tuple[str, str, str | None], list[dict[str, Any]]] = {}
    for kind, aggregates in bm25_sources:
        for aggregate in aggregates:
            phase = owner_phase(aggregate["span_id"])
            if phase is None:
                continue
            key = (phase, kind, aggregate["label"] if kind == "bm25_boundary" else None)
            canonical_bm25.setdefault(key, []).append(aggregate)

    legacy_bm25: dict[tuple[str, str, str | None], list[dict[str, Any]]] = {}
    for name, phase in phases.items():
        census = phase["bm25_census_build"]
        if census["calls"]:
            legacy_bm25[(name, "bm25_census_build", None)] = [census]
        for label, value in phase["bm25_boundary"].items():
            legacy_bm25[(name, "bm25_boundary", label)] = [value]
        if phase["bm25_live_build"]:
            legacy_bm25[(name, "bm25_live_build", None)] = [
                {"calls": 1, **value} for value in phase["bm25_live_build"]
            ]

    def bm25_totals(kind: str, records: list[dict[str, Any]]) -> dict[str, int]:
        totals = dict.fromkeys(bm25_fields[kind], 0)
        for record in records:
            for field in totals:
                next_value = totals[field] + record[field]
                if next_value > U64_MAX:
                    raise ValueError(f"BM25 {kind} phase {field} aggregate exceeds u64")
                totals[field] = next_value
        return totals

    bm25_comparisons = []
    for key in sorted(legacy_bm25.keys() & canonical_bm25.keys()):
        phase, kind, label = key
        canonical_records = canonical_bm25[key]
        legacy_records = legacy_bm25[key]
        canonical = bm25_totals(kind, canonical_records)
        legacy = bm25_totals(kind, legacy_records)
        calls_match = canonical["calls"] == legacy["calls"]
        mismatch_fields = (
            ["calls"]
            if not calls_match
            else [field for field in bm25_fields[kind] if canonical[field] != legacy[field]]
        )
        owner_outcomes = {spans[row["span_id"]]["outcome"] for row in canonical_records}
        # Legacy live-build markers are one record per successful build. Do not
        # infer per-call values from a canonical event that represents many calls.
        live_calls_are_atomic = kind != "bm25_live_build" or all(
            row["calls"] == 1 for row in canonical_records
        )
        qualified_call_envelope = owner_outcomes == {"ok"} and live_calls_are_atomic
        live_multiset_mismatch = (
            kind == "bm25_live_build"
            and live_calls_are_atomic
            and calls_match
            and (
                Counter(
                    tuple(row[field] for field in bm25_fields[kind]) for row in canonical_records
                )
                != Counter(
                    tuple(row[field] for field in bm25_fields[kind]) for row in legacy_records
                )
            )
        )
        if not qualified_call_envelope:
            fields_status = "unsupported_owner_or_call_envelope"
            calls_status = fields_status
            sizes_status = fields_status
        elif not calls_match:
            fields_status = "mismatch"
            calls_status = "mismatch"
            sizes_status = "unsupported_call_count_mismatch"
        elif mismatch_fields or live_multiset_mismatch:
            fields_status = "mismatch"
            calls_status = "verified"
            sizes_status = "mismatch"
        else:
            fields_status = "verified"
            calls_status = "verified"
            sizes_status = "verified"
        if kind != "bm25_live_build":
            live_call_multiset_status = None
        elif not qualified_call_envelope:
            live_call_multiset_status = "unsupported_owner_or_call_envelope"
        elif not calls_match:
            live_call_multiset_status = "unsupported_call_count_mismatch"
        else:
            live_call_multiset_status = "mismatch" if live_multiset_mismatch else "verified"
        bm25_comparisons.append(
            {
                "phase": phase,
                "kind": kind,
                "label": label,
                "legacy": legacy,
                "canonical": canonical,
                "fields_status": fields_status,
                "calls_status": calls_status,
                "sizes_status": sizes_status,
                "mismatch_fields": mismatch_fields,
                "live_call_multiset_status": live_call_multiset_status,
                "elapsed_status": "unsupported_timing_comparison",
            }
        )
    if any(row["fields_status"] == "mismatch" for row in bm25_comparisons):
        bm25_common_parity = "mismatch"
    elif (
        bm25_comparisons
        and capture["status"] == "complete"
        and phase_parity == "verified"
        and all(row["fields_status"] == "verified" for row in bm25_comparisons)
    ):
        bm25_common_parity = "verified"
    else:
        bm25_common_parity = "unsupported"

    residual: list[dict[str, Any]] = []
    for (phase, label), value in sorted(legacy_sync.items()):
        if (phase, label) not in canonical_sync or not matching_counts((phase, label)):
            residual.append(
                {
                    "source": "stderr QI_CAUSAL_V1 markers",
                    "phase": phase,
                    "kind": "sync",
                    "label": label,
                    "metrics": value.copy(),
                    "reason": "canonical_count_mismatch"
                    if (phase, label) in canonical_sync
                    else "no_equivalent_canonical_phase_label",
                }
            )
        else:
            residual.append(
                {
                    "source": "stderr QI_CAUSAL_V1 markers",
                    "phase": phase,
                    "kind": "sync_elapsed_ns",
                    "label": label,
                    "metrics": {"elapsed_ns": value["elapsed_sum_ns"]},
                    "reason": "different_timer_envelopes",
                }
            )
    for name, phase in sorted(phases.items()):
        scan = phase["exact_live_token_scan"]
        if scan["calls"]:
            residual.append(
                {
                    "source": "stderr QI_CAUSAL_V1 markers",
                    "phase": name,
                    "kind": "exact_live_token_scan",
                    "metrics": scan.copy(),
                    "reason": "no_semantically_equivalent_canonical_metric",
                }
            )
        residual.append(
            {
                "source": "summary.json phase_resources",
                "phase": name,
                "kind": "process_write_io",
                "metrics": phase["process_write_io"],
                "unavailable_reason": phase["process_write_io_unavailable_reason"],
                "reason": "no_semantically_equivalent_canonical_metric",
            }
        )
    comparisons_by_key = {
        (row["phase"], row["kind"], row["label"]): row for row in bm25_comparisons
    }
    for key, records in sorted(legacy_bm25.items()):
        phase, kind, label = key
        comparison = comparisons_by_key.get(key)
        if comparison is None or comparison["fields_status"] != "verified":
            residual.append(
                {
                    "source": "stderr QI_CAUSAL_V1 markers",
                    "phase": phase,
                    "kind": kind,
                    "label": label,
                    "metrics": [row.copy() for row in records],
                    "reason": "canonical_field_mismatch"
                    if comparison is not None and comparison["fields_status"] == "mismatch"
                    else "canonical_call_envelope_unproven"
                    if comparison is not None
                    else "no_equivalent_canonical_phase_label",
                }
            )
        else:
            residual.append(
                {
                    "source": "stderr QI_CAUSAL_V1 markers",
                    "phase": phase,
                    "kind": kind,
                    "label": label,
                    "metrics": [
                        {field: row[field] for field in bm25_timer_fields[kind]} for row in records
                    ],
                    "reason": "timing_comparison_unproven",
                }
            )
    return {
        "status": "partial_unqualified",
        "phase_parity": phase_parity,
        "sync_common_parity": sync_common_parity,
        "sync_comparisons": comparisons,
        "bm25_common_parity": bm25_common_parity,
        "bm25_comparisons": bm25_comparisons,
        "canonical_sync_without_legacy_phase_label": [
            {"phase": phase, "label": label, "metrics": canonical_sync[(phase, label)]}
            for phase, label in sorted(canonical_sync.keys() - legacy_sync.keys())
        ],
        "canonical_bm25_without_legacy_phase_label": [
            {
                "phase": phase,
                "kind": kind,
                "label": label,
                "metrics": bm25_totals(kind, canonical_bm25[key]),
            }
            for key in sorted(canonical_bm25.keys() - legacy_bm25.keys())
            for phase, kind, label in [key]
        ],
        "legacy_residual": residual,
    }


def chrome_export(
    capture: dict[str, Any],
    legacy_profile: dict[str, Any] | None = None,
    *,
    execution_binding: dict[str, Any] | None = None,
) -> dict[str, Any]:
    """Render only proven same-clock links as flows; preserve exact IDs as strings."""
    events: list[dict[str, Any]] = []
    process_id = int(capture["header"]["process_instance"][:8], 16)
    if capture["last_observed_ns"] > JS_SAFE_INTEGER or any(
        span["start_ns"] > JS_SAFE_INTEGER
        or (span["end_ns"] is not None and span["end_ns"] > JS_SAFE_INTEGER)
        for span in capture["spans"].values()
    ):
        raise ValueError("causal timestamp exceeds exact Chrome JSON range")
    # Chrome nesting implies parentage. Overlapping unrelated spans need separate tracks.
    tracks: dict[int, list[list[tuple[int, int]]]] = {}
    track_for_span: dict[int, str] = {}
    final_ns = capture["last_observed_ns"]
    for span in sorted(
        capture["spans"].values(), key=lambda row: (row["start_ns"], row["span_id"])
    ):
        interval = (span["start_ns"], final_ns if span["end_ns"] is None else span["end_ns"])
        lane_tracks = tracks.setdefault(span["lane_id"], [])
        index = 0
        for active_ends in lane_tracks:
            while active_ends and active_ends[0][0] <= interval[0]:
                heapq.heappop(active_ends)
            if not active_ends or (
                span["parent_span_id"] == active_ends[0][1] and active_ends[0][0] >= interval[1]
            ):
                heapq.heappush(active_ends, (interval[1], span["span_id"]))
                break
            index += 1
        else:
            index = len(lane_tracks)
            lane_tracks.append([(interval[1], span["span_id"])])
        track_for_span[span["span_id"]] = f"{span['lane_id']}:{index}"
    for span in capture["spans"].values():
        ts = span["start_ns"] / 1000
        name = span.get("phase") or span["stage"]
        args = {
            "span_id": str(span["span_id"]),
            "parent_span_id": None
            if span["parent_span_id"] is None
            else str(span["parent_span_id"]),
            "outcome": span["outcome"],
            "last_observed_ns": str(final_ns) if span["end_ns"] is None else None,
            "display_only_observed_extent": span["end_ns"] is None,
            "counters": {key: str(value) for key, value in span["counters"].items()},
            "request_identity": None
            if "request_identity" not in span
            else {
                **span["request_identity"],
                "request_id": str(span["request_identity"]["request_id"]),
                "connection_id": str(span["request_identity"]["connection_id"]),
            },
        }
        if span["end_ns"] is None:
            events.append(
                {
                    "name": name,
                    "cat": "unclosed_observed_extent",
                    "ph": "X",
                    "pid": process_id,
                    "tid": track_for_span[span["span_id"]],
                    "ts": ts,
                    "dur": (final_ns - span["start_ns"]) / 1000,
                    "args": args,
                }
            )
        else:
            events.append(
                {
                    "name": name,
                    "cat": span["stage"],
                    "ph": "X",
                    "pid": process_id,
                    "tid": track_for_span[span["span_id"]],
                    "ts": ts,
                    "dur": (span["end_ns"] - span["start_ns"]) / 1000,
                    "args": args,
                }
            )
            if span["outcome"] != "ok":
                events.append(
                    {
                        "name": f"{name}:{span['outcome']}",
                        "cat": "terminal",
                        "ph": "i",
                        "s": "t",
                        "pid": process_id,
                        "tid": track_for_span[span["span_id"]],
                        "ts": span["end_ns"] / 1000,
                        "args": args,
                    }
                )
    for counter in capture["counters"]:
        if counter["span_id"] not in track_for_span:
            continue
        # Chrome counters are JS numbers. Large exact values remain string metadata.
        if counter["value"] <= JS_SAFE_INTEGER:
            events.append(
                {
                    "name": counter["key"],
                    "cat": "work",
                    "ph": "C",
                    "pid": process_id,
                    "tid": track_for_span[counter["span_id"]],
                    "ts": counter["ts_ns"] / 1000,
                    "args": {counter["key"]: counter["value"]},
                }
            )
        else:
            events.append(
                {
                    "name": counter["key"],
                    "cat": "work_exact",
                    "ph": "i",
                    "s": "t",
                    "pid": process_id,
                    "tid": track_for_span[counter["span_id"]],
                    "ts": counter["ts_ns"] / 1000,
                    "args": {
                        "value_u64": str(counter["value"]),
                        "numeric_counter_unavailable": "exceeds_js_safe_integer",
                    },
                }
            )
    for aggregate in capture["sync_aggregates"]:
        span_id = aggregate["span_id"]
        if span_id not in track_for_span:
            continue
        if aggregate["ts_ns"] > JS_SAFE_INTEGER:
            raise ValueError("causal timestamp exceeds exact Chrome JSON range")
        events.append(
            {
                "name": f"sync:{aggregate['label']}",
                "cat": "work_aggregate",
                "ph": "i",
                "s": "t",
                "pid": process_id,
                "tid": track_for_span[span_id],
                "ts": aggregate["ts_ns"] / 1000,
                "args": {
                    "span_id": str(span_id),
                    "calls": str(aggregate["calls"]),
                    "elapsed_sum_ns": str(aggregate["elapsed_sum_ns"]),
                    "elapsed_max_ns": str(aggregate["elapsed_max_ns"]),
                    "failures": str(aggregate["failures"]),
                },
            }
        )
    for aggregate in capture["object_kind_aggregates"]:
        span_id = aggregate["span_id"]
        if span_id not in track_for_span:
            continue
        if aggregate["ts_ns"] > JS_SAFE_INTEGER:
            raise ValueError("causal timestamp exceeds exact Chrome JSON range")
        events.append(
            {
                "name": f"object_kind:{aggregate['kind']}",
                "cat": "work_aggregate",
                "ph": "i",
                "s": "t",
                "pid": process_id,
                "tid": track_for_span[span_id],
                "ts": aggregate["ts_ns"] / 1000,
                "args": {
                    "span_id": str(span_id),
                    "new_objects": str(aggregate["new_objects"]),
                    "reused_objects": str(aggregate["reused_objects"]),
                },
            }
        )
    for aggregate in capture["adoption_group_aggregates"]:
        span_id = aggregate["span_id"]
        if span_id not in track_for_span:
            continue
        if aggregate["ts_ns"] > JS_SAFE_INTEGER:
            raise ValueError("causal timestamp exceeds exact Chrome JSON range")
        events.append(
            {
                "name": f"adoption_group:{aggregate['reason']}",
                "cat": "work_aggregate",
                "ph": "i",
                "s": "t",
                "pid": process_id,
                "tid": track_for_span[span_id],
                "ts": aggregate["ts_ns"] / 1000,
                "args": {
                    "span_id": str(span_id),
                    "refs_bucket": aggregate["refs_bucket"],
                    "bytes_bucket": aggregate["bytes_bucket"],
                    "groups": str(aggregate["groups"]),
                    "failed_groups": str(aggregate["failed_groups"]),
                },
            }
        )
    for event_type, aggregates in (
        ("bm25_census_aggregate", capture["bm25_census_aggregates"]),
        ("bm25_boundary_aggregate", capture["bm25_boundary_aggregates"]),
        ("bm25_live_build_aggregate", capture["bm25_live_build_aggregates"]),
    ):
        for aggregate in aggregates:
            span_id = aggregate["span_id"]
            if span_id not in track_for_span:
                continue
            if aggregate["ts_ns"] > JS_SAFE_INTEGER:
                raise ValueError("causal timestamp exceeds exact Chrome JSON range")
            events.append(
                {
                    "name": event_type,
                    "cat": "work_aggregate",
                    "ph": "i",
                    "s": "t",
                    "pid": process_id,
                    "tid": track_for_span[span_id],
                    "ts": aggregate["ts_ns"] / 1000,
                    "args": {
                        key: str(value) if type(value) is int else value
                        for key, value in aggregate.items()
                        if key != "ts_ns"
                    },
                }
            )
    proven = {id(link) for link in capture["verified_flows"]}
    for link in capture["links"]:
        source = capture["spans"].get(link["source_span_id"])
        if source is None:
            continue
        target = capture["spans"].get(link["target_span_id"])
        if id(link) in proven and target is not None:
            flow_id = f"{link['source_span_id']}:{link['target_span_id']}:{link['ts_ns']}"
            events.extend(
                [
                    {
                        "name": "explicit_link",
                        "cat": "causal",
                        "ph": "s",
                        "id": flow_id,
                        "pid": process_id,
                        "tid": track_for_span[target["span_id"]],
                        "ts": target["start_ns"] / 1000,
                    },
                    {
                        "name": "explicit_link",
                        "cat": "causal",
                        "ph": "f",
                        "id": flow_id,
                        "pid": process_id,
                        "tid": track_for_span[source["span_id"]],
                        "ts": source["start_ns"] / 1000,
                    },
                ]
            )
            continue
        events.append(
            {
                "name": "explicit_link",
                "cat": "causal",
                "ph": "i",
                "s": "t",
                "pid": process_id,
                "tid": track_for_span[source["span_id"]],
                "ts": link["ts_ns"] / 1000,
                "args": {
                    **link,
                    "source_span_id": str(link["source_span_id"]),
                    "target_span_id": str(link["target_span_id"]),
                    "verified_flow": False,
                },
            }
        )
    for row in capture["health"]:
        events.append(
            {
                "name": row["code"],
                "cat": "trace_health",
                "ph": "i",
                "s": "g",
                "pid": process_id,
                "tid": "trace_health",
                "ts": row["ts_ns"] / 1000,
                "args": {key: value for key, value in row.items() if key != "ts_ns"},
            }
        )
    summary = summarize(capture, legacy_profile, execution_binding=execution_binding)
    for row in summary["spans"]:
        row["span_id"] = str(row["span_id"])
        if row["parent_span_id"] is not None:
            row["parent_span_id"] = str(row["parent_span_id"])
        row["counters"] = {key: str(value) for key, value in row["counters"].items()}
        row["bm25_aggregates"] = [
            {key: str(value) if type(value) is int else value for key, value in aggregate.items()}
            for aggregate in row["bm25_aggregates"]
        ]
        if row["request_identity"] is not None:
            row["request_identity"]["request_id"] = str(row["request_identity"]["request_id"])
            row["request_identity"]["connection_id"] = str(row["request_identity"]["connection_id"])
    for link in summary["links"]:
        link["source_span_id"] = str(link["source_span_id"])
        link["target_span_id"] = str(link["target_span_id"])
    for row in (
        *summary["sync_aggregates"],
        *summary["object_kind_aggregates"],
        *summary["adoption_group_aggregates"],
    ):
        row["span_id"] = str(row["span_id"])
        for key in (
            "calls",
            "elapsed_sum_ns",
            "elapsed_max_ns",
            "failures",
            "groups",
            "failed_groups",
            "new_objects",
            "reused_objects",
        ):
            if key in row:
                row[key] = str(row[key])
    for aggregates in (
        summary["bm25_census_aggregates"],
        summary["bm25_boundary_aggregates"],
        summary["bm25_live_build_aggregates"],
    ):
        for row in aggregates:
            row.update({key: str(value) for key, value in row.items() if type(value) is int})
    return {
        "traceEvents": events,
        "displayTimeUnit": "ns",
        "quantaIndexSummary": summary,
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--input", type=Path, required=True)
    parser.add_argument("--out", type=Path, required=True)
    args = parser.parse_args()
    result = chrome_export(parse(args.input.read_bytes()))
    with args.out.open("xb") as stream:
        stream.write(
            json.dumps(result, sort_keys=True, separators=(",", ":"), allow_nan=False).encode()
            + b"\n"
        )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
