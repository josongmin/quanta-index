"""Contract tests for retrieval contract proof result production."""

from __future__ import annotations

import importlib.util
import json
import subprocess
import sys
from pathlib import Path

import pytest

from tools.benchmark.retrieval.proof_inventory import verify_inventory_authority
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
        if name == "retrieval-sdk-proof":
            assert commands.count("metadata --format-version 1 --no-deps") == 1
            assert "set -e -o pipefail; target_dir=" in commands
            assert 'QUANTA_INDEX_SEARCHD_BIN="$target_dir/debug/quanta-index-searchd"' in commands
            assert '--runner-bin "$target_dir/debug/quanta-index-retrieval-bench"' in commands


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
    cases = "".join(f'<testcase name="test_{index}" />' for index in range(130))
    cases += '<testcase name="skip_1"><skipped /></testcase>'
    cases += '<testcase name="skip_2"><skipped /></testcase>'
    junit.write_text(
        '<testsuites tests="132" failures="0" errors="0" skipped="2" time="1.0">'
        f'<testsuite tests="132" failures="0" errors="0" skipped="2">{cases}</testsuite>'
        "</testsuites>",
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
        '<testsuites><testsuite name="pytest" tests="3" failures="0" errors="0" skipped="1">'
        '<testcase name="one"/><testcase name="two"/>'
        '<testcase name="three"><skipped/></testcase>'
        "</testsuite></testsuites>",
        encoding="utf-8",
    )
    summary = MODULE.pytest_summary(junit)
    assert summary["selected"] == 3
    assert summary["executed"] == 2
    assert summary["passed"] == 2


def test_pytest_summary_rejects_failed_evidence(tmp_path: Path) -> None:
    junit = tmp_path / "pytest.xml"
    junit.write_text(
        '<testsuites><testsuite tests="2" failures="1" errors="0" skipped="0">'
        '<testcase name="one"/><testcase name="two"><failure/></testcase>'
        "</testsuite></testsuites>",
        encoding="utf-8",
    )
    with pytest.raises(SystemExit, match="reports failures"):
        MODULE.pytest_summary(junit)


@pytest.mark.parametrize(
    ("xml", "error"),
    [
        ('<testsuite tests="1" failures="0" errors="0" skipped="0"/>', "count disagrees"),
        (
            '<testsuites tests="2" failures="0" errors="0" skipped="0">'
            '<testsuite tests="1" failures="0" errors="0" skipped="0">'
            '<testcase name="one"/></testsuite></testsuites>',
            "root tests count disagrees",
        ),
        (
            '<testsuite tests="1" failures="0" errors="0" skipped="0">'
            '<testcase name="one"><failure/></testcase></testsuite>',
            "failures count disagrees",
        ),
        (
            '<testsuite tests="2" failures="0" errors="0" skipped="0">'
            '<testcase name="one"/><testcase name="one"/></testsuite>',
            "duplicate pytest JUnit testcase",
        ),
        (
            '<testsuite tests="1" failures="0" errors="0" skipped="0">'
            '<testcase name="hidden"/>'
            '<testsuite tests="1" failures="0" errors="0" skipped="0">'
            '<testcase name="visible"/></testsuite></testsuite>',
            "nested suite hides direct testcases",
        ),
    ],
)
def test_pytest_summary_rejects_false_green_xml(tmp_path: Path, xml: str, error: str) -> None:
    junit = tmp_path / "pytest.xml"
    junit.write_text(xml, encoding="utf-8")
    with pytest.raises(SystemExit, match=error):
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


