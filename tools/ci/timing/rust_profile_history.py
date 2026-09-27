#!/usr/bin/env python3
"""Append and summarize rust profile / cargo lane history."""

from __future__ import annotations

import argparse
import json
import math
import os
import statistics
from collections import defaultdict
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

LOG_RELATIVE_PATH = Path("build-profile") / "history.jsonl"
LOG_SCHEMA_VERSION = 2


@dataclass(frozen=True)
class AggregateRow:
    key: str
    count: int
    failures: int
    total_duration_ms: int
    avg_duration_ms: int
    p50_duration_ms: int
    p95_duration_ms: int
    last_ended_at_utc: str


def utc_now_iso() -> str:
    return datetime.now(timezone.utc).isoformat(timespec="milliseconds")


def default_log_path() -> Path:
    state_root = os.environ.get("QUANTA_INDEX_STATE_ROOT")
    if not state_root:
        raise RuntimeError("QUANTA_INDEX_STATE_ROOT is required")
    return Path(state_root) / LOG_RELATIVE_PATH


def build_common_event(exit_code: int, duration_ms: int) -> dict[str, Any]:
    if type(exit_code) is not int or exit_code < 0:
        raise ValueError("cargo/profile exit code must be a nonnegative integer")
    if type(duration_ms) is not int or duration_ms < 0:
        raise ValueError("cargo/profile duration must be a nonnegative integer")
    return {
        "v": LOG_SCHEMA_VERSION,
        "ts": utc_now_iso(),
        "ms": duration_ms,
        "rc": exit_code,
    }


def append_event(path: Path, payload: dict[str, Any]) -> None:
    import fcntl

    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("a", encoding="utf-8") as handle:
        fcntl.flock(handle.fileno(), fcntl.LOCK_EX)
        try:
            handle.write(json.dumps(payload, sort_keys=True) + "\n")
            handle.flush()
        finally:
            fcntl.flock(handle.fileno(), fcntl.LOCK_UN)


