"""Adversarial tests for raw proof runner outcome authority."""

from __future__ import annotations

import hashlib
import json
import os
import subprocess
import sys
from pathlib import Path

import pytest

from tools.ci import proof_execution_result as MODULE
from tools.ci.nextest_events import NextestEvidenceError, parse_nextest_inventory_bytes
from tools.ci.proof_execution_result import (
    ExecutionResultError,
    collect_pytest_inventory,
    derive_test_result,
)
from tools.ci.proof_run_source import ProofRunSource, write_run_binding


@pytest.mark.parametrize(
    "nodeid",
    (
        "tools/ci/tests/test_identity.py::test_plain",
        "tools/ci/tests/test_identity.py::TestIdentity::test_method",
        "tools/ci/tests/test_identity.py::test_parameter[Rust::Variant]",
        "tools/ci/tests/test_identity.py::TestIdentity::test_method[[nested]::Variant]",
        "tools/ci/tests/test_identity.py::test_parameter[::]",
    ),
)
def test_pytest_identity_matches_runner_mangling(nodeid: str) -> None:
    from _pytest.junitxml import mangle_test_address

    from tools.benchmark.retrieval.proof_inventory import junit_identity

    expected = ".".join(mangle_test_address(nodeid))
    assert MODULE.pytest_junit_identity(nodeid) == expected
    assert junit_identity(nodeid) == expected


def test_real_pytest_inventory_matches_parameterized_junit(tmp_path: Path) -> None:
    selector = "tools/ci/tests/test_identity.py"
    source = tmp_path / selector
    source.parent.mkdir(parents=True)
    source.write_text(
        "import pytest\n"
        "def test_plain():\n    assert True\n"
        "class TestIdentity:\n    def test_method(self):\n        assert True\n"
        "@pytest.mark.parametrize('value', ['Rust::Variant', '[nested]::Variant', '::'])\n"
        "def test_parameter(value):\n    assert value\n",
        encoding="utf-8",
    )
    collector = (
        f"import sys; sys.path.insert(0, {str(MODULE.ROOT)!r}); "
        "from pathlib import Path; "
        "from tools.ci.proof_execution_result import collect_pytest_inventory; "
        f"collect_pytest_inventory([{selector!r}], Path('inventory.json'))"
    )
    subprocess.run([sys.executable, "-c", collector], cwd=tmp_path, check=True)
    subprocess.run(
        [sys.executable, "-m", "pytest", "-q", selector, "--junitxml=junit.xml"],
        cwd=tmp_path,
        check=True,
    )
    counts, identities = MODULE._pytest_result(tmp_path / "junit.xml", tmp_path / "inventory.json")
    assert counts["selected"] == counts["passed"] == 5
    assert identities == {
        "tools.ci.tests.test_identity.test_plain",
        "tools.ci.tests.test_identity.TestIdentity.test_method",
        "tools.ci.tests.test_identity.test_parameter[Rust::Variant]",
        "tools.ci.tests.test_identity.test_parameter[[nested]::Variant]",
        "tools.ci.tests.test_identity.test_parameter[::]",
    }


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
    from tools.ci.tests.test_write_proof_manifest import _build_fixture_root

    root, _ = _build_fixture_root(tmp_path.resolve())
    selector = "tools/ci/tests/test_proof_execution_result.py"
    raw_dir = tmp_path / "raw"
    original_run = subprocess.run

    def collect(checkout: Path, selectors: list[str], output: Path, env: dict[str, str]) -> None:
        assert checkout != root
        assert env["PYTHONPATH"] == str(checkout)
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
        if command[:3] != [sys.executable, "-m", "pytest"]:
            return original_run(command, **kwargs)
        assert kwargs["cwd"] != MODULE.ROOT
        assert kwargs["env"]["PYTHONPATH"] == str(kwargs["cwd"])
        assert command[-1] == f"--junitxml={raw_dir / 'p12a-junit.xml'}"
        (raw_dir / "p12a-junit.xml").write_text(
            '<testsuite tests="1" failures="0" errors="0" skipped="0">'
            '<testcase classname="tools.ci.tests.test_proof_execution_result" '
            'name="test_case"/></testsuite>',
            encoding="utf-8",
        )
        return subprocess.CompletedProcess(command, 0)

    monkeypatch.chdir(root)
    monkeypatch.setattr(MODULE, "ROOT", root)
    monkeypatch.setattr(MODULE, "_collect_frozen_pytest", collect)
    monkeypatch.setattr(MODULE.subprocess, "run", run)
    assert MODULE.run_p12a_pytest([selector], raw_dir) == 0
    record = json.loads((raw_dir / "p12a-run-binding.json").read_bytes())
    assert record["source"]["head"] == subprocess.check_output(
        ["git", "rev-parse", "HEAD"], text=True,
    ).strip()
    assert record["cargo_lock_sha256"] is None
    assert record["daemon_sha256"] is None
    assert record["artifacts"] == {
        "inventory": hashlib.sha256((raw_dir / "p12a-inventory.json").read_bytes()).hexdigest(),
        "events": hashlib.sha256((raw_dir / "p12a-junit.xml").read_bytes()).hexdigest(),
    }
    with pytest.raises(ExecutionResultError, match="already exists"):
        MODULE.run_p12a_pytest([selector], raw_dir)


