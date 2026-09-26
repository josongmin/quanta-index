#!/usr/bin/env python3
"""Write a schema-validated, immutable-by-digest test execution receipt."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import subprocess
from pathlib import Path

from nextest_events import NextestEvidenceError, parse_nextest, parse_nextest_inventory_bytes
from source_closure import ClosureError, load_and_verify


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
    head = subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip()
    value = os.environ.get("GITHUB_SHA", "").strip()
    if value and value != head:
        raise SystemExit(f"GITHUB_SHA differs from checked-out HEAD: {value} != {head}")
    return head


def _nextest_evidence_summary(
    evidence: Path, inventory: Path | None
) -> tuple[str, int, str | None]:
    try:
        # Parse execution first so malformed or failing runs remain the primary
        # error even when their collection artifact is also missing.
        parse_nextest(evidence)
        inventory_bytes = inventory.read_bytes() if inventory is not None else None
        expected = (
            parse_nextest_inventory_bytes(inventory_bytes)
            if inventory_bytes is not None
            else None
        )
        parsed = parse_nextest(evidence, expected=expected)
    except (NextestEvidenceError, OSError) as error:
        raise SystemExit(f"{error}: {evidence}") from error
    inventory_digest = (
        hashlib.sha256(inventory_bytes).hexdigest() if inventory_bytes is not None else None
    )
    return parsed.sha256, parsed.selected, inventory_digest


def _summary_json_evidence_summary(evidence: Path, command: str) -> tuple[str, int]:
    raw = evidence.read_bytes()

    def unique_object(pairs: list[tuple[str, object]]) -> dict[str, object]:
        result: dict[str, object] = {}
        for key, value in pairs:
            if key in result:
                raise ValueError(f"duplicate summary JSON key: {key}")
            result[key] = value
        return result

    def reject_constant(value: str) -> None:
        raise ValueError(f"non-finite summary JSON value: {value}")

    try:
        payload = json.loads(raw, object_pairs_hook=unique_object, parse_constant=reject_constant)
    except (UnicodeDecodeError, json.JSONDecodeError, ValueError) as error:
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
    if payload["command"] != command:
        raise SystemExit(f"summary JSON evidence command differs from receipt command: {evidence}")
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
    if executed != selected:
        raise SystemExit(f"summary JSON evidence execution differs from selection: {evidence}")
    return hashlib.sha256(raw).hexdigest(), executed


def _input_evidence(values: list[str]) -> list[dict[str, str]]:
    inputs: list[dict[str, str]] = []
    seen: set[str] = set()
    for value in values:
        role, separator, raw_path = value.partition("=")
        if not separator or not role or not raw_path:
            raise SystemExit("--input-evidence must use ROLE=PATH")
        if role in seen:
            raise SystemExit(f"duplicate input evidence role: {role}")
        if any(character not in "abcdefghijklmnopqrstuvwxyz0123456789-_" for character in role):
            raise SystemExit(f"invalid input evidence role: {role}")
        path = Path(raw_path).resolve()
        if not path.is_file():
            raise SystemExit(f"missing input evidence: {path}")
        inputs.append(
            {
                "role": role,
                "sha256": hashlib.sha256(path.read_bytes()).hexdigest(),
            }
        )
        seen.add(role)
    return sorted(inputs, key=lambda entry: entry["role"])


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--rail", required=True)
    parser.add_argument(
        "--tier", required=True, choices=("pr", "merge", "main", "correctness", "nightly", "weekly")
    )
    parser.add_argument("--command", required=True)
    parser.add_argument(
        "--evidence-format",
        choices=("nextest-jsonl", "summary-json"),
        default="nextest-jsonl",
    )
    parser.add_argument("--evidence", required=True, type=Path)
    parser.add_argument(
        "--inventory", type=Path, help="nextest list JSON from the same workspace selection"
    )
    parser.add_argument("--out", required=True, type=Path)
    parser.add_argument(
        "--source-closure",
        type=Path,
        help="verified source_closure.py manifest; emits a retrieval-authoritative v2 receipt",
    )
    parser.add_argument(
        "--input-evidence",
        action="append",
        default=[],
        metavar="ROLE=PATH",
        help="raw machine evidence consumed by the summary producer; repeat per input",
    )
    args = parser.parse_args()
    evidence = args.evidence.resolve()
    if not evidence.is_file():
        raise SystemExit(f"missing test evidence: {evidence}")
    source_closure = None
    if args.source_closure is None:
        if args.input_evidence:
            raise SystemExit("--input-evidence requires --source-closure and receipt schema v2")
        revision = _revision()
    else:
        try:
            source_closure = load_and_verify(args.source_closure.resolve())
        except ClosureError as error:
            raise SystemExit(f"invalid source closure: {error}") from error
        revision = source_closure["revision"]
    input_evidence = _input_evidence(args.input_evidence)
    if source_closure is not None and not input_evidence:
        raise SystemExit("retrieval-authoritative receipt v2 requires raw --input-evidence")
    if args.evidence_format == "summary-json":
        if source_closure is None:
            raise SystemExit("summary JSON requires source closure and raw input evidence")
        if args.inventory is not None:
            raise SystemExit("--inventory is only valid for nextest evidence")
        digest, test_event_count = _summary_json_evidence_summary(evidence, args.command)
    else:
        inventory = args.inventory.resolve() if args.inventory is not None else None
        if "workspace-nextest" in args.rail and inventory is None:
            raise SystemExit("workspace nextest receipt requires --inventory")
        digest, test_event_count, inventory_digest = _nextest_evidence_summary(evidence, inventory)
    receipt = {
        "schema_version": 2 if source_closure is not None else 1,
        "revision": revision,
        "rail": args.rail,
        "tier": args.tier,
        "command": args.command,
        "evidence_path": args.evidence.as_posix(),
        "evidence_sha256": digest,
        "test_event_count": test_event_count,
    }
    if source_closure is not None:
        receipt["source_closure"] = source_closure
        receipt["input_evidence"] = input_evidence
    if args.inventory is not None:
        receipt["inventory_path"] = args.inventory.resolve().as_posix()
        assert inventory_digest is not None
        receipt["inventory_sha256"] = inventory_digest
    args.out.parent.mkdir(parents=True, exist_ok=True)
    try:
        with args.out.open("x", encoding="utf-8") as stream:
            stream.write(json.dumps(receipt, sort_keys=True, indent=2) + "\n")
            stream.flush()
            os.fsync(stream.fileno())
    except FileExistsError as error:
        raise SystemExit(f"refusing existing verification receipt: {args.out}") from error
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
