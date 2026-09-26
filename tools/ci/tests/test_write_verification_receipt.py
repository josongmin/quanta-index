"""Contract tests for versioned nextest execution receipts."""

from __future__ import annotations

import hashlib
import importlib.util
import json
import os
import subprocess
import sys
from pathlib import Path

import jsonschema
import pytest

from tools.ci import source_closure

REPO_ROOT = Path(__file__).resolve().parents[3]
WRITER = REPO_ROOT / "tools" / "ci" / "write-verification-receipt.py"
SCHEMA = REPO_ROOT / "tools" / "ci" / "verification-receipt.schema.json"


def _writer_module():
    sys.path.insert(0, str(WRITER.parent))
    spec = importlib.util.spec_from_file_location("write_verification_receipt", WRITER)
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def _writer_env(**overrides: str) -> dict[str, str]:
    env = os.environ.copy()
    env.pop("GITHUB_SHA", None)
    env.update(overrides)
    return env


def _clean_repo(tmp_path: Path) -> Path:
    repo = tmp_path / "source"
    repo.mkdir()
    (repo / "tracked.txt").write_text("source\n", encoding="utf-8")
    subprocess.run(["git", "init", "--quiet"], cwd=repo, check=True)
    subprocess.run(
        ["git", "config", "user.email", "receipt-test@example.invalid"], cwd=repo, check=True
    )
    subprocess.run(["git", "config", "user.name", "Receipt Test"], cwd=repo, check=True)
    subprocess.run(["git", "add", "tracked.txt"], cwd=repo, check=True)
    subprocess.run(["git", "commit", "--quiet", "-m", "source"], cwd=repo, check=True)
    return repo


def _one_test_nextest(path: Path) -> Path:
    path.write_text(
        '{"type":"suite","event":"started","test_count":1}\n'
        '{"type":"test","event":"started","name":"first"}\n'
        '{"type":"test","event":"ok","name":"first"}\n'
        '{"type":"suite","event":"ok","passed":1,"failed":0,"ignored":0}\n',
        encoding="utf-8",
    )
    return path


def test_receipt_binds_revision_evidence_digest_and_test_count(tmp_path: Path) -> None:
    source = _clean_repo(tmp_path)
    evidence = tmp_path / "nextest.jsonl"
    evidence.write_text(
        '{"type":"suite","event":"started","test_count":2,"nextest":{"crate":"demo","test_binary":"demo","kind":"lib"}}\n'
        '{"type":"test","event":"started","name":"demo::demo$first"}\n'
        '{"type":"test","event":"ok","name":"demo::demo$first"}\n'
        '{"type":"test","event":"started","name":"demo::demo$second"}\n'
        '{"type":"test","event":"ok","name":"demo::demo$second"}\n'
        '{"type":"suite","event":"ok","passed":2,"failed":0,"ignored":0,"nextest":{"crate":"demo","test_binary":"demo","kind":"lib"}}\n',
        encoding="utf-8",
    )
    inventory = tmp_path / "inventory.json"
    inventory.write_text(
        json.dumps(
            {
                "test-count": 2,
                "rust-suites": {
                    "demo": {
                        "package-name": "demo",
                        "binary-name": "demo",
                        "kind": "lib",
                        "status": "listed",
                        "testcases": {
                            name: {"filter-match": {"status": "matches"}, "ignored": False}
                            for name in ("first", "second")
                        },
                    }
                },
            }
        ),
        encoding="utf-8",
    )
    output = tmp_path / "receipt.json"
    subprocess.run(
        [
            sys.executable,
            str(WRITER),
            "--rail",
            "pr-workspace-nextest",
            "--tier",
            "pr",
            "--command",
            "./scripts/cargow nextest run --workspace --all-features --locked",
            "--evidence",
            str(evidence),
            "--inventory",
            str(inventory),
            "--out",
            str(output),
        ],
        check=True,
        cwd=source,
        env=_writer_env(),
    )
    receipt = json.loads(output.read_text(encoding="utf-8"))
    schema = json.loads(SCHEMA.read_text(encoding="utf-8"))
    jsonschema.validate(receipt, schema)
    assert receipt["test_event_count"] == 2
    assert receipt["evidence_sha256"] == hashlib.sha256(evidence.read_bytes()).hexdigest()
    assert receipt["inventory_sha256"] == hashlib.sha256(inventory.read_bytes()).hexdigest()


