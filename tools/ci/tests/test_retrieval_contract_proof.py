"""Contract tests for retrieval contract proof result production."""

from __future__ import annotations

import importlib.util
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[3]
SCRIPT = ROOT / "tools" / "benchmark" / "retrieval" / "contract_proof.py"
SPEC = importlib.util.spec_from_file_location("retrieval_contract_proof", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


def test_pytest_summary_uses_junit_counts(tmp_path: Path) -> None:
    junit = tmp_path / "pytest.xml"
    junit.write_text(
        '<testsuites tests="132" failures="0" errors="0" skipped="2" time="1.0"></testsuites>',
        encoding="utf-8",
    )
    assert MODULE.pytest_summary(junit) == {
        "command": "python3 -m pytest tools/ci/tests/test_retrieval_benchmark.py -q",
        "selected": 132,
        "executed": 130,
        "passed": 130,
        "failed": 0,
    }


def test_pytest_summary_accepts_pytest_nested_testsuite(tmp_path: Path) -> None:
    junit = tmp_path / "pytest.xml"
    junit.write_text(
        '<testsuites><testsuite name="pytest" tests="3" failures="0" errors="0" skipped="1" /></testsuites>',
        encoding="utf-8",
    )
    summary = MODULE.pytest_summary(junit)
    assert summary["selected"] == 3
    assert summary["executed"] == 2
    assert summary["passed"] == 2


def test_pytest_summary_rejects_failed_evidence(tmp_path: Path) -> None:
    junit = tmp_path / "pytest.xml"
    junit.write_text(
        '<testsuites tests="2" failures="1" errors="0" skipped="0"></testsuites>',
        encoding="utf-8",
    )
    with pytest.raises(SystemExit, match="reports failures"):
        MODULE.pytest_summary(junit)


def test_nextest_summary_uses_terminal_events(tmp_path: Path) -> None:
    evidence = tmp_path / "nextest.jsonl"
    evidence.write_text(
        '{"type":"suite","event":"started"}\n'
        '{"type":"test","event":"started","name":"one"}\n'
        '{"type":"test","event":"ok","name":"one"}\n'
        '{"type":"test","event":"ignored","name":"two"}\n'
        '{"type":"suite","event":"ok","passed":1,"failed":0,"ignored":1}\n',
        encoding="utf-8",
    )
    summary = MODULE.nextest_summary(evidence)
    assert summary["selected"] == 2
    assert summary["executed"] == 1
    assert summary["passed"] == 1
    assert summary["failed"] == 0


def test_nextest_summary_rejects_count_divergence(tmp_path: Path) -> None:
    evidence = tmp_path / "nextest.jsonl"
    evidence.write_text(
        '{"type":"suite","event":"started"}\n'
        '{"type":"test","event":"ok","name":"one"}\n'
        '{"type":"suite","event":"ok","passed":2,"failed":0,"ignored":0}\n',
        encoding="utf-8",
    )
    with pytest.raises(SystemExit, match="pass counts disagree"):
        MODULE.nextest_summary(evidence)