@pytest.mark.parametrize(
    ("events", "error"),
    [
        (
            '{"type":"suite","event":"started"}\n'
            '{"type":"test","event":"ok","name":"one"}\n'
            '{"type":"suite","event":"ok","passed":1,"failed":0,"ignored":0}\n',
            "lacks a start event",
        ),
        (
            '{"type":"suite","event":"started"}\n'
            '{"type":"test","event":"started","name":"one"}\n'
            '{"type":"test","event":"ok","name":"one"}\n'
            '{"type":"test","event":"ok","name":"one"}\n'
            '{"type":"suite","event":"ok","passed":2,"failed":0,"ignored":0}\n',
            "duplicate nextest test outcome",
        ),
        (
            '{"type":"suite","event":"started"}\n'
            '{"type":"test","event":"started","name":"one"}\n'
            '{"type":"test","event":"ok","name":"one","name":"two"}\n'
            '{"type":"suite","event":"ok","passed":1,"failed":0,"ignored":0}\n',
            "duplicate nextest JSON key",
        ),
        (
            '{"type":"suite","event":"started"}\n'
            '{"type":"test","event":"started","name":"one"}\n'
            '{"type":"test","event":"started","name":"two"}\n'
            '{"type":"test","event":"ok","name":"one"}\n'
            '{"type":"suite","event":"ok","passed":1,"failed":0,"ignored":0}\n',
            "incomplete test events",
        ),
    ],
)
def test_nextest_summary_rejects_false_green_events(
    tmp_path: Path, events: str, error: str
) -> None:
    evidence = tmp_path / "nextest.jsonl"
    evidence.write_text(events, encoding="utf-8")
    with pytest.raises(SystemExit, match=error):
        MODULE.nextest_summary(evidence)


def _python_inventory(tmp_path: Path, names: list[str]) -> Path:
    path = tmp_path / "python-inventory.json"
    path.write_text(
        json.dumps(
            {
                "schema_version": 1,
                "kind": "pytest",
                "selector": "tools/ci/tests/test_retrieval_benchmark.py",
                "tests": sorted(names),
            }
        ),
        encoding="utf-8",
    )
    return path


def test_pytest_inventory_requires_every_collected_test_to_pass(tmp_path: Path) -> None:
    required = [
        "tools.ci.tests.test_retrieval_benchmark.test_one",
        "tools.ci.tests.test_retrieval_benchmark.test_two",
    ]
    inventory = _python_inventory(tmp_path, required)
    junit = tmp_path / "pytest.xml"
    case = '<testcase classname="tools.ci.tests.test_retrieval_benchmark" name="test_one"/>'
    junit.write_text(
        '<testsuite tests="1" failures="0" errors="0" skipped="0">' + case + "</testsuite>",
        encoding="utf-8",
    )
    with pytest.raises(SystemExit, match="differs from collected"):
        MODULE.pytest_summary(junit, inventory)
    junit.write_text(
        '<testsuite tests="2" failures="0" errors="0" skipped="0">'
        + case
        + '<testcase classname="tools.ci.tests.test_retrieval_benchmark" name="test_two"/>'
        + "</testsuite>",
        encoding="utf-8",
    )
    assert MODULE.pytest_summary(junit, inventory)["passed"] == 2
    junit.write_text(
        '<testsuite tests="2" failures="0" errors="0" skipped="1">'
        + case
        + '<testcase classname="tools.ci.tests.test_retrieval_benchmark" name="test_two"><skipped/></testcase>'
        + "</testsuite>",
        encoding="utf-8",
    )
    with pytest.raises(SystemExit, match="differs from collected"):
        MODULE.pytest_summary(junit, inventory)


def test_pytest_duplicate_across_leaf_suites_is_rejected(tmp_path: Path) -> None:
    junit = tmp_path / "pytest.xml"
    suite = (
        '<testsuite tests="1" failures="0" errors="0" skipped="0">'
        '<testcase classname="a" name="same"/></testsuite>'
    )
    junit.write_text(f"<testsuites>{suite}{suite}</testsuites>", encoding="utf-8")
    with pytest.raises(SystemExit, match="duplicate pytest JUnit testcase"):
        MODULE.pytest_summary(junit)


