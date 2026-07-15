#!/usr/bin/env python3
"""Write a schema-validated, immutable-by-digest test execution receipt."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import subprocess
from pathlib import Path


def _revision() -> str:
    value = os.environ.get("GITHUB_SHA", "").strip()
    if value:
        return value
    return subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip()


def _test_event_count(evidence: Path) -> int:
    count = 0
    for line in evidence.read_text(encoding="utf-8").splitlines():
        try:
            event = json.loads(line)
        except json.JSONDecodeError as error:
            raise SystemExit(f"invalid nextest JSON evidence {evidence}: {error}") from error
        # nextest emits a `started` and a terminal event for each test. Count
        # only terminal outcomes; otherwise a healthy run is reported twice.
        if event.get("type") == "test" and event.get("event") in {
            "ok",
            "failed",
            "ignored",
            "timeout",
        }:
            count += 1
    if count == 0:
        raise SystemExit(f"nextest evidence has no test events: {evidence}")
    return count


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--rail", required=True)
    parser.add_argument("--tier", required=True, choices=("pr", "merge", "correctness", "nightly", "weekly"))
    parser.add_argument("--command", required=True)
    parser.add_argument("--evidence", required=True, type=Path)
    parser.add_argument("--out", required=True, type=Path)
    args = parser.parse_args()
    evidence = args.evidence.resolve()
    if not evidence.is_file():
        raise SystemExit(f"missing test evidence: {evidence}")
    digest = hashlib.sha256(evidence.read_bytes()).hexdigest()
    receipt = {
        "schema_version": 1,
        "revision": _revision(),
        "rail": args.rail,
        "tier": args.tier,
        "command": args.command,
        "evidence_path": args.evidence.as_posix(),
        "evidence_sha256": digest,
        "test_event_count": _test_event_count(evidence),
    }
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(json.dumps(receipt, sort_keys=True, indent=2) + "\n", encoding="utf-8")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
