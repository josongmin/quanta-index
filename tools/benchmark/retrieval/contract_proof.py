#!/usr/bin/env python3
"""Build retrieval contract summaries from pytest and nextest machine evidence."""

from __future__ import annotations

import argparse
import json
import os
import sys
import xml.etree.ElementTree as ET
from pathlib import Path

try:
    from tools.ci.nextest_events import NextestEvidenceError, parse_nextest
except ModuleNotFoundError:  # direct script invocation
    sys.path.insert(0, str(Path(__file__).resolve().parents[3]))
    from tools.ci.nextest_events import NextestEvidenceError, parse_nextest


def _count(value: str | None, label: str) -> int:
    if value is None:
        raise SystemExit(f"missing {label} count")
    try:
        parsed = int(value)
    except ValueError as error:
        raise SystemExit(f"invalid {label} count: {value!r}") from error
    if parsed < 0:
        raise SystemExit(f"negative {label} count")
    return parsed


def pytest_summary(path: Path) -> dict[str, object]:
    try:
        root = ET.parse(path).getroot()
    except (OSError, ET.ParseError) as error:
        raise SystemExit(f"invalid pytest JUnit evidence {path}: {error}") from error
    if root.tag not in {"testsuite", "testsuites"}:
        raise SystemExit("pytest JUnit evidence has an unknown root")
    if root.tag == "testsuites" and root.findall("testcase"):
        raise SystemExit("pytest JUnit testcase outside a test suite")
    if any(
        suite.findall("testcase") for suite in root.iter("testsuite") if suite.findall("testsuite")
    ):
        raise SystemExit("pytest JUnit nested suite hides direct testcases")
    suites = [suite for suite in root.iter("testsuite") if not suite.findall("testsuite")]
    if not suites:
        raise SystemExit("pytest JUnit evidence has no counted test suite")
    totals = {key: 0 for key in ("tests", "failures", "errors", "skipped")}
    for suite in suites:
        cases = suite.findall("testcase")
        seen_cases: set[tuple[str, str]] = set()
        observed = {"tests": len(cases), "failures": 0, "errors": 0, "skipped": 0}
        for case in cases:
            name = case.get("name")
            if not name:
                raise SystemExit("pytest JUnit testcase lacks a name")
            identity = (case.get("classname", ""), name)
            if identity in seen_cases:
                raise SystemExit("duplicate pytest JUnit testcase")
            seen_cases.add(identity)
            outcomes = [
                key for key in ("failure", "error", "skipped") if case.find(key) is not None
            ]
            if len(outcomes) > 1:
                raise SystemExit("pytest JUnit testcase has contradictory outcomes")
            if outcomes:
                outcome_key = {"failure": "failures", "error": "errors", "skipped": "skipped"}
                observed[outcome_key[outcomes[0]]] += 1
        for key, value in observed.items():
            if _count(suite.get(key), key) != value:
                raise SystemExit(f"pytest JUnit {key} count disagrees with testcases")
            totals[key] += value
    if root not in suites and root.get("tests") is not None:
        for key, value in totals.items():
            if _count(root.get(key), key) != value:
                raise SystemExit(f"pytest JUnit root {key} count disagrees with testcases")
    tests = totals["tests"]
    failures = totals["failures"]
    errors = totals["errors"]
    skipped = totals["skipped"]
    failed = failures + errors
    executed = tests - skipped
    passed = executed - failed
    if tests < 1 or executed < 1 or passed < 1:
        raise SystemExit("pytest evidence has no passing executed tests")
    if failed:
        raise SystemExit("pytest evidence reports failures or errors")
    if executed < 0 or passed < 0:
        raise SystemExit("pytest evidence has inconsistent counts")
    return {
        "command": "python3 -m pytest tools/ci/tests/test_retrieval_benchmark.py -q",
        "selected": tests,
        "executed": executed,
        "passed": passed,
        "failed": failed,
    }


def nextest_summary(path: Path) -> dict[str, object]:
    try:
        evidence = parse_nextest(path)
    except NextestEvidenceError as error:
        raise SystemExit(f"{error}: {path}") from error
    return {
        "command": (
            "./scripts/cargow nextest run -p quanta-index-retrieval-bench "
            "--lib --test chunking_contract --all-features --locked"
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
    parser.add_argument("--nextest", required=True, type=Path)
    parser.add_argument("--python-out", required=True, type=Path)
    parser.add_argument("--rust-out", required=True, type=Path)
    args = parser.parse_args()
    _write(args.python_out, pytest_summary(args.pytest_junit))
    _write(args.rust_out, nextest_summary(args.nextest))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
