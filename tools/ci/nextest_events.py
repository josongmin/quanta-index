"""Validate and count nextest libtest-json-plus 0.1 machine events."""

from __future__ import annotations

import hashlib
import json
from dataclasses import dataclass
from pathlib import Path


class NextestEvidenceError(ValueError):
    """Nextest evidence is incomplete, contradictory, or malformed."""


def _unique_object(pairs: list[tuple[str, object]]) -> dict[str, object]:
    result: dict[str, object] = {}
    for key, value in pairs:
        if key in result:
            raise NextestEvidenceError(f"duplicate nextest JSON key: {key}")
        result[key] = value
    return result


def _reject_constant(value: str) -> None:
    raise NextestEvidenceError(f"invalid nextest JSON constant: {value}")


def _nonnegative(value: object, label: str) -> int:
    if type(value) is not int or value < 0:
        raise NextestEvidenceError(f"nextest suite has invalid {label} count")
    return value


@dataclass(frozen=True)
class NextestEvidence:
    sha256: str
    selected: int
    executed: int
    passed: int
    failed: int
    passed_names: frozenset[str]


def parse_nextest_inventory(path: Path) -> dict[str, tuple[str, str, str]]:
    """Return selected event names and their suite identity from nextest list JSON."""
    try:
        with path.open("r", encoding="utf-8") as stream:
            payload = json.load(
                stream, object_pairs_hook=_unique_object, parse_constant=_reject_constant
            )
    except (OSError, UnicodeDecodeError, json.JSONDecodeError) as error:
        raise NextestEvidenceError(f"invalid nextest inventory: {error}") from error
    if (
        not isinstance(payload, dict)
        or _nonnegative(payload.get("test-count"), "inventory test") < 1
    ):
        raise NextestEvidenceError("nextest inventory has no selected tests")
    suites = payload.get("rust-suites")
    if not isinstance(suites, dict) or not suites:
        raise NextestEvidenceError("nextest inventory lacks rust-suites")
    expected: dict[str, tuple[str, str, str]] = {}
    listed_count = 0
    for binary_id, suite in suites.items():
        if not isinstance(binary_id, str) or not isinstance(suite, dict):
            raise NextestEvidenceError("nextest inventory has invalid suite")
        package = suite.get("package-name")
        binary = suite.get("binary-name")
        kind = suite.get("kind")
        cases = suite.get("testcases")
        if (
            not all(isinstance(value, str) and value for value in (package, binary, kind))
            or not isinstance(cases, dict)
            or suite.get("status") != "listed"
            or not binary_id.startswith(f"{package}::")
            and binary_id != package
        ):
            raise NextestEvidenceError("nextest inventory suite identity is invalid")
        for name, case in cases.items():
            listed_count += 1
            if not isinstance(name, str) or not name or not isinstance(case, dict):
                raise NextestEvidenceError("nextest inventory has invalid testcase")
            match = case.get("filter-match")
            if not isinstance(match, dict) or match.get("status") != "matches":
                continue
            if case.get("ignored") is not False:
                raise NextestEvidenceError("nextest inventory contains ignored required test")
            event_name = f"{package}::{binary}${name}"
            if event_name in expected:
                raise NextestEvidenceError(f"duplicate nextest inventory test: {event_name}")
            expected[event_name] = (package, binary, kind)
    if listed_count != payload["test-count"] or not expected:
        raise NextestEvidenceError("nextest inventory test-count disagrees with testcases")
    return expected