@pytest.mark.parametrize("matching", [False, True])
def test_receipt_binds_github_sha_to_checked_out_head(tmp_path: Path, matching: bool) -> None:
    source = _clean_repo(tmp_path)
    head = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=source, text=True).strip()
    evidence = _one_test_nextest(tmp_path / "nextest.jsonl")
    output = tmp_path / "receipt.json"
    result = subprocess.run(
        [
            sys.executable,
            str(WRITER),
            "--rail",
            "proof",
            "--tier",
            "correctness",
            "--command",
            "proof",
            "--evidence",
            str(evidence),
            "--out",
            str(output),
        ],
        cwd=source,
        env=_writer_env(GITHUB_SHA=head if matching else "0" * 40),
        capture_output=True,
        text=True,
    )
    if matching:
        assert result.returncode == 0, result.stderr
        assert json.loads(output.read_text(encoding="utf-8"))["revision"] == head
    else:
        assert result.returncode != 0
        assert "GITHUB_SHA differs from checked-out HEAD" in result.stderr
        assert not output.exists()


@pytest.mark.parametrize(
    ("events", "error"),
    [
        (
            '{"type":"suite","event":"started"}\n'
            '{"type":"test","event":"ignored","name":"only"}\n'
            '{"type":"suite","event":"ok","passed":0,"failed":0,"ignored":1}\n',
            "no passing tests",
        ),
        (
            '{"type":"suite","event":"started"}\n'
            '{"type":"test","event":"ok","name":"first"}\n'
            '{"type":"test","event":"failed","name":"second"}\n',
            "failed or timed-out tests",
        ),
        (
            '{"type":"suite","event":"started"}\n'
            '{"type":"test","event":"ok","name":"first"}\n'
            '{"type":"test","event":"timeout","name":"second"}\n',
            "failed or timed-out tests",
        ),
        (
            '{"type":"suite","event":"started"}\n{"type":"test","event":"ok","name":"first"}\n',
            "incomplete suite events",
        ),
        (
            '{"type":"suite","event":"started"}\n'
            '{"type":"test","event":"ok","name":"first"}\n'
            '{"type":"suite","event":"failed","passed":1,"failed":1}\n',
            "nextest suite failed",
        ),
        (
            '{"type":"suite","event":"started"}\n'
            '{"type":"test","event":"ok","name":"first"}\n'
            '{"type":"suite","event":"ok","passed":2,"failed":0,"ignored":0}\n',
            "suite/test pass counts disagree",
        ),
        (
            '{"type":"suite","event":"started"}\n'
            '{"type":"test","event":["ok"],"name":"first"}\n'
            '{"type":"suite","event":"ok","passed":1,"failed":0}\n',
            "unknown nextest test outcome",
        ),
        (
            '{"type":"suite","event":"started"}\n'
            '{"type":"notice","event":"error"}\n'
            '{"type":"test","event":"ok","name":"first"}\n'
            '{"type":"suite","event":"ok","passed":1,"failed":0}\n',
            "unknown nextest event type",
        ),
    ],
)
def test_receipt_rejects_non_green_evidence(tmp_path: Path, events: str, error: str) -> None:
    source = _clean_repo(tmp_path)
    evidence = tmp_path / "nextest.jsonl"
    evidence.write_text(events, encoding="utf-8")
    output = tmp_path / "receipt.json"
    result = subprocess.run(
        [
            sys.executable,
            str(WRITER),
            "--rail",
            "proof-nextest",
            "--tier",
            "pr",
            "--command",
            "./scripts/cargow nextest run --workspace --all-features --locked",
            "--evidence",
            str(evidence),
            "--out",
            str(output),
        ],
        cwd=source,
        env=_writer_env(),
        capture_output=True,
        text=True,
    )
    assert result.returncode != 0
    assert error in result.stderr
    assert not output.exists()


