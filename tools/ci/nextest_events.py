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


def parse_nextest(path: Path) -> NextestEvidence:
    digest = hashlib.sha256()
    counts = {"ok": 0, "failed": 0, "ignored": 0, "timeout": 0}
    started: set[str] = set()
    terminal: dict[str, str] = {}
    suites_started = suites_finished = active_suites = 0
    suites_passed = suites_ignored = 0
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
                    if outcome == "started":
                        suites_started += 1
                        active_suites += 1
                    elif outcome == "ok":
                        if not active_suites:
                            raise NextestEvidenceError("nextest suite finished before start")
                        active_suites -= 1
                        suites_finished += 1
                        passed = _nonnegative(event.get("passed"), "passed")
                        failed = _nonnegative(event.get("failed"), "failed")
                        ignored = _nonnegative(event.get("ignored"), "ignored")
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
                if outcome == "started":
                    if name in started or name in terminal:
                        raise NextestEvidenceError(f"duplicate nextest test start: {name}")
                    started.add(name)
                    continue
                if not isinstance(outcome, str) or outcome not in counts:
                    raise NextestEvidenceError(f"unknown nextest test outcome: {outcome!r}")
                if name in terminal:
                    raise NextestEvidenceError(f"duplicate nextest test outcome: {name}")
                terminal[name] = outcome
                counts[outcome] += 1
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
    return NextestEvidence(
        sha256=digest.hexdigest(),
        selected=sum(counts.values()),
        executed=counts["ok"] + counts["failed"] + counts["timeout"],
        passed=counts["ok"],
        failed=counts["failed"] + counts["timeout"],
        passed_names=frozenset(name for name, outcome in terminal.items() if outcome == "ok"),
    )
