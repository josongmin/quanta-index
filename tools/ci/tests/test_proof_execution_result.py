"""Adversarial tests for raw proof runner outcome authority."""

from __future__ import annotations

import json
import subprocess
from pathlib import Path

import pytest

from tools.ci import proof_execution_result as MODULE
from tools.ci.proof_execution_result import (
    ExecutionResultError,
    collect_pytest_inventory,
    derive_test_result,
)


@pytest.mark.parametrize("variable", ("PYTEST_ADDOPTS", "PYTEST_PLUGINS"))
def test_pytest_collection_refuses_environment_selection(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, variable: str
) -> None:
    monkeypatch.setenv(variable, "-k selected" if variable == "PYTEST_ADDOPTS" else "selector")
    output = tmp_path / "inventory.json"
    with pytest.raises(ExecutionResultError, match=variable):
        collect_pytest_inventory(["tools/ci/tests/test_proof_execution_result.py"], output)
    assert not output.exists()


@pytest.mark.parametrize(
    "selectors",
    (
        ["tools/ci/tests/test_proof_execution_result.py", "-k", "selected"],
        [
            "tools/ci/tests/test_proof_execution_result.py::test_nextest_result_is_derived_from_complete_inventory"
        ],
    ),
)
def test_pytest_collection_refuses_partial_selectors(tmp_path: Path, selectors: list[str]) -> None:
    output = tmp_path / "inventory.json"
    with pytest.raises(ExecutionResultError, match="complete test file selectors"):
        collect_pytest_inventory(selectors, output)
    assert not output.exists()


def test_pytest_collection_never_overwrites_existing_inventory(tmp_path: Path) -> None:
    output = tmp_path / "inventory.json"
    output.write_bytes(b"earlier run\n")
    with pytest.raises(ExecutionResultError, match="already exists"):
        collect_pytest_inventory(["tools/ci/tests/test_proof_execution_result.py"], output)
    assert output.read_bytes() == b"earlier run\n"


def test_run_p12a_emits_complete_junit_pair(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    selector = "tools/ci/tests/test_proof_execution_result.py"
    raw_dir = tmp_path / "raw"

    def collect(selectors: list[str], output: Path) -> None:
        assert selectors == [selector]
        output.parent.mkdir(parents=True)
        output.write_text(
            json.dumps(
                {
                    "schema_version": 1,
                    "kind": "pytest",
                    "selector": selector,
                    "tests": ["tools.ci.tests.test_proof_execution_result.test_case"],
                }
            ),
            encoding="utf-8",
        )

    def run(command: list[str], **kwargs: object) -> subprocess.CompletedProcess[str]:
        assert kwargs["cwd"] == MODULE.ROOT
        assert command[-1] == f"--junitxml={raw_dir / 'p12a-junit.xml'}"
        (raw_dir / "p12a-junit.xml").write_text(
            '<testsuite tests="1" failures="0" errors="0" skipped="0">'
            '<testcase classname="tools.ci.tests.test_proof_execution_result" '
            'name="test_case"/></testsuite>',
            encoding="utf-8",
        )
        return subprocess.CompletedProcess(command, 0)

    monkeypatch.setattr(MODULE, "collect_pytest_inventory", collect)
    monkeypatch.setattr(MODULE.subprocess, "run", run)
    assert MODULE.run_p12a_pytest([selector], raw_dir) == 0
    assert (raw_dir / "p12a-inventory.json").is_file()
    with pytest.raises(ExecutionResultError, match="already exists"):
        MODULE.run_p12a_pytest([selector], raw_dir)


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
        {("nextest-jsonl", "quanta-index-core::journal_owner$passes")},
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
                "selector": "tools/ci/tests/test_example.py",
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


@pytest.mark.parametrize(
    "placement",
    (
        '<error message="collection failed"/>',
        '<properties><property name="status"><failure/></property></properties>',
        "<unknown/>",
    ),
)
def test_pytest_junit_rejects_unaccounted_outcomes(tmp_path: Path, placement: str) -> None:
    inventory = tmp_path / "inventory.json"
    events = tmp_path / "events.xml"
    inventory.write_text(
        json.dumps(
            {
                "schema_version": 1,
                "kind": "pytest",
                "selector": "tools/ci/tests/test_example.py",
                "tests": ["tools.ci.tests.test_example.test_ok"],
            }
        ),
        encoding="utf-8",
    )
    events.write_text(
        '<testsuite tests="1" failures="0" errors="0" skipped="0">'
        '<testcase classname="tools.ci.tests.test_example" name="test_ok"/>'
        + placement
        + "</testsuite>",
        encoding="utf-8",
    )
    with pytest.raises(ExecutionResultError):
        MODULE._pytest_result(events, inventory)


@pytest.mark.parametrize("mutation", ("outside", "omitted", "duplicate", "partial"))
def test_pytest_inventory_binds_complete_file_selectors(tmp_path: Path, mutation: str) -> None:
    inventory = tmp_path / "inventory.json"
    events = tmp_path / "events.xml"
    selector = "tools/ci/tests/test_example.py"
    if mutation == "outside":
        selector = "tools/ci/tests/test_other.py"
    elif mutation == "omitted":
        selector += " tools/ci/tests/test_other.py"
    elif mutation == "duplicate":
        selector += " " + selector
    else:
        selector += "::test_ok"
    inventory.write_text(
        json.dumps(
            {
                "schema_version": 1,
                "kind": "pytest",
                "selector": selector,
                "tests": ["tools.ci.tests.test_example.test_ok"],
            }
        ),
        encoding="utf-8",
    )
    events.write_text(
        '<testsuite tests="1" failures="0" errors="0" skipped="0">'
        '<testcase classname="tools.ci.tests.test_example" name="test_ok"/>'
        "</testsuite>",
        encoding="utf-8",
    )
    with pytest.raises(ExecutionResultError):
        MODULE._pytest_result(events, inventory)
