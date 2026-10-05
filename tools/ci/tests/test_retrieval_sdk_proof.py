"""Contract tests for retrieval SDK proof result production."""

from __future__ import annotations

import hashlib
import importlib.util
import json
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[3]
SCRIPT = ROOT / "tools" / "benchmark" / "retrieval" / "sdk_proof.py"
SPEC = importlib.util.spec_from_file_location("retrieval_sdk_proof", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


def _fixture(tmp_path: Path) -> tuple[Path, Path, Path]:
    runner = tmp_path / "runner"
    runner.write_bytes(b"actual runner bytes")
    digest = hashlib.sha256(runner.read_bytes()).hexdigest()
    record = tmp_path / "record.json"
    record.write_text(
        json.dumps(
            {
                "schema_version": 5,
                "span_accounting_version": 1,
                "captures": {
                    "run-lexical": {
                        "runner_binary": {"name": "runner", "digest": digest},
                        "receipt_digest": "a" * 64,
                        "activation_digest": "b" * 64,
                    }
                },
                "route_provenance": {"lexical": {"capture_id": "run-lexical"}},
            }
        ),
        encoding="utf-8",
    )
    nextest = tmp_path / "nextest.jsonl"
    nextest.write_text(
        '{"type":"suite","event":"started"}\n'
        '{"type":"test","event":"started","name":"actual_runner_binary_emits_receipt_bound_v5_record"}\n'
        '{"type":"test","event":"ok","name":"actual_runner_binary_emits_receipt_bound_v5_record"}\n'
        '{"type":"suite","event":"ok","passed":1,"failed":0,"ignored":0}\n',
        encoding="utf-8",
    )
    return record, nextest, runner


def test_build_summary_binds_actual_runner_and_machine_counts(tmp_path: Path) -> None:
    record, nextest, runner = _fixture(tmp_path)
    summary = MODULE.build_summary(record, nextest, runner)
    assert summary == {
        "command": "just retrieval-sdk-proof",
        "separate_process": True,
        "sealed_receipt_digest": "a" * 64,
        "activation_ack_digest": "b" * 64,
        "empty_check": True,
        "binary_digest": hashlib.sha256(runner.read_bytes()).hexdigest(),
        "sdk_route": "lexical",
        "selected": 1,
        "executed": 1,
        "passed": 1,
        "failed": 0,
    }


def test_build_summary_fresh_command_is_explicit_and_closed(tmp_path: Path) -> None:
    record, nextest, runner = _fixture(tmp_path)
    fresh = MODULE.build_summary(record, nextest, runner, command=MODULE.SDK_FRESH_COMMAND)
    assert fresh["command"] == "just retrieval-sdk-proof-fresh"
    with pytest.raises(ValueError, match="unsupported SDK proof command"):
        MODULE.build_summary(record, nextest, runner, command="just unrelated-command")


def test_large_runner_binary_is_hashed_without_control_document_admission(tmp_path, monkeypatch):
    record, nextest, runner = _fixture(tmp_path)
    content = b"binary" * (3 * 1024 * 1024)
    runner.write_bytes(content)
    expected = hashlib.sha256(content).hexdigest()
    payload = json.loads(record.read_text())
    payload["captures"]["run-lexical"]["runner_binary"]["digest"] = expected
    record.write_text(json.dumps(payload))
    monkeypatch.setattr(Path, "read_bytes", lambda *_: pytest.fail("whole binary read"))
    result = MODULE.build_summary(record, nextest, runner)
    assert result["passed"] == 1
    assert result["binary_digest"] == expected


def test_build_summary_rejects_binary_substitution(tmp_path: Path) -> None:
    record, nextest, runner = _fixture(tmp_path)
    runner.write_bytes(b"substituted")
    with pytest.raises(SystemExit, match="differs from the executable"):
        MODULE.build_summary(record, nextest, runner)


def test_build_summary_rejects_downgraded_current_record(tmp_path: Path) -> None:
    record, nextest, runner = _fixture(tmp_path)
    payload = json.loads(record.read_text(encoding="utf-8"))
    payload.pop("span_accounting_version")
    record.write_text(json.dumps(payload), encoding="utf-8")
    with pytest.raises(SystemExit, match="lacks indexed-span protocol"):
        MODULE.build_summary(record, nextest, runner)


def test_build_summary_rejects_missing_proof_test(tmp_path: Path) -> None:
    record, nextest, runner = _fixture(tmp_path)
    nextest.write_text(
        '{"type":"suite","event":"started"}\n'
        '{"type":"test","event":"started","name":"different_test"}\n'
        '{"type":"test","event":"ok","name":"different_test"}\n'
        '{"type":"suite","event":"ok","passed":1,"failed":0,"ignored":0}\n',
        encoding="utf-8",
    )
    with pytest.raises(SystemExit, match="lacks passing"):
        MODULE.build_summary(record, nextest, runner)


def test_build_summary_rejects_proof_name_substring(tmp_path: Path) -> None:
    record, nextest, runner = _fixture(tmp_path)
    spoof = f"not_{MODULE.PROOF_TEST}_other"
    nextest.write_text(
        '{"type":"suite","event":"started"}\n'
        + json.dumps({"type": "test", "event": "started", "name": spoof})
        + "\n"
        + json.dumps({"type": "test", "event": "ok", "name": spoof})
        + "\n"
        + '{"type":"suite","event":"ok","passed":1,"failed":0,"ignored":0}\n',
        encoding="utf-8",
    )
    with pytest.raises(SystemExit, match="lacks passing"):
        MODULE.build_summary(record, nextest, runner)


def test_sdk_inventory_rejects_omitted_negative_test(tmp_path: Path) -> None:
    record, nextest, runner = _fixture(tmp_path)
    inventory = tmp_path / "nextest-inventory.json"
    names = [MODULE.PROOF_TEST, "sdk_rejects_wrong_daemon_identity"]
    inventory.write_text(
        json.dumps(
            {
                "test-count": len(names),
                "rust-suites": {
                    "quanta-index-retrieval-bench::sdk_roundtrip": {
                        "package-name": "quanta-index-retrieval-bench",
                        "binary-name": "sdk_roundtrip",
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
    meta = {
        "crate": "quanta-index-retrieval-bench",
        "test_binary": "sdk_roundtrip",
        "kind": "test",
    }

    def events(selected: list[str]) -> str:
        rows = [{"type": "suite", "event": "started", "test_count": len(selected), "nextest": meta}]
        for name in selected:
            identity = f"quanta-index-retrieval-bench::sdk_roundtrip${name}"
            rows.extend(
                [
                    {"type": "test", "event": "started", "name": identity},
                    {"type": "test", "event": "ok", "name": identity},
                ]
            )
        rows.append(
            {
                "type": "suite",
                "event": "ok",
                "passed": len(selected),
                "failed": 0,
                "ignored": 0,
                "nextest": meta,
            }
        )
        return "".join(json.dumps(row) + "\n" for row in rows)

    nextest.write_text(events(names[:1]), encoding="utf-8")
    with pytest.raises(SystemExit, match="differs from collected"):
        MODULE.build_summary(record, nextest, runner, inventory)
    nextest.write_text(events(names), encoding="utf-8")
    assert MODULE.build_summary(record, nextest, runner, inventory)["passed"] == 2
