"""Owner-local tests for the SEP-21 lane handoff semantic validator."""

from __future__ import annotations

import copy
import hashlib
import importlib.util
import json
import shutil
import subprocess
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[3]
HANDOFF_CHECKER_PATH = REPO_ROOT / "tools/ci/lint/check-lane-handoff.py"
PROOF_CHECKER_PATH = REPO_ROOT / "tools/ci/lint/check-proof-authority.py"


def _load(name: str, path: Path):
    spec = importlib.util.spec_from_file_location(name, path)
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


HANDOFF = _load("check_lane_handoff", HANDOFF_CHECKER_PATH)
PROOF = _load("check_lane_handoff_proof", PROOF_CHECKER_PATH)


def _run(root: Path, *args: str) -> str:
    return subprocess.run(
        [*args], cwd=root, check=True, capture_output=True, text=True
    ).stdout.strip()


def _fixture(tmp_path: Path) -> tuple[Path, dict, Path]:
    root = tmp_path / "repo"
    root.mkdir()
    for relative in (
        "Justfile",
        "tools/ci/lint/check-proof-authority.py",
        "tools/ci/lint/handoff_validation.py",
        "tools/ci/proof-authority.toml",
        "tools/ci/proof-manifest.schema.json",
        "tools/ci/proof-aggregate.schema.json",
        "tools/ci/test-authority.toml",
        "docs/plans/sep-21-search-plane-sota-hardening/tickets/handoffs/lane-handoff.schema.json",
    ):
        destination = root / relative
        destination.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(REPO_ROOT / relative, destination)

    registry = PROOF._read_toml(root / "tools/ci/proof-authority.toml")
    for proof in registry["proofs"]:
        owner = root / proof["owner"]
        owner.parent.mkdir(parents=True, exist_ok=True)
        owner.touch(exist_ok=True)

    _run(root, "git", "init", "-q")
    _run(root, "git", "config", "user.name", "Fixture")
    _run(root, "git", "config", "user.email", "fixture@example.invalid")
    (root / "tracked").write_text("base\n", encoding="utf-8")
    _run(root, "git", "add", ".")
    _run(root, "git", "commit", "-qm", "base")
    base_sha = _run(root, "git", "rev-parse", "HEAD")
    (root / "tracked").write_text("result\n", encoding="utf-8")
    _run(root, "git", "add", "tracked")
    _run(root, "git", "commit", "-qm", "result")
    result_sha = _run(root, "git", "rev-parse", "HEAD")

    evidence = root / "artifacts/raw/p00.log"
    evidence.parent.mkdir(parents=True)
    evidence.write_text("passed\n", encoding="utf-8")
    evidence_digest = hashlib.sha256(evidence.read_bytes()).hexdigest()
    evidence_archive = root / PROOF.content_archive_relative_path("evidence", evidence_digest)
    evidence_archive.parent.mkdir(parents=True, exist_ok=True)
    evidence_archive.write_bytes(evidence.read_bytes())
    proof = next(item for item in registry["proofs"] if item["id"] == "p00-authority-freeze")
    source = PROOF.proof_source_snapshot(
        root,
        manifest_path=root / proof["artifact"],
        proof=proof,
        excluded_paths=[evidence],
    )
    dirty = source["dirty_digest"].removeprefix("sha256:")
    manifest = {
        "schema_version": 1,
        "proof_id": "p00-authority-freeze",
        "family": "S",
        "status": "passed",
        "source": source,
        "source_pair": None,
        "invocation": {
            "command": "just proof-p00-authority-freeze",
            "profile": "none",
            "target": "proof-authority",
            "filter": "none",
        },
        "counts": {"selected": 1, "executed": 1, "passed": 1, "failed": 0, "ignored": 0},
        "environment": {
            "toolchain": "python fixture",
            "features": [],
            "os": "linux",
            "arch": "x86_64",
            "host": {
                "profile": "fixture",
                "cpu_count": 1,
                "memory_bytes": 1,
                "identity_digest": "sha256:" + "2" * 64,
            },
        },
        "daemon_binary": None,
        "state_root_format": "not-applicable",
        "inputs": {
            "fixture": None,
            "corpus": None,
            "config": None,
            "model": None,
            "provider": None,
        },
        "started_at": "2026-09-21T00:00:00Z",
        "ended_at": "2026-09-21T00:00:01Z",
        "dependency_receipts": [],
        "artifacts": [
            {
                "source_path": "artifacts/raw/p00.log",
                "path": evidence_archive.relative_to(root).as_posix(),
                "sha256": evidence_digest,
            }
        ],
    }
    manifest_bytes = (json.dumps(manifest, sort_keys=True, indent=2) + "\n").encode()
    manifest_digest = hashlib.sha256(manifest_bytes).hexdigest()
    manifest_relative = PROOF.proof_archive_relative_path(manifest, manifest_digest)
    manifest_path = root / manifest_relative
    manifest_path.parent.mkdir(parents=True)
    manifest_path.write_bytes(manifest_bytes)
    handoff = {
        "schema_version": 1,
        "lane": "P00",
        "ticket": "S21-00+S21-13A",
        "status": "OWNER_PROOF_GREEN",
        "base_sha": base_sha,
        "result_sha": result_sha,
        "dirty_digest": dirty,
        "write_set": ["tracked"],
        "exported_contracts": ["tracked@sha256:" + hashlib.sha256(b"result\n").hexdigest()],
        "proofs": [
            {
                "id": "p00-authority-freeze",
                "status": "RECORDED",
                "manifest": manifest_relative,
                "manifest_sha256": manifest_digest,
                "command": "just proof-p00-authority-freeze",
                "required_host": "any",
                "selected": 1,
                "executed": 1,
                "passed": 1,
                "failed": 0,
                "ignored": 0,
            }
        ],
        "not_run": [],
        "blockers": [],
    }
    return root, handoff, manifest_path


