#!/usr/bin/env python3
"""Compare a fresh ``cargo --timings`` summary against a committed baseline.

Designed for the BLD-06 compile-regression CI gate. Reads two JSON files
produced by ``summarize_cargo_timings.py --json``: one baseline (checked into
the repo under ``tools/ci/timing/baselines/``) and one current run (typically
piped in or written by ``just rust-timings-fast``). Reports per-crate deltas
and exits non-zero if any in-repo crate has grown by more than the configured
relative + absolute thresholds.

Why both thresholds: noise on small crates (< 0.2s) easily produces large
relative swings; we only care about meaningful regressions, so a delta must
exceed *both* ``--rel-threshold`` (default 15%) and ``--abs-threshold``
(default 0.10 seconds) to count as a regression.
"""

from __future__ import annotations

import argparse
import json
import math
import sys
from dataclasses import dataclass
from pathlib import Path


@dataclass(frozen=True)
class CrateRow:
    name: str
    duration: float
    units: int


@dataclass(frozen=True)
class TimingEvidence:
    crates: dict[str, CrateRow]
    profile: str
    rustc: str


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("baseline", type=Path, help="committed baseline JSON")
    parser.add_argument(
        "current",
        type=Path,
        help="fresh JSON from `summarize_cargo_timings.py --json`",
    )
    parser.add_argument(
        "--rel-threshold",
        type=float,
        default=0.15,
        help="relative growth threshold (default 0.15 = 15%%)",
    )
    parser.add_argument(
        "--abs-threshold",
        type=float,
        default=0.10,
        help="absolute growth threshold in seconds (default 0.10s)",
    )
    parser.add_argument(
        "--update-baseline",
        action="store_true",
        help="overwrite baseline with current and exit 0",
    )
    return parser.parse_args()


def load_crates(path: Path) -> dict[str, CrateRow]:
    return parse_crates(path.read_bytes(), str(path))


def parse_crates(raw: bytes, source: str) -> dict[str, CrateRow]:
    return parse_evidence(raw, source).crates


def parse_evidence(raw: bytes, source: str) -> TimingEvidence:
    def unique_object(pairs: list[tuple[str, object]]) -> dict[str, object]:
        result: dict[str, object] = {}
        for key, value in pairs:
            if key in result:
                raise ValueError(f"{source}: duplicate JSON key {key!r}")
            result[key] = value
        return result

    def reject_constant(value: str) -> None:
        raise ValueError(f"{source}: non-finite JSON value {value}")

    payload = json.loads(raw, object_pairs_hook=unique_object, parse_constant=reject_constant)
    if not isinstance(payload, dict) or not isinstance(payload.get("top_repo_crates"), list):
        raise ValueError(f"{source}: missing top_repo_crates array")
    rows = payload["top_repo_crates"]
    if not rows:
        raise ValueError(f"{source}: top_repo_crates is empty")
    crates: dict[str, CrateRow] = {}
    for row in rows:
        if not isinstance(row, dict):
            raise ValueError(f"{source}: invalid crate row")
        name, duration, units = row.get("name"), row.get("duration"), row.get("units")
        if not isinstance(name, str) or not name or name in crates:
            raise ValueError(f"{source}: empty or duplicate crate name: {name!r}")
        if (
            isinstance(duration, bool)
            or not isinstance(duration, (int, float))
            or not math.isfinite(duration)
            or duration < 0
        ):
            raise ValueError(f"{source}: invalid duration for {name}")
        if type(units) is not int or units < 1:
            raise ValueError(f"{source}: invalid units for {name}")
        crates[name] = CrateRow(name=name, duration=float(duration), units=units)
    summary = payload.get("summary")
    if not isinstance(summary, dict):
        raise ValueError(f"{source}: missing timing summary")
    profile, rustc = summary.get("profile"), summary.get("rustc")
    if not isinstance(profile, str) or not profile or not isinstance(rustc, str) or not rustc:
        raise ValueError(f"{source}: missing timing profile or compiler identity")
    counts: dict[str, int] = {}
    for field in ("fresh_units", "dirty_units", "total_units"):
        value = summary.get(field)
        if not isinstance(value, str) or not value.isascii() or not value.isdecimal():
            raise ValueError(f"{source}: missing or invalid summary {field}")
        counts[field] = int(value)
    if counts["fresh_units"] != 0:
        raise ValueError(f"{source}: warm timing evidence cannot qualify cold-build regression")
    if counts["dirty_units"] <= 0 or counts["total_units"] != counts["dirty_units"]:
        raise ValueError(f"{source}: inconsistent cold-build unit inventory")
    if sum(row.units for row in crates.values()) > counts["dirty_units"]:
        raise ValueError(f"{source}: crate unit counts exceed compiled unit inventory")
    return TimingEvidence(crates, profile, rustc)