def test_summary_parser_binds_command_and_complete_counts(tmp_path: Path) -> None:
    evidence = tmp_path / "summary.json"
    evidence.write_text(
        json.dumps(
            {
                "command": "just retrieval-sdk-proof",
                "selected": 8,
                "executed": 8,
                "passed": 8,
                "failed": 0,
                "separate_process": True,
            },
            sort_keys=True,
        )
        + "\n",
        encoding="utf-8",
    )
    digest, count = _writer_module()._summary_json_evidence_summary(
        evidence, "just retrieval-sdk-proof"
    )
    assert count == 8
    assert digest == hashlib.sha256(evidence.read_bytes()).hexdigest()


def test_summary_parser_rejects_wrong_command_and_partial_execution(tmp_path: Path) -> None:
    evidence = tmp_path / "summary.json"
    evidence.write_text(
        json.dumps({"command": "wrong", "selected": 100, "executed": 1, "passed": 1, "failed": 0})
    )
    module = _writer_module()
    with pytest.raises(SystemExit, match="command differs"):
        module._summary_json_evidence_summary(evidence, "expected")
    with pytest.raises(SystemExit, match="execution differs from selection"):
        module._summary_json_evidence_summary(evidence, "wrong")


@pytest.mark.parametrize(
    ("raw", "error"),
    [
        (
            '{"command":"proof","selected":1,"selected":2,"executed":1,"passed":1,"failed":0}',
            "duplicate summary JSON key",
        ),
        (
            '{"command":"proof","selected":1,"executed":1,"passed":1,"failed":0,"extra":NaN}',
            "non-finite summary JSON value",
        ),
    ],
)
def test_summary_parser_rejects_ambiguous_json(tmp_path: Path, raw: str, error: str) -> None:
    evidence = tmp_path / "summary.json"
    evidence.write_text(raw, encoding="utf-8")
    with pytest.raises(SystemExit, match=error):
        _writer_module()._summary_json_evidence_summary(evidence, "proof")


def test_standalone_summary_cannot_issue_v1_receipt(tmp_path: Path) -> None:
    source = _clean_repo(tmp_path)
    evidence = tmp_path / "summary.json"
    evidence.write_text(
        json.dumps({"command": "proof", "selected": 1, "executed": 1, "passed": 1, "failed": 0})
    )
    output = tmp_path / "receipt.json"
    result = subprocess.run(
        [
            sys.executable,
            str(WRITER),
            "--rail",
            "proof",
            "--tier",
            "correctness",
            "--command",
            "proof",
            "--evidence-format",
            "summary-json",
            "--evidence",
            str(evidence),
            "--out",
            str(output),
        ],
        cwd=source,
        env=_writer_env(),
        capture_output=True,
        text=True,
    )
    assert result.returncode != 0
    assert "requires source closure" in result.stderr
    assert not output.exists()


def test_workspace_receipt_refuses_partial_nextest_execution(tmp_path: Path) -> None:
    source = _clean_repo(tmp_path)
    evidence = tmp_path / "nextest.jsonl"
    identity = '"nextest":{"crate":"demo","test_binary":"demo","kind":"lib"}'
    evidence.write_text(
        f'{{"type":"suite","event":"started","test_count":1,{identity}}}\n'
        '{"type":"test","event":"started","name":"demo::demo$first"}\n'
        '{"type":"test","event":"ok","name":"demo::demo$first"}\n'
        f'{{"type":"suite","event":"ok","passed":1,"failed":0,"ignored":0,{identity}}}\n'
    )
    inventory = tmp_path / "inventory.json"
    inventory.write_text(
        json.dumps(
            {
                "test-count": 2,
                "rust-suites": {
                    "demo": {
                        "package-name": "demo",
                        "binary-name": "demo",
                        "kind": "lib",
                        "status": "listed",
                        "testcases": {
                            name: {"filter-match": {"status": "matches"}, "ignored": False}
                            for name in ("first", "second")
                        },
                    }
                },
            }
        )
    )
    output = tmp_path / "receipt.json"
    result = subprocess.run(
        [
            sys.executable,
            str(WRITER),
            "--rail",
            "pr-workspace-nextest",
            "--tier",
            "pr",
            "--command",
            "./scripts/cargow nextest run --workspace --all-features --locked",
            "--evidence",
            str(evidence),
            "--inventory",
            str(inventory),
            "--out",
            str(output),
        ],
        cwd=source,
        env=_writer_env(),
        capture_output=True,
        text=True,
    )
    assert result.returncode != 0
    assert "execution differs from collected tests" in result.stderr
    assert not output.exists()


