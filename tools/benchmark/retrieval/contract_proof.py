#!/usr/bin/env python3
"""Build retrieval contract summaries from pytest and nextest machine evidence."""

from __future__ import annotations

import argparse
import json
import os
import xml.etree.ElementTree as ET
from pathlib import Path


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
    if root.get("tests") is not None:
        suites = [root]
    else:
        suites = [
            suite for suite in root.iter("testsuite") if not list(suite.iter("testsuite"))[1:]
        ]
    if not suites:
        raise SystemExit("pytest JUnit evidence has no counted test suite")
    tests = sum(_count(suite.get("tests"), "tests") for suite in suites)
    failures = sum(_count(suite.get("failures"), "failures") for suite in suites)
    errors = sum(_count(suite.get("errors"), "errors") for suite in suites)
    skipped = sum(_count(suite.get("skipped"), "skipped") for suite in suites)
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
    counts = {"ok": 0, "failed": 0, "ignored": 0, "timeout": 0}
    suites_started = 0
    suites_finished = 0
    suites_passed = 0
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
                    passed = event.get("passed")
                    failed = event.get("failed")
                    if (
                        type(passed) is not int
                        or passed < 0
                        or type(failed) is not int
                        or failed < 0
                    ):
                        raise SystemExit("nextest suite has invalid counts")
                    suites_passed += passed
                elif outcome == "failed":
                    raise SystemExit("nextest suite failed")
                else:
                    raise SystemExit(f"unknown nextest suite outcome: {outcome!r}")
                continue
            outcome = event.get("event")
            if outcome == "started":
                continue
            if outcome not in counts:
                raise SystemExit(f"unknown nextest test outcome: {outcome!r}")
            counts[outcome] += 1
    if suites_started < 1 or suites_started != suites_finished:
        raise SystemExit("nextest evidence has incomplete suite events")
    if counts["failed"] or counts["timeout"]:
        raise SystemExit("nextest evidence contains failed or timed-out tests")
    if suites_passed != counts["ok"]:
        raise SystemExit("nextest suite/test pass counts disagree")
    selected = sum(counts.values())
    executed = counts["ok"] + counts["failed"] + counts["timeout"]
    if executed < 1:
        raise SystemExit("nextest evidence has no executed tests")
    return {
        "command": (
            "./scripts/cargow nextest run -p quanta-index-retrieval-bench "
            "--lib --test chunking_contract --all-features --locked"
        ),
        "selected": selected,
        "executed": executed,
        "passed": counts["ok"],
        "failed": counts["failed"] + counts["timeout"],
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
