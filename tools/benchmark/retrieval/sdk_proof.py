#!/usr/bin/env python3
"""Build the retrieval SDK proof summary from machine evidence."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import sys
from pathlib import Path

try:
    from tools.benchmark.retrieval.proof_inventory import verify_inventory_authority
    from tools.ci.nextest_events import (
        NextestEvidenceError,
        parse_nextest,
        parse_nextest_inventory,
    )
except ModuleNotFoundError:  # direct script invocation
    sys.path.insert(0, str(Path(__file__).resolve().parents[3]))
    from tools.benchmark.retrieval.proof_inventory import verify_inventory_authority
    from tools.ci.nextest_events import (
        NextestEvidenceError,
        parse_nextest,
        parse_nextest_inventory,
    )

PROOF_TEST = "actual_runner_binary_emits_receipt_bound_v5_record"


def _unique_object(pairs: list[tuple[str, object]]) -> dict[str, object]:
    result: dict[str, object] = {}
    for key, value in pairs:
        if key in result:
            raise ValueError(f"duplicate runner record JSON key: {key}")
        result[key] = value
    return result


def _reject_constant(value: str) -> None:
    raise ValueError(f"invalid runner record JSON constant: {value}")


def _hex64(value: object, label: str) -> str:
    if (
        not isinstance(value, str)
        or len(value) != 64
        or any(char not in "0123456789abcdef" for char in value)
    ):
        raise SystemExit(f"{label} must be a lowercase sha256")
    return value


def _nextest_counts(path: Path, inventory: Path | None = None) -> tuple[int, int, int, int]:
    try:
        expected = parse_nextest_inventory(inventory) if inventory is not None else None
        evidence = parse_nextest(path, expected)
    except NextestEvidenceError as error:
        raise SystemExit(f"{error}: {path}") from error
    if not any(
        name == PROOF_TEST or name.endswith(f"${PROOF_TEST}") for name in evidence.passed_names
    ):
        raise SystemExit(f"nextest evidence lacks passing {PROOF_TEST}")
    return evidence.selected, evidence.executed, evidence.passed, evidence.failed


def build_summary_from_evidence(
    record_path: Path,
    nextest_path: Path,
    runner_digest: str,
    inventory_path: Path | None = None,
    *,
    searchd_digest: str | None = None,
) -> dict[str, object]:
    try:
        record = json.loads(
            record_path.read_bytes(),
            object_pairs_hook=_unique_object,
            parse_constant=_reject_constant,
        )
    except (OSError, UnicodeDecodeError, json.JSONDecodeError, ValueError) as error:
        raise SystemExit(f"invalid runner record {record_path}: {error}") from error
    if (
        not isinstance(record, dict)
        or type(record.get("schema_version")) is not int
        or record["schema_version"] != 5
    ):
        raise SystemExit("runner record must be a v5 object")
    if type(record.get("span_accounting_version")) is not int or record["span_accounting_version"] != 1:
        raise SystemExit("current runner record lacks indexed-span protocol")
    captures = record.get("captures")
    routes = record.get("route_provenance")
    if not isinstance(captures, dict) or not captures:
        raise SystemExit("runner record has no captures")
    if not isinstance(routes, dict) or not routes:
        raise SystemExit("runner record has no route provenance")

    binary_digest = _hex64(runner_digest, "runner binary digest")
    receipt_digests: set[str] = set()
    activation_digests: set[str] = set()
    capture_binary_digests: set[str] = set()
    capture_searchd_digests: set[str] = set()
    for capture_id, capture in captures.items():
        if not isinstance(capture, dict):
            raise SystemExit(f"capture {capture_id} must be an object")
        runner = capture.get("runner_binary")
        if not isinstance(runner, dict):
            raise SystemExit(f"capture {capture_id} lacks runner_binary")
        capture_binary_digests.add(_hex64(runner.get("digest"), "runner binary digest"))
        if searchd_digest is not None:
            searchd = capture.get("searchd_binary")
            if not isinstance(searchd, dict):
                raise SystemExit(f"capture {capture_id} lacks searchd_binary")
            capture_searchd_digests.add(
                _hex64(searchd.get("binary_digest"), "searchd binary digest")
            )
        receipt_digests.add(_hex64(capture.get("receipt_digest"), "receipt digest"))
        activation_digests.add(_hex64(capture.get("activation_digest"), "activation digest"))
    if capture_binary_digests != {binary_digest}:
        raise SystemExit("record runner binary digest differs from the executable")
    if searchd_digest is not None and capture_searchd_digests != {
        _hex64(searchd_digest, "searchd binary digest")
    }:
        raise SystemExit("record searchd binary digest differs from the executable")
    if len(receipt_digests) != 1 or len(activation_digests) != 1:
        raise SystemExit("record captures do not share one receipt and activation ACK")

    for route, provenance in routes.items():
        if not isinstance(provenance, dict) or provenance.get("capture_id") not in captures:
            raise SystemExit(f"route {route} refers to an absent capture")
    selected, executed, passed, failed = _nextest_counts(nextest_path, inventory_path)
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


def build_summary(
    record_path: Path,
    nextest_path: Path,
    runner_path: Path,
    inventory_path: Path | None = None,
    *,
    searchd_path: Path | None = None,
) -> dict[str, object]:
    binary_digest = hashlib.sha256(runner_path.read_bytes()).hexdigest()
    return build_summary_from_evidence(
        record_path,
        nextest_path,
        binary_digest,
        inventory_path,
        searchd_digest=hashlib.sha256(searchd_path.read_bytes()).hexdigest()
        if searchd_path is not None
        else None,
    )


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--record", required=True, type=Path)
    parser.add_argument("--nextest", required=True, type=Path)
    parser.add_argument("--nextest-inventory", required=True, type=Path)
    parser.add_argument("--runner-bin", required=True, type=Path)
    parser.add_argument("--out", required=True, type=Path)
    args = parser.parse_args()
    try:
        verify_inventory_authority(args.nextest_inventory, "sdk")
    except ValueError as error:
        raise SystemExit(str(error)) from error
    summary = build_summary(args.record, args.nextest, args.runner_bin, args.nextest_inventory)
    args.out.parent.mkdir(parents=True, exist_ok=True)
    temporary = args.out.with_name(f".{args.out.name}.{os.getpid()}.tmp")
    temporary.write_text(json.dumps(summary, sort_keys=True, indent=2) + "\n", encoding="utf-8")
    os.replace(temporary, args.out)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