def test_valid_handoff_binds_git_result_and_immutable_manifest(tmp_path: Path) -> None:
    root, handoff, _ = _fixture(tmp_path)
    assert (
        HANDOFF.validate_handoff(
            handoff,
            handoff_path=root / "P00.json",
            root=root,
            require_result_head=True,
        )
        == []
    )


def test_leaf_handoff_validator_accepts_injected_proof_checker(tmp_path: Path) -> None:
    root, handoff, _ = _fixture(tmp_path)
    assert (
        HANDOFF.CHAIN_VALIDATOR.validate_handoff(
            handoff,
            handoff_path=root / "P00.json",
            root=root,
            proof_checker=PROOF,
            require_result_head=True,
        )
        == []
    )


def test_handoff_refuses_count_and_not_run_drift(tmp_path: Path) -> None:
    root, handoff, _ = _fixture(tmp_path)
    broken = copy.deepcopy(handoff)
    broken["proofs"][0]["selected"] = 2
    broken["not_run"] = ["unrelated prose"]
    errors = HANDOFF.validate_handoff(
        broken,
        handoff_path=root / "P00.json",
        root=root,
    )
    assert "proof p00-authority-freeze must satisfy selected == executed == passed" in errors
    assert "not_run must exactly equal NOT_RUN proof IDs in proof order" in errors


def test_handoff_schema_refuses_current_alias(tmp_path: Path) -> None:
    root, handoff, _ = _fixture(tmp_path)
    handoff["proofs"][0]["manifest"] = "artifacts/proof-authority/p00-authority-freeze.json"
    errors = HANDOFF.validate_handoff(
        handoff,
        handoff_path=root / "P00.json",
        root=root,
    )
    assert errors and any(error.startswith("schema proofs.0") for error in errors)


def test_handoff_refuses_archive_tampering(tmp_path: Path) -> None:
    root, handoff, manifest_path = _fixture(tmp_path)
    manifest_path.write_bytes(manifest_path.read_bytes() + b"tampered\n")
    errors = HANDOFF.validate_handoff(
        handoff,
        handoff_path=root / "P00.json",
        root=root,
    )
    assert any("manifest digest mismatch" in error for error in errors)


def test_handoff_refuses_lane_ticket_and_required_proof_relabel(tmp_path: Path) -> None:
    root, handoff, _ = _fixture(tmp_path)
    handoff["lane"] = "P01"
    handoff["ticket"] = "S21-01"

    errors = HANDOFF.validate_handoff(
        handoff,
        handoff_path=root / "P01.json",
        root=root,
    )

    assert any("proof IDs differ from P01 authority" in error for error in errors)


def test_handoff_refuses_false_write_set_and_empty_exports(tmp_path: Path) -> None:
    root, handoff, _ = _fixture(tmp_path)
    handoff["write_set"] = ["not-the-real-diff"]
    handoff["exported_contracts"] = []

    errors = HANDOFF.validate_handoff(
        handoff,
        handoff_path=root / "P00.json",
        root=root,
    )

    assert any("schema exported_contracts" in error for error in errors)

    handoff["exported_contracts"] = ["tracked@sha256:" + hashlib.sha256(b"result\n").hexdigest()]
    errors = HANDOFF.validate_handoff(
        handoff,
        handoff_path=root / "P00.json",
        root=root,
    )
    assert any("write_set differs from exact base..result Git delta" in error for error in errors)


def test_handoff_refuses_noop_result_commit(tmp_path: Path) -> None:
    root, handoff, _ = _fixture(tmp_path)
    handoff["base_sha"] = handoff["result_sha"]
    handoff["write_set"] = []
    errors = HANDOFF.validate_handoff(handoff, handoff_path=root / "P00.json", root=root)
    assert any("schema write_set" in error for error in errors)