def _unique_object(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    result = {}
    for key, value in pairs:
        if key in result:
            raise ValueError(f"duplicate history key: {key}")
        result[key] = value
    return result


def _reject_json_constant(value: str) -> None:
    raise ValueError(f"invalid JSON constant: {value}")


def _validate_event(event: Any) -> dict[str, Any]:
    if not isinstance(event, dict):
        raise ValueError("history event is not an object")
    if (
        type(event.get("v")) is int
        and event["v"] == LOG_SCHEMA_VERSION
        and "schema_version" not in event
    ):
        kind, timestamp, duration, exit_code = (
            event.get("k"),
            event.get("ts"),
            event.get("ms"),
            event.get("rc"),
        )
    elif (
        type(event.get("schema_version")) is int
        and event["schema_version"] == 1
        and "v" not in event
    ):
        kind, timestamp, duration, exit_code = (
            event.get("event_kind"),
            event.get("recorded_at_utc"),
            event.get("duration_ms"),
            event.get("exit_code"),
        )
    else:
        raise ValueError("unknown or conflicting history schema")
    if kind not in {"cargo", "profile"}:
        raise ValueError("history event kind is missing or invalid")
    if not isinstance(timestamp, str) or not timestamp:
        raise ValueError("history timestamp is missing or invalid")
    if type(duration) is not int or duration < 0:
        raise ValueError("history duration is missing or invalid")
    if type(exit_code) is not int or exit_code < 0:
        raise ValueError("history exit code is missing or invalid")
    key = "lane" if kind == "cargo" else "profile"
    if not isinstance(event.get(key), str) or not event[key]:
        raise ValueError(f"history {key} is missing or invalid")
    return event


def load_events(path: Path) -> list[dict[str, Any]]:
    if not path.exists():
        return []

    events: list[dict[str, Any]] = []
    for line_number, line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
        stripped = line.strip()
        if not stripped:
            raise ValueError(f"history line {line_number} is empty")
        try:
            event = json.loads(
                stripped,
                object_pairs_hook=_unique_object,
                parse_constant=_reject_json_constant,
            )
            events.append(_validate_event(event))
        except (json.JSONDecodeError, ValueError) as error:
            raise ValueError(f"invalid history line {line_number}: {error}") from error
    return events


def percentile(values: list[int], rank: float) -> int:
    if not values:
        return 0
    ordered = sorted(values)
    index = max(0, math.ceil(rank * len(ordered)) - 1)
    return ordered[index]


def event_kind(event: dict[str, Any]) -> str:
    return event["k"] if event.get("v") == LOG_SCHEMA_VERSION else event["event_kind"]


def event_timestamp(event: dict[str, Any]) -> str:
    return event["ts"] if event.get("v") == LOG_SCHEMA_VERSION else event["recorded_at_utc"]


def event_duration_ms(event: dict[str, Any]) -> int:
    return event["ms"] if event.get("v") == LOG_SCHEMA_VERSION else event["duration_ms"]


def event_exit_code(event: dict[str, Any]) -> int:
    return event["rc"] if event.get("v") == LOG_SCHEMA_VERSION else event["exit_code"]


def event_lane_source(event: dict[str, Any]) -> str | None:
    value = event.get("src")
    if value is None:
        value = event.get("lane_source")
    if value is None:
        return None
    return str(value)


def event_cargo_subcommand(event: dict[str, Any]) -> str | None:
    value = event.get("cmd")
    if value is None:
        value = event.get("cargo_subcommand")
    if value is None:
        argv = event.get("argv")
        if isinstance(argv, list) and argv:
            value = argv[0]
    if value is None:
        return None
    return str(value)


def event_recipe(event: dict[str, Any]) -> str | None:
    value = event.get("recipe")
    if value is None:
        value = event.get("delegated_recipe")
    if value is None:
        return None
    return str(value)


def aggregate_rows(
    events: list[dict[str, Any]], kind_name: str, key_field: str
) -> list[AggregateRow]:
    grouped: dict[str, list[dict[str, Any]]] = defaultdict(list)
    for event in events:
        if event_kind(event) != kind_name:
            continue
        key = event[key_field]
        grouped[key].append(event)

    rows: list[AggregateRow] = []
    for key, items in grouped.items():
        durations = [event_duration_ms(item) for item in items]
        last_ended = max(event_timestamp(item) for item in items)
        failures = sum(1 for item in items if event_exit_code(item) != 0)
        total = sum(durations)
        rows.append(
            AggregateRow(
                key=key,
                count=len(items),
                failures=failures,
                total_duration_ms=total,
                avg_duration_ms=int(statistics.fmean(durations)),
                p50_duration_ms=percentile(durations, 0.50),
                p95_duration_ms=percentile(durations, 0.95),
                last_ended_at_utc=last_ended,
            )
        )

    return sorted(
        rows,
        key=lambda row: (row.failures, row.total_duration_ms, row.count),
        reverse=True,
    )


def latest_failures(events: list[dict[str, Any]], limit: int) -> list[dict[str, Any]]:
    failures = [event for event in events if event_exit_code(event) != 0]
    ordered = sorted(failures, key=event_timestamp, reverse=True)
    return [normalize_failure(event) for event in ordered[:limit]]


def normalize_failure(event: dict[str, Any]) -> dict[str, Any]:
    kind = event_kind(event)
    payload: dict[str, Any] = {
        "recorded_at_utc": event_timestamp(event),
        "event_kind": kind,
        "exit_code": event_exit_code(event),
        "duration_ms": event_duration_ms(event),
    }
    if kind == "cargo":
        payload["lane"] = str(event.get("lane", "unknown"))
        lane_source = event_lane_source(event)
        if lane_source is not None:
            payload["lane_source"] = lane_source
        cargo_subcommand = event_cargo_subcommand(event)
        if cargo_subcommand is not None:
            payload["cargo_subcommand"] = cargo_subcommand
    if kind == "profile":
        payload["profile"] = str(event.get("profile", "unknown"))
        recipe = event_recipe(event)
        if recipe is not None:
            payload["delegated_recipe"] = recipe
    return payload


def render_rows(title: str, rows: list[AggregateRow]) -> list[str]:
    lines = [title]
    if not rows:
        lines.append("  none")
        return lines

    for row in rows:
        lines.append(
            "  "
            f"{row.key} "
            f"count={row.count} "
            f"failures={row.failures} "
            f"avg={row.avg_duration_ms}ms "
            f"p50={row.p50_duration_ms}ms "
            f"p95={row.p95_duration_ms}ms "
            f"last={row.last_ended_at_utc}"
        )
    return lines


def render_summary(events: list[dict[str, Any]], failure_limit: int) -> str:
    profile_rows = aggregate_rows(events, "profile", "profile")
    cargo_rows = aggregate_rows(events, "cargo", "lane")
    failures = latest_failures(events, failure_limit)

    lines = [
        f"log_path: {default_log_path()}",
        f"total_events: {len(events)}",
        "",
    ]
    lines.extend(render_rows("profiles", profile_rows))
    lines.append("")
    lines.extend(render_rows("cargo_lanes", cargo_rows))
    lines.append("")
    lines.append("latest_failures")
    if not failures:
        lines.append("  none")
    else:
        for failure in failures:
            key = failure.get("profile") or failure.get("lane") or "unknown"
            detail = failure.get("delegated_recipe") or failure.get("cargo_subcommand")
            detail_text = f" detail={detail}" if detail else ""
            lines.append(
                "  "
                f"{failure.get('recorded_at_utc', '')} "
                f"{failure.get('event_kind', 'unknown')} "
                f"{key} "
                f"exit={failure.get('exit_code', 'unknown')} "
                f"duration={failure.get('duration_ms', 'unknown')}ms"
                f"{detail_text}"
            )
    return "\n".join(lines)


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser()
    subparsers = parser.add_subparsers(dest="command", required=True)

    append_cargo = subparsers.add_parser("append-cargo")
    append_cargo.add_argument("lane")
    append_cargo.add_argument("lane_source")
    append_cargo.add_argument("exit_code", type=int)
    append_cargo.add_argument("duration_ms", type=int)
    append_cargo.add_argument("cargo_subcommand", nargs="?")

    append_profile = subparsers.add_parser("append-profile")
    append_profile.add_argument("profile")
    append_profile.add_argument("delegated_recipe")
    append_profile.add_argument("exit_code", type=int)
    append_profile.add_argument("duration_ms", type=int)

    summary = subparsers.add_parser("summary")
    summary.add_argument("--json", action="store_true")
    summary.add_argument("--failure-limit", type=int, default=10)

    return parser


def handle_append_cargo(args: argparse.Namespace) -> int:
    payload = build_common_event(args.exit_code, args.duration_ms)
    payload.update(
        {
            "k": "cargo",
            "lane": args.lane,
            "src": args.lane_source,
        }
    )
    if args.cargo_subcommand:
        payload["cmd"] = args.cargo_subcommand
    append_event(default_log_path(), payload)
    return 0


def handle_append_profile(args: argparse.Namespace) -> int:
    payload = build_common_event(args.exit_code, args.duration_ms)
    payload.update(
        {
            "k": "profile",
            "profile": args.profile,
            "recipe": args.delegated_recipe,
        }
    )
    append_event(default_log_path(), payload)
    return 0


def handle_summary(args: argparse.Namespace) -> int:
    path = default_log_path()
    events = load_events(path)
    if args.json:
        payload = {
            "log_path": str(path),
            "total_events": len(events),
            "profiles": [row.__dict__ for row in aggregate_rows(events, "profile", "profile")],
            "cargo_lanes": [row.__dict__ for row in aggregate_rows(events, "cargo", "lane")],
            "latest_failures": latest_failures(events, args.failure_limit),
        }
        print(json.dumps(payload, indent=2, sort_keys=True))
    else:
        print(render_summary(events, args.failure_limit))
    return 0


def main() -> int:
    args = build_parser().parse_args()
    try:
        if args.command == "append-cargo":
            return handle_append_cargo(args)
        if args.command == "append-profile":
            return handle_append_profile(args)
        if args.command == "summary":
            return handle_summary(args)
    except (OSError, ValueError) as error:
        raise SystemExit(f"rust profile history refused: {error}") from error
    raise RuntimeError(f"unknown command: {args.command}")


if __name__ == "__main__":
    raise SystemExit(main())
