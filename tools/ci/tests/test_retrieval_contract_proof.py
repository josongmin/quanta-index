"""Contract tests for retrieval contract proof result production."""

from __future__ import annotations

import importlib.util
import subprocess
import sys
from pathlib import Path

import pytest

from tools.ci import source_closure

ROOT = Path(__file__).resolve().parents[3]
SCRIPT = ROOT / "tools" / "benchmark" / "retrieval" / "contract_proof.py"
SPEC = importlib.util.spec_from_file_location("retrieval_contract_proof", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


def test_proof_recipes_capture_source_once_before_execution(tmp_path: Path) -> None:
    for name in ("retrieval-contract-proof", "retrieval-sdk-proof"):
        completed = subprocess.run(
            ["just", "--dry-run", name, str(tmp_path / name)],
            cwd=ROOT,
            check=True,
            capture_output=True,
            text=True,
        )
        commands = completed.stdout + completed.stderr
        assert commands.count("source_closure.py capture --profile retrieval") == 1
        assert "source_closure.py check --profile retrieval" not in commands
        assert "mkdir -p" not in commands
        assert commands.index("test ! -e") < commands.index("source_closure.py capture")
        first_execution = (
            "--lane test-daemon-lane" if name == "retrieval-sdk-proof" else "python3 -m pytest"
        )
        assert commands.index("source_closure.py capture") < commands.index(first_execution)


def test_retrieval_local_runs_both_rust_targets_once() -> None:
    completed = subprocess.run(
        ["just", "--dry-run", "retrieval-contract-local"],
        cwd=ROOT,
        check=True,
        capture_output=True,
        text=True,
    )
    commands = completed.stdout + completed.stderr
    assert commands.count("test -p quanta-index-retrieval-bench") == 1
    assert "--lib --test chunking_contract" in commands


def test_capture_rejects_dirty_source_before_creating_output(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    repo = tmp_path / "repo"
    repo.mkdir()

    def git(*args: str) -> None:
        subprocess.run(["git", *args], cwd=repo, check=True, capture_output=True)

    git("init", "-q")
    tracked = repo / "tracked.txt"
    tracked.write_text("initial\n", encoding="utf-8")
    git("add", "tracked.txt")
    git(
        "-c",
        "user.name=Proof Test",
        "-c",
        "user.email=proof@example.test",
        "commit",
        "-qm",
        "initial",
    )
    tracked.write_text("dirty\n", encoding="utf-8")

    monkeypatch.setitem(
        source_closure.PROFILES,
        "test-capture",
        {"cargo_packages": (), "paths": ("tracked.txt",)},
    )
    monkeypatch.chdir(repo)
    output = tmp_path / "proof" / "source-closure.json"
    monkeypatch.setattr(
        sys,
        "argv",
        ["source_closure.py", "capture", "--profile", "test-capture", "--out", str(output)],
    )
    with pytest.raises(SystemExit, match="refusing dirty relevant source"):
        source_closure.main()
    assert not output.parent.exists()

    git("add", "tracked.txt")
    git(
        "-c",
        "user.name=Proof Test",
        "-c",
        "user.email=proof@example.test",
        "commit",
        "-qm",
        "clean",
    )
    assert source_closure.main() == 0
    assert source_closure.load_and_verify(output, repo)["profile"] == "test-capture"


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
