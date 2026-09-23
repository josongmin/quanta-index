"""Retrieval consumers refuse the legacy CI verification receipt."""

from __future__ import annotations

import json
from pathlib import Path

import jsonschema
import pytest

from tools.benchmark.retrieval import run as retrieval_run


def test_retrieval_refuses_legacy_verification_receipt() -> None:
    schema = json.loads(
        Path("tools/ci/verification-receipt.schema.json").read_text(encoding="utf-8")
    )
    legacy = {
        "schema_version": 1,
        "revision": "abcdef123456",
        "rail": "pr-workspace-nextest",
        "tier": "pr",
        "command": "./scripts/cargow nextest run --workspace",
        "evidence_path": "nextest.jsonl",
        "evidence_sha256": "0" * 64,
        "test_event_count": 1,
    }
    jsonschema.validate(legacy, schema)
    with pytest.raises(retrieval_run.RunError):
        retrieval_run._validate_receipt_shape(legacy, "retrieval receipt")