def _nextest_inventory(tmp_path: Path, names: list[str]) -> Path:
    path = tmp_path / "nextest-inventory.json"
    path.write_text(
        json.dumps(
            {
                "test-count": len(names),
                "rust-suites": {
                    "quanta-index-retrieval-bench::chunking_contract": {
                        "package-name": "quanta-index-retrieval-bench",
                        "binary-name": "chunking_contract",
                        "kind": "test",
                        "status": "listed",
                        "testcases": {
                            name: {"ignored": False, "filter-match": {"status": "matches"}}
                            for name in names
                        },
                    }
                },
            }
        ),
        encoding="utf-8",
    )
    return path


def _events(names: list[str], announced: int) -> str:
    meta = {
        "crate": "quanta-index-retrieval-bench",
        "test_binary": "chunking_contract",
        "kind": "test",
    }
    rows = [{"type": "suite", "event": "started", "test_count": announced, "nextest": meta}]
    for name in names:
        full = f"quanta-index-retrieval-bench::chunking_contract${name}"
        rows += [
            {"type": "test", "event": "started", "name": full},
            {"type": "test", "event": "ok", "name": full},
        ]
    rows.append(
        {
            "type": "suite",
            "event": "ok",
            "passed": len(names),
            "failed": 0,
            "ignored": 0,
            "nextest": meta,
        }
    )
    return "".join(json.dumps(row) + "\n" for row in rows)


def test_nextest_inventory_requires_all_collected_tests_and_announced_count(tmp_path: Path) -> None:
    inventory = _nextest_inventory(tmp_path, ["one", "two"])
    events = tmp_path / "nextest.jsonl"
    events.write_text(_events(["one"], 1), encoding="utf-8")
    with pytest.raises(SystemExit, match="differs from collected"):
        MODULE.nextest_summary(events, inventory)
    events.write_text(_events(["one", "two"], 99), encoding="utf-8")
    with pytest.raises(SystemExit, match="announced test count disagrees"):
        MODULE.nextest_summary(events, inventory)
    events.write_text(_events(["one", "two"], 2), encoding="utf-8")
    assert MODULE.nextest_summary(events, inventory)["passed"] == 2
    events.write_text(_events(["one", "other"], 2), encoding="utf-8")
    with pytest.raises(SystemExit, match="unexpected nextest test"):
        MODULE.nextest_summary(events, inventory)
    wrong_binary = _events(["one", "two"], 2).replace(
        '"test_binary": "chunking_contract"', '"test_binary": "other_binary"'
    )
    events.write_text(wrong_binary, encoding="utf-8")
    with pytest.raises(SystemExit, match="binary identity mismatch"):
        MODULE.nextest_summary(events, inventory)


def test_source_controlled_inventory_refuses_coordinated_partial_proof(tmp_path: Path) -> None:
    required = [
        "tools.ci.tests.test_retrieval_benchmark.test_one",
        "tools.ci.tests.test_retrieval_benchmark.test_two",
    ]
    authority = tmp_path / "required.json"
    authority.write_text(
        json.dumps({"schema_version": 1, "python": required, "rust": ["r"], "sdk": ["s"]}),
        encoding="utf-8",
    )
    inventory = _python_inventory(tmp_path, required[:1])
    junit = tmp_path / "pytest.xml"
    junit.write_text(
        '<testsuite tests="1" failures="0" errors="0" skipped="0">'
        '<testcase classname="tools.ci.tests.test_retrieval_benchmark" name="test_one"/>'
        "</testsuite>",
        encoding="utf-8",
    )
    # Raw evidence and a forged smaller collection agree, but neither may replace
    # the source-controlled authority for the rail.
    assert MODULE.pytest_summary(junit, inventory)["passed"] == 1
    with pytest.raises(ValueError, match="differs from source-controlled"):
        verify_inventory_authority(inventory, "python", authority)
    _python_inventory(tmp_path, required)
    verify_inventory_authority(inventory, "python", authority)
