"""Observed closed worker and maintenance scopes, including incomplete captures.

This report never confers complete-capture custody. A closed span does not
prove that all its events survived output truncation. No missing counter or
unrecorded scope is treated as zero, and no public completion is inferred.
"""

from __future__ import annotations

import re
from collections import defaultdict
from typing import Any

from tools.benchmark.retrieval.causal_cost_timeline import _union_ns

PHASES = frozenset(
    {
        "build_prepare",
        "finalize",
        "semantic_prepare",
        "native_coverage_save",
        "native_work_seal",
        "native_publish",
    }
)
OUTCOMES = frozenset({"ok", "error", "cancelled", "aborted"})


def _number(value: Any, label: str) -> int:
    if type(value) is not int or value < 0:
        raise ValueError(f"invalid {label}")
    return value


def _nearest_parent_phase(
    child: dict[str, Any], spans: dict[int, dict[str, Any]], cache: dict[int, str]
) -> str:
    """Find the nearest named ancestor, excluding the child's own stage."""
    cursor = child.get("parent_span_id")
    trail: list[int] = []
    visiting: set[int] = set()
    while cursor is not None and cursor not in cache:
        if cursor in visiting:
            raise ValueError("canonical span ancestry cycles")
        ancestor = spans.get(cursor)
        if ancestor is None:
            raise ValueError("canonical span ancestry loses a parent")
        visiting.add(cursor)
        trail.append(cursor)
        if ancestor.get("stage") in PHASES:
            cache[cursor] = ancestor["stage"]
            break
        cursor = ancestor.get("parent_span_id")
    resolved = cache.get(cursor, "outside_phase")
    for ancestor_id in reversed(trail):
        if ancestor_id in cache:
            resolved = cache[ancestor_id]
        else:
            cache[ancestor_id] = resolved
    return resolved