@pytest.mark.parametrize("during", ["collection", "execution", "transient execution"])
def test_run_p12a_refuses_same_head_source_drift(tmp_path, monkeypatch, during):
    from tools.ci.tests.test_write_proof_manifest import _build_fixture_root

    root, _ = _build_fixture_root(tmp_path.resolve())
    selector = "tools/ci/tests/test_source_drift.py"
    body = root / selector
    body.parent.mkdir(parents=True)
    transient = during == "transient execution"
    original_body = "def test_case():\n    assert False\n" if transient else "def test_case():\n    assert True\n"
    body.write_text(original_body)
    subprocess.run(["git", "add", selector], cwd=root, check=True)
    subprocess.run(["git", "commit", "-qm", "Track tested body"], cwd=root, check=True)
    raw = tmp_path / "external-evidence"
    runner_root = MODULE.ROOT
    original_collect = MODULE._collect_frozen_pytest
    original_run = subprocess.run

    def collect(checkout, selectors, output, env):
        # The fixture supplies the tested program. The installed runner supplies
        # the pytest collection plugin.
        collection_env = {**env, "PYTHONPATH": os.pathsep.join((str(checkout), str(runner_root)))}
        original_collect(checkout, selectors, output, collection_env)
        if during == "collection":
            body.write_text("def test_case():\n    assert 2 + 2 == 4\n")

    def run(args, **kwargs):
        if transient and args[:3] == [sys.executable, "-m", "pytest"]:
            body.write_text("def test_case():\n    assert True\n")
            try:
                assert kwargs["cwd"] != root
                assert (kwargs["cwd"] / selector).read_text() == original_body
                return original_run(args, **kwargs)
            finally:
                body.write_text(original_body)
        result = original_run(args, **kwargs)
        if during == "execution" and args[:3] == [sys.executable, "-m", "pytest"]:
            body.write_text("def test_case():\n    assert 2 + 2 == 4\n")
        return result

    monkeypatch.chdir(root)
    monkeypatch.setattr(MODULE, "ROOT", root)
    monkeypatch.setattr(MODULE, "_collect_frozen_pytest", collect)
    monkeypatch.setattr(MODULE.subprocess, "run", run)
    if transient:
        assert MODULE.run_p12a_pytest([selector], raw) == 1
        assert body.read_text() == original_body
    else:
        with pytest.raises(ExecutionResultError, match="proof source changed during"):
            MODULE.run_p12a_pytest([selector], raw)
    assert not (raw / "p12a-run-binding.json").exists()
    if during == "collection":
        assert not (raw / "p12a-junit.xml").exists()


def _nextest_fixture(
    root: Path, *, kind: str = "test"
) -> tuple[dict, list[dict[str, str]], Path]:
    raw = root / "raw"
    raw.mkdir()
    events = raw / "events.jsonl"
    inventory = raw / "inventory.json"
    package, binary = "quanta-index-core", "journal_owner"
    name = f"{package}::{binary}$passes"
    metadata = {"crate": package, "test_binary": binary, "kind": kind}
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
                        "kind": kind,
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
    _bind_artifact_digests(root, artifacts)
    return result, artifacts, events


def _bind_artifact_digests(root: Path, artifacts: list[dict[str, str]]) -> None:
    for artifact in artifacts:
        artifact["sha256"] = hashlib.sha256((root / artifact["path"]).read_bytes()).hexdigest()


def test_nextest_result_is_derived_from_complete_inventory(tmp_path: Path) -> None:
    result, artifacts, _ = _nextest_fixture(tmp_path)
    assert derive_test_result(tmp_path, result, artifacts) == (
        {"selected": 1, "executed": 1, "passed": 1, "failed": 0, "ignored": 0},
        {("nextest-jsonl", "quanta-index-core::journal_owner$passes")},
    )


