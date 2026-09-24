"""Contract tests for versioned nextest execution receipts."""

from __future__ import annotations

import hashlib
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


def test_receipt_binds_revision_evidence_digest_and_test_count(tmp_path: Path) -> None:
    source = _clean_repo(tmp_path)
    evidence = tmp_path / "nextest.jsonl"
    evidence.write_text(
        '{"type":"suite","event":"started"}\n'
        '{"type":"test","event":"started","name":"first"}\n'
        '{"type":"test","event":"ok","name":"first"}\n'
        '{"type":"suite","event":"ok","passed":1,"failed":0,"ignored":0}\n'
        '{"type":"suite","event":"started"}\n'
        '{"type":"test","event":"started","name":"second"}\n'
        '{"type":"test","event":"ok","name":"second"}\n'
        '{"type":"test","event":"ignored","name":"third"}\n'
        '{"type":"suite","event":"ok","passed":1,"failed":0,"ignored":1}\n',
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
    assert receipt["test_event_count"] == 3
    assert receipt["evidence_sha256"] == hashlib.sha256(evidence.read_bytes()).hexdigest()


@pytest.mark.parametrize("matching", [False, True])
def test_receipt_binds_github_sha_to_checked_out_head(tmp_path: Path, matching: bool) -> None:
    source = _clean_repo(tmp_path)
    head = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=source, text=True).strip()
    evidence = tmp_path / "summary.json"
    evidence.write_text(
        json.dumps({"command": "proof", "selected": 1, "executed": 1, "passed": 1, "failed": 0}),
        encoding="utf-8",
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
            "pr-workspace-nextest",
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


def test_receipt_binds_valid_summary_json(tmp_path: Path) -> None:
    source = _clean_repo(tmp_path)
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
    output = tmp_path / "receipt.json"
    subprocess.run(
        [
            sys.executable,
            str(WRITER),
            "--rail",
            "retrieval-sdk-proof",
            "--tier",
            "correctness",
            "--command",
            "just retrieval-sdk-proof",
            "--evidence-format",
            "summary-json",
            "--evidence",
            str(evidence),
            "--out",
            str(output),
        ],
        check=True,
        cwd=source,
        env=_writer_env(),
    )
    receipt = json.loads(output.read_text(encoding="utf-8"))
    assert receipt["test_event_count"] == 8
    assert receipt["evidence_sha256"] == hashlib.sha256(evidence.read_bytes()).hexdigest()


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
            "executed more tests than selected",
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
    source = _clean_repo(tmp_path)
    evidence = tmp_path / "summary.json"
    evidence.write_text(json.dumps(payload) + "\n", encoding="utf-8")
    output = tmp_path / "receipt.json"
    result = subprocess.run(
        [
            sys.executable,
            str(WRITER),
            "--rail",
            "retrieval-sdk-proof",
            "--tier",
            "correctness",
            "--command",
            "just retrieval-sdk-proof",
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
    assert error in result.stderr
    assert not output.exists()


def test_receipt_rejects_dirty_source_before_emitting(tmp_path: Path) -> None:
    source = _clean_repo(tmp_path)
    (source / "tracked.txt").write_text("changed\n", encoding="utf-8")
    evidence = tmp_path / "summary.json"
    evidence.write_text(
        json.dumps({"command": "proof", "selected": 1, "executed": 1, "passed": 1, "failed": 0})
        + "\n",
        encoding="utf-8",
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


def test_receipt_refuses_overwriting_existing_output(tmp_path: Path) -> None:
    source = _clean_repo(tmp_path)
    evidence = tmp_path / "summary.json"
    evidence.write_text(
        json.dumps({"command": "proof", "selected": 1, "executed": 1, "passed": 1, "failed": 0})
        + "\n",
        encoding="utf-8",
    )
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
    assert "refusing existing verification receipt" in result.stderr
    assert output.read_text(encoding="utf-8") == "keep\n"