def main() -> int:
    args = parse_args()

    if (
        not math.isfinite(args.rel_threshold)
        or args.rel_threshold < 0
        or not math.isfinite(args.abs_threshold)
        or args.abs_threshold < 0
    ):
        print("timing thresholds must be finite and non-negative", file=sys.stderr)
        return 2

    if args.update_baseline:
        try:
            candidate = args.current.read_bytes()
            parse_crates(candidate, str(args.current))
        except (OSError, ValueError, json.JSONDecodeError) as error:
            print(f"invalid baseline candidate: {error}", file=sys.stderr)
            return 2
        args.baseline.write_bytes(candidate)
        print(f"updated baseline {args.baseline}")
        return 0

    try:
        baseline_evidence = parse_evidence(args.baseline.read_bytes(), str(args.baseline))
        current_evidence = parse_evidence(args.current.read_bytes(), str(args.current))
        baseline = baseline_evidence.crates
        current = current_evidence.crates
    except (OSError, ValueError, json.JSONDecodeError) as error:
        print(f"invalid timing evidence: {error}", file=sys.stderr)
        return 2
    if (baseline_evidence.profile, baseline_evidence.rustc) != (
        current_evidence.profile,
        current_evidence.rustc,
    ):
        print("timing profile or compiler identity differs from baseline", file=sys.stderr)
        return 2
    missing = sorted(set(baseline) - set(current))
    if missing:
        print(
            f"current timing evidence omits baseline crates: {', '.join(missing)}", file=sys.stderr
        )
        return 2

    reduced = sorted(name for name in baseline if current[name].units < baseline[name].units)
    if reduced:
        print(
            f"current timing evidence omits compiled crate units: {', '.join(reduced)}",
            file=sys.stderr,
        )
        return 2

    all_crates = sorted(set(baseline) | set(current))
    regressed: list[tuple[str, float, float, float, float]] = []
    print(f"{'crate':40s} {'baseline':>10s} {'current':>10s} {'delta':>10s} {'rel':>8s}")
    print("-" * 82)
    for name in all_crates:
        base = baseline.get(name)
        cur = current.get(name)
        base_s = base.duration if base else 0.0
        cur_s = cur.duration if cur else 0.0
        delta = cur_s - base_s
        rel = (delta / base_s) if base_s > 0 else float("inf") if delta > 0 else 0.0
        rel_pct = f"{rel * 100:+.1f}%" if base_s > 0 else "  new"
        marker = ""
        if delta > args.abs_threshold and (base_s == 0 or rel > args.rel_threshold):
            regressed.append((name, base_s, cur_s, delta, rel))
            marker = " <-- REGRESSION"
        print(f"{name:40s} {base_s:>9.2f}s {cur_s:>9.2f}s {delta:>+9.2f}s {rel_pct:>8s}{marker}")

    if regressed:
        print()
        print(
            f"FAIL: {len(regressed)} crate(s) regressed by more than "
            f"{args.rel_threshold * 100:.0f}% AND {args.abs_threshold:.2f}s"
        )
        print("To accept a deliberate change: re-run with --update-baseline.")
        return 1

    print()
    print("OK: no compile-time regressions over thresholds.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
