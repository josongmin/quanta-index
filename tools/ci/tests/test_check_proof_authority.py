"""Contract tests for the SEP-21 proof authority and manifest validator."""

from __future__ import annotations

import copy
import hashlib
import importlib.util
import json
import subprocess
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[3]
SCRIPT_PATH = REPO_ROOT / "tools/ci/lint/check-proof-authority.py"
REGISTRY_PATH = REPO_ROOT / "tools/ci/proof-authority.toml"
SCHEMA_PATH = REPO_ROOT / "tools/ci/proof-manifest.schema.json"


def _load_module():
    spec = importlib.util.spec_from_file_location("check_proof_authority", SCRIPT_PATH)
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    sys.modules["check_proof_authority"] = module
    spec.loader.exec_module(module)
    return module


MODULE = _load_module()


def _sha(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def _proof(root: Path) -> dict:
    return {
        "id": "fixture-proof",
        "ticket": "S21-00",
        "family": "S",
        "checkpoint": "M0",
        "owner": "owner.md",
        "command": "python3 check.py",
        "gate": "pr",
        "profile": "none",
        "target": "fixture",
        "filter": "none",
        "source_binding": "exact",
        "binary_binding": "none",
        "artifact_schema": "tools/ci/proof-manifest.schema.json",
        "test_authority_targets": [],
        "dependencies": [],
        "artifact": "artifacts/proof.json",
        "required_host": "any",
    }


def _manifest(root: Path, proof: dict) -> dict:
    binary = root / "bin/searchd"
    evidence = root / "artifacts/raw.jsonl"
    binary.parent.mkdir(parents=True)
    evidence.parent.mkdir(parents=True)
    binary.write_bytes(b"release-daemon")
    evidence.write_bytes(b'{"terminal":"passed"}\n')
    digest = "sha256:" + "1" * 64
    return {
        "schema_version": 1,
        "proof_id": proof["id"],
        "family": proof["family"],
        "status": "passed",
        "source": {
            "head": "a" * 40,
            "dirty_digest": digest,
            "branch": "fixture",
            "upstream": "origin/main",
            "merge_base": "b" * 40,
        },
        "invocation": {
            "command": proof["command"],
            "profile": proof["profile"],
            "target": proof["target"],
            "filter": proof["filter"],
        },
        "counts": {"selected": 1, "executed": 1, "passed": 1, "failed": 0, "ignored": 0},
        "environment": {
            "toolchain": "rustc fixture",
            "features": [],
            "os": "linux",
            "arch": "x86_64",
            "host": {
                "profile": "fixture",
                "cpu_count": 1,
                "memory_bytes": 1,
                "identity_digest": digest,
            },
        },
        "daemon_binary": {"path": "bin/searchd", "sha256": _sha(binary)},
        "state_root_format": "fixture-v1",
        "inputs": {
            "fixture": digest,
            "corpus": None,
            "config": None,
            "model": None,
            "provider": None,
        },
        "started_at": "2026-09-21T00:00:00Z",
        "ended_at": "2026-09-21T00:00:01Z",
        "dependency_receipts": [],
        "artifacts": [{"path": "artifacts/raw.jsonl", "sha256": _sha(evidence)}],
    }


def _messages(findings) -> list[str]:
    return [finding.message for finding in findings]


def test_repository_registry_is_complete() -> None:
    registry = MODULE._read_toml(REGISTRY_PATH)
    assert MODULE.check_registry(registry, root=REPO_ROOT, path=REGISTRY_PATH) == []


def test_registry_refuses_a_missing_family() -> None:
    registry = MODULE._read_toml(REGISTRY_PATH)
    broken = copy.deepcopy(registry)
    del broken["families"]["X"]
    messages = _messages(MODULE.check_registry(broken, root=REPO_ROOT, path=REGISTRY_PATH))
    assert any("families must be exactly" in message for message in messages)


def test_valid_manifest_binds_files_and_counts(tmp_path: Path) -> None:
    (tmp_path / "owner.md").write_text("owner\n", encoding="utf-8")
    proof = _proof(tmp_path)
    payload = _manifest(tmp_path, proof)
    schema = json.loads(SCHEMA_PATH.read_text(encoding="utf-8"))
    assert (
        MODULE.check_manifest(
            payload,
            manifest_path=tmp_path / "proof.json",
            proof=proof,
            schema=schema,
            root=tmp_path,
            bind_source=False,
        )
        == []
    )


def test_passed_manifest_refuses_zero_execution_and_ignored_only(tmp_path: Path) -> None:
    proof = _proof(tmp_path)
    payload = _manifest(tmp_path, proof)
    payload["counts"] = {"selected": 2, "executed": 0, "passed": 0, "failed": 0, "ignored": 2}
    schema = json.loads(SCHEMA_PATH.read_text(encoding="utf-8"))
    messages = _messages(
        MODULE.check_manifest(
            payload,
            manifest_path=tmp_path / "proof.json",
            proof=proof,
            schema=schema,
            root=tmp_path,
            bind_source=False,
        )
    )
    assert any("requires selected, executed and passed > 0" in message for message in messages)
    assert any("cannot contain ignored tests" in message for message in messages)


def test_manifest_refuses_short_head(tmp_path: Path) -> None:
    proof = _proof(tmp_path)
    payload = _manifest(tmp_path, proof)
    payload["source"]["head"] = "abc1234"
    schema = json.loads(SCHEMA_PATH.read_text(encoding="utf-8"))
    messages = _messages(
        MODULE.check_manifest(
            payload,
            manifest_path=tmp_path / "proof.json",
            proof=proof,
            schema=schema,
            root=tmp_path,
            bind_source=False,
        )
    )
    assert any("does not match" in message for message in messages)


def test_manifest_refuses_timestamp_inversion(tmp_path: Path) -> None:
    proof = _proof(tmp_path)
    payload = _manifest(tmp_path, proof)
    payload["ended_at"] = "2026-09-20T23:59:59Z"
    schema = json.loads(SCHEMA_PATH.read_text(encoding="utf-8"))
    messages = _messages(
        MODULE.check_manifest(
            payload,
            manifest_path=tmp_path / "proof.json",
            proof=proof,
            schema=schema,
            root=tmp_path,
            bind_source=False,
        )
    )
    assert any("ended_at precedes started_at" in message for message in messages)


def test_manifest_refuses_wrong_binary_and_missing_artifact(tmp_path: Path) -> None:
    proof = _proof(tmp_path)
    payload = _manifest(tmp_path, proof)
    payload["daemon_binary"]["sha256"] = "0" * 64
    (tmp_path / payload["artifacts"][0]["path"]).unlink()
    schema = json.loads(SCHEMA_PATH.read_text(encoding="utf-8"))
    messages = _messages(
        MODULE.check_manifest(
            payload,
            manifest_path=tmp_path / "proof.json",
            proof=proof,
            schema=schema,
            root=tmp_path,
            bind_source=False,
        )
    )
    assert "daemon binary digest mismatch" in messages
    assert any("proof artifact is missing" in message for message in messages)


def test_manifest_refuses_registry_invocation_and_dependency_drift(tmp_path: Path) -> None:
    proof = _proof(tmp_path)
    proof["dependencies"] = ["parent-proof"]
    payload = _manifest(tmp_path, proof)
    payload["invocation"]["profile"] = "wrong-profile"
    schema = json.loads(SCHEMA_PATH.read_text(encoding="utf-8"))
    messages = _messages(
        MODULE.check_manifest(
            payload,
            manifest_path=tmp_path / "proof.json",
            proof=proof,
            schema=schema,
            root=tmp_path,
            bind_source=False,
        )
    )
    assert "invocation.profile differs from proof authority" in messages
    assert "dependency receipt IDs differ from proof authority" in messages


def test_manifest_refuses_failed_count_with_passed_status(tmp_path: Path) -> None:
    proof = _proof(tmp_path)
    payload = _manifest(tmp_path, proof)
    payload["counts"] = {"selected": 1, "executed": 1, "passed": 0, "failed": 1, "ignored": 0}
    schema = json.loads(SCHEMA_PATH.read_text(encoding="utf-8"))
    messages = _messages(
        MODULE.check_manifest(
            payload,
            manifest_path=tmp_path / "proof.json",
            proof=proof,
            schema=schema,
            root=tmp_path,
            bind_source=False,
        )
    )
    assert any("requires selected, executed and passed > 0" in message for message in messages)
    assert "passed proof cannot contain failed tests" in messages


def test_bind_source_refuses_stale_head_and_dirty_digest(tmp_path: Path) -> None:
    subprocess.run(["git", "init", "-q", str(tmp_path)], check=True)
    subprocess.run(["git", "-C", str(tmp_path), "config", "user.name", "Fixture"], check=True)
    subprocess.run(
        ["git", "-C", str(tmp_path), "config", "user.email", "fixture@example.invalid"],
        check=True,
    )
    (tmp_path / "tracked").write_text("tracked\n", encoding="utf-8")
    subprocess.run(["git", "-C", str(tmp_path), "add", "tracked"], check=True)
    subprocess.run(["git", "-C", str(tmp_path), "commit", "-qm", "fixture"], check=True)
    proof = _proof(tmp_path)
    payload = _manifest(tmp_path, proof)
    schema = json.loads(SCHEMA_PATH.read_text(encoding="utf-8"))
    messages = _messages(
        MODULE.check_manifest(
            payload,
            manifest_path=tmp_path / "proof.json",
            proof=proof,
            schema=schema,
            root=tmp_path,
            bind_source=True,
        )
    )
    assert "source.head is not current HEAD" in messages
    assert "source.dirty_digest is not current working tree" in messages


def test_cli_refuses_unregistered_proof_id(tmp_path: Path) -> None:
    registry = tmp_path / "proof-authority.toml"
    registry.write_bytes(REGISTRY_PATH.read_bytes())
    schema = tmp_path / "proof-manifest.schema.json"
    schema.write_bytes(SCHEMA_PATH.read_bytes())
    manifest = tmp_path / "unknown.json"
    manifest.write_text('{"proof_id":"unknown-proof"}\n', encoding="utf-8")
    assert (
        MODULE.main(
            [
                "--root",
                str(REPO_ROOT),
                "--registry",
                str(registry),
                "--schema",
                str(schema),
                "--manifest",
                str(manifest),
            ]
        )
        == 1
    )


def test_require_all_refuses_absent_manifests(tmp_path: Path) -> None:
    registry = tmp_path / "proof-authority.toml"
    registry.write_bytes(REGISTRY_PATH.read_bytes())
    schema = tmp_path / "proof-manifest.schema.json"
    schema.write_bytes(SCHEMA_PATH.read_bytes())
    assert (
        MODULE.main(
            [
                "--root",
                str(REPO_ROOT),
                "--registry",
                str(registry),
                "--schema",
                str(schema),
                "--require-all",
            ]
        )
        == 1
    )
