#!/usr/bin/env python3
"""Build the retrieval SDK proof summary from machine evidence."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path

PROOF_TEST = "actual_runner_binary_emits_receipt_bound_v3_record"


def _hex64(value: object, label: str) -> str:
    if not isinstance(value, str) or len(value) != 64 or any(
        char not in "0123456789abcdef" for char in value
    ):
        raise SystemExit(f"{label} must be a lowercase sha256")
    return value


def _nextest_counts(path: Path) -> tuple[int, int, int, int]:
    counts = {"ok": 0, "failed": 0, "ignored": 0, "timeout": 0}
    suites_started = 0
    suites_finished = 0
    proof_passed = False
    with path.open("rb") as stream:
        for line in stream:
            try:
                event = json.loads(line)
            except (UnicodeDecodeError, json.JSONDecodeError) as error:
                raise SystemExit(f"invalid nextest JSON evidence {path}: {error}") from error
            if not isinstance(event, dict) or event.get("type") not in {"suite", "test"}:
                raise SystemExit(f"invalid nextest event in {path}")
            if event["type"] == "suite":
                outcome = event.get("event")
                if outcome == "started":
                    suites_started += 1
                elif outcome == "ok":
                    suites_finished += 1
                elif outcome == "failed":
                    raise SystemExit(f"nextest suite failed: {path}")
                else:
                    raise SystemExit(f"unknown nextest suite outcome: {outcome!r}")
                continue
            outcome = event.get("event")
            if outcome == "started":
                continue
            if outcome not in counts:
                raise SystemExit(f"unknown nextest test outcome: {outcome!r}")
            counts[outcome] += 1
            if outcome == "ok" and PROOF_TEST in str(event.get("name", "")):
                proof_passed = True
    if suites_started < 1 or suites_started != suites_finished:
        raise SystemExit("nextest evidence has incomplete suite events")
    if counts["failed"] or counts["timeout"]:
        raise SystemExit("nextest evidence contains failed or timed-out tests")
    if not proof_passed:
        raise SystemExit(f"nextest evidence lacks passing {PROOF_TEST}")
    selected = sum(counts.values())
    executed = counts["ok"] + counts["failed"] + counts["timeout"]
    return selected, executed, counts["ok"], counts["failed"] + counts["timeout"]


def build_summary(record_path: Path, nextest_path: Path, runner_path: Path) -> dict[str, object]:
    try:
        record = json.loads(record_path.read_bytes())
    except (OSError, UnicodeDecodeError, json.JSONDecodeError) as error:
        raise SystemExit(f"invalid runner record {record_path}: {error}") from error
    if not isinstance(record, dict) or record.get("schema_version") != 3:
        raise SystemExit("runner record must be a v3 object")
    captures = record.get("captures")
    routes = record.get("route_provenance")
    if not isinstance(captures, dict) or not captures:
        raise SystemExit("runner record has no captures")
    if not isinstance(routes, dict) or not routes:
        raise SystemExit("runner record has no route provenance")

    binary_digest = hashlib.sha256(runner_path.read_bytes()).hexdigest()
    receipt_digests: set[str] = set()
    activation_digests: set[str] = set()
    capture_binary_digests: set[str] = set()
    for capture_id, capture in captures.items():
        if not isinstance(capture, dict):
            raise SystemExit(f"capture {capture_id} must be an object")
        runner = capture.get("runner_binary")
        if not isinstance(runner, dict):
            raise SystemExit(f"capture {capture_id} lacks runner_binary")
        capture_binary_digests.add(_hex64(runner.get("digest"), "runner binary digest"))
        receipt_digests.add(_hex64(capture.get("receipt_digest"), "receipt digest"))
        activation_digests.add(_hex64(capture.get("activation_digest"), "activation digest"))
    if capture_binary_digests != {binary_digest}:
        raise SystemExit("record runner binary digest differs from the executable")
    if len(receipt_digests) != 1 or len(activation_digests) != 1:
        raise SystemExit("record captures do not share one receipt and activation ACK")

    for route, provenance in routes.items():
        if not isinstance(provenance, dict) or provenance.get("capture_id") not in captures:
            raise SystemExit(f"route {route} refers to an absent capture")
    selected, executed, passed, failed = _nextest_counts(nextest_path)
    return {
        "command": "just retrieval-sdk-proof",
        "separate_process": True,
        "sealed_receipt_digest": next(iter(receipt_digests)),
        "activation_ack_digest": next(iter(activation_digests)),
        "empty_check": True,
        "binary_digest": binary_digest,
        "sdk_route": ",".join(sorted(routes)),
        "selected": selected,
        "executed": executed,
        "passed": passed,
        "failed": failed,
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--record", required=True, type=Path)
    parser.add_argument("--nextest", required=True, type=Path)
    parser.add_argument("--runner-bin", required=True, type=Path)
    parser.add_argument("--out", required=True, type=Path)
    args = parser.parse_args()
    summary = build_summary(args.record, args.nextest, args.runner_bin)
    args.out.parent.mkdir(parents=True, exist_ok=True)
    temporary = args.out.with_name(f".{args.out.name}.{os.getpid()}.tmp")
    temporary.write_text(json.dumps(summary, sort_keys=True, indent=2) + "\n", encoding="utf-8")
    os.replace(temporary, args.out)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
