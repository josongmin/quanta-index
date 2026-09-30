#!/usr/bin/env python3
"""Build retrieval contract summaries from pytest and nextest machine evidence."""

from __future__ import annotations

import argparse
import json
import os
import sys
from pathlib import Path

try:
    from tools.benchmark.retrieval.proof_inventory import (
        PYTHON_SELECTOR,
        verify_inventory_authority,
    )
    from tools.ci.junit_events import JUnitEvidenceError, parse_pytest_junit_bytes
    from tools.ci.nextest_events import (
        NextestEvidenceError,
        parse_nextest,
        parse_nextest_bytes,
        parse_nextest_inventory_bytes,
    )
except ModuleNotFoundError:  # direct script invocation
    sys.path.insert(0, str(Path(__file__).resolve().parents[3]))
    from tools.benchmark.retrieval.proof_inventory import (
        PYTHON_SELECTOR,
        verify_inventory_authority,
    )
    from tools.ci.junit_events import JUnitEvidenceError, parse_pytest_junit_bytes
    from tools.ci.nextest_events import (
        NextestEvidenceError,
        parse_nextest,
        parse_nextest_bytes,
        parse_nextest_inventory_bytes,
    )

from tools.benchmark.evidence import RawFile, read_control


def _evidence_bytes(value: Path | RawFile | bytes) -> bytes:
    return read_control(value)


def _unique_object(pairs: list[tuple[str, object]]) -> dict[str, object]:
    result: dict[str, object] = {}
    for key, value in pairs:
        if key in result:
            raise SystemExit(f"duplicate pytest inventory JSON key: {key}")
        result[key] = value
    return result


def _load_pytest_inventory(path: Path | RawFile | bytes) -> set[str]:
    try:
        payload = json.loads(_evidence_bytes(path), object_pairs_hook=_unique_object)
    except (OSError, UnicodeDecodeError, json.JSONDecodeError) as error:
        raise SystemExit(f"invalid pytest inventory {path}: {error}") from error
    if (
        not isinstance(payload, dict)
        or set(payload) != {"schema_version", "kind", "selector", "tests"}
        or type(payload["schema_version"]) is not int
        or payload["schema_version"] != 1
        or payload["kind"] != "pytest"
        or payload["selector"] != PYTHON_SELECTOR
        or not isinstance(payload["tests"], list)
        or not payload["tests"]
        or any(not isinstance(test, str) or not test for test in payload["tests"])
        or payload["tests"] != sorted(set(payload["tests"]))
    ):
        raise SystemExit("invalid pytest inventory schema or identities")
    return set(payload["tests"])


def pytest_summary(
    path: Path | RawFile | bytes, inventory: Path | RawFile | bytes | None = None
) -> dict[str, object]:
    try:
        expected = _load_pytest_inventory(inventory) if inventory is not None else None
        counts, _ = parse_pytest_junit_bytes(_evidence_bytes(path), expected)
    except (OSError, JUnitEvidenceError) as error:
        raise SystemExit(f"invalid pytest JUnit evidence {path}: {error}") from error
    return {
        "command": f"python3 -m pytest {PYTHON_SELECTOR} -q",
        **{key: counts[key] for key in ("selected", "executed", "passed", "failed")},
    }


def nextest_summary(
    path: Path | RawFile | bytes, inventory: Path | RawFile | bytes | None = None
) -> dict[str, object]:
    try:
        expected = (
            parse_nextest_inventory_bytes(_evidence_bytes(inventory))
            if inventory is not None
            else None
        )
        evidence = (
            parse_nextest_bytes(path, expected)
            if isinstance(path, bytes)
            else parse_nextest(path, expected)
        )
    except NextestEvidenceError as error:
        raise SystemExit(f"{error}: {path}") from error
    return {
        "command": (
            "./scripts/cargow nextest run -p quanta-index-retrieval-bench "
            "--lib --test chunking_contract --test l5_parser_regressions --all-features --locked"
        ),
        "selected": evidence.selected,
        "executed": evidence.executed,
        "passed": evidence.passed,
        "failed": evidence.failed,
    }


def _write(path: Path, payload: dict[str, object]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_name(f".{path.name}.{os.getpid()}.tmp")
    temporary.write_text(json.dumps(payload, sort_keys=True, indent=2) + "\n", encoding="utf-8")
    os.replace(temporary, path)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--pytest-junit", required=True, type=Path)
    parser.add_argument("--pytest-inventory", required=True, type=Path)
    parser.add_argument("--nextest", required=True, type=Path)
    parser.add_argument("--nextest-inventory", required=True, type=Path)
    parser.add_argument("--python-out", required=True, type=Path)
    parser.add_argument("--rust-out", required=True, type=Path)
    args = parser.parse_args()
    try:
        verify_inventory_authority(args.pytest_inventory, "python")
        verify_inventory_authority(args.nextest_inventory, "rust")
    except ValueError as error:
        raise SystemExit(str(error)) from error
    _write(args.python_out, pytest_summary(args.pytest_junit, args.pytest_inventory))
    _write(args.rust_out, nextest_summary(args.nextest, args.nextest_inventory))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
