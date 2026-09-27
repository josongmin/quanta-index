"""Validate and count nextest libtest-json-plus 0.1 machine events."""

from __future__ import annotations

import hashlib
import io
import json
import sys
from collections.abc import Iterable, Iterator, Mapping
from contextlib import AbstractContextManager, nullcontext
from dataclasses import dataclass
from pathlib import Path
from types import MappingProxyType

try:
    from tools.benchmark.evidence import (
        CONTROL_DOCUMENT_BYTES,
        EvidenceError,
        RawFile,
        read_control,
    )
except ModuleNotFoundError:  # direct CI script invocation
    sys.path.insert(0, str(Path(__file__).resolve().parents[2]))
    from tools.benchmark.evidence import (
        CONTROL_DOCUMENT_BYTES,
        EvidenceError,
        RawFile,
        read_control,
    )


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


SuiteIdentity = tuple[str, str, str]
_INVENTORY_TOKEN = object()


@dataclass(frozen=True, init=False)
class NextestInventory(Mapping[str, SuiteIdentity]):
    """Read-only selected identities plus explicit collection exclusions.

    Mapping operations expose selected tests only. Ignored tests never become
    required successes; their identities and suite counters remain bound to
    the same parsed inventory bytes rather than a caller-supplied allowlist.
    """

    _selected: Mapping[str, SuiteIdentity]
    ignored: Mapping[str, SuiteIdentity]
    suite_counts: Mapping[SuiteIdentity, tuple[int, int, int]]

    def __init__(
        self,
        selected: Mapping[str, SuiteIdentity],
        ignored: Mapping[str, SuiteIdentity],
        suite_counts: Mapping[SuiteIdentity, tuple[int, int, int]],
        *,
        _token: object,
    ) -> None:
        if _token is not _INVENTORY_TOKEN:
            raise NextestEvidenceError("nextest inventory must be parsed from collection bytes")
        object.__setattr__(self, "_selected", MappingProxyType(dict(selected)))
        object.__setattr__(self, "ignored", MappingProxyType(dict(ignored)))
        object.__setattr__(self, "suite_counts", MappingProxyType(dict(suite_counts)))

    def __getitem__(self, key: str) -> SuiteIdentity:
        return self._selected[key]

    def __iter__(self) -> Iterator[str]:
        return iter(self._selected)

    def __len__(self) -> int:
        return len(self._selected)


def parse_nextest_inventory(path: Path | RawFile) -> NextestInventory:
    """Return selected event names and their suite identity from nextest list JSON."""
    try:
        raw = read_control(path)
    except (OSError, EvidenceError) as error:
        raise NextestEvidenceError(f"invalid nextest inventory: {error}") from error
    return parse_nextest_inventory_bytes(raw)


