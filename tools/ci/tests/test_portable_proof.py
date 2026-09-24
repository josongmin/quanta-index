"""Focused checks for the shell-free retrieval proof prerequisite producer."""

from __future__ import annotations

import hashlib
import json
import subprocess
from pathlib import Path

import pytest

from tools.benchmark.retrieval import portable_proof


def test_collected_pytest_identity_normalizes_windows_separator() -> None:
    nodeid = r"tools\ci\tests\test_retrieval_benchmark.py::test_one"
    assert portable_proof.proof_inventory.junit_identity(nodeid) == (
        "tools.ci.tests.test_retrieval_benchmark.test_one"
    )


def _rust_inventory(binary: str, test: str) -> bytes:
    return json.dumps(
        {
            "test-count": 1,
            "rust-suites": {
                f"quanta-index-retrieval-bench::{binary}": {
                    "package-name": "quanta-index-retrieval-bench",
                    "binary-name": binary,
                    "kind": "test",
                    "status": "listed",
                    "testcases": {test: {"ignored": False, "filter-match": {"status": "matches"}}},
                }
            },
        }
    ).encode()


def _events(binary: str, test: str) -> bytes:
    identity = f"quanta-index-retrieval-bench::{binary}${test}"
    meta = {"crate": "quanta-index-retrieval-bench", "test_binary": binary, "kind": "test"}
    rows = [
        {"type": "suite", "event": "started", "test_count": 1, "nextest": meta},
        {"type": "test", "event": "started", "name": identity},
        {"type": "test", "event": "ok", "name": identity},
        {"type": "suite", "event": "ok", "passed": 1, "failed": 0, "ignored": 0, "nextest": meta},
    ]
    return ("\n".join(json.dumps(row) for row in rows) + "\n").encode()


@pytest.fixture
def fake_execution(tmp_path: Path, monkeypatch: pytest.MonkeyPatch):
    out = tmp_path / "proof"
    target = tmp_path / "target"
    runner = target / "debug" / "quanta-index-retrieval-bench"
    searchd = target / "debug" / "quanta-index-searchd"
    tools = {
        name: {"path": f"/fake/{name}", "sha256": "a" * 64, "version": "fixture"}
        for name in ("python", "cargo", "cargo-nextest", "rustc", "git")
    }
    monkeypatch.setattr(portable_proof, "_source_revision", lambda: "b" * 40)
    monkeypatch.setattr(portable_proof, "_tools", lambda: tools)
    monkeypatch.setattr(portable_proof, "_os_identity", lambda: {"system": "fixture"})
    monkeypatch.setattr(
        portable_proof.proof_inventory, "verify_inventory_authority", lambda *_: None
    )
    calls = []

    def run(argv, **kwargs):
        assert kwargs["cwd"] == portable_proof.ROOT
        assert kwargs["capture_output"] is True
        assert kwargs["check"] is False
        calls.append((argv, kwargs["env"]))
        if "proof_inventory.py" in argv[1]:
            (out / "python-inventory.json").write_text(
                json.dumps(
                    {
                        "schema_version": 1,
                        "kind": "pytest",
                        "selector": portable_proof.proof_inventory.PYTHON_SELECTOR,
                        "tests": ["tools.ci.tests.test_retrieval_benchmark.test_one"],
                    }
                ),
                encoding="utf-8",
            )
            raw = b""
        elif argv[1:3] == ["nextest", "list"]:
            binary = "sdk_roundtrip" if "sdk_roundtrip" in argv else "chunking_contract"
            test = portable_proof.sdk_proof.PROOF_TEST if binary == "sdk_roundtrip" else "one"
            raw = _rust_inventory(binary, test)
        elif argv[1:3] == ["nextest", "run"]:
            binary = "sdk_roundtrip" if "sdk_roundtrip" in argv else "chunking_contract"
            test = portable_proof.sdk_proof.PROOF_TEST if binary == "sdk_roundtrip" else "one"
            raw = _events(binary, test)
            if binary == "sdk_roundtrip":
                digest = hashlib.sha256(runner.read_bytes()).hexdigest()
                (out / "actual-runner-record.json").write_text(
                    json.dumps(
                        {
                            "schema_version": 3,
                            "captures": {
                                "run": {
                                    "runner_binary": {"name": "runner", "digest": digest},
                                    "searchd_binary": {
                                        "binary_digest": hashlib.sha256(
                                            searchd.read_bytes()
                                        ).hexdigest()
                                    },
                                    "receipt_digest": "c" * 64,
                                    "activation_digest": "d" * 64,
                                }
                            },
                            "route_provenance": {"lexical": {"capture_id": "run"}},
                        }
                    ),
                    encoding="utf-8",
                )
        elif argv[1] == "-m":
            (out / "python-junit.xml").write_text(
                '<testsuite tests="1" failures="0" errors="0" skipped="0">'
                '<testcase classname="tools.ci.tests.test_retrieval_benchmark" name="test_one"/>'
                "</testsuite>",
                encoding="utf-8",
            )
            raw = b""
        elif argv[1] == "build":
            runner.parent.mkdir(parents=True, exist_ok=True)
            runner.write_bytes(b"runner")
            searchd.write_bytes(b"searchd")
            raw = b""
        elif argv[1] == "metadata":
            raw = json.dumps({"target_directory": str(target)}).encode()
        else:
            raise AssertionError(argv)
        return subprocess.CompletedProcess(argv, 0, raw, b"")

    monkeypatch.setattr(portable_proof.subprocess, "run", run)
    return out, runner, calls


