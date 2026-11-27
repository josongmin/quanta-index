"""Observed closed worker and maintenance scopes, including incomplete captures.

This report never confers complete-capture custody. A closed span does not
prove that all its events survived output truncation. No missing counter or
unrecorded scope is treated as zero, and no public completion is inferred.
"""

from __future__ import annotations

import re
from collections import Counter, defaultdict
from collections.abc import Iterable
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
# Match the exact span stage and lane emitted by each producer.
DRIVER_STAGES = {
    ("catalog_lock_wait", 0): "catalog_lock_wait",
    ("sqlite_work", 11): "sqlite_wal_sync",
    ("sqlite_work", 12): "sqlite_main_sync",
    ("sqlite_work", 25): "sqlite_wal_backfill",
    ("managed_object_install", 3): "managed_object_install",
    ("semantic_build", 3): "semantic_build",
    ("semantic_seal", 3): "semantic_seal",
}
OWNED_CONTROL_LANES = {
    3: "grant",
    4: "reply_handoff",
    5: "worker_quiescence",
    6: "catalog_settlement",
    7: "worker_queue",
}


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


def _observed_wall_partition(
    start: int, end: int, intervals: Iterable[tuple[str, int, int]]
) -> Counter[tuple[str, ...]]:
    changes: dict[int, Counter[str]] = defaultdict(Counter, {end: Counter()})
    for stage, left, right in intervals:
        changes[left][stage] += 1
        changes[right][stage] -= 1
    wall_by_active_stages: Counter[tuple[str, ...]] = Counter()
    active: Counter[str] = Counter()
    previous = start
    for boundary, deltas in sorted(changes.items()):
        if boundary > previous:
            wall_by_active_stages[tuple(sorted(active))] += boundary - previous
        for stage, delta in deltas.items():
            active[stage] += delta
            if active[stage] == 0:
                del active[stage]
        previous = boundary
    return wall_by_active_stages


def _observed_phase_wall(
    start: int,
    end: int,
    groups: dict[tuple[str, int, str], list[dict[str, Any]]],
) -> dict[str, Any]:
    wall = _observed_wall_partition(
        start,
        end,
        (
            (stage, span["start_ns"], span["end_ns"])
            for (stage, _lane, _phase), members in groups.items()
            if stage in PHASES
            for span in members
        ),
    )
    return {
        "exclusive_by_phase": {
            stages[0]: duration for stages, duration in sorted(wall.items()) if len(stages) == 1
        },
        "overlapping_phases": sum(duration for stages, duration in wall.items() if len(stages) > 1),
        "outside_observed_phases": wall[()],
    }


def _observed_driver_wall(
    start: int,
    end: int,
    groups: dict[tuple[str, int, str], list[dict[str, Any]]],
) -> dict[str, Any]:
    wall = _observed_wall_partition(
        start,
        end,
        (
            (label, span["start_ns"], span["end_ns"])
            for (stage, lane, _phase), members in groups.items()
            if (label := DRIVER_STAGES.get((stage, lane))) is not None
            for span in members
        ),
    )
    return {
        "exclusive_by_stage": {
            stages[0]: duration for stages, duration in sorted(wall.items()) if len(stages) == 1
        },
        "overlap_by_stage_set": [
            {"stages": list(stages), "duration_ns": duration}
            for stages, duration in sorted(wall.items())
            if len(stages) > 1
        ],
        "outside_observed_stages": wall[()],
    }


