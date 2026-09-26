#!/usr/bin/env python3
"""J7Q-08 quality integration summary.

Aggregates the per-dimension search-quality rails into one integration summary
*without erasing dimension boundaries*. Each dimension keeps its own claim type
and an explicit blocking status. The canonical ``quality-full`` profile supplies
the live family set, so the aggregate cannot silently omit a newly registered
authority rail (per MEASUREMENT_MATRIX.md / NO-GO-RULES.md).

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
SCRIPT_DIR = Path(__file__).resolve().parent
if str(SCRIPT_DIR) not in sys.path:
    sys.path.insert(0, str(SCRIPT_DIR))

from manifest import ManifestError, load_manifest  # noqa: E402

QUALITY_PROFILE = "quality-full"
# The manifest owns the live family set and artifact globs. Tickets stay here
# because they are report labels, not producer or artifact authority.
QUALITY_TICKETS = {
    "relevance": "J7Q-01A",
    "ambiguity": "J7Q-06",
    "snippet": "J7Q-02",
    "scale": "J7Q-03",
    "tail": "J7Q-04",
    "ann": "QI-BB-027",
    "concurrency": "QI-BB-010",
    "freshness": "BQ-05",
    "open-loop": "BQ-06",
    "ops": "J7Q-05",
    "ui": "J7Q-07",
}
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


def validate_evidence() -> int:
    """Apply the canonical artifact gate before publishing an aggregate verdict."""
    validator = SCRIPT_DIR.parent / "ci" / "lint" / "check-bench-artifacts.py"
    return subprocess.run(
        [
            sys.executable,
            str(validator),
            "--repo-root",
            str(ROOT),
            "--profile",
            QUALITY_PROFILE,
            "--require",
            "--require-clean-worktree",
            "--skip-baselines",
        ],
        cwd=ROOT,
        check=False,
    ).returncode


def quality_dimensions() -> list[tuple[str, str, str]]:
    """Resolve the aggregate's live families from the canonical manifest."""
    try:
        manifest = load_manifest(ROOT / "tools" / "benchmark" / "registry.toml")
    except ManifestError as exc:
        raise RuntimeError(f"invalid benchmark manifest: {exc}") from exc
    profiles = manifest["profiles"]
    families = manifest["families"]
    assert isinstance(profiles, dict) and isinstance(families, dict)
    profile = profiles.get(QUALITY_PROFILE)
    if not isinstance(profile, dict):
        raise RuntimeError(f"benchmark manifest has no {QUALITY_PROFILE!r} profile")
    names = profile["families"]
    assert isinstance(names, list)
    name_set = set(names)
    missing_tickets = sorted(name_set - set(QUALITY_TICKETS))
    stale_tickets = sorted(set(QUALITY_TICKETS) - name_set)
    if missing_tickets or stale_tickets:
        details = []
        if missing_tickets:
            details.append(f"missing ticket(s): {', '.join(missing_tickets)}")
        if stale_tickets:
            details.append(f"stale ticket(s): {', '.join(stale_tickets)}")
        raise RuntimeError(f"quality summary metadata drift ({'; '.join(details)})")
    dimensions: list[tuple[str, str, str]] = []
    for name in names:
        assert isinstance(name, str)
        family = families[name]
        assert isinstance(family, dict)
        artifact_glob = family["artifact_glob"]
        assert isinstance(artifact_glob, str)
        dimensions.append((name, QUALITY_TICKETS[name], artifact_glob))
    return dimensions


def load_summaries(artifact_glob: str) -> list[tuple[Path, dict]]:
    paths = sorted(ROOT.glob(artifact_glob))
    summaries: list[tuple[Path, dict]] = []
    for path in paths:
        try:
            summaries.append((path, json.loads(path.read_text(encoding="utf-8"))))
        except (OSError, json.JSONDecodeError):
            return []
    return summaries


def rail_verdict(summary: dict, *, dimension: str, head: str) -> bool | None:
    """The rail's pass/fail verdict, wherever the artifact keeps it.

    Every live rail's ``summary.json`` is a schema-2 ``BenchArtifactV1`` whose
    dimension-specific verdict lives under ``detail.passed``. A schema-1
    top-level ``passed`` field is deliberately not accepted: it lacks the
    common provenance/resource envelope and cannot close an aggregate rail.
    """
    if summary.get("schema_version") == 2:
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
    return None


def build() -> tuple[dict, bool]:
    head = resolve_head()
    rows = []
    all_live_passed = True
    for dimension, ticket, artifact_glob in quality_dimensions():
        row = {
            "dimension": dimension,
            "owner_ticket": ticket,
            "blocking": True,
            "status": "live",
        }
        summaries = load_summaries(artifact_glob)
        if (
            dimension == "concurrency"
            and {path.name for path, _ in summaries} != CONCURRENCY_ARTIFACTS
        ):
            row["passed"] = False
            row["error"] = "required concurrency c1/c8/c32 artifacts incomplete"
            all_live_passed = False
        elif not summaries:
            # A registered dimension without its artifact is a fail-closed
            # integration error, not a silent pass.
            row["passed"] = False
            row["error"] = "declared live but summary artifact missing"
            all_live_passed = False
        else:
            row["artifacts"] = [
                str(path.relative_to(ROOT)) if path.is_relative_to(ROOT) else str(path)
                for path, _ in summaries
            ]
            verdicts = [
                rail_verdict(summary, dimension=dimension, head=head) for _, summary in summaries
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
        rows.append(row)

    doc = {
        "schema_version": 1,
        "dimension": "integration",
        "git_head": head,
        "live_dimensions_passed": all_live_passed,
        "note": (
            "aggregate of every registered quality-full dimension; this summary "
            "is not a substitute for per-dimension closeout"
        ),
        "dimensions": rows,
    }
    return doc, all_live_passed


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--out", required=True)
    args = parser.parse_args()

    validation = validate_evidence()
    if validation:
        print("quality integration RED: required evidence did not validate", file=sys.stderr)
        return validation
    try:
        doc, all_live_passed = build()
    except RuntimeError as exc:
        print(f"quality integration RED: {exc}", file=sys.stderr)
        return 2
    out = Path(args.out)
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(json.dumps(doc, indent=2) + "\n")

    print(f"quality integration: {len(doc['dimensions'])} live")
    for d in doc["dimensions"]:
        mark = "PASS" if d["passed"] else "FAIL"
        print(f"  [{mark}] {d['dimension']} ({d['owner_ticket']})")
    if not all_live_passed:
        print("quality integration RED: a live dimension failed", file=sys.stderr)
        return 1
    print("quality integration green for live dimensions")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
