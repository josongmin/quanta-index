"""Project Native publication request costs from one parsed daemon capture."""

from __future__ import annotations

from collections import Counter, defaultdict
from typing import Any

from tools.benchmark.retrieval.causal_cost_timeline import (
    MANAGED_CROSS_GROUP_CHILD_COUNTERS,
    MANAGED_EXTERNAL_CHILD_COUNTERS,
    MANAGED_INSTALL_CUSTODY_STAGES,
    MANAGED_REUSED_EXTERNAL_CHILD_COUNTER,
    OBJECT_PORT_READ_COUNTERS,
    _union_ns,
    managed_install_census,
)

KERNEL_OWNERS = frozenset({("native_source_apply", 3), ("build_prepare", 3)})
# Existing Literal and FileMap producer phases. They attribute nested work but
# cannot establish request custody without the enclosing Kernel owner.
MEMBER_BUILD_PHASES = frozenset(("build_prepare", lane) for lane in range(40, 52))
MEMBERS = frozenset(
    {
        "native_source_owner_prepare",
        "native_source_path_validate",
        "native_source_path_install",
        "native_source_map_apply",
        "lexical_build",
        "native_text_inventory_prepare",
        "native_text_inventory_diff",
        "native_text_inventory_map_apply",
        "native_text_inventory_read",
        "native_text_occurrence_read",
        "native_text_posting_edit",
        "native_text_postings",
        "native_text_root",
        "native_coverage_scan",
        "native_coverage_map_apply",
        "native_coverage_save",
        "native_searchable_prepare",
        "native_member_assemble",
        "native_work_seal",
        "native_publish",
        "semantic_prepare",
        "finalize",
    }
)
INSTALL_AND_SQL = frozenset(
    {
        "managed_object_reserve",
        "managed_object_install",
        "managed_object_confirm",
        "managed_object_advance",
        "sqlite_work",
    }
)
SQLITE_LANES = frozenset({3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 22, 25})
EXTRA_STAGES = frozenset({"catalog_lock_wait", "catalog_lock_hold", "map_apply"})
WORK_COUNTERS = frozenset({"work_reads", "work_writes", "work_bytes", "work_mutations"})


def _kind(span: dict) -> tuple[str, int]:
    return span["stage"], span["lane_id"]


def _key(kind: tuple[str, int]) -> str:
    return f"{kind[0]}/{kind[1]}"


def _is_phase(kind: tuple[str, int]) -> bool:
    return kind in KERNEL_OWNERS or kind in MEMBER_BUILD_PHASES or kind[0] in MEMBERS


