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
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
ARTIFACT_ROOT = ROOT / "artifacts" / "search-quality"

# (dimension, owner_ticket, blocking, status, artifact filename/glob). `live`
# dimensions must have a passing current-source artifact; `pending` dimensions
# are declared but not yet implemented.
DIMENSIONS = [
    ("relevance", "J7Q-01A", True, "live", "summary.json"),
    ("ambiguity", "J7Q-06", True, "live", "summary.json"),
    ("snippet", "J7Q-02", True, "live", "summary.json"),
    ("scale", "J7Q-03", True, "live", "summary.json"),
    ("tail", "J7Q-04", True, "live", "summary.json"),
    ("ann", "QI-BB-027", True, "live", "summary.json"),
    ("concurrency", "QI-BB-010", True, "live", "summary-c*.json"),
    ("freshness", "BQ-05", True, "live", "summary.json"),
    ("open-loop", "BQ-06", True, "live", "summary.json"),
    ("ops", "J7Q-05", True, "live", "summary.json"),
    ("ui", "J7Q-07", True, "live", "summary.json"),
]
FULL_HEAD_RE = re.compile(r"^[0-9a-f]{40}$")
CONCURRENCY_ARTIFACTS = {"summary-c1.json", "summary-c8.json", "summary-c32.json"}


def resolve_head() -> str:
    completed = subprocess.run(
        ["git", "-C", str(ROOT), "rev-parse", "HEAD"],
        check=False,
        capture_output=True,
        text=True,
    )
    head = completed.stdout.strip()
    if completed.returncode != 0 or not FULL_HEAD_RE.match(head):
        raise RuntimeError("cannot resolve a full 40-character checkout HEAD")
    return head


def load_summaries(dimension: str, filename: str) -> list[tuple[Path, dict]]:
    paths = sorted((ARTIFACT_ROOT / dimension / "latest").glob(filename))
    summaries: list[tuple[Path, dict]] = []
    for path in paths:
        try:
            summaries.append((path, json.loads(path.read_text(encoding="utf-8"))))
        except (OSError, json.JSONDecodeError):
            return []
    return summaries


def rail_verdict(summary: dict, *, dimension: str, head: str) -> bool | None:
    """The rail's pass/fail verdict, wherever the artifact keeps it.

    A benchmark rail's ``summary.json`` is a ``BenchArtifactV1`` (schema 2,
    QI-BB-010) whose dimension-specific verdict lives under ``detail.passed``;
    the verdict-record rails keep ``passed`` at the top level. Neither shape
    is defaulted: an artifact with no verdict is ``None``.
    """
    if isinstance(summary.get("schema_version"), int) and summary["schema_version"] >= 2:
        provenance = summary.get("provenance")
        if (
            summary.get("dimension") != dimension
            or not isinstance(provenance, dict)
            or provenance.get("git_head") != head
        ):
            return None
        detail = summary.get("detail")
        if isinstance(detail, dict) and isinstance(detail.get("passed"), bool):
            return detail["passed"]
        return None
    if (
        summary.get("schema_version") == 1
        and summary.get("dimension") == dimension
        and summary.get("git_rev") == head
        and isinstance(summary.get("passed"), bool)
    ):
        return summary["passed"]
    return None


def build() -> tuple[dict, bool]:
    head = resolve_head()
    rows = []
    all_live_passed = True
    for dimension, ticket, blocking, status, filename in DIMENSIONS:
        row = {
            "dimension": dimension,
            "owner_ticket": ticket,
            "blocking": blocking,
            "status": status,
        }
        if status == "live":
            summaries = load_summaries(dimension, filename)
            if (
                dimension == "concurrency"
                and {path.name for path, _ in summaries} != CONCURRENCY_ARTIFACTS
            ):
                row["passed"] = False
                row["error"] = "required concurrency c1/c8/c32 artifacts incomplete"
                all_live_passed = False
            elif not summaries:
                # A dimension declared live but missing its artifact is a
                # fail-closed integration error, not a silent pass.
                row["passed"] = False
                row["error"] = "declared live but summary artifact missing"
                all_live_passed = False
            else:
                row["artifacts"] = [
                    str(path.relative_to(ROOT)) if path.is_relative_to(ROOT) else str(path)
                    for path, _ in summaries
                ]
                verdicts = [
                    rail_verdict(summary, dimension=dimension, head=head)
                    for _, summary in summaries
                ]
                if any(verdict is None for verdict in verdicts):
                    # A parsed artifact without a verdict is malformed: surface it
                    # as a flagged failure rather than silently scoring it FAIL.
                    passed = False
                    row["error"] = "artifact present but missing 'passed' verdict"
                else:
                    passed = all(verdicts)
                row["passed"] = passed
                if not passed:
                    all_live_passed = False
        else:
            row["passed"] = None  # pending: explicitly not evaluated
        rows.append(row)

    doc = {
        "schema_version": 1,
        "dimension": "integration",
        "git_head": head,
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

    try:
        doc, all_live_passed = build()
    except RuntimeError as exc:
        print(f"quality integration RED: {exc}", file=sys.stderr)
        return 2
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
