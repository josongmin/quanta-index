"""Contract tests for versioned nextest execution receipts."""

from __future__ import annotations

import hashlib
import json
import subprocess
import sys
from pathlib import Path

import jsonschema
import pytest

REPO_ROOT = Path(__file__).resolve().parents[3]
WRITER = REPO_ROOT / "tools" / "ci" / "write-verification-receipt.py"
SCHEMA = REPO_ROOT / "tools" / "ci" / "verification-receipt.schema.json"


def test_receipt_binds_revision_evidence_digest_and_test_count(tmp_path: Path) -> None:
    evidence = tmp_path / "nextest.jsonl"
    evidence.write_text(
        '{"type":"suite","event":"started"}\n'
        '{"type":"test","event":"ok","name":"first"}\n'
        '{"type":"suite","event":"ok","passed":1,"failed":0,"ignored":0}\n'
        '{"type":"suite","event":"started"}\n'
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
        cwd=REPO_ROOT,
    )
    receipt = json.loads(output.read_text(encoding="utf-8"))
    schema = json.loads(SCHEMA.read_text(encoding="utf-8"))
    jsonschema.validate(receipt, schema)
    assert receipt["test_event_count"] == 3
    assert receipt["evidence_sha256"] == hashlib.sha256(evidence.read_bytes()).hexdigest()


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
            '{"type":"suite","event":"ok","passed":2,"failed":0}\n',
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
        cwd=REPO_ROOT,
        capture_output=True,
        text=True,
    )
    assert result.returncode != 0
    assert error in result.stderr
    assert not output.exists()