@pytest.mark.parametrize("substitute", ["lock", "daemon", "events"])
def test_bound_nextest_refuses_substituted_archived_input(tmp_path: Path, substitute: str) -> None:
    result, artifacts, events = _nextest_fixture(tmp_path)
    (run,) = result["runs"]
    original = ProofRunSource("p04-read-view-lifetime", "1" * 40, "sha256:" + "2" * 64, "3" * 64)
    daemon_sha256 = "4" * 64
    binding = "raw/run-binding.json"
    write_run_binding(
        tmp_path / binding, original, format=run["format"],
        digests=dict(zip(("events", "inventory"), (row["sha256"] for row in artifacts), strict=True)),
        daemon_sha256=daemon_sha256,
    )
    run["run_binding"] = binding
    artifacts.append({"path": binding, "source_path": binding})
    _bind_artifact_digests(tmp_path, artifacts)
    counts, _ = derive_test_result(
        tmp_path, result, artifacts, run_source=original, daemon_sha256=daemon_sha256,
    )
    assert counts["passed"] == 1

    source = original
    if substitute == "lock":
        source = ProofRunSource(original.proof_id, original.head, original.dirty_digest, "5" * 64)
    elif substitute == "daemon":
        daemon_sha256 = "5" * 64
    else:
        events.write_bytes(b" " + events.read_bytes())
        _bind_artifact_digests(tmp_path, artifacts)
    with pytest.raises(ValueError, match="captured execution source differs"):
        derive_test_result(
            tmp_path, result, artifacts, run_source=source, daemon_sha256=daemon_sha256,
        )


@pytest.mark.parametrize("kind", ["test", "lib"])
def test_p04_archive_binds_process_observations_to_integration_target(
    tmp_path: Path, kind: str
) -> None:
    result, artifacts, events = _nextest_fixture(tmp_path, kind=kind)
    name = "quanta-index-core::journal_owner$passes"
    digest = "f" * 64
    nonce = "a" * 32
    run = result["runs"][0]
    run["daemon_observations"] = "raw/daemon-observations.json"
    (tmp_path / run["daemon_observations"]).write_text(json.dumps({
        "nonce": nonce,
        "events_sha256": hashlib.sha256(events.read_bytes()).hexdigest(),
        "inventory_sha256": hashlib.sha256((tmp_path / run["inventory"]).read_bytes()).hexdigest(),
        "observations": {
            name: {"schema_version": 1, "test": name, "nonce": nonce, "executables": [digest]},
        },
    }))
    artifacts.append({"source_path": run["daemon_observations"], "path": run["daemon_observations"]})
    _bind_artifact_digests(tmp_path, artifacts)
    observed: dict[str, str] = {}
    arguments = {"daemon_observations": observed, "daemon_tests": {name: 1}}
    if kind == "lib":
        with pytest.raises(ValueError, match="require Cargo integration test targets"):
            derive_test_result(tmp_path, result, artifacts, **arguments)
        assert observed == {}
    else:
        counts, _ = derive_test_result(tmp_path, result, artifacts, **arguments)
        assert counts["passed"] == 1 and observed == {name: digest}


@pytest.mark.parametrize(
    ("kind", "required_case", "accepted"),
    [("test", "passes", False), ("lib", "missing", False), ("lib", "passes", True)],
)
def test_required_lib_case_needs_the_exact_passing_library_body(
    tmp_path: Path, kind: str, required_case: str, accepted: bool
) -> None:
    result, artifacts, _ = _nextest_fixture(tmp_path, kind=kind)
    required = {"quanta-index-core": [required_case]}
    if not accepted:
        with pytest.raises(ExecutionResultError, match="registered nextest lib cases did not pass"):
            derive_test_result(tmp_path, result, artifacts, required_lib_cases=required)
    else:
        counts, _ = derive_test_result(tmp_path, result, artifacts, required_lib_cases=required)
        assert counts["passed"] == 1


@pytest.mark.parametrize(
    "filter_match",
    [
        None,
        False,
        [],
        {},
        {"status": "unknown"},
        {"status": "mismatch"},
        {"status": "mismatch", "reason": "unknown"},
    ],
)
def test_nextest_inventory_refuses_malformed_exclusion(
    tmp_path: Path, filter_match: object
) -> None:
    _nextest_fixture(tmp_path)
    inventory = json.loads((tmp_path / "raw/inventory.json").read_bytes())
    inventory["test-count"] = 2
    suite = next(iter(inventory["rust-suites"].values()))
    suite["testcases"]["omitted"] = {"ignored": False, "filter-match": filter_match}
    with pytest.raises(NextestEvidenceError, match="filter"):
        parse_nextest_inventory_bytes(json.dumps(inventory).encode())


