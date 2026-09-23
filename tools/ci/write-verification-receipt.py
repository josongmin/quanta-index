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
    status = subprocess.check_output(
        ["git", "status", "--porcelain=v1", "--untracked-files=all"], text=True
    )
    dirty = []
    for line in status.splitlines():
        path = line[3:]
        if line.startswith("?? ") and path.startswith("artifacts/"):
            continue
        dirty.append(line)
    if dirty:
        sample = ", ".join(dirty[:5])
        raise SystemExit(f"refusing verification receipt from dirty source: {sample}")
    value = os.environ.get("GITHUB_SHA", "").strip()
    if value:
        return value
    return subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip()


def _nextest_evidence_summary(evidence: Path) -> tuple[str, int]:
    digest = hashlib.sha256()
    counts = {"ok": 0, "failed": 0, "ignored": 0, "timeout": 0}
    suites_started = 0
    suites_finished = 0
    suites_passed = 0
    with evidence.open("rb") as stream:
        for line in stream:
            digest.update(line)
            try:
                event = json.loads(line)
            except (UnicodeDecodeError, json.JSONDecodeError) as error:
                raise SystemExit(f"invalid nextest JSON evidence {evidence}: {error}") from error
            if not isinstance(event, dict):
                raise SystemExit(f"invalid nextest JSON event in {evidence}: expected an object")
            if event.get("type") not in {"suite", "test"}:
                raise SystemExit(f"unknown nextest event type in {evidence}: {event.get('type')!r}")
            if event.get("type") == "suite":
                outcome = event.get("event")
                if outcome == "started":
                    suites_started += 1
                elif outcome == "ok":
                    suites_finished += 1
                    passed = event.get("passed")
                    failed = event.get("failed")
                    if (
                        not isinstance(passed, int)
                        or isinstance(passed, bool)
                        or passed < 0
                        or not isinstance(failed, int)
                        or isinstance(failed, bool)
                        or failed < 0
                    ):
                        raise SystemExit(f"nextest suite has invalid counts: {evidence}")
                    if failed:
                        raise SystemExit(f"nextest suite reports failures: {evidence}")
                    suites_passed += passed
                elif outcome == "failed":
                    raise SystemExit(f"nextest suite failed: {evidence}")
                else:
                    raise SystemExit(f"unknown nextest suite outcome in {evidence}: {outcome!r}")
            # Nextest emits a `started` and a terminal event for each test.
            # Count only terminal outcomes, not both records for one test.
            if event.get("type") == "test":
                outcome = event.get("event")
                if isinstance(outcome, str) and outcome in counts:
                    counts[outcome] += 1
                elif outcome != "started":
                    raise SystemExit(f"unknown nextest test outcome in {evidence}: {outcome!r}")
    if counts["failed"] or counts["timeout"]:
        raise SystemExit(
            f"nextest evidence contains failed or timed-out tests: {evidence} "
            f"(failed={counts['failed']}, timeout={counts['timeout']})"
        )
    if suites_started == 0 or suites_finished != suites_started:
        raise SystemExit(
            f"nextest evidence has incomplete suite events: {evidence} "
            f"(started={suites_started}, finished={suites_finished})"
        )
    if suites_passed != counts["ok"]:
        raise SystemExit(
            f"nextest suite/test pass counts disagree: {evidence} "
            f"(suite={suites_passed}, test={counts['ok']})"
        )
    if not counts["ok"]:
        raise SystemExit(f"nextest evidence has no passing tests: {evidence}")
    return digest.hexdigest(), sum(counts.values())


def _summary_json_evidence_summary(evidence: Path) -> tuple[str, int]:
    raw = evidence.read_bytes()
    try:
        payload = json.loads(raw)
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        raise SystemExit(f"invalid summary JSON evidence {evidence}: {error}") from error
    if not isinstance(payload, dict):
        raise SystemExit(f"invalid summary JSON evidence {evidence}: expected an object")
    required = {"command", "selected", "executed", "passed", "failed"}
    missing = sorted(required - payload.keys())
    if missing:
        raise SystemExit(
            f"summary JSON evidence is missing required fields in {evidence}: {', '.join(missing)}"
        )
    if not isinstance(payload["command"], str) or not payload["command"].strip():
        raise SystemExit(f"summary JSON evidence has invalid command: {evidence}")
    for key in ("selected", "executed", "passed", "failed"):
        value = payload[key]
        if type(value) is not int or value < 0:
            raise SystemExit(
                f"summary JSON evidence has invalid {key} count in {evidence}: {value!r}"
            )
    selected = payload["selected"]
    executed = payload["executed"]
    passed = payload["passed"]
    failed = payload["failed"]
    if failed:
        raise SystemExit(f"summary JSON evidence reports failures: {evidence}")
    if executed < 1 or passed < 1:
        raise SystemExit(f"summary JSON evidence has no passing tests: {evidence}")
    if passed + failed != executed:
        raise SystemExit(f"summary JSON evidence has inconsistent execution counts: {evidence}")
    if executed > selected:
        raise SystemExit(f"summary JSON evidence executed more tests than selected: {evidence}")
    return hashlib.sha256(raw).hexdigest(), executed


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--rail", required=True)
    parser.add_argument(
        "--tier", required=True, choices=("pr", "merge", "correctness", "nightly", "weekly")
    )
    parser.add_argument("--command", required=True)
    parser.add_argument(
        "--evidence-format",
        choices=("nextest-jsonl", "summary-json"),
        default="nextest-jsonl",
    )
    parser.add_argument("--evidence", required=True, type=Path)
    parser.add_argument("--out", required=True, type=Path)
    args = parser.parse_args()
    evidence = args.evidence.resolve()
    if not evidence.is_file():
        raise SystemExit(f"missing test evidence: {evidence}")
    revision = _revision()
    if args.evidence_format == "summary-json":
        digest, test_event_count = _summary_json_evidence_summary(evidence)
    else:
        digest, test_event_count = _nextest_evidence_summary(evidence)
    receipt = {
        "schema_version": 1,
        "revision": revision,
        "rail": args.rail,
        "tier": args.tier,
        "command": args.command,
        "evidence_path": args.evidence.as_posix(),
        "evidence_sha256": digest,
        "test_event_count": test_event_count,
    }
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(json.dumps(receipt, sort_keys=True, indent=2) + "\n", encoding="utf-8")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
