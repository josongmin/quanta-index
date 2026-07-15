"""Contract tests for versioned nextest execution receipts."""

from __future__ import annotations

import json
import subprocess
import sys
from pathlib import Path

import jsonschema

REPO_ROOT = Path(__file__).resolve().parents[3]
WRITER = REPO_ROOT / "tools" / "ci" / "write-verification-receipt.py"
SCHEMA = REPO_ROOT / "tools" / "ci" / "verification-receipt.schema.json"


def test_receipt_binds_revision_evidence_digest_and_test_count(tmp_path: Path) -> None:
    evidence = tmp_path / "nextest.jsonl"
    evidence.write_text(
        '{"type":"suite","event":"started"}\n'
        '{"type":"test","event":"ok","name":"first"}\n'
        '{"type":"test","event":"ok","name":"second"}\n',
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
    assert receipt["test_event_count"] == 2
    assert receipt["evidence_sha256"]