def test_strict_handoff_refuses_current_source_drift(tmp_path: Path) -> None:
    root, handoff, _ = _fixture(tmp_path)
    (root / "tracked").write_text("dirty after handoff\n", encoding="utf-8")

    errors = HANDOFF.validate_handoff(
        handoff,
        handoff_path=root / "P00.json",
        root=root,
        require_result_head=True,
    )

    assert any("source.dirty_digest is not current working tree" in error for error in errors)


def test_handoff_refuses_noncanonical_registry(tmp_path: Path) -> None:
    root, handoff, _ = _fixture(tmp_path)
    registry = root / "tools/ci/proof-authority.toml"
    registry.write_text(
        registry.read_text(encoding="utf-8").replace(
            'dependencies = ["p00-authority-freeze"]',
            "dependencies = []",
            1,
        ),
        encoding="utf-8",
    )

    errors = HANDOFF.validate_handoff(
        handoff,
        handoff_path=root / "P00.json",
        root=root,
    )

    assert any("proof registry is invalid" in error for error in errors)


def test_product_chain_refuses_missing_historical_handoff(tmp_path: Path) -> None:
    root, handoff, _ = _fixture(tmp_path)
    directory = root / "handoffs"
    directory.mkdir()
    (directory / "P00.json").write_text(json.dumps(handoff), encoding="utf-8")

    errors = HANDOFF.validate_product_handoff_directory(directory=directory, root=root)

    assert any("P01.json: unreadable handoff" in error for error in errors)
    assert any("P11.json: unreadable handoff" in error for error in errors)
    assert not any("P00.json:" in error for error in errors)


def test_ledger_keeps_historical_product_and_infrastructure_separate(tmp_path: Path) -> None:
    root, handoff, _ = _fixture(tmp_path)
    directory = root / "artifacts/sep-21/handoffs"
    directory.mkdir(parents=True)
    (directory / "P00.json").write_text(json.dumps(handoff), encoding="utf-8")

    ledger, findings = HANDOFF.CHAIN_VALIDATOR.inspect_handoff_ledger(
        root=root, proof_checker=PROOF
    )

    assert ledger["product_handoffs"][0]["status"] == "VERIFIED"
    assert (
        ledger["product_handoffs"][0]["sha256"]
        == hashlib.sha256((directory / "P00.json").read_bytes()).hexdigest()
    )
    assert ledger["product_chain_status"] == "NOT_RUN"
    assert ledger["infrastructure_handoff"]["lane"] == "P12A"
    assert ledger["infrastructure_handoff"]["status"] == "NOT_RUN"
    assert any("P12A.json: handoff is missing" in finding for finding in findings)

    (directory / "P01.json").symlink_to(directory / "P00.json")
    ledger, findings = HANDOFF.CHAIN_VALIDATOR.inspect_handoff_ledger(
        root=root, proof_checker=PROOF
    )
    assert ledger["product_handoffs"][1]["status"] == "FAILED"
    assert ledger["product_chain_status"] == "FAILED"
    assert any("P01.json: handoff is not a regular non-symlink file" in item for item in findings)


def test_ledger_refuses_blocked_infrastructure_even_after_single_validation(
    tmp_path: Path, monkeypatch
) -> None:
    root, _, _ = _fixture(tmp_path)
    directory = root / "artifacts/sep-21/handoffs"
    directory.mkdir(parents=True)
    (directory / "P12A.json").write_text(
        json.dumps({"lane": "P12A", "status": "BLOCKED"}), encoding="utf-8"
    )
    monkeypatch.setattr(HANDOFF.CHAIN_VALIDATOR, "validate_handoff", lambda *_args, **_kw: [])

    ledger, findings = HANDOFF.CHAIN_VALIDATOR.inspect_handoff_ledger(
        root=root, proof_checker=PROOF
    )

    assert ledger["infrastructure_handoff"]["status"] == "FAILED"
    assert any("P12A.json: no recorded owner-proof handoff" in item for item in findings)


def test_ledger_classifies_malformed_proof_archive_as_failed(tmp_path: Path) -> None:
    root, handoff, manifest_path = _fixture(tmp_path)
    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    manifest["source"] = "forged source"
    manifest_path.write_text(json.dumps(manifest), encoding="utf-8")
    handoff["proofs"][0]["manifest_sha256"] = hashlib.sha256(manifest_path.read_bytes()).hexdigest()
    directory = root / "artifacts/sep-21/handoffs"
    directory.mkdir(parents=True)
    (directory / "P00.json").write_text(json.dumps(handoff), encoding="utf-8")

    ledger, findings = HANDOFF.CHAIN_VALIDATOR.inspect_handoff_ledger(
        root=root, proof_checker=PROOF
    )

    assert ledger["product_handoffs"][0]["status"] == "FAILED"
    assert ledger["product_chain_status"] == "FAILED"
    assert any("P00.json: unreadable or invalid handoff" in item for item in findings)
