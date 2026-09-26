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
    payload = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(payload, dict) or not isinstance(payload.get("top_repo_crates"), list):
        raise ValueError(f"{path}: missing top_repo_crates array")
    rows = payload["top_repo_crates"]
    if not rows:
        raise ValueError(f"{path}: top_repo_crates is empty")
    crates: dict[str, CrateRow] = {}
    for row in rows:
        if not isinstance(row, dict):
            raise ValueError(f"{path}: invalid crate row")
        name, duration, units = row.get("name"), row.get("duration"), row.get("units")
        if not isinstance(name, str) or not name or name in crates:
            raise ValueError(f"{path}: empty or duplicate crate name: {name!r}")
        if isinstance(duration, bool) or not isinstance(duration, (int, float)) or not math.isfinite(duration) or duration < 0:
            raise ValueError(f"{path}: invalid duration for {name}")
        if type(units) is not int or units < 1:
            raise ValueError(f"{path}: invalid units for {name}")
        crates[name] = CrateRow(name=name, duration=float(duration), units=units)
    return crates


def main() -> int:
    args = parse_args()

    if args.update_baseline:
        args.baseline.write_text(
            args.current.read_text(encoding="utf-8"),
            encoding="utf-8",
        )
        print(f"updated baseline {args.baseline}")
        return 0

    try:
        baseline = load_crates(args.baseline)
        current = load_crates(args.current)
    except (OSError, ValueError, json.JSONDecodeError) as error:
        print(f"invalid timing evidence: {error}", file=sys.stderr)
        return 2
    missing = sorted(set(baseline) - set(current))
    if missing:
        print(f"current timing evidence omits baseline crates: {', '.join(missing)}", file=sys.stderr)
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
