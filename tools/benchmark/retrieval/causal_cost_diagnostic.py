"""Reusable, unqualified worker RCA index over one canonical Native capture.

Build once from ``causal_cost_timeline.parse_stream`` output, then query the
small JSON projection without decoding the qtrace again. Independent daemon
and harness clocks are never aligned. This does not prove public Completed,
source/binary custody, or Off/Record performance equivalence.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import sys
from collections import defaultdict
from pathlib import Path
from typing import Any

from tools.benchmark.retrieval.causal_cost_timeline import (
    MAX_CAPTURE_BYTES,
    _union_ns,
    parse_stream,
)

EVENT = "native_owned_work_refusal"
TRACE_ID = re.compile(r"[0-9a-f]{32}\Z")
PHASE_STAGES = frozenset(
    {"build_prepare", "finalize", "semantic_prepare", "native_coverage_save", "native_publish"}
)
WRITE_KEYS = (
    "sqlite_write_calls",
    "sqlite_write_requested_bytes",
    "sqlite_write_failures",
    "sqlite_wal_write_calls",
    "sqlite_main_write_calls",
    "sqlite_other_write_calls",
)
RECORD_KEYS = frozenset(
    {
        "event",
        "operation_key",
        "budget_diagnostic",
        "execution",
        "phase",
        "outcome",
        "sample_ordinal",
        "sample_limit",
    }
)
EXECUTION_KEYS = frozenset(
    {"dispatch_ordinal", "assignment_dispatches", "assignment_deadline_ms", "claim", "trace"}
)
MAX_RECORD_BYTES = 256 * 1024
MAX_STDERR_BYTES = 16 * 1024 * 1024
MAX_INDEX_BYTES = 128 * 1024 * 1024
SCOPE = {
    "time": "inclusive per-group unions; groups can overlap and are not additive CPU",
    "phase": "nearest daemon worker descendant phase, not cross-process harness phase",
    "sqlite": "lane11/12 sync includes VFS witness; lane22 bytes are requested, not successful or device bytes",
    "outcome": "worker trace terminal is not public WorkStatus Completed",
    "qualification": "no source/binary custody or Off/Record timing qualification",
}


def _natural(value: Any, label: str, *, positive: bool = False) -> int:
    if type(value) is not int or value < (1 if positive else 0):
        raise ValueError(f"{label} must be a {'positive' if positive else 'nonnegative'} integer")
    return value


def _complete(capture: dict[str, Any]) -> tuple[str, dict[int, dict[str, Any]]]:
    if not isinstance(capture, dict) or capture.get("status") != "complete":
        raise ValueError("worker index requires a complete canonical capture")
    capture_id = capture.get("capture_id")
    digest = capture.get("sha256")
    footer = capture.get("footer")
    if (
        not isinstance(capture_id, str)
        or not TRACE_ID.fullmatch(capture_id)
        or not isinstance(digest, str)
        or not re.fullmatch(r"[0-9a-f]{64}", digest)
        or capture.get("schema_version") != 4
        or not isinstance(footer, dict)
        or footer.get("capture_id") != capture_id
        or footer.get("status") != "complete"
        or any(
            footer.get(key) != 0
            for key in ("events_dropped", "events_unwritten", "open_spans", "producer_leases")
        )
        or capture.get("health") != []
        or capture.get("truncated_tail") is not False
        or capture.get("orphan_events") != 0
    ):
        raise ValueError("worker index requires healthy, lossless capture custody")
    spans = capture.get("spans")
    if not isinstance(spans, dict) or any(type(key) is not int for key in spans):
        raise ValueError("worker index requires ID-indexed canonical spans")
    return capture_id, spans


def _worker(spans: dict[int, dict[str, Any]], span_id: int) -> bool:
    span = spans[span_id]
    parent_id = span.get("parent_span_id")
    parent = spans.get(parent_id)
    return (
        span.get("stage") == "operation"
        and span.get("lane_id") == 1
        and span.get("root_kind") is None
        and isinstance(parent, dict)
        and parent.get("stage") == "operation"
        and parent.get("lane_id") == 0
        and parent.get("root_kind") == "operation"
        and parent.get("parent_span_id") is None
    )


def _unique(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    result = {}
    for key, value in pairs:
        if key in result:
            raise ValueError(f"duplicate refusal key: {key}")
        result[key] = value
    return result


def _reject_constant(value: str) -> None:
    raise ValueError(f"nonfinite refusal constant: {value}")


def _json(raw: str, label: str) -> Any:
    if len(raw.encode("utf-8")) > MAX_RECORD_BYTES:
        raise ValueError(f"{label} exceeds bounded record size")
    try:
        return json.loads(raw, object_pairs_hook=_unique, parse_constant=_reject_constant)
    except json.JSONDecodeError as error:
        raise ValueError(f"malformed {label}: {error}") from error


def _records(raw_log: str) -> list[dict[str, Any]]:
    if not isinstance(raw_log, str) or len(raw_log.encode("utf-8")) > MAX_STDERR_BYTES:
        raise ValueError("refusal log exceeds diagnostic input bound")
    found = []

    def append(raw: str) -> None:
        value = _json(raw, "owned refusal")
        if not isinstance(value, dict) or set(value) != RECORD_KEYS or value["event"] != EVENT:
            raise ValueError("owned refusal has malformed fields")
        found.append(value)

    for line in raw_log.splitlines():
        if EVENT not in line:
            continue
        if line.startswith("native-ann-diagnostic "):
            prefix, separator, raw = line.partition(" record=")
            if not separator or not prefix.startswith("native-ann-diagnostic phase="):
                raise ValueError("malformed relayed refusal")
            append(raw)
        elif line.startswith("Error: "):
            message = _json(line[len("Error: ") :], "libtest Error string")
            if not isinstance(message, str):
                raise ValueError("libtest Error refusal is not a string")
            nested = [row for row in message.splitlines() if EVENT in row]
            if not nested or any(not row.startswith("{") for row in nested):
                raise ValueError("libtest Error has malformed embedded refusal")
            for row in nested:
                append(row)
        elif line.startswith("{"):
            append(line)
        else:
            raise ValueError("unrecognized refusal carrier")
    return found


def _validate_record(record: dict[str, Any]) -> None:
    if not isinstance(record["operation_key"], dict) or not isinstance(record["outcome"], str):
        raise ValueError("refusal lacks operation or outcome")
    if (
        record["sample_limit"] != 8
        or _natural(record["sample_ordinal"], "sample ordinal", positive=True) > 8
    ):
        raise ValueError("refusal sample contract differs")
    execution = record["execution"]
    if record["phase"] == "before_grant":
        if execution is not None or record["budget_diagnostic"] is not None:
            raise ValueError("pregrant refusal invented execution or budget")
        return
    if record["phase"] != "granted_execution":
        raise ValueError("unknown refusal phase")
    if (
        not isinstance(record["budget_diagnostic"], str)
        or not isinstance(execution, dict)
        or set(execution) != EXECUTION_KEYS
    ):
        raise ValueError("granted refusal lacks original execution evidence")
    dispatch = _natural(execution["dispatch_ordinal"], "dispatch ordinal", positive=True)
    if (
        _natural(execution["assignment_dispatches"], "assignment dispatches", positive=True)
        != dispatch
    ):
        raise ValueError("original dispatch and assignment disagree")
    _natural(execution["assignment_deadline_ms"], "assignment deadline", positive=True)
    claim = execution["claim"]
    if claim is not None:
        if not isinstance(claim, dict) or set(claim) != {"intent", "fence", "lease_until_ms"}:
            raise ValueError("materialization claim is malformed")
        _natural(claim["fence"], "claim fence", positive=True)
        _natural(claim["lease_until_ms"], "claim lease", positive=True)
    trace = execution["trace"]
    if trace is not None:
        if (
            not isinstance(trace, dict)
            or set(trace) != {"capture_id", "span_id"}
            or not isinstance(trace["capture_id"], str)
            or not TRACE_ID.fullmatch(trace["capture_id"])
        ):
            raise ValueError("refusal trace reference is malformed")
        _natural(trace["span_id"], "refusal span ID", positive=True)


def _terminal(outcome: str) -> str:
    if outcome == "Err(Cancelled)":
        return "cancelled"
    if outcome.startswith("Err(") and outcome.endswith(")"):
        return "error"
    if outcome.startswith("Terminal(Refused(") and outcome.endswith("))"):
        return "error"
    raise ValueError("granted refusal has no worker terminal mapping")


def _join_refusals(
    capture_id: str,
    spans: dict[int, dict[str, Any]],
    workers: dict[str, dict[str, Any]],
    raw_log: str | None,
) -> dict[str, Any]:
    if raw_log is None:
        return {
            "status": "NOT_RUN_NO_REFUSAL_LOG",
            "matched": [],
            "unassigned": [],
            "foreign": [],
            "raw_candidates_seen": None,
        }
    matched, unassigned, foreign = [], [], []
    records = _records(raw_log)
    seen: set[int] = set()
    ordinals: set[int] = set()
    for record in records:
        _validate_record(record)
        identity = {key: record[key] for key in RECORD_KEYS if key != "event"}
        if record["phase"] == "before_grant":
            unassigned.append({**identity, "reason": "before_grant_no_execution"})
            continue
        trace = record["execution"]["trace"]
        if trace is None:
            unassigned.append({**identity, "reason": "capture_or_span_unavailable"})
            continue
        if trace["capture_id"] != capture_id:
            foreign.append({**identity, "reason": "different_capture"})
            continue
        ordinals.add(record["sample_ordinal"])
        if len(ordinals) > 8:
            raise ValueError("current capture exceeds process-local refusal sample bound")
        span_id = trace["span_id"]
        if span_id in seen or str(span_id) not in workers or not _worker(spans, span_id):
            raise ValueError("refusal references duplicate, missing, root or nonworker span")
        seen.add(span_id)
        if spans[span_id].get("outcome") != _terminal(record["outcome"]):
            raise ValueError("refusal and worker terminal disagree")
        matched.append({**identity, "worker_span_id": span_id})
    status = (
        "DIAGNOSTIC_PARTIAL_JOIN"
        if matched and unassigned
        else "DIAGNOSTIC_MATCHED"
        if matched
        else "UNJOINED_SAMPLED_REFUSAL"
        if unassigned
        else "NOT_APPLICABLE_NO_CURRENT_CAPTURE_REFUSAL"
        if foreign
        else "NOT_APPLICABLE_NO_SAMPLED_REFUSAL"
    )
    return {
        "status": status,
        "matched": matched,
        "unassigned": unassigned,
        "foreign": foreign,
        "raw_candidates_seen": len(records),
        "current_capture_sample_ordinals": sorted(ordinals),
    }


def build_index(capture: dict[str, Any], *, refusal_log: str | None = None) -> dict[str, Any]:
    """Project one complete canonical capture; retain no raw qtrace or inferred public status."""
    capture_id, spans = _complete(capture)
    workers = {span_id for span_id in spans if _worker(spans, span_id)}
    owner_cache: dict[int, int | None] = {}

    def owner(span_id: int) -> int | None:
        if span_id in owner_cache:
            return owner_cache[span_id]
        trail = []
        cursor: int | None = span_id
        visiting = set()
        while cursor is not None and cursor not in owner_cache and cursor not in workers:
            if cursor in visiting or cursor not in spans:
                raise ValueError("span ancestry cycles or loses a parent")
            visiting.add(cursor)
            trail.append(cursor)
            cursor = spans[cursor].get("parent_span_id")
        result = cursor if cursor in workers else owner_cache.get(cursor)
        for item in trail:
            owner_cache[item] = result
        return result

    def phase(span_id: int, worker_id: int) -> str:
        cursor = spans[span_id].get("parent_span_id")
        while cursor is not None and cursor != worker_id:
            if spans[cursor]["stage"] in PHASE_STAGES:
                return spans[cursor]["stage"]
            cursor = spans[cursor].get("parent_span_id")
        return "outside_phase"

    buckets: dict[int, dict[tuple[str, int, str], list[dict[str, Any]]]] = {
        worker_id: defaultdict(list) for worker_id in workers
    }
    for span_id, span in spans.items():
        start = _natural(span.get("start_ns"), f"span {span_id} start")
        end = _natural(span.get("end_ns"), f"span {span_id} end")
        if end < start or not isinstance(span.get("outcome"), str):
            raise ValueError(f"span {span_id} is open or regressed")
        parent = span.get("parent_span_id")
        if parent is not None:
            if parent not in spans:
                raise ValueError(f"span {span_id} has missing parent")
        worker_id = owner(span_id)
        if worker_id is None or worker_id == span_id:
            continue
        worker_span = spans[worker_id]
        if not worker_span["start_ns"] <= start <= end <= worker_span["end_ns"]:
            raise ValueError(f"span {span_id} escapes owned worker interval")
        stage, lane = span.get("stage"), span.get("lane_id")
        if not isinstance(stage, str) or not stage or type(lane) is not int:
            raise ValueError(f"span {span_id} has invalid stage or lane")
        if not isinstance(span.get("counters"), dict):
            raise ValueError(f"span {span_id} has invalid counters")
        buckets[worker_id][(stage, lane, phase(span_id, worker_id))].append(span)

    result_workers: dict[str, dict[str, Any]] = {}
    for worker_id in sorted(workers):
        worker = spans[worker_id]
        groups = []
        for (stage, lane, nearest_phase), rows in sorted(buckets[worker_id].items()):
            intervals = [(row["start_ns"], row["end_ns"]) for row in rows]
            counters: dict[str, int] = defaultdict(int)
            for row in rows:
                for key, value in row["counters"].items():
                    if not isinstance(key, str):
                        raise ValueError("counter key is not a string")
                    counters[key] += _natural(value, f"counter {key}")
                if stage == "sqlite_work" and lane == 22:
                    if set(WRITE_KEYS) - set(row["counters"]):
                        raise ValueError("SQLite AfterWrite aggregate has missing counters")
                    if (
                        row["counters"]["sqlite_write_calls"]
                        != sum(row["counters"][key] for key in WRITE_KEYS[3:])
                        or row["counters"]["sqlite_write_failures"]
                        > row["counters"]["sqlite_write_calls"]
                    ):
                        raise ValueError("SQLite write count partition is inconsistent")
            groups.append(
                {
                    "stage": stage,
                    "lane_id": lane,
                    "nearest_worker_phase": nearest_phase,
                    "count": len(rows),
                    "sum_ns": sum(end - start for start, end in intervals),
                    "union_ns": _union_ns(intervals),
                    "max_ns": max(end - start for start, end in intervals),
                    "direct_counters": dict(sorted(counters.items())),
                }
            )
        result_workers[str(worker_id)] = {
            "span_id": worker_id,
            "parent_span_id": worker["parent_span_id"],
            "outcome": worker["outcome"],
            "duration_ns": worker["end_ns"] - worker["start_ns"],
            "groups": groups,
        }
    refusals = _join_refusals(capture_id, spans, result_workers, refusal_log)
    return {
        "schema_version": 1,
        "status": "UNBOUND_DIAGNOSTIC_ONLY",
        "capture": {
            "capture_id": capture_id,
            "stream_sha256": capture["sha256"],
            "schema_version": capture.get("schema_version"),
            "status": capture["status"],
            "footer": capture["footer"],
            "health": capture["health"],
            "truncated_tail": capture["truncated_tail"],
        },
        "source_binary_binding": "NOT_PROVIDED",
        "worker_count": len(result_workers),
        "workers": result_workers,
        "refusals": refusals,
        "scope": SCOPE.copy(),
    }


def _validate_index(index: dict[str, Any]) -> None:
    """Reject corrupt cache shape; this cannot authenticate a coherent forged cache."""
    if not isinstance(index, dict) or set(index) != {
        "schema_version",
        "status",
        "capture",
        "source_binary_binding",
        "worker_count",
        "workers",
        "refusals",
        "scope",
    }:
        raise ValueError("cached worker index envelope is malformed")
    if (
        type(index["schema_version"]) is not int
        or index["schema_version"] != 1
        or index["status"] != "UNBOUND_DIAGNOSTIC_ONLY"
        or index["source_binary_binding"] != "NOT_PROVIDED"
    ):
        raise ValueError("cached worker index claims unsupported authority")
    capture = index["capture"]
    if not isinstance(capture, dict) or set(capture) != {
        "capture_id",
        "stream_sha256",
        "schema_version",
        "status",
        "footer",
        "health",
        "truncated_tail",
    }:
        raise ValueError("cached worker index capture is malformed")
    capture_id, digest, footer = (
        capture["capture_id"],
        capture["stream_sha256"],
        capture["footer"],
    )
    if (
        not isinstance(capture_id, str)
        or not TRACE_ID.fullmatch(capture_id)
        or not isinstance(digest, str)
        or not re.fullmatch(r"[0-9a-f]{64}", digest)
        or type(capture["schema_version"]) is not int
        or capture["schema_version"] != 4
        or capture["status"] != "complete"
        or capture["health"] != []
        or capture["truncated_tail"] is not False
        or not isinstance(footer, dict)
        or footer.get("capture_id") != capture_id
        or footer.get("status") != "complete"
        or any(
            type(footer.get(key)) is not int or footer[key] != 0
            for key in ("events_dropped", "events_unwritten", "open_spans", "producer_leases")
        )
    ):
        raise ValueError("cached worker index lacks healthy capture identity")
    scope = index["scope"]
    if scope != SCOPE:
        raise ValueError("cached worker index lacks interpretation scope")
    workers = index["workers"]
    if not isinstance(workers, dict) or _natural(index["worker_count"], "worker count") != len(
        workers
    ):
        raise ValueError("cached worker index worker count differs")
    for key, worker in workers.items():
        if not isinstance(worker, dict) or set(worker) != {
            "span_id",
            "parent_span_id",
            "outcome",
            "duration_ns",
            "groups",
        }:
            raise ValueError("cached worker row is malformed")
        span_id = _natural(worker["span_id"], "worker span ID", positive=True)
        if key != str(span_id):
            raise ValueError("cached worker ID key differs")
        _natural(worker["parent_span_id"], "worker root ID", positive=True)
        duration = _natural(worker["duration_ns"], "worker duration")
        if (
            not isinstance(worker["outcome"], str)
            or not worker["outcome"]
            or not isinstance(worker["groups"], list)
        ):
            raise ValueError("cached worker outcome or groups are malformed")
        seen_groups = set()
        for group in worker["groups"]:
            if not isinstance(group, dict) or set(group) != {
                "stage",
                "lane_id",
                "nearest_worker_phase",
                "count",
                "sum_ns",
                "union_ns",
                "max_ns",
                "direct_counters",
            }:
                raise ValueError("cached worker group is malformed")
            stage, lane, phase = (group["stage"], group["lane_id"], group["nearest_worker_phase"])
            if (
                not isinstance(stage, str)
                or not stage
                or type(lane) is not int
                or lane < 0
                or not isinstance(phase, str)
                or phase not in PHASE_STAGES | {"outside_phase"}
                or (stage, lane, phase) in seen_groups
            ):
                raise ValueError("cached worker group dimension is malformed")
            seen_groups.add((stage, lane, phase))
            count = _natural(group["count"], "group count", positive=True)
            total = _natural(group["sum_ns"], "group sum")
            union = _natural(group["union_ns"], "group union")
            longest = _natural(group["max_ns"], "group maximum")
            if not longest <= union <= min(total, duration) or total > count * duration:
                raise ValueError("cached worker group intervals are inconsistent")
            counters = group["direct_counters"]
            if not isinstance(counters, dict) or any(
                not isinstance(name, str) or not name or type(value) is not int or value < 0
                for name, value in counters.items()
            ):
                raise ValueError("cached worker counters are malformed")
            if stage == "sqlite_work" and lane == 22:
                if (
                    set(WRITE_KEYS) - set(counters)
                    or counters["sqlite_write_calls"]
                    != sum(counters[key] for key in WRITE_KEYS[3:])
                    or counters["sqlite_write_failures"] > counters["sqlite_write_calls"]
                ):
                    raise ValueError("cached SQLite write-call partition differs")
    refusals = index["refusals"]
    if (
        not isinstance(refusals, dict)
        or not {
            "status",
            "matched",
            "unassigned",
            "foreign",
            "raw_candidates_seen",
        }
        <= set(refusals)
        or set(refusals)
        - {
            "status",
            "matched",
            "unassigned",
            "foreign",
            "raw_candidates_seen",
            "current_capture_sample_ordinals",
        }
    ):
        raise ValueError("cached refusal projection is malformed")
    matched, unassigned, foreign = (
        refusals["matched"],
        refusals["unassigned"],
        refusals["foreign"],
    )
    if any(not isinstance(rows, list) for rows in (matched, unassigned, foreign)):
        raise ValueError("cached refusal classes are malformed")
    count = refusals["raw_candidates_seen"]
    if refusals["status"] == "NOT_RUN_NO_REFUSAL_LOG":
        if (
            count is not None
            or matched
            or unassigned
            or foreign
            or "current_capture_sample_ordinals" in refusals
        ):
            raise ValueError("cached no-log refusal status has records")
        return
    if _natural(count, "raw refusal count") != len(matched) + len(unassigned) + len(foreign):
        raise ValueError("cached refusal class counts differ")
    expected_status = (
        "DIAGNOSTIC_PARTIAL_JOIN"
        if matched and unassigned
        else "DIAGNOSTIC_MATCHED"
        if matched
        else "UNJOINED_SAMPLED_REFUSAL"
        if unassigned
        else "NOT_APPLICABLE_NO_CURRENT_CAPTURE_REFUSAL"
        if foreign
        else "NOT_APPLICABLE_NO_SAMPLED_REFUSAL"
    )
    if refusals["status"] != expected_status:
        raise ValueError("cached refusal status contradicts records")
    ordinals = set()
    matched_ids = set()
    for kind, rows in (("matched", matched), ("unassigned", unassigned), ("foreign", foreign)):
        for row in rows:
            expected_keys = RECORD_KEYS - {"event"}
            expected_keys |= {"worker_span_id"} if kind == "matched" else {"reason"}
            if not isinstance(row, dict) or set(row) != expected_keys:
                raise ValueError("cached refusal row fields differ")
            _validate_record({"event": EVENT, **{key: row[key] for key in RECORD_KEYS - {"event"}}})
            execution = row["execution"]
            if kind == "matched":
                span_id = _natural(row["worker_span_id"], "matched worker ID", positive=True)
                if (
                    span_id in matched_ids
                    or str(span_id) not in workers
                    or execution["trace"] != {"capture_id": capture_id, "span_id": span_id}
                    or workers[str(span_id)]["outcome"] != _terminal(row["outcome"])
                ):
                    raise ValueError("cached matched refusal references wrong worker")
                matched_ids.add(span_id)
                ordinals.add(row["sample_ordinal"])
            elif kind == "unassigned":
                if not (
                    row["reason"] == "before_grant_no_execution"
                    and execution is None
                    or row["reason"] == "capture_or_span_unavailable"
                    and execution is not None
                    and execution["trace"] is None
                ):
                    raise ValueError("cached unassigned refusal has a trace reference")
            elif (
                row["reason"] != "different_capture"
                or execution is None
                or (execution["trace"] is None or execution["trace"]["capture_id"] == capture_id)
            ):
                raise ValueError("cached foreign refusal refers to current capture")
    listed = refusals.get("current_capture_sample_ordinals")
    if not isinstance(listed, list) or listed != sorted(ordinals) or len(ordinals) > 8:
        raise ValueError("cached current-capture sample ordinals differ")


def query_index(
    index: dict[str, Any],
    *,
    worker_span_id: int | None = None,
    stage: str | None = None,
    lane: int | None = None,
    refusals_only: bool = False,
) -> dict[str, Any]:
    """Filter an already-built index without opening the source qtrace."""
    _validate_index(index)
    selected = {}
    for key, worker in index["workers"].items():
        if worker_span_id is not None and worker["span_id"] != worker_span_id:
            continue
        groups = [
            row
            for row in worker["groups"]
            if (stage is None or row["stage"] == stage) and (lane is None or row["lane_id"] == lane)
        ]
        if (stage is not None or lane is not None) and not groups:
            continue
        if refusals_only and not any(
            row["worker_span_id"] == worker["span_id"] for row in index["refusals"]["matched"]
        ):
            continue
        selected[key] = {**worker, "groups": groups}
    selected_ids = {row["span_id"] for row in selected.values()}
    return {
        "status": index["status"],
        "capture": index["capture"],
        "matched_workers": len(selected),
        "workers": selected,
        "refusals": {
            **index["refusals"],
            "matched": [
                row for row in index["refusals"]["matched"] if row["worker_span_id"] in selected_ids
            ],
        },
        "scope": index["scope"],
    }


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    build = commands.add_parser("build", help="parse one canonical capture once")
    build.add_argument("--input", type=Path, required=True)
    build.add_argument("--out", type=Path, required=True)
    build.add_argument("--stderr", type=Path)
    build.add_argument("--max-capture-bytes", type=int, default=MAX_CAPTURE_BYTES)
    query = commands.add_parser("query", help="query a cached diagnostic index")
    query.add_argument("--index", type=Path, required=True)
    query.add_argument("--worker-span-id", type=int)
    query.add_argument("--stage")
    query.add_argument("--lane", type=int)
    query.add_argument("--refusals-only", action="store_true")
    args = parser.parse_args(argv)
    if args.command == "build":
        with args.input.open("rb") as stream:
            if os.fstat(stream.fileno()).st_size > args.max_capture_bytes:
                raise ValueError("capture exceeds configured offline replay bound")
            capture = parse_stream(stream, max_capture_bytes=args.max_capture_bytes)
        raw_log = None
        if args.stderr is not None:
            with args.stderr.open("rb") as stream:
                if os.fstat(stream.fileno()).st_size > MAX_STDERR_BYTES:
                    raise ValueError("stderr exceeds diagnostic input bound")
                raw_log = stream.read().decode("utf-8")
        result = build_index(capture, refusal_log=raw_log)
        descriptor = os.open(args.out, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
        with os.fdopen(descriptor, "w", encoding="utf-8") as stream:
            json.dump(result, stream, separators=(",", ":"), sort_keys=True, allow_nan=False)
            stream.write("\n")
    else:
        with args.index.open("rb") as stream:
            if os.fstat(stream.fileno()).st_size > MAX_INDEX_BYTES:
                raise ValueError("cached diagnostic index exceeds read bound")
            raw = stream.read(MAX_INDEX_BYTES + 1)
            if len(raw) > MAX_INDEX_BYTES:
                raise ValueError("cached diagnostic index grew past read bound")
        try:
            index = json.loads(
                raw.decode("utf-8"),
                object_pairs_hook=_unique,
                parse_constant=_reject_constant,
            )
        except (UnicodeError, json.JSONDecodeError) as error:
            raise ValueError(f"cached diagnostic index is malformed: {error}") from error
        result = query_index(
            index,
            worker_span_id=args.worker_span_id,
            stage=args.stage,
            lane=args.lane,
            refusals_only=args.refusals_only,
        )
        json.dump(result, sys.stdout, separators=(",", ":"), sort_keys=True, allow_nan=False)
        sys.stdout.write("\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