def _observed_window_occupancy(
    capture: dict[str, Any], roots: set[int], window_ns: tuple[int, int]
) -> dict[str, int]:
    header = capture.get("header")
    if not isinstance(header, dict):
        raise ValueError("worker window requires a canonical capture header")
    capture_start = _number(header.get("started_ns"), "capture start")
    capture_end = _number(capture.get("last_observed_ns"), "last observed capture time")
    if capture_end < capture_start:
        raise ValueError("observed capture interval regressed")
    start, end = window_ns
    observed_start = min(end, max(start, capture_start))
    observed_end = max(observed_start, min(end, capture_end))
    intervals = []
    boundary_crossing = open_starts = prior_open_starts = 0
    for key in roots:
        root = capture["spans"][key]
        left = _number(root.get("start_ns"), "worker start")
        if not capture_start <= left <= capture_end:
            raise ValueError("worker start escapes observed capture")
        if root.get("end_ns") is None:
            open_starts += observed_start <= left < observed_end
            prior_open_starts += observed_start < observed_end and left < observed_start
            continue
        right = _number(root["end_ns"], "worker end")
        if not capture_start <= left <= right <= capture_end:
            raise ValueError("worker interval escapes observed capture")
        clipped = (max(left, observed_start), min(right, observed_end))
        if clipped[0] < clipped[1]:
            intervals.append(clipped)
            boundary_crossing += clipped != (left, right)
    worker_union = _union_ns(intervals)
    return {
        "closed_worker_union_ns": worker_union,
        "observed_without_closed_worker_ns": observed_end - observed_start - worker_union,
        "before_capture_ns": observed_start - start,
        "after_observed_capture_ns": end - observed_end,
        "contributing_closed_worker_roots": len(intervals),
        "boundary_crossing_closed_worker_roots": boundary_crossing,
        "open_worker_roots_started_in_observed_window": open_starts,
        "open_worker_roots_started_before_observed_window": prior_open_starts,
    }


def _capture_summary(capture: dict[str, Any]) -> dict[str, Any]:
    return {
        "capture_id": capture["capture_id"],
        "sha256": capture["sha256"],
        "schema_version": capture["schema_version"],
        "status": capture["status"],
        "footer": capture.get("footer"),
        "health": capture["health"],
        "truncated_tail": capture["truncated_tail"],
        "orphan_events": capture.get("orphan_events"),
    }


def _linked_worker_controls(
    capture: dict[str, Any], roots: set[int]
) -> dict[int, dict[str, dict[str, Any]]]:
    spans = capture["spans"]
    controls: dict[int, dict[str, dict[str, Any]]] = defaultdict(dict)
    linked_controls: set[int] = set()
    for link in capture.get("links", ()):
        if link["target_capture_id"] != capture["capture_id"]:
            continue
        source, target = link["source_span_id"], link["target_span_id"]
        if source in roots:
            worker, control = source, target
            allowed_lanes = (3, 7)
        elif target in roots:
            worker, control = target, source
            allowed_lanes = (4, 5, 6)
        else:
            continue
        span = spans.get(control)
        if (
            span is None
            or span["stage"] != "operation"
            or span["lane_id"] not in allowed_lanes
            or span["parent_span_id"] != spans[worker]["parent_span_id"]
            or span["end_ns"] is None
            or span["outcome"] not in OUTCOMES
        ):
            continue
        phase = OWNED_CONTROL_LANES[span["lane_id"]]
        if control in linked_controls or phase in controls[worker]:
            raise ValueError(f"ambiguous {phase} control phase for worker {worker}")
        linked_controls.add(control)
        start, end = span["start_ns"], span["end_ns"]
        controls[worker][phase] = {
            "span_id": control,
            "start_ns": start,
            "end_ns": end,
            "duration_ns": end - start,
            "outcome": span["outcome"],
        }
    return controls


