"""Recompute proof test outcomes from archived runner events and collection inventories."""

from __future__ import annotations

import json
import sys
import xml.etree.ElementTree as ET
from pathlib import Path
from typing import Any

try:
    from tools.ci.nextest_events import NextestEvidenceError, parse_nextest, parse_nextest_inventory
except ModuleNotFoundError:  # direct tool entrypoints place only their own directory on sys.path
    sys.path.insert(0, str(Path(__file__).resolve().parents[2]))
    from tools.ci.nextest_events import NextestEvidenceError, parse_nextest, parse_nextest_inventory


class ExecutionResultError(ValueError):
    """The archived runner evidence does not establish the claimed outcome."""


def _unique_object(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    result: dict[str, Any] = {}
    for key, value in pairs:
        if key in result:
            raise ExecutionResultError(f"duplicate inventory key: {key}")
        result[key] = value
    return result


def _pytest_result(events: Path, inventory: Path) -> tuple[dict[str, int], set[str]]:
    try:
        expected = json.loads(
            inventory.read_text(encoding="utf-8"), object_pairs_hook=_unique_object
        )
        root = ET.parse(events).getroot()
    except (OSError, UnicodeError, ValueError, ET.ParseError) as error:
        raise ExecutionResultError(f"invalid pytest result: {error}") from error
    if (
        not isinstance(expected, dict)
        or set(expected) != {"schema_version", "kind", "selector", "tests"}
        or type(expected["schema_version"]) is not int
        or expected["schema_version"] != 1
        or expected["kind"] != "pytest"
        or not isinstance(expected["selector"], str)
        or not expected["selector"]
        or not isinstance(expected["tests"], list)
        or not expected["tests"]
        or any(not isinstance(name, str) or not name for name in expected["tests"])
        or expected["tests"] != sorted(set(expected["tests"]))
    ):
        raise ExecutionResultError("invalid pytest collection inventory")
    if root.tag not in {"testsuite", "testsuites"}:
        raise ExecutionResultError("invalid pytest JUnit root")
    suites = [root] if root.tag == "testsuite" else list(root.findall("testsuite"))
    if (
        not suites
        or (root.tag == "testsuites" and root.findall("testcase"))
        or any(suite.findall("testsuite") for suite in suites)
        or len(list(root.iter("testcase")))
        != sum(len(suite.findall("testcase")) for suite in suites)
    ):
        raise ExecutionResultError("pytest JUnit has missing or nested suites")
    observed: set[str] = set()
    for suite in suites:
        cases = suite.findall("testcase")
        if not cases:
            raise ExecutionResultError("pytest JUnit has an empty suite")
        for case in cases:
            name = case.get("name")
            classname = case.get("classname")
            if not name or not classname:
                raise ExecutionResultError("pytest testcase lacks identity")
            identity = f"{classname}.{name}"
            if identity in observed:
                raise ExecutionResultError("duplicate pytest testcase")
            observed.add(identity)
            if any(case.find(outcome) is not None for outcome in ("failure", "error", "skipped")):
                raise ExecutionResultError(f"pytest testcase did not pass: {identity}")
        for field, actual in (
            ("tests", len(cases)),
            ("failures", 0),
            ("errors", 0),
            ("skipped", 0),
        ):
            if suite.get(field) != str(actual):
                raise ExecutionResultError(f"pytest suite {field} disagrees with testcases")
    if root.tag == "testsuites":
        for field, actual in (
            ("tests", len(observed)),
            ("failures", 0),
            ("errors", 0),
            ("skipped", 0),
        ):
            if root.get(field) is not None and root.get(field) != str(actual):
                raise ExecutionResultError(f"pytest root {field} disagrees with testcases")
    if observed != set(expected["tests"]):
        raise ExecutionResultError("pytest execution differs from collected required tests")
    return {
        "selected": len(observed),
        "executed": len(observed),
        "passed": len(observed),
        "failed": 0,
        "ignored": 0,
    }, observed


def derive_test_result(
    root: Path, result: Any, artifacts: list[dict[str, str]]
) -> tuple[dict[str, int], set[str]]:
    if (
        not isinstance(result, dict)
        or set(result) != {"schema_version", "runs"}
        or type(result["schema_version"]) is not int
        or result["schema_version"] != 1
        or not isinstance(result["runs"], list)
        or not result["runs"]
    ):
        raise ExecutionResultError("passed test proof requires versioned execution runs")
    by_source = {item["source_path"]: item for item in artifacts}
    if len(by_source) != len(artifacts):
        raise ExecutionResultError("duplicate archived artifact source")
    counts = {key: 0 for key in ("selected", "executed", "passed", "failed", "ignored")}
    names: set[str] = set()
    used_paths: set[str] = set()
    for run in result["runs"]:
        if not isinstance(run, dict) or set(run) != {"format", "events", "inventory"}:
            raise ExecutionResultError("execution run has invalid fields")
        if run["format"] not in {"nextest-jsonl", "pytest-junit"}:
            raise ExecutionResultError("execution run format is not allowlisted")
        paths = (run["events"], run["inventory"])
        if any(not isinstance(path, str) or path not in by_source for path in paths):
            raise ExecutionResultError("execution run source is not an archived proof artifact")
        if paths[0] == paths[1] or used_paths.intersection(paths):
            raise ExecutionResultError("duplicate execution evidence")
        used_paths.update(paths)
        event_path = root / by_source[paths[0]]["path"]
        inventory_path = root / by_source[paths[1]]["path"]
        if run["format"] == "nextest-jsonl":
            try:
                parsed = parse_nextest(event_path, parse_nextest_inventory(inventory_path))
            except NextestEvidenceError as error:
                raise ExecutionResultError(str(error)) from error
            current = {
                "selected": parsed.selected,
                "executed": parsed.executed,
                "passed": parsed.passed,
                "failed": parsed.failed,
                "ignored": parsed.selected - parsed.executed,
            }
            current_names = set(parsed.passed_names)
        else:
            current, current_names = _pytest_result(event_path, inventory_path)
        if names.intersection(current_names):
            raise ExecutionResultError("duplicate selected test across execution runs")
        names.update(current_names)
        for key in counts:
            counts[key] += current[key]
    if counts["ignored"] or counts["failed"] or not counts["passed"]:
        raise ExecutionResultError("passed proof contains skipped, failed or empty execution")
    return counts, names
