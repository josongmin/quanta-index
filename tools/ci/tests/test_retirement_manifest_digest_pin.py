"""The roster reports the manifest bytes that passed reuse validation."""

from __future__ import annotations

import hashlib
import json
from types import SimpleNamespace

import pytest

from tools.ci import retirement_nextest


@pytest.fixture
def reusable_manifest(tmp_path, monkeypatch):
    checkout = tmp_path / "source"
    checkout.mkdir()
    evidence = tmp_path / "evidence"
    evidence.mkdir()
    run_id = "11111111-1111-1111-1111-111111111111"
    event = "quanta-index-lexical::replacement$case"
    selection = {
        "package": "quanta-index-lexical",
        "test_targets": ["replacement"],
        "expected_events": [event],
        "lane": "proof-lane",
    }
    inventory = evidence / f"{run_id}-inventory.json"
    inventory.write_bytes(b"{}")
    inventory_digest = hashlib.sha256(inventory.read_bytes()).hexdigest()
    terminal_digest = "sha256:" + "f" * 64
    manifest = evidence / f"{run_id}-source-manifest.json"
    manifest.write_text(
        json.dumps(
            {
                "source_tree_sha256": "frozen-tree",
                "execution": {"selection": selection},
                "inventory_sha256": inventory_digest,
                "events_sha256": terminal_digest,
            }
        ),
        encoding="utf-8",
    )
    monkeypatch.setattr(retirement_nextest, "_require_frozen_checkout", lambda *_: None)
    monkeypatch.setattr(retirement_nextest, "release_tree_sha256", lambda *_: "frozen-tree")
    monkeypatch.setattr(
        retirement_nextest,
        "release_capture_batches",
        lambda *_: [{key: value for key, value in selection.items() if key != "lane"}],
    )
    monkeypatch.setattr(retirement_nextest, "retained_executable_digests", lambda: lambda *_: "")
    monkeypatch.setattr(retirement_nextest, "parse_nextest_inventory", lambda *_: None)
    monkeypatch.setattr(
        retirement_nextest,
        "parse_nextest",
        lambda *_: SimpleNamespace(sha256=terminal_digest, passed_names={event}),
    )
    return checkout, evidence, manifest


@pytest.mark.parametrize("explicit_digest_sink", [False, True])
def test_roster_rejects_reused_manifest_replaced_after_validation(
    reusable_manifest, monkeypatch, explicit_digest_sink
):
    checkout, evidence, manifest = reusable_manifest

    def replace_after_validation(*_args, **_kwargs):
        manifest.write_bytes(b"replaced after validation")

    monkeypatch.setattr(retirement_nextest, "check_frozen_inputs", replace_after_validation)
    options = {"_manifest_digests": {}} if explicit_digest_sink else {}
    with pytest.raises(ValueError, match="manifest changed after validated capture"):
        list(
            retirement_nextest.capture_release_roster(
                checkout, "a" * 40, evidence, lane="proof-lane", **options
            )
        )


def test_roster_retains_original_validated_digest_after_yield(reusable_manifest, monkeypatch):
    checkout, evidence, manifest = reusable_manifest
    monkeypatch.setattr(retirement_nextest, "check_frozen_inputs", lambda *_args, **_kwargs: None)
    original = retirement_nextest.RawFile.capture(manifest).sha256
    digests = {}
    capture = retirement_nextest.capture_release_roster(
        checkout, "a" * 40, evidence, lane="proof-lane", _manifest_digests=digests
    )
    assert next(capture) == manifest
    manifest.write_bytes(b"replaced after yield")
    assert digests[manifest] == original
    assert retirement_nextest.RawFile.capture(manifest).sha256 != original
    capture.close()