def parse_nextest(
    path: Path, expected: dict[str, tuple[str, str, str]] | None = None
) -> NextestEvidence:
    digest = hashlib.sha256()
    counts = {"ok": 0, "failed": 0, "ignored": 0, "timeout": 0}
    started: set[str] = set()
    terminal: dict[str, str] = {}
    suites_started = suites_finished = 0
    suites_passed = suites_ignored = 0
    active_suites: dict[tuple[str, str, str] | None, dict[str, int | None]] = {}
    started_suites: dict[str, tuple[str, str, str] | None] = {}
    try:
        with path.open("rb") as stream:
            for line in stream:
                digest.update(line)
                try:
                    event = json.loads(
                        line,
                        object_pairs_hook=_unique_object,
                        parse_constant=_reject_constant,
                    )
                except (UnicodeDecodeError, json.JSONDecodeError) as error:
                    raise NextestEvidenceError(f"invalid nextest JSON evidence: {error}") from error
                if (
                    not isinstance(event, dict)
                    or not isinstance(event.get("type"), str)
                    or event["type"] not in {"suite", "test"}
                ):
                    raise NextestEvidenceError("unknown nextest event type")
                outcome = event.get("event")
                if event["type"] == "suite":
                    metadata = event.get("nextest")
                    if metadata is not None:
                        if not isinstance(metadata, dict) or not all(
                            isinstance(metadata.get(key), str) and metadata[key]
                            for key in ("crate", "test_binary", "kind")
                        ):
                            raise NextestEvidenceError("nextest suite identity is invalid")
                        suite_identity: tuple[str, str, str] | None = (
                            metadata["crate"],
                            metadata["test_binary"],
                            metadata["kind"],
                        )
                    else:
                        suite_identity = None
                    if outcome == "started":
                        if suite_identity in active_suites:
                            raise NextestEvidenceError("duplicate nextest suite start")
                        if suite_identity is None and active_suites:
                            raise NextestEvidenceError("unidentified nextest suites overlap")
                        suites_started += 1
                        test_count = (
                            _nonnegative(event["test_count"], "announced test")
                            if "test_count" in event
                            else None
                        )
                        if expected is not None and (test_count is None or suite_identity is None):
                            raise NextestEvidenceError("nextest suite lacks collection identity")
                        active_suites[suite_identity] = {"test_count": test_count, "terminal": 0}
                    elif outcome == "ok":
                        if suite_identity not in active_suites:
                            raise NextestEvidenceError("nextest suite finished before start")
                        suite_state = active_suites.pop(suite_identity)
                        suites_finished += 1
                        passed = _nonnegative(event.get("passed"), "passed")
                        failed = _nonnegative(event.get("failed"), "failed")
                        ignored = _nonnegative(event.get("ignored"), "ignored")
                        if (
                            suite_state["test_count"] is not None
                            and suite_state["terminal"] != suite_state["test_count"]
                        ):
                            raise NextestEvidenceError("nextest announced test count disagrees")
                        if suite_state["terminal"] != passed + failed + ignored:
                            raise NextestEvidenceError("nextest suite/test pass counts disagree")
                        if failed:
                            raise NextestEvidenceError("nextest suite reports failures")
                        suites_passed += passed
                        suites_ignored += ignored
                    elif outcome == "failed":
                        raise NextestEvidenceError("nextest suite failed")
                    else:
                        raise NextestEvidenceError(f"unknown nextest suite outcome: {outcome!r}")
                    continue
                if not active_suites:
                    raise NextestEvidenceError("nextest test event outside an active suite")
                name = event.get("name")
                if not isinstance(name, str) or not name:
                    raise NextestEvidenceError("nextest test event lacks a name")
                if expected is not None:
                    if name not in expected:
                        raise NextestEvidenceError(f"unexpected nextest test: {name}")
                    test_suite: tuple[str, str, str] | None = expected[name]
                elif name in started_suites:
                    test_suite = started_suites[name]
                else:
                    matches = [
                        identity
                        for identity in active_suites
                        if identity is not None
                        and name.startswith(f"{identity[0]}::{identity[1]}$")
                    ]
                    if len(matches) == 1:
                        test_suite = matches[0]
                    elif len(active_suites) == 1:
                        test_suite = next(iter(active_suites))
                    else:
                        raise NextestEvidenceError(f"nextest test suite is ambiguous: {name}")
                if test_suite not in active_suites:
                    raise NextestEvidenceError(f"nextest binary identity mismatch: {name}")
                if outcome == "started":
                    if name in started or name in terminal:
                        raise NextestEvidenceError(f"duplicate nextest test start: {name}")
                    started.add(name)
                    started_suites[name] = test_suite
                    continue
                if not isinstance(outcome, str) or outcome not in counts:
                    raise NextestEvidenceError(f"unknown nextest test outcome: {outcome!r}")
                if name in terminal:
                    raise NextestEvidenceError(f"duplicate nextest test outcome: {name}")
                if name in started_suites and started_suites[name] != test_suite:
                    raise NextestEvidenceError(f"nextest test changed suite: {name}")
                terminal[name] = outcome
                counts[outcome] += 1
                active_suites[test_suite]["terminal"] += 1
    except OSError as error:
        raise NextestEvidenceError(f"cannot read nextest evidence: {error}") from error
    if counts["failed"] or counts["timeout"]:
        raise NextestEvidenceError("nextest evidence contains failed or timed-out tests")
    if not suites_started or suites_started != suites_finished or active_suites:
        raise NextestEvidenceError("nextest evidence has incomplete suite events")
    if suites_passed != counts["ok"] or suites_ignored != counts["ignored"]:
        raise NextestEvidenceError("nextest suite/test pass counts disagree")
    if any(name not in started for name, outcome in terminal.items() if outcome != "ignored"):
        raise NextestEvidenceError("nextest test outcome lacks a start event")
    if started - terminal.keys():
        raise NextestEvidenceError("nextest evidence has incomplete test events")
    if not counts["ok"]:
        raise NextestEvidenceError("nextest evidence has no passing tests")
    if expected is not None:
        if set(terminal) != set(expected):
            raise NextestEvidenceError("nextest execution differs from collected tests")
        if any(outcome != "ok" for outcome in terminal.values()):
            raise NextestEvidenceError("nextest required test did not pass")
    return NextestEvidence(
        sha256=digest.hexdigest(),
        selected=sum(counts.values()),
        executed=counts["ok"] + counts["failed"] + counts["timeout"],
        passed=counts["ok"],
        failed=counts["failed"] + counts["timeout"],
        passed_names=frozenset(name for name, outcome in terminal.items() if outcome == "ok"),
    )