@pytest.mark.parametrize(
    ("payload", "error"),
    [
        ({"command": "proof", "selected": 1, "executed": 1, "passed": 1}, "missing required"),
        (
            {"command": "proof", "selected": 1, "executed": 1, "passed": 0, "failed": 1},
            "reports failures",
        ),
        (
            {"command": "proof", "selected": 2, "executed": 2, "passed": 1, "failed": 0},
            "inconsistent execution counts",
        ),
        (
            {"command": "proof", "selected": 1, "executed": 2, "passed": 2, "failed": 0},
            "execution differs from selection",
        ),
        (
            {"command": "proof", "selected": True, "executed": 1, "passed": 1, "failed": 0},
            "invalid selected count",
        ),
    ],
)
def test_receipt_rejects_invalid_summary_json(
    tmp_path: Path, payload: dict[str, object], error: str
) -> None:
    evidence = tmp_path / "summary.json"
    evidence.write_text(json.dumps(payload) + "\n", encoding="utf-8")
    with pytest.raises(SystemExit, match=error):
        _writer_module()._summary_json_evidence_summary(evidence, "proof")


def test_receipt_rejects_dirty_source_before_emitting(tmp_path: Path) -> None:
    source = _clean_repo(tmp_path)
    (source / "tracked.txt").write_text("changed\n", encoding="utf-8")
    evidence = _one_test_nextest(tmp_path / "nextest.jsonl")
    output = tmp_path / "receipt.json"
    result = subprocess.run(
        [
            sys.executable,
            str(WRITER),
            "--rail",
            "proof",
            "--tier",
            "correctness",
            "--command",
            "proof",
            "--evidence",
            str(evidence),
            "--out",
            str(output),
        ],
        cwd=source,
        env=_writer_env(),
        capture_output=True,
        text=True,
    )
    assert result.returncode != 0
    assert "dirty source" in result.stderr
    assert not output.exists()


