"""Interpret pytest JUnit outcomes for both archived and retrieval evidence.

Collection and source authority belong to callers. This owner admits the XML
grammar, identities and counters; no outcome can hide outside a counted case.
"""

from __future__ import annotations

import xml.etree.ElementTree as ET


class JUnitEvidenceError(ValueError):
    """JUnit bytes do not establish the claimed test execution."""


def parse_pytest_junit_bytes(
    raw: bytes, expected: set[str] | None = None
) -> tuple[dict[str, int], set[str]]:
    try:
        root = ET.fromstring(raw)
    except (ValueError, ET.ParseError) as error:
        raise JUnitEvidenceError(f"invalid pytest JUnit evidence: {error}") from error
    if root.tag not in {"testsuite", "testsuites"}:
        raise JUnitEvidenceError("pytest JUnit evidence has an unknown root")
    if any(suite.findall("testsuite") for suite in root.iter("testsuite")):
        raise JUnitEvidenceError("pytest JUnit nested suite hides direct testcases")
    grammar = {
        "testsuites": {"testsuite"},
        "testsuite": {"testcase", "properties", "system-out", "system-err"},
        "testcase": {"properties", "system-out", "system-err", "failure", "error", "skipped"},
        "properties": {"property"},
        "property": set(),
        "system-out": set(),
        "system-err": set(),
        "failure": set(),
        "error": set(),
        "skipped": set(),
    }
    for node in root.iter():
        allowed = grammar.get(node.tag)
        if allowed is None or any(child.tag not in allowed for child in node):
            raise JUnitEvidenceError("pytest JUnit contains an unsupported element placement")
    suites = [root] if root.tag == "testsuite" else root.findall("testsuite")
    if not suites:
        raise JUnitEvidenceError("pytest JUnit evidence has no counted test suite")
    totals = {key: 0 for key in ("tests", "failures", "errors", "skipped")}
    identities: set[str] = set()
    passed_names: set[str] = set()

    def require_count(node: ET.Element, field: str, count: int, *, prefix: str = "") -> None:
        if node.get(field) != str(count):
            raise JUnitEvidenceError(f"pytest JUnit {prefix}{field} count disagrees with testcases")

    for suite in suites:
        cases = suite.findall("testcase")
        observed = {"tests": len(cases), "failures": 0, "errors": 0, "skipped": 0}
        for case in cases:
            name = case.get("name")
            classname = case.get("classname", "")
            if not name or (expected is not None and not classname):
                raise JUnitEvidenceError("pytest testcase lacks identity")
            identity = f"{classname}.{name}"
            if identity in identities:
                raise JUnitEvidenceError("duplicate pytest JUnit testcase")
            identities.add(identity)
            outcomes = [node.tag for node in case if node.tag in {"failure", "error", "skipped"}]
            if len(outcomes) > 1:
                raise JUnitEvidenceError("pytest JUnit testcase has contradictory outcomes")
            if outcomes:
                field = {"failure": "failures", "error": "errors", "skipped": "skipped"}
                observed[field[outcomes[0]]] += 1
            else:
                passed_names.add(identity)
        for key, value in observed.items():
            require_count(suite, key, value)
            totals[key] += value
        if not cases:
            raise JUnitEvidenceError("pytest JUnit has an empty suite")
    if root.tag == "testsuites":
        for key, value in totals.items():
            if root.get(key) is not None:
                require_count(root, key, value, prefix="root ")
    failed = totals["failures"] + totals["errors"]
    if failed:
        raise JUnitEvidenceError(
            "pytest evidence reports failures or errors; testcase did not pass"
        )
    executed = totals["tests"] - totals["skipped"]
    if executed < 1:
        raise JUnitEvidenceError(
            "pytest evidence has no passing executed tests; testcase did not pass"
        )
    if expected is not None:
        if totals["skipped"]:
            raise JUnitEvidenceError("pytest testcase did not pass: required test was skipped")
        if identities != expected:
            raise JUnitEvidenceError("pytest execution differs from collected required tests")
    return {
        "selected": totals["tests"],
        "executed": executed,
        "passed": executed,
        "failed": 0,
        "ignored": totals["skipped"],
    }, passed_names