def _build_closed_scope_report(capture: dict[str, Any], *, maintenance: bool) -> dict[str, Any]:
    """Project one disjoint kind of recorded subtree without changing custody."""
    if (
        not isinstance(capture, dict)
        or capture.get("status") not in ("complete", "partial")
        or type(capture.get("schema_version")) is not int
        or capture["schema_version"] not in (4, 5)
        or not isinstance(capture.get("capture_id"), str)
        or not re.fullmatch(r"[0-9a-f]{32}", capture["capture_id"])
        or not isinstance(capture.get("sha256"), str)
        or not re.fullmatch(r"[0-9a-f]{64}", capture["sha256"])
        or capture.get("health") != []
        or type(capture.get("truncated_tail")) is not bool
        or not isinstance(capture.get("spans"), dict)
    ):
        raise ValueError("closed-scope report requires a healthy canonical capture")
    spans = capture["spans"]
    for key, span in spans.items():
        if type(key) is not int or key <= 0 or not isinstance(span, dict):
            raise ValueError("invalid canonical span table")
        if span.get("span_id") != key:
            raise ValueError("span table identity differs")
    roots = {
        key
        for key, span in spans.items()
        if span.get("stage") == ("runtime_maintenance" if maintenance else "operation")
        and span.get("lane_id") == (0 if maintenance else 1)
        and span.get("root_kind") is None
        and (parent := spans.get(span.get("parent_span_id"))) is not None
        and parent.get("stage") == "operation"
        and parent.get("lane_id") == 0
        and parent.get("root_kind") == "operation"
        and parent.get("parent_span_id") is None
    }
    owners: dict[int, int | None] = {}

    def owner(key: int) -> int | None:
        trail, visiting = [], set()
        cursor = key
        while cursor is not None and cursor not in owners and cursor not in roots:
            if cursor in visiting:
                raise ValueError("canonical span ancestry cycles")
            if cursor not in spans:
                break
            visiting.add(cursor)
            trail.append(cursor)
            cursor = spans[cursor].get("parent_span_id")
        result = cursor if cursor in roots else owners.get(cursor)
        for item in trail:
            owners[item] = result
        return result

    def closed(span: dict[str, Any]) -> bool:
        return span.get("end_ns") is not None and span.get("outcome") in OUTCOMES

    incomplete = {key for key in roots if not closed(spans[key])}
    children: dict[int, list[int]] = defaultdict(list)
    for key, span in spans.items():
        selected = owner(key)
        if selected is not None:
            if not closed(span):
                incomplete.add(selected)
            elif key != selected:
                children[selected].append(key)

    rows = []
    for key in sorted(roots - incomplete, key=lambda item: (spans[item]["start_ns"], item)):
        root = spans[key]
        start = _number(root.get("start_ns"), "scope start")
        end = _number(root.get("end_ns"), "scope end")
        if end < start:
            raise ValueError("scope interval regressed")
        groups: dict[tuple[str, int, str], list[dict[str, Any]]] = defaultdict(list)
        checkpoints = []
        # Nearest named ancestor is independent of each child's own stage.
        # Compress every visited path once, bounded by this selected subtree.
        nearest_phase: dict[int, str] = {key: "outside_phase"}

        for child in children[key]:
            span = spans[child]
            child_start = _number(span.get("start_ns"), "child start")
            child_end = _number(span.get("end_ns"), "child end")
            parent = spans.get(span.get("parent_span_id"))
            if parent is None or not start <= child_start <= child_end <= end:
                raise ValueError("child escapes recorded scope")
            if (
                parent.get("start_ns") is None
                or child_start < parent["start_ns"]
                or parent.get("end_ns") is not None
                and child_end > parent["end_ns"]
            ):
                raise ValueError("child escapes recorded parent")
            stage, lane = span.get("stage"), span.get("lane_id")
            if (
                not isinstance(stage, str)
                or not stage
                or type(lane) is not int
                or not 0 <= lane <= 65535
            ):
                raise ValueError("invalid observed stage or lane")
            if maintenance and stage == "sqlite_work" and lane == 26:
                checkpoints.append(
                    {
                        "span_id": child,
                        "start_ns": child_start,
                        "end_ns": child_end,
                        "duration_ns": child_end - child_start,
                        "outcome": span["outcome"],
                    }
                )
            phase = _nearest_parent_phase(span, spans, nearest_phase)
            groups[(stage, lane, phase)].append(span)
        costs = []
        for (stage, lane, phase), members in sorted(groups.items()):
            intervals = [(span["start_ns"], span["end_ns"]) for span in members]
            counters: dict[str, dict[str, int]] = {}
            cpu_samples = []
            for span in members:
                if not isinstance(span.get("counters"), dict):
                    raise ValueError("invalid observed counters")
                for name, value in span["counters"].items():
                    if not isinstance(name, str) or not name:
                        raise ValueError("invalid counter name")
                    value = _number(value, "counter value")
                    # Retain each measured synchronous scope without claiming
                    # group CPU coverage or adding nested scopes together.
                    if name == "thread_cpu_ns":
                        cpu_samples.append(
                            {
                                "span_id": span["span_id"],
                                "wall_ns": span["end_ns"] - span["start_ns"],
                                "thread_cpu_ns": value,
                            }
                        )
                        continue
                    observation = counters.setdefault(name, {"sum": 0, "observed_spans": 0})
                    observation["sum"] += value
                    observation["observed_spans"] += 1
            costs.append(
                {
                    "stage": stage,
                    "lane_id": lane,
                    ("nearest_phase" if maintenance else "nearest_worker_phase"): phase,
                    "observed_count": len(members),
                    "sum_ns": sum(right - left for left, right in intervals),
                    "union_ns": _union_ns(intervals),
                    "max_ns": max(right - left for left, right in intervals),
                    "observed_counters": counters,
                    "thread_cpu_samples": cpu_samples,
                }
            )
        rows.append(
            {
                "span_id": key,
                "start_ns": start,
                "end_ns": end,
                "duration_ns": end - start,
                "outcome": root["outcome"],
                "groups": costs,
                **(
                    {
                        "reached_explicit_checkpoints": sorted(
                            checkpoints, key=lambda row: (row["start_ns"], row["span_id"])
                        )
                    }
                    if maintenance
                    else {}
                ),
            }
        )
    return {
        "schema_version": 1,
        "status": "OBSERVED_CLOSED_SCOPES_ONLY",
        "capture": {
            "capture_id": capture["capture_id"],
            "sha256": capture["sha256"],
            "schema_version": capture["schema_version"],
            "status": capture["status"],
            "footer": capture.get("footer"),
            "health": capture["health"],
            "truncated_tail": capture["truncated_tail"],
            "orphan_events": capture.get("orphan_events"),
        },
        "selection": {
            ("observed_maintenance_roots" if maintenance else "observed_worker_roots"): len(roots),
            "selected_closed_subtrees": len(rows),
            "excluded_open_subtrees": len(incomplete),
        },
        ("maintenance" if maintenance else "workers"): rows,
        "scope": {
            "custody": "Partial remains partial. Closed observed subtrees can still lack unrecorded events; no full-run or public completion claim.",
            "time": "One capture clock; observed inclusive unions overlap across groups and are not additive. Selection can bias distributions.",
            "counters": "Only observed values/counts. Absence is unknown; presence does not establish complete observation. Per-span CPU is not group CPU.",
            "qualification": "No source/binary, performance, cache validity or completeness qualification.",
            **(
                {
                    "checkpoint": "Only reached sqlite_work/lane26 spans under a runtime_maintenance subtree. Outcome observes the SQL query, not its later busy-row validation or durable checkpoint completion. Absence does not prove no WAL check was attempted; root, hold and nested times overlap."
                }
                if maintenance
                else {}
            ),
        },
    }


def build_closed_worker_report(capture: dict[str, Any]) -> dict[str, Any]:
    """Preserve the existing closed-worker projection and partial semantics."""
    return _build_closed_scope_report(capture, maintenance=False)


def build_closed_maintenance_report(capture: dict[str, Any]) -> dict[str, Any]:
    """Project only closed runtime maintenance subtrees and reached checkpoints."""
    return _build_closed_scope_report(capture, maintenance=True)