def parse_nextest_inventory_bytes(raw: bytes) -> NextestInventory:
    """Parse exactly the bytes whose digest is recorded by the receipt writer."""
    try:
        raw = read_control(raw)
        payload = json.loads(raw, object_pairs_hook=_unique_object, parse_constant=_reject_constant)
    except (UnicodeDecodeError, json.JSONDecodeError, EvidenceError) as error:
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
    ignored_tests: dict[str, SuiteIdentity] = {}
    suite_counts: dict[SuiteIdentity, tuple[int, int, int]] = {}
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
        identity = (package, binary, kind)
        if identity in suite_counts:
            raise NextestEvidenceError("duplicate nextest inventory suite identity")
        selected_count = ignored_count = filtered_count = 0
        for name, case in cases.items():
            listed_count += 1
            if not isinstance(name, str) or not name or not isinstance(case, dict):
                raise NextestEvidenceError("nextest inventory has invalid testcase")
            match = case.get("filter-match")
            if type(case.get("ignored")) is not bool or not isinstance(match, dict):
                raise NextestEvidenceError(
                    "nextest inventory has invalid filter-match or ignored flag"
                )
            event_name = f"{package}::{binary}${name}"
            if case["ignored"]:
                ignored_count += 1
            if match.get("status") == "mismatch":
                # nextest-metadata's explicit MismatchReason contract. Unknown
                # variants require deliberate admission, never silent exclusion.
                reason = match.get("reason")
                if (
                    set(match) != {"status", "reason"}
                    or not isinstance(reason, str)
                    or reason
                    not in {
                        "not-benchmark",
                        "ignored",
                        "string",
                        "expression",
                        "partition",
                        "rerun-already-passed",
                        "default-filter",
                    }
                ):
                    raise NextestEvidenceError("nextest inventory has invalid filter mismatch")
                if reason == "ignored":
                    if case["ignored"] is not True:
                        raise NextestEvidenceError("nextest ignored exclusion lacks ignored flag")
                    ignored_tests[event_name] = identity
                elif case["ignored"] is False:
                    filtered_count += 1
                continue
            if match != {"status": "matches"}:
                raise NextestEvidenceError("nextest inventory has invalid filter-match")
            if case.get("ignored") is not False:
                raise NextestEvidenceError("nextest inventory contains ignored required test")
            if event_name in expected:
                raise NextestEvidenceError(f"duplicate nextest inventory test: {event_name}")
            expected[event_name] = identity
            selected_count += 1
        suite_counts[identity] = (selected_count, ignored_count, filtered_count)
    if listed_count != payload["test-count"] or not expected:
        raise NextestEvidenceError("nextest inventory test-count disagrees with testcases")
    return NextestInventory(expected, ignored_tests, suite_counts, _token=_INVENTORY_TOKEN)


def parse_nextest(
    path: Path | RawFile, expected: Mapping[str, SuiteIdentity] | None = None
) -> NextestEvidence:
    try:
        raw = RawFile.capture(path) if isinstance(path, Path) else path
        return raw.consume_lines(lambda lines: _parse_nextest_stream(nullcontext(lines), expected))
    except (OSError, EvidenceError) as error:
        raise NextestEvidenceError(f"cannot read nextest evidence: {error}") from error


def parse_nextest_bytes(
    raw: bytes, expected: Mapping[str, SuiteIdentity] | None = None
) -> NextestEvidence:
    """Interpret the same immutable bytes captured and hashed by the caller."""
    with io.BytesIO(raw) as stream:
        lines = iter(lambda: stream.readline(CONTROL_DOCUMENT_BYTES + 1), b"")
        return _parse_nextest_stream(nullcontext(lines), expected)