def _group(rows: list[tuple[dict, str | None]], kind: tuple[str, int]) -> dict:
    closed = [(row, owner) for row, owner in rows if row["end_ns"] is not None]
    intervals = [(row["start_ns"], row["end_ns"]) for row, _ in closed]
    counters = [row["counters"] for row, _ in closed]
    result = {
        "count": len(rows),
        "closed_count": len(closed),
        "union_ns": _union_ns(intervals) if closed else None,
        "thread_cpu_ns_sum": (
            sum(counter["thread_cpu_ns"] for counter in counters)
            if len(closed) == len(rows) and all("thread_cpu_ns" in c for c in counters)
            else None
        ),
        "outcomes": dict(sorted(Counter(row["outcome"] for row, _ in closed).items())),
    }
    result["work_usage"] = (
        {key: sum(counter[key] for counter in counters) for key in sorted(WORK_COUNTERS)}
        if len(closed) == len(rows) and all(WORK_COUNTERS <= c.keys() for c in counters)
        else None
    )
    if kind in KERNEL_OWNERS:
        observed = sum(OBJECT_PORT_READ_COUNTERS <= counter.keys() for counter in counters)
        result["object_port_read_spans_observed"] = observed
        result["object_port_reads"] = (
            {
                key: sum(counter[key] for counter in counters)
                for key in sorted(OBJECT_PORT_READ_COUNTERS)
            }
            if observed == len(rows)
            else None
        )
    if kind == ("managed_object_install", 3):
        # Each span is one physical group. A sum of group-local distinct
        # references would not be distinct across the whole run.
        result["external_child_groups"] = []
        for row, owner in sorted(closed, key=lambda item: item[0]["span_id"]):
            observed = set(row["counters"]) & MANAGED_EXTERNAL_CHILD_COUNTERS
            cross_group_observed = set(row["counters"]) & MANAGED_CROSS_GROUP_CHILD_COUNTERS
            status = (
                "COMPLETE"
                if observed == MANAGED_EXTERNAL_CHILD_COUNTERS
                else "PARTIAL"
                if observed
                else "MISSING"
            )
            result["external_child_groups"].append(
                {
                    "span_id": row["span_id"],
                    "nearest_phase": owner or "outside_observed_phase",
                    "outcome": row["outcome"],
                    "observation": status,
                    "counts": (
                        {
                            key: row["counters"][key]
                            for key in sorted(MANAGED_EXTERNAL_CHILD_COUNTERS)
                        }
                        if status == "COMPLETE"
                        else None
                    ),
                    "observed_keys": sorted(observed) if status == "PARTIAL" else [],
                    "reused_external_child_validations": row["counters"].get(
                        MANAGED_REUSED_EXTERNAL_CHILD_COUNTER
                    ),
                    "cross_group_child_checks": {
                        "observation": (
                            "COMPLETE"
                            if cross_group_observed == MANAGED_CROSS_GROUP_CHILD_COUNTERS
                            else "PARTIAL"
                            if cross_group_observed
                            else "MISSING"
                        ),
                        "counts": (
                            {
                                key: row["counters"][key]
                                for key in sorted(MANAGED_CROSS_GROUP_CHILD_COUNTERS)
                            }
                            if cross_group_observed == MANAGED_CROSS_GROUP_CHILD_COUNTERS
                            else None
                        ),
                        "observed_keys": (
                            sorted(cross_group_observed)
                            if cross_group_observed != MANAGED_CROSS_GROUP_CHILD_COUNTERS
                            else []
                        ),
                    },
                }
            )
    if kind[0] in INSTALL_AND_SQL:
        by_owner = defaultdict(list)
        for row, owner in closed:
            by_owner[owner or "outside_observed_phase"].append((row["start_ns"], row["end_ns"]))
        result["wall_union_ns_by_nearest_phase"] = {
            owner: _union_ns(values) for owner, values in sorted(by_owner.items())
        }
    return result


