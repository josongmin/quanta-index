#!/usr/bin/env python3
"""Summarize `cargo --timings` HTML output.

This is a thin parser over Cargo's generated HTML so local loops and CI jobs can
extract the high-signal numbers without opening the browser artifact manually.
"""

from __future__ import annotations

import argparse
import json
import re
from dataclasses import asdict, dataclass
from html import unescape
from pathlib import Path


@dataclass(frozen=True)
class UnitSummary:
    name: str
    target: str
    start: float
    duration: float


SUMMARY_LABELS = {
    "Targets": "targets",
    "Profile": "profile",
    "Fresh units": "fresh_units",
    "Dirty units": "dirty_units",
    "Total units": "total_units",
    "Max concurrency": "max_concurrency",
    "Build start": "build_start",
    "Total time": "total_time",
    "rustc": "rustc",
}


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("html_path", type=Path)
    parser.add_argument("--top-units", type=int, default=10)
    parser.add_argument("--top-crates", type=int, default=10)
    parser.add_argument("--repo-prefix", default="quanta-index-")
    parser.add_argument("--json", action="store_true")
    return parser.parse_args()


def strip_tags(value: str) -> str:
    no_tags = re.sub(r"<[^>]+>", " ", value)
    return " ".join(unescape(no_tags).split())


def parse_summary_fields(html: str) -> dict[str, str]:
    summary: dict[str, str] = {}
    for label, key in SUMMARY_LABELS.items():
        match = re.search(
            rf"<td>{re.escape(label)}:</td><td>(.*?)</td>",
            html,
            re.S,
        )
        if match:
            summary[key] = strip_tags(match.group(1))
    return summary


def parse_units(html: str) -> list[UnitSummary]:
    match = re.search(
        r"const UNIT_DATA = (\[.*?\n\]);\nconst CONCURRENCY_DATA =",
        html,
        re.S,
    )
    if not match:
        raise RuntimeError("could not locate UNIT_DATA in cargo timings HTML")
    raw_units = json.loads(match.group(1))
    return [
        UnitSummary(
            name=unit["name"],
            target=unit["target"],
            start=float(unit["start"]),
            duration=float(unit["duration"]),
        )
        for unit in raw_units
    ]


def top_units(units: list[UnitSummary], limit: int) -> list[UnitSummary]:
    return sorted(units, key=lambda unit: unit.duration, reverse=True)[:limit]


def aggregate_repo_crates(
    units: list[UnitSummary], repo_prefix: str, limit: int
) -> list[dict[str, str | float | int]]:
    totals: dict[str, float] = {}
    counts: dict[str, int] = {}
    for unit in units:
        if not unit.name.startswith(repo_prefix):
            continue
        totals[unit.name] = totals.get(unit.name, 0.0) + unit.duration
        counts[unit.name] = counts.get(unit.name, 0) + 1
    ordered = sorted(totals.items(), key=lambda item: item[1], reverse=True)
    return [
        {
            "name": name,
            "duration": duration,
            "units": counts[name],
        }
        for name, duration in ordered[:limit]
    ]


def render_text(
    summary: dict[str, str],
    units: list[UnitSummary],
    crate_totals: list[dict[str, str | float | int]],
) -> str:
    lines = [
        "summary",
        f"  profile: {summary.get('profile', 'unknown')}",
        f"  total_time: {summary.get('total_time', 'unknown')}",
        f"  dirty_units: {summary.get('dirty_units', 'unknown')}",
        f"  fresh_units: {summary.get('fresh_units', 'unknown')}",
        f"  max_concurrency: {summary.get('max_concurrency', 'unknown')}",
        "",
        "top_units",
    ]
    for unit in units:
        lines.append(
            f"  {unit.duration:>6.2f}s start={unit.start:>5.2f}s {unit.name} {unit.target}".rstrip()
        )
    lines.append("")
    lines.append("top_repo_crates")
    for crate in crate_totals:
        lines.append(
            f"  {float(crate['duration']):>6.2f}s units={int(crate['units']):>2d} {crate['name']}"
        )
    return "\n".join(lines)


def main() -> int:
    args = parse_args()
    html = args.html_path.read_text(encoding="utf-8")
    summary = parse_summary_fields(html)
    units = parse_units(html)
    top_unit_list = top_units(units, args.top_units)
    top_repo_crate_list = aggregate_repo_crates(units, args.repo_prefix, args.top_crates)

    if args.json:
        payload = {
            "summary": summary,
            "top_units": [asdict(unit) for unit in top_unit_list],
            "top_repo_crates": top_repo_crate_list,
        }
        print(json.dumps(payload, indent=2))
    else:
        print(render_text(summary, top_unit_list, top_repo_crate_list))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