@pytest.mark.parametrize("rail", ["contract", "sdk"])
def test_producer_and_validator_bind_execution_and_inputs(fake_execution, rail: str) -> None:
    out, runner, calls = fake_execution
    receipt = portable_proof.produce(rail, out)
    assert portable_proof.validate(receipt)["rail"] == rail
    assert len(calls) == (4 if rail == "contract" else 5)
    assert all(isinstance(argv, list) for argv, _ in calls)
    if rail == "sdk":
        runner.write_bytes(b"changed")
        with pytest.raises(ValueError, match="binary identity changed"):
            portable_proof.validate(receipt)
    else:
        (out / "rust-test.stdout").write_bytes(b"truncated")
        with pytest.raises(ValueError, match="command output changed"):
            portable_proof.validate(receipt)


def test_validator_rejects_rewritten_command_and_missing_evidence(fake_execution) -> None:
    out, _, _ = fake_execution
    receipt = portable_proof.produce("contract", out)
    data = json.loads(receipt.read_text(encoding="utf-8"))
    data["commands"][3]["argv"][2] = "list"
    receipt.write_text(json.dumps(data), encoding="utf-8")
    with pytest.raises(ValueError, match="prescribed rail"):
        portable_proof.validate(receipt)
    data["commands"][3]["argv"][2] = "run"
    data["evidence"].pop("python-junit.xml")
    receipt.write_text(json.dumps(data), encoding="utf-8")
    with pytest.raises(ValueError, match="evidence set"):
        portable_proof.validate(receipt)


def test_validator_rejects_environment_and_sdk_record_substitution(fake_execution) -> None:
    out, _, _ = fake_execution
    receipt = portable_proof.produce("sdk", out)
    data = json.loads(receipt.read_text(encoding="utf-8"))
    data["commands"][4]["environment"]["QUANTA_INDEX_SEARCHD_BIN"] = "/other/searchd"
    receipt.write_text(json.dumps(data), encoding="utf-8")
    with pytest.raises(ValueError, match="prescribed rail"):
        portable_proof.validate(receipt)

    data["commands"][4]["environment"]["QUANTA_INDEX_SEARCHD_BIN"] = data["binaries"]["searchd"][
        "path"
    ]
    receipt.write_text(json.dumps(data), encoding="utf-8")
    record = out / "actual-runner-record.json"
    payload = json.loads(record.read_text(encoding="utf-8"))
    payload["captures"]["run"]["searchd_binary"]["binary_digest"] = "e" * 64
    record.write_text(json.dumps(payload), encoding="utf-8")
    data["evidence"]["actual-runner-record.json"] = portable_proof._sha(record)
    receipt.write_text(json.dumps(data), encoding="utf-8")
    with pytest.raises(SystemExit, match="searchd binary digest differs"):
        portable_proof.validate(receipt)


def test_sdk_record_rejects_duplicate_json_key(fake_execution) -> None:
    out, _, _ = fake_execution
    receipt = portable_proof.produce("sdk", out)
    record = out / "actual-runner-record.json"
    record.write_bytes(
        record.read_bytes().replace(
            b'"schema_version": 3', b'"schema_version": 3, "schema_version": 3'
        )
    )
    data = json.loads(receipt.read_text(encoding="utf-8"))
    data["evidence"]["actual-runner-record.json"] = portable_proof._sha(record)
    receipt.write_text(json.dumps(data), encoding="utf-8")
    with pytest.raises(SystemExit, match="duplicate runner record JSON key"):
        portable_proof.validate(receipt)


def test_failed_command_cannot_emit_receipt(
    fake_execution, monkeypatch: pytest.MonkeyPatch
) -> None:
    out, _, _ = fake_execution

    def fail(*_args, **_kwargs):
        return subprocess.CompletedProcess([], 1, b"partial", b"failure")

    monkeypatch.setattr(portable_proof.subprocess, "run", fail)
    with pytest.raises(ValueError, match="exit 1"):
        portable_proof.produce("contract", out)
    assert not (out / "portable-proof-receipt.json").exists()
