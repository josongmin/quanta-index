"""Adversarial tests for raw proof runner outcome authority."""

from __future__ import annotations

import json
from pathlib import Path

import pytest

from tools.ci.proof_execution_result import ExecutionResultError, derive_test_result


def _nextest_fixture(root: Path) -> tuple[dict, list[dict[str, str]], Path]:
    raw = root / "raw"
    raw.mkdir()
    events = raw / "events.jsonl"
    inventory = raw / "inventory.json"
    package, binary = "quanta-index-core", "journal_owner"
    name = f"{package}::{binary}$passes"
    metadata = {"crate": package, "test_binary": binary, "kind": "test"}
    rows = [
        {"type": "suite", "event": "started", "test_count": 1, "nextest": metadata},
        {"type": "test", "event": "started", "name": name},
        {"type": "test", "event": "ok", "name": name},
        {
            "type": "suite",
            "event": "ok",
            "passed": 1,
            "failed": 0,
            "ignored": 0,
            "nextest": metadata,
        },
    ]
    events.write_text("".join(json.dumps(row) + "\n" for row in rows), encoding="utf-8")
    inventory.write_text(
        json.dumps(
            {
                "test-count": 1,
                "rust-suites": {
                    f"{package}::{binary}": {
                        "package-name": package,
                        "binary-name": binary,
                        "kind": "test",
                        "status": "listed",
                        "testcases": {
                            "passes": {"ignored": False, "filter-match": {"status": "matches"}}
                        },
                    },
                },
            }
        ),
        encoding="utf-8",
    )
    result = {
        "schema_version": 1,
        "runs": [
            {
                "format": "nextest-jsonl",
                "events": "raw/events.jsonl",
                "inventory": "raw/inventory.json",
            }
        ],
    }
    artifacts = [
        {"source_path": "raw/events.jsonl", "path": "raw/events.jsonl"},
        {"source_path": "raw/inventory.json", "path": "raw/inventory.json"},
    ]
    return result, artifacts, events


def test_nextest_result_is_derived_from_complete_inventory(tmp_path: Path) -> None:
    result, artifacts, _ = _nextest_fixture(tmp_path)
    assert derive_test_result(tmp_path, result, artifacts) == (
        {"selected": 1, "executed": 1, "passed": 1, "failed": 0, "ignored": 0},
        {"quanta-index-core::journal_owner$passes"},
    )


@pytest.mark.parametrize("mutation", ["missing", "duplicate", "wrong-test", "arbitrary-log"])
def test_nextest_result_rejects_false_evidence(tmp_path: Path, mutation: str) -> None:
    result, artifacts, events = _nextest_fixture(tmp_path)
    rows = [json.loads(line) for line in events.read_text().splitlines()]
    if mutation == "missing":
        rows.pop(-2)
    elif mutation == "duplicate":
        rows.insert(-1, rows[-2])
    elif mutation == "wrong-test":
        rows[1]["name"] = "quanta-index-core::journal_owner$other"
        rows[2]["name"] = "quanta-index-core::journal_owner$other"
    else:
        events.write_text("proof passed\n", encoding="utf-8")
    if mutation != "arbitrary-log":
        events.write_text("".join(json.dumps(row) + "\n" for row in rows), encoding="utf-8")
    with pytest.raises(ExecutionResultError):
        derive_test_result(tmp_path, result, artifacts)


def test_execution_result_rejects_reused_or_missing_evidence(tmp_path: Path) -> None:
    result, artifacts, _ = _nextest_fixture(tmp_path)
    result["runs"].append(dict(result["runs"][0]))
    with pytest.raises(ExecutionResultError, match="duplicate execution evidence"):
        derive_test_result(tmp_path, result, artifacts)
    result["runs"].pop()
    result["runs"][0]["events"] = "raw/arbitrary.log"
    with pytest.raises(ExecutionResultError, match="not an archived proof artifact"):
        derive_test_result(tmp_path, result, artifacts)


def test_pytest_result_rejects_skipped_case(tmp_path: Path) -> None:
    raw = tmp_path / "raw"
    raw.mkdir()
    inventory = raw / "inventory.json"
    junit = raw / "junit.xml"
    inventory.write_text(
        json.dumps(
            {
                "schema_version": 1,
                "kind": "pytest",
                "selector": "tools/ci/tests",
                "tests": ["tools.ci.tests.test_example.test_ok"],
            }
        ),
        encoding="utf-8",
    )
    junit.write_text(
        '<testsuite tests="1" failures="0" errors="0" skipped="1">'
        '<testcase classname="tools.ci.tests.test_example" name="test_ok">'
        "<skipped/></testcase></testsuite>",
        encoding="utf-8",
    )
    result = {
        "schema_version": 1,
        "runs": [
            {
                "format": "pytest-junit",
                "events": "raw/junit.xml",
                "inventory": "raw/inventory.json",
            }
        ],
    }
    artifacts = [
        {"source_path": "raw/junit.xml", "path": "raw/junit.xml"},
        {"source_path": "raw/inventory.json", "path": "raw/inventory.json"},
    ]
    with pytest.raises(ExecutionResultError, match="did not pass"):
        derive_test_result(tmp_path, result, artifacts)
    junit.write_text(
        '<testsuite tests="1" failures="0" errors="0" skipped="0">'
        '<testcase classname="tools.ci.tests.test_example" name="test_ok"/>'
        "</testsuite>",
        encoding="utf-8",
    )
    assert derive_test_result(tmp_path, result, artifacts)[0]["passed"] == 1
