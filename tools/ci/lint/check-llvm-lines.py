#!/usr/bin/env python3
"""Monomorphization regression guard via `cargo llvm-lines`.

The serde-derive ban exists to keep cold-build wall-clock bounded. This script
verifies the *result* of that policy by snapshotting per-package
monomorphization output (LLVM IR line counts) and failing PRs that exceed the
baseline by more than `TOLERANCE_RATIO`.

Usage:
    check-llvm-lines.py                   # CI mode: measure + compare baseline
    check-llvm-lines.py --update-baseline # write a new baseline (intentional)

Requires:
    cargo install cargo-llvm-lines
"""

from __future__ import annotations

import argparse
import json
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
BASELINE_PATH = ROOT / "tools" / "ci" / "lint" / "baselines" / "llvm-lines.json"

TARGET_PACKAGES: list[str] = [
    "quanta-index-contract",
    "quanta-index-core",
]

TOLERANCE_RATIO = 1.15  # fail if total LLVM lines grow > 15% over baseline
TOLERANCE_ABS = 5_000   # ignore absolute deltas under 5k lines (noise floor)


def measure_llvm_lines(package: str) -> int:
    """Return total LLVM IR lines reported for `package` (release profile)."""
    cmd = ["cargo", "llvm-lines", "--release", "-p", package, "--lib"]
    result = subprocess.run(cmd, cwd=ROOT, capture_output=True, text=True)
    if result.returncode != 0:
        raise RuntimeError(
            f"cargo llvm-lines failed for {package}: {result.stderr}"
        )
    return parse_total(result.stdout)


def parse_total(stdout: str) -> int:
    """Parse the trailing `(TOTAL)` line from cargo-llvm-lines output."""
    # cargo-llvm-lines prints something like:
    #     30000   500  (TOTAL)
    for line in reversed(stdout.splitlines()):
        if "(TOTAL)" in line:
            match = re.match(r"\s*(\d+)\s+\d+\s+\(TOTAL\)", line)
            if match:
                return int(match.group(1))
    raise RuntimeError("could not locate (TOTAL) line in cargo-llvm-lines output")


def load_baseline() -> dict[str, int]:
    if not BASELINE_PATH.exists():
        return {}
    return json.loads(BASELINE_PATH.read_text(encoding="utf-8"))


def write_baseline(values: dict[str, int]) -> None:
    BASELINE_PATH.parent.mkdir(parents=True, exist_ok=True)
    BASELINE_PATH.write_text(json.dumps(values, indent=2, sort_keys=True) + "\n")


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--update-baseline", action="store_true")
    parser.add_argument(
        "--packages",
        nargs="*",
        default=TARGET_PACKAGES,
        help="packages to measure (default: contract + core)",
    )
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    measured = {pkg: measure_llvm_lines(pkg) for pkg in args.packages}

    if args.update_baseline:
        write_baseline(measured)
        for pkg, lines in measured.items():
            print(f"baseline {pkg}: {lines}")
        return 0

    baseline = load_baseline()
    if not baseline:
        print(
            f"no baseline at {BASELINE_PATH}. Run with --update-baseline once.",
            file=sys.stderr,
        )
        return 2

    bad = False
    for pkg, lines in measured.items():
        base = baseline.get(pkg)
        if base is None:
            print(f"{pkg}: no baseline entry (current: {lines})", file=sys.stderr)
            bad = True
            continue
        delta = lines - base
        ratio = lines / base if base else float("inf")
        status = "ok"
        if delta > TOLERANCE_ABS and ratio > TOLERANCE_RATIO:
            status = "FAIL"
            bad = True
        print(
            f"{pkg}: {lines} (baseline {base}, "
            f"delta {delta:+d}, ratio {ratio:.2f}) [{status}]"
        )

    if bad:
        print(
            "\nMonomorphization budget exceeded. Investigate the new generic "
            "instantiation, or run with --update-baseline if intentional.",
            file=sys.stderr,
        )
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
