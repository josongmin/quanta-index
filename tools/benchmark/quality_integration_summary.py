#!/usr/bin/env python3
"""J7Q-08 quality integration summary.

Aggregates the per-dimension search-quality rails into one integration summary
*without erasing dimension boundaries*. Each dimension keeps its own claim type
and blocking/advisory status. Dimensions whose rail is not yet implemented are
recorded as ``pending`` — never as passing — so the aggregate can never overclaim
quality closure from partial rails (per MEASUREMENT_MATRIX.md / NO-GO-RULES.md).

This script is read-only over the per-dimension ``summary.json`` artifacts; it
does not run rails itself (the Justfile recipe runs the live rails first).
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
ARTIFACT_ROOT = ROOT / "artifacts" / "search-quality"

# (dimension, owner_ticket, blocking, status). `live` dimensions must have a
# passing artifact; `pending` dimensions are declared but not yet implemented.
DIMENSIONS = [
    ("relevance", "J7Q-01A", True, "live"),
    ("ambiguity", "J7Q-06", True, "live"),
    ("snippet", "J7Q-02", True, "live"),
    ("scale", "J7Q-03", True, "live"),
    ("tail", "J7Q-04", True, "live"),
    ("ops", "J7Q-05", True, "live"),
    ("ui", "J7Q-07", True, "live"),
]


def load_summary(dimension: str) -> dict | None:
    path = ARTIFACT_ROOT / dimension / "latest" / "summary.json"
    if not path.is_file():
        return None
    try:
        return json.loads(path.read_text())
    except (OSError, json.JSONDecodeError):
        return None


def build() -> tuple[dict, bool]:
    rows = []
    all_live_passed = True
    for dimension, ticket, blocking, status in DIMENSIONS:
        row = {
            "dimension": dimension,
            "owner_ticket": ticket,
            "blocking": blocking,
            "status": status,
        }
        if status == "live":
            summary = load_summary(dimension)
            if summary is None:
                # A dimension declared live but missing its artifact is a
                # fail-closed integration error, not a silent pass.
                row["passed"] = False
                row["error"] = "declared live but summary artifact missing"
                all_live_passed = False
            else:
                row["artifact"] = str(
                    (ARTIFACT_ROOT / dimension / "latest" / "summary.json").relative_to(ROOT)
                )
                if "passed" not in summary:
                    # A parsed artifact without a 'passed' key is malformed: surface
                    # it as a flagged failure rather than silently scoring it FAIL.
                    passed = False
                    row["error"] = "artifact present but missing 'passed' key"
                else:
                    passed = bool(summary["passed"])
                row["passed"] = passed
                if not passed:
                    all_live_passed = False
        else:
            row["passed"] = None  # pending: explicitly not evaluated
        rows.append(row)

    doc = {
        "schema_version": 1,
        "dimension": "integration",
        "live_dimensions_passed": all_live_passed,
        "note": (
            "aggregate of live per-dimension rails; pending dimensions are NOT "
            "evaluated and this summary is not a substitute for per-dimension closeout"
        ),
        "dimensions": rows,
    }
    return doc, all_live_passed


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--out", required=True)
    args = parser.parse_args()

    doc, all_live_passed = build()
    out = Path(args.out)
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(json.dumps(doc, indent=2) + "\n")

    live = [d for d in doc["dimensions"] if d["status"] == "live"]
    pending = [d for d in doc["dimensions"] if d["status"] == "pending"]
    print(f"quality integration: {len(live)} live, {len(pending)} pending")
    for d in doc["dimensions"]:
        mark = {True: "PASS", False: "FAIL", None: "pending"}[d["passed"]]
        print(f"  [{mark}] {d['dimension']} ({d['owner_ticket']})")
    if not all_live_passed:
        print("quality integration RED: a live dimension failed", file=sys.stderr)
        return 1
    print("quality integration green for live dimensions")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