def _parse_nextest_stream(
    stream: AbstractContextManager[Iterable[bytes]], expected: Mapping[str, SuiteIdentity] | None
) -> NextestEvidence:
    digest = hashlib.sha256()
    counts = {"ok": 0, "failed": 0, "ignored": 0, "timeout": 0}
    started: set[str] = set()
    terminal: dict[str, str] = {}
    suites_started = suites_finished = 0
    suites_passed = suites_ignored = 0
    active_suites: dict[tuple[str, str, str] | None, dict[str, int | None]] = {}
    started_suites: dict[str, tuple[str, str, str] | None] = {}
    inventory = expected if isinstance(expected, NextestInventory) else None
    metadata_bytes = 0
    try:
        with stream as lines:
            for line in lines:
                if len(line) > CONTROL_DOCUMENT_BYTES:
                    raise NextestEvidenceError("nextest event line exceeds explicit byte limit")
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
                        metadata_bytes += len(json.dumps(suite_identity).encode("utf-8"))
                        if metadata_bytes > CONTROL_DOCUMENT_BYTES:
                            raise NextestEvidenceError(
                                "nextest retained identities exceed byte limit"
                            )
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
                        if inventory is not None:
                            bound_counts = inventory.suite_counts.get(suite_identity)
                            if bound_counts is None:
                                raise NextestEvidenceError(
                                    "nextest binary identity mismatch: suite not collected"
                                )
                            if test_count != sum(bound_counts[:2]):
                                raise NextestEvidenceError(
                                    "nextest execution differs from collected tests: announced test count disagrees"
                                )
                        active_suites[suite_identity] = {
                            "test_count": test_count,
                            "terminal": 0,
                            "passed": 0,
                            "ignored_started": 0,
                            "ignored": 0,
                        }
                    elif outcome == "ok":
                        if suite_identity not in active_suites:
                            raise NextestEvidenceError("nextest suite finished before start")
                        suite_state = active_suites.pop(suite_identity)
                        suites_finished += 1
                        passed = _nonnegative(event.get("passed"), "passed")
                        failed = _nonnegative(event.get("failed"), "failed")
                        ignored = _nonnegative(event.get("ignored"), "ignored")
                        if inventory is not None:
                            selected, collected_ignored, filtered = inventory.suite_counts[
                                suite_identity
                            ]
                            if (
                                not suite_state["terminal"]
                                or passed != suite_state["passed"]
                                or ignored != collected_ignored
                            ):
                                raise NextestEvidenceError(
                                    "nextest suite/test pass counts disagree"
                                )
                            if suite_state["terminal"] != suite_state["test_count"]:
                                # 0.9.104's reporter decrements nonignored
                                # `running` for TestSkippedIgnored and may close
                                # then reopen a suite. Its counters describe the
                                # entire inventory in every fragment. Require
                                # those exact counters, not a partial-pass shim.
                                if (
                                    not collected_ignored
                                    or "filtered_out" not in event
                                    or _nonnegative(event["filtered_out"], "filtered")
                                    != (
                                        filtered
                                        + max(0, selected - passed - suite_state["ignored_started"])
                                    )
                                ):
                                    raise NextestEvidenceError(
                                        "nextest announced test count disagrees"
                                    )
                            elif (
                                "filtered_out" in event
                                and _nonnegative(event["filtered_out"], "filtered") != filtered
                            ):
                                raise NextestEvidenceError(
                                    "nextest filtered count differs from inventory"
                                )
                            suites_ignored += suite_state["ignored"]
                        else:
                            if (
                                suite_state["test_count"] is not None
                                and suite_state["terminal"] != suite_state["test_count"]
                            ):
                                raise NextestEvidenceError("nextest announced test count disagrees")
                            if suite_state["terminal"] != passed + failed + ignored:
                                raise NextestEvidenceError(
                                    "nextest suite/test pass counts disagree"
                                )
                            suites_ignored += ignored
                        if failed:
                            raise NextestEvidenceError("nextest suite reports failures")
                        suites_passed += passed
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
                if name not in started and name not in terminal:
                    metadata_bytes += len(json.dumps(name).encode("utf-8"))
                    if metadata_bytes > CONTROL_DOCUMENT_BYTES:
                        raise NextestEvidenceError("nextest retained identities exceed byte limit")
                if expected is not None:
                    if name not in expected:
                        if (
                            inventory is None
                            or name not in inventory.ignored
                            or outcome not in {"started", "ignored"}
                        ):
                            raise NextestEvidenceError(f"unexpected nextest test: {name}")
                        test_suite: SuiteIdentity | None = inventory.ignored[name]
                    else:
                        if outcome == "ignored":
                            raise NextestEvidenceError("nextest required test did not pass")
                        test_suite = expected[name]
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
                    if inventory is not None and name in inventory.ignored:
                        active_suites[test_suite]["ignored_started"] += 1
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
                if outcome == "ok":
                    active_suites[test_suite]["passed"] += 1
                elif outcome == "ignored":
                    active_suites[test_suite]["ignored"] += 1
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
        ignored_names = set(inventory.ignored) if inventory is not None else set()
        if ignored_names - started:
            raise NextestEvidenceError("nextest ignored outcome lacks a start event")
        if set(terminal) != set(expected) | ignored_names:
            raise NextestEvidenceError("nextest execution differs from collected tests")
        if any(terminal[name] != "ok" for name in expected) or any(
            terminal[name] != "ignored" for name in ignored_names
        ):
            raise NextestEvidenceError("nextest required test did not pass")
    return NextestEvidence(
        sha256=digest.hexdigest(),
        selected=len(expected) if expected is not None else sum(counts.values()),
        executed=counts["ok"] + counts["failed"] + counts["timeout"],
        passed=counts["ok"],
        failed=counts["failed"] + counts["timeout"],
        passed_names=frozenset(name for name, outcome in terminal.items() if outcome == "ok"),
    )