def _build_closed_scope_report(
    capture: dict[str, Any],
    *,
    maintenance: bool,
    window_ns: tuple[int, int] | None = None,
    minimum_closed_workers: int = 1,
    occupancy_only: bool = False,
) -> dict[str, Any]:
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
    if occupancy_only:
        if maintenance or window_ns is None:
            raise ValueError("worker occupancy requires a capture window")
        return {
            "schema_version": 1,
            "status": "OBSERVED_WINDOW_OCCUPANCY_ONLY",
            "capture": _capture_summary(capture),
            "window": {"start_ns": window_ns[0], "end_ns": window_ns[1]},
            "window_occupancy": _observed_window_occupancy(capture, roots, window_ns),
            "scope": {
                "custody": "Partial remains partial; missing events cannot establish complete worker coverage.",
                "time": "Closed worker root intervals are clipped to the capture clock window and unioned once. An unobserved end contributes no duration.",
                "residual": "Time outside observed closed roots is unknown, not proven pump wait or idle time. A prior open start does not prove the worker reached this window.",
                "sample": "No per-worker phase sample or public WorkStatus completion is established.",
            },
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

    controls = {} if maintenance else _linked_worker_controls(capture, roots)
    rows = []
    for key in sorted(roots - incomplete, key=lambda item: (spans[item]["start_ns"], item)):
        root = spans[key]
        start = _number(root.get("start_ns"), "scope start")
        end = _number(root.get("end_ns"), "scope end")
        if end < start:
            raise ValueError("scope interval regressed")
        if window_ns is not None and not window_ns[0] <= start <= end <= window_ns[1]:
            continue
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
                **({"linked_control_phases": controls[key]} if key in controls else {}),
                **(
                    {
                        "observed_phase_wall_ns": _observed_phase_wall(start, end, groups),
                        "observed_driver_wall_ns": _observed_driver_wall(start, end, groups),
                    }
                    if window_ns is not None
                    else {}
                ),
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
    report = {
        "schema_version": 1,
        "status": "OBSERVED_CLOSED_SCOPES_ONLY",
        "capture": _capture_summary(capture),
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
    if window_ns is not None:
        if len(rows) < minimum_closed_workers:
            raise ValueError(
                f"capture window found {len(rows)} fully contained closed workers; "
                f"expected at least {minimum_closed_workers}"
            )
        report["window"] = {
            "start_ns": window_ns[0],
            "end_ns": window_ns[1],
            "minimum_closed_workers": minimum_closed_workers,
            "fully_contained_closed_workers": len(rows),
            "excluded_closed_workers_outside_window": len(roots) - len(incomplete) - len(rows),
        }
        report["window_occupancy"] = _observed_window_occupancy(capture, roots, window_ns)
        report["scope"]["window"] = (
            "Only fully contained closed workers on the capture clock. Distinct phase overlap "
            "is reported separately. Time outside observed phases includes unknown or "
            "uninstrumented work; nested I/O is not added to phase wall time."
        )
        report["scope"]["driver_wall"] = (
            "A second partition of the same worker wall, using the observed Catalog wait, "
            "SQLite sync and backfill, managed install, semantic build and semantic seal spans. "
            "Each active stage set has disjoint wall time even when its spans are siblings. "
            "The native ANN seal span covers region registration. Carry and consolidation "
            "also execute outside that span. Managed install and SQLite sync are span envelopes."
        )
        report["scope"]["window_occupancy"] = (
            "Union of observed worker root intervals clipped to the window, including roots "
            "with incomplete descendants. Roots with an unobserved end contribute only a "
            "start count. Residual time may include pump work, maintenance, incomplete "
            "worker roots or lost events. Time outside the observed capture is shown separately."
        )
    if controls:
        report["scope"]["linked_control_phases"] = (
            "Closed control spans linked to a worker in the same capture and operation root. "
            "Absolute times may extend outside the selected worker window. "
            "Queue covers handoff to the worker until it begins processing the command. "
            "Reply handoff and quiescence may overlap. An absent phase is unobserved."
        )
    return report


def validate_closed_worker_options(
    *,
    window_ns: tuple[int, int] | None,
    minimum_closed_workers: int,
    occupancy_only: bool,
) -> None:
    """Validate report options independently of the capture."""
    if window_ns is not None and (
        not isinstance(window_ns, tuple)
        or len(window_ns) != 2
        or any(type(bound) is not int for bound in window_ns)
        or not 0 <= window_ns[0] < window_ns[1]
    ):
        raise ValueError("invalid worker capture window")
    if type(minimum_closed_workers) is not int or minimum_closed_workers < 1:
        raise ValueError("minimum closed workers must be positive")
    if window_ns is None and minimum_closed_workers != 1:
        raise ValueError("minimum closed workers requires a capture window")
    if occupancy_only and window_ns is None:
        raise ValueError("worker occupancy requires a capture window")
    if occupancy_only and minimum_closed_workers != 1:
        raise ValueError("worker occupancy does not accept a minimum closed worker sample")


def build_closed_worker_report(
    capture: dict[str, Any],
    *,
    window_ns: tuple[int, int] | None = None,
    minimum_closed_workers: int = 1,
    occupancy_only: bool = False,
) -> dict[str, Any]:
    """Preserve partial custody and optionally report a bounded observed window."""
    validate_closed_worker_options(
        window_ns=window_ns,
        minimum_closed_workers=minimum_closed_workers,
        occupancy_only=occupancy_only,
    )
    return _build_closed_scope_report(
        capture,
        maintenance=False,
        window_ns=window_ns,
        minimum_closed_workers=minimum_closed_workers,
        occupancy_only=occupancy_only,
    )


def build_closed_maintenance_report(capture: dict[str, Any]) -> dict[str, Any]:
    """Project only closed runtime maintenance subtrees and reached checkpoints."""
    return _build_closed_scope_report(capture, maintenance=True)