def test_source_closure_allows_unrelated_dirty_but_rejects_relevant_drift(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    source = _clean_repo(tmp_path)
    monkeypatch.setitem(
        source_closure.PROFILES,
        "fixture",
        {"cargo_packages": (), "paths": ("tracked.txt",)},
    )
    manifest = source_closure.build_manifest(source, "fixture")

    (source / "unrelated.txt").write_text("dirty but out of closure\n", encoding="utf-8")
    assert source_closure.verify_manifest(source, manifest) == manifest

    (source / "tracked.txt").write_text("changed\n", encoding="utf-8")
    with pytest.raises(source_closure.ClosureError, match="dirty relevant source"):
        source_closure.verify_manifest(source, manifest)


def test_source_closure_rejects_manifest_tampering(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    source = _clean_repo(tmp_path)
    monkeypatch.setitem(
        source_closure.PROFILES,
        "fixture",
        {"cargo_packages": (), "paths": ("tracked.txt",)},
    )
    manifest = source_closure.build_manifest(source, "fixture")
    manifest["files"][0]["sha256"] = "0" * 64
    with pytest.raises(source_closure.ClosureError, match="digest mismatch"):
        source_closure.verify_manifest(source, manifest)


def test_retrieval_source_closure_includes_transitive_execution_owners() -> None:
    paths = set(source_closure.PROFILES["retrieval"]["paths"])
    assert {
        "scripts/cargow",
        "scripts/quanta-index-env.sh",
        "rust-toolchain.toml",
        "tools/ci/timing/rust_profile_history.py",
    } <= paths


def test_retrieval_source_closure_binds_both_ticket_contract_packets() -> None:
    # The active remediation packet is a source-bound contract exactly like
    # the original sep-23 benchmark packet: both must stay inside the
    # retrieval closure so contract edits invalidate existing receipts.
    paths = set(source_closure.PROFILES["retrieval"]["paths"])
    assert {
        "docs/plans/sep-23-retrieval-bench",
        "docs/plans/sep-26-retrieval-remediation",
    } <= paths


def test_source_closure_rejects_remediation_contract_drift(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    # Additions, edits, deletions, and committed changes under the SEP-26
    # remediation packet must each invalidate a previously captured closure
    # manifest before any receipt bound to it is trusted.
    source = _clean_repo(tmp_path)
    tickets = source / "docs/plans/sep-26-retrieval-remediation/tickets"
    tickets.mkdir(parents=True)
    ticket = tickets / "INDEX.md"
    ticket.write_text("# ticket contract\n", encoding="utf-8")
    subprocess.run(["git", "add", "docs"], cwd=source, check=True)
    subprocess.run(["git", "commit", "--quiet", "-m", "tickets"], cwd=source, check=True)
    monkeypatch.setitem(
        source_closure.PROFILES,
        "fixture",
        {"cargo_packages": (), "paths": ("docs/plans/sep-26-retrieval-remediation",)},
    )
    manifest = source_closure.build_manifest(source, "fixture")
    assert [entry["path"] for entry in manifest["files"]] == [
        "docs/plans/sep-26-retrieval-remediation/tickets/INDEX.md"
    ]

    # Uncommitted modification of a bound contract document.
    ticket.write_text("# ticket contract changed\n", encoding="utf-8")
    with pytest.raises(source_closure.ClosureError, match="dirty relevant source"):
        source_closure.verify_manifest(source, manifest)
    subprocess.run(["git", "checkout", "--", "."], cwd=source, check=True)

    # Untracked addition inside the contract directory.
    (tickets / "RBR-13-new.md").write_text("# untracked addition\n", encoding="utf-8")
    with pytest.raises(source_closure.ClosureError, match="dirty relevant source"):
        source_closure.verify_manifest(source, manifest)
    (tickets / "RBR-13-new.md").unlink()

    # Uncommitted deletion of a bound contract document.
    ticket.unlink()
    with pytest.raises(source_closure.ClosureError, match="dirty relevant source"):
        source_closure.verify_manifest(source, manifest)
    subprocess.run(["git", "checkout", "--", "."], cwd=source, check=True)

    # Committed contract change moves HEAD: the manifest revision binding
    # must reject it even though the working tree is clean again.
    ticket.write_text("# ticket contract changed\n", encoding="utf-8")
    subprocess.run(["git", "add", "docs"], cwd=source, check=True)
    subprocess.run(["git", "commit", "--quiet", "-m", "tickets changed"], cwd=source, check=True)
    with pytest.raises(source_closure.ClosureError, match="revision changed"):
        source_closure.verify_manifest(source, manifest)


def test_receipt_refuses_overwriting_existing_output(tmp_path: Path) -> None:
    source = _clean_repo(tmp_path)
    evidence = _one_test_nextest(tmp_path / "nextest.jsonl")
    output = tmp_path / "receipt.json"
    output.write_text("keep\n", encoding="utf-8")
    result = subprocess.run(
        [
            sys.executable,
            str(WRITER),
            "--rail",
            "proof",
            "--tier",
            "correctness",
            "--command",
            "proof",
            "--evidence",
            str(evidence),
            "--out",
            str(output),
        ],
        cwd=source,
        env=_writer_env(),
        capture_output=True,
        text=True,
    )
    assert result.returncode != 0
    assert "refusing existing verification receipt" in result.stderr
    assert output.read_text(encoding="utf-8") == "keep\n"