@pytest.mark.parametrize(
    "reason",
    [
        "not-benchmark",
        "ignored",
        "string",
        "expression",
        "partition",
        "rerun-already-passed",
        "default-filter",
    ],
)
def test_nextest_inventory_admits_explicit_exclusion(tmp_path: Path, reason: str) -> None:
    _nextest_fixture(tmp_path)
    inventory = json.loads((tmp_path / "raw/inventory.json").read_bytes())
    inventory["test-count"] = 2
    suite = next(iter(inventory["rust-suites"].values()))
    suite["testcases"]["excluded"] = {
        "ignored": reason == "ignored",
        "filter-match": {"status": "mismatch", "reason": reason},
    }
    expected = parse_nextest_inventory_bytes(json.dumps(inventory).encode())
    assert set(expected) == {"quanta-index-core::journal_owner$passes"}


@pytest.mark.parametrize("artifact_index", [0, 1])
def test_execution_result_refuses_archive_digest_substitution(
    tmp_path: Path, artifact_index: int
) -> None:
    result, artifacts, _ = _nextest_fixture(tmp_path)
    artifact = artifacts[artifact_index]
    path = tmp_path / artifact["path"]
    # Whitespace preserves the successful result but changes its authority bytes.
    path.write_bytes(b" " + path.read_bytes())
    with pytest.raises(ExecutionResultError, match="digest"):
        derive_test_result(tmp_path, result, artifacts)


@pytest.mark.parametrize("artifact_index", [0, 1])
def test_execution_result_refuses_symlinked_archive(tmp_path: Path, artifact_index: int) -> None:
    result, artifacts, _ = _nextest_fixture(tmp_path)
    path = tmp_path / artifacts[artifact_index]["path"]
    replacement = tmp_path / "replacement"
    replacement.write_bytes(path.read_bytes())
    path.unlink()
    path.symlink_to(replacement)
    with pytest.raises(ExecutionResultError):
        derive_test_result(tmp_path, result, artifacts)


def test_execution_result_parses_captured_bytes_without_path_reopen(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    result, artifacts, _ = _nextest_fixture(tmp_path)
    read = MODULE._read_repo_regular_bytes
    captured = []

    def capture(root: Path, path: str, *, label: str) -> bytes:
        raw = read(root, path, label=label)
        captured.append(path)
        # Replacement after descriptor capture must not influence parsing.
        (root / path).write_bytes(b"untrusted replacement")
        return raw

    monkeypatch.setattr(MODULE, "_read_repo_regular_bytes", capture)
    counts, names = derive_test_result(tmp_path, result, artifacts)
    assert counts == {"selected": 1, "executed": 1, "passed": 1, "failed": 0, "ignored": 0}
    assert names == {("nextest-jsonl", "quanta-index-core::journal_owner$passes")}
    assert captured == ["raw/events.jsonl", "raw/inventory.json"]


def test_pytest_execution_custody_binds_digest_and_captured_bytes(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    events = tmp_path / "events.xml"
    inventory = tmp_path / "inventory.json"
    events.write_bytes(
        b'<testsuite tests="1" failures="0" errors="0" skipped="0">'
        b'<testcase classname="tools.ci.tests.test_example" name="test_ok"/>'
        b"</testsuite>"
    )
    inventory.write_text(
        json.dumps(
            {
                "schema_version": 1,
                "kind": "pytest",
                "selector": "tools/ci/tests/test_example.py",
                "tests": ["tools.ci.tests.test_example.test_ok"],
            }
        )
    )
    result = {
        "schema_version": 1,
        "runs": [
            {
                "format": "pytest-junit",
                "events": "events.xml",
                "inventory": "inventory.json",
            }
        ],
    }
    artifacts = [{"source_path": path.name, "path": path.name} for path in (events, inventory)]
    _bind_artifact_digests(tmp_path, artifacts)
    events.write_bytes(b" " + events.read_bytes())
    with pytest.raises(ExecutionResultError, match="digest"):
        derive_test_result(tmp_path, result, artifacts)
    events.write_bytes(events.read_bytes()[1:])
    read = MODULE._read_repo_regular_bytes

    def capture(root: Path, path: str, *, label: str) -> bytes:
        raw = read(root, path, label=label)
        (root / path).write_bytes(b"untrusted replacement")
        return raw

    monkeypatch.setattr(MODULE, "_read_repo_regular_bytes", capture)
    counts, names = derive_test_result(tmp_path, result, artifacts)
    assert counts["passed"] == 1
    assert names == {("pytest-junit", "tools.ci.tests.test_example.test_ok")}


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
    _bind_artifact_digests(tmp_path, artifacts)
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
    _bind_artifact_digests(tmp_path, artifacts)
    with pytest.raises(ExecutionResultError, match="did not pass"):
        derive_test_result(tmp_path, result, artifacts)
    junit.write_text(
        '<testsuite tests="1" failures="0" errors="0" skipped="0">'
        '<testcase classname="tools.ci.tests.test_example" name="test_ok"/>'
        "</testsuite>",
        encoding="utf-8",
    )
    _bind_artifact_digests(tmp_path, artifacts)
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