def build_native_ingest_report(capture: dict[str, Any], *, limit: int = 32) -> dict[str, Any]:
    """Report the slowest observed requests; never join independent clocks or infer ACK."""
    if limit < 1:
        raise ValueError("native ingest report requires a positive request limit")
    spans = capture["spans"]
    roots = {
        span_id
        for span_id, row in spans.items()
        if row["stage"] == "request"
        and row["parent_span_id"] is None
        and row.get("root_kind") == "request"
        and row.get("request_identity") is not None
    }
    custody = (
        capture["status"] == "complete"
        and not capture["health"]
        and not capture["truncated_tail"]
        and capture["orphan_events"] == 0
    )
    # Cache request and nearest phase ownership. Missing parents in a prefix
    # do not establish request custody; such spans are not silently attached.
    ancestry: dict[int, tuple[int | None, str | None]] = {}

    def owner(span_id: int | None) -> tuple[int | None, str | None]:
        trail = []
        cursor = span_id
        while cursor in spans and cursor not in roots and cursor not in ancestry:
            trail.append(cursor)
            cursor = spans[cursor]["parent_span_id"]
        request, phase = ancestry.get(cursor, (cursor if cursor in roots else None, None))
        for item in reversed(trail):
            kind = _kind(spans[item])
            if _is_phase(kind):
                phase = _key(kind)
            ancestry[item] = request, phase
        return request, phase

    def wall(span_id: int) -> int | None:
        row = spans[span_id]
        return None if row["end_ns"] is None else row["end_ns"] - row["start_ns"]

    matched = set()
    unmatched_kernel_spans = 0
    for row in spans.values():
        if _kind(row) in KERNEL_OWNERS:
            request, _ = owner(row["parent_span_id"])
            if request is None:
                unmatched_kernel_spans += 1
            else:
                matched.add(request)
    ordered = sorted(matched, key=lambda key: (wall(key) is None, -(wall(key) or 0), key))
    # Keep open requests visible after the longest closed requests even on
    # failure prefixes that already contain many successful mutations.
    selected = ordered[:limit]
    if len(ordered) > limit and wall(ordered[-1]) is None and wall(selected[-1]) is not None:
        selected[-1] = next(key for key in ordered[limit:] if wall(key) is None)
    groups = {key: defaultdict(list) for key in selected}
    for row in spans.values():
        kind = _kind(row)
        stage, lane = kind
        if (
            not _is_phase(kind)
            and stage not in EXTRA_STAGES
            and (
                stage not in INSTALL_AND_SQL
                or (stage == "sqlite_work" and lane not in SQLITE_LANES)
            )
        ):
            continue
        request, phase = owner(row["parent_span_id"])
        if request in groups:
            groups[request][kind].append((row, phase))
    return {
        "schema_version": 1,
        "status": "COMPLETE_DIAGNOSTIC" if custody else "PARTIAL_DIAGNOSTIC",
        "capture_id": capture["capture_id"],
        "trace_sha256": capture["sha256"],
        "last_observed_ns": capture["last_observed_ns"],
        "capture_health": capture["health"],
        "matched_requests": len(ordered),
        "open_requests": sum(wall(key) is None for key in ordered),
        "omitted_requests": len(ordered) - len(selected),
        "kernel_spans_without_request": unmatched_kernel_spans,
        "requests": [
            {
                "request_span_id": key,
                "request_identity": spans[key]["request_identity"],
                "outcome": spans[key]["outcome"],
                "start_ns": spans[key]["start_ns"],
                "end_ns": spans[key]["end_ns"],
                "wall_ns": wall(key),
                "observed_prefix_ns": (
                    capture["last_observed_ns"] - spans[key]["start_ns"]
                    if wall(key) is None
                    else None
                ),
                "stages": {
                    _key(kind): _group(rows, kind) for kind, rows in sorted(groups[key].items())
                },
                "install_census": managed_install_census(
                    capture,
                    [
                        span["span_id"]
                        for (stage, _), rows in groups[key].items()
                        if stage in MANAGED_INSTALL_CUSTODY_STAGES
                        for span, _ in rows
                        if span["outcome"] == "ok" and spans[key]["outcome"] == "ok"
                    ],
                ),
            }
            for key in selected
        ],
        "scope": {
            "request": "identified daemon request with a Kernel SourceApply/3 or BuildPrepare/3 descendant; not a harness phase or public ACK",
            "clock": "request start and end use the capture clock; an open request has no observed end",
            "wall": "closed span wall unions by stage/lane; overlapping groups and owner buckets are not additive",
            "cpu": "sum of direct thread CPU counters when every span in the group is measured; not host CPU or scheduling delay",
            "reads": "object-port route counts and successful wire bytes; null when incomplete, not physical disk reads",
            "work_usage": "logical WorkBudget usage from direct counters when every span in the group is measured",
            "external_children": "closed ManagedObjectInstall/3 spans only; planned counts and distinct full references belong to one physical group, and group uniques must not be summed into run-wide distinct references; actual reused validations are an independent reached count, including a failed group's prefix; missing values are not zero",
            "install_census": "unique committed managed Catalog registrations and edges, and verified Storage CAS reuse confirmed by Catalog, across custody spans belonging to this request",
            "partial": "observed spans only; open prefix is elapsed until the last capture event, not a completed duration",
            "qualification": "standalone diagnostic; no source/build binding or Off/Record qualification",
        },
    }
