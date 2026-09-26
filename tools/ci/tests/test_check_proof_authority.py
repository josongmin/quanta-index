"""Contract tests for the SEP-21 proof authority and manifest validator."""

from __future__ import annotations

import copy
import hashlib
import importlib.util
import io
import json
import subprocess
import sys
from pathlib import Path

import pytest

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
        "authority_state": "executable",
        "execution_mode": "non-test-assertion",
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
    evidence = root / "artifacts/raw.jsonl"
    evidence.parent.mkdir(parents=True, exist_ok=True)
    evidence.write_bytes(b'{"terminal":"passed"}\n')
    digest = MODULE.CLEAN_DIRTY_DIGEST
    evidence_digest = _sha(evidence)
    evidence_archive = root / MODULE.content_archive_relative_path("evidence", evidence_digest)
    evidence_archive.parent.mkdir(parents=True, exist_ok=True)
    evidence_archive.write_bytes(evidence.read_bytes())
    daemon_binary = None
    if proof["binary_binding"] == "release-daemon":
        binary = root / "bin/searchd"
        binary.parent.mkdir(parents=True, exist_ok=True)
        binary.write_bytes(b"release-daemon")
        binary_digest = _sha(binary)
        binary_archive = root / MODULE.content_archive_relative_path("binary", binary_digest)
        binary_archive.parent.mkdir(parents=True, exist_ok=True)
        binary_archive.write_bytes(binary.read_bytes())
        daemon_binary = {
            "source_path": "bin/searchd",
            "path": binary_archive.relative_to(root).as_posix(),
            "sha256": binary_digest,
        }
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
        "source_pair": None,
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
        "daemon_binary": daemon_binary,
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
        "artifacts": [
            {
                "source_path": "artifacts/raw.jsonl",
                "path": evidence_archive.relative_to(root).as_posix(),
                "sha256": evidence_digest,
            }
        ],
    }


def _messages(findings) -> list[str]:
    return [finding.message for finding in findings]


def _init_repo(root: Path) -> None:
    subprocess.run(["git", "init", "-q", str(root)], check=True)
    subprocess.run(["git", "-C", str(root), "config", "user.name", "Fixture"], check=True)
    subprocess.run(
        ["git", "-C", str(root), "config", "user.email", "fixture@example.invalid"],
        check=True,
    )
    (root / "tracked").write_bytes(b"tracked\n")
    subprocess.run(["git", "-C", str(root), "add", "tracked"], check=True)
    subprocess.run(["git", "-C", str(root), "commit", "-qm", "fixture"], check=True)


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


def _dependency_diamond(tmp_path: Path, depth: int = 1):
    proofs = {}
    payloads = {}
    plan = [("leaf", [])]
    dependencies = ["leaf"]
    for level in range(depth):
        names = ["left", "right"] if level == 0 else [f"left-{level}", f"right-{level}"]
        plan.extend((name, dependencies) for name in names)
        dependencies = names
    plan.append(("root", dependencies))
    for name, dependencies in plan:
        proof = {
            **_proof(tmp_path),
            "id": name,
            "dependencies": dependencies,
            "binary_binding": "release-daemon",
        }
        payload = _manifest(tmp_path, proof)
        payload["dependency_receipts"] = [
            {
                "proof_id": dependency,
                "path": payloads[dependency][1],
                "sha256": payloads[dependency][2],
            }
            for dependency in dependencies
        ]
        raw = json.dumps(payload).encode()
        digest = hashlib.sha256(raw).hexdigest()
        path = MODULE.proof_archive_relative_path(payload, digest)
        target = tmp_path / path
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_bytes(raw)
        proofs[name] = proof
        payloads[name] = (payload, path, digest)
    return proofs, payloads


@pytest.mark.parametrize("depth", [1, 3, 6])
def test_dependency_diamond_parses_each_content_once(
    tmp_path: Path, monkeypatch, depth: int
) -> None:
    schema = json.loads(SCHEMA_PATH.read_text())
    proofs, payloads = _dependency_diamond(tmp_path, depth)
    observed = []
    original = MODULE.jsonschema.Draft202012Validator

    class CountingValidator:
        def __init__(self, *args, **kwargs):
            self.validator = original(*args, **kwargs)

        def iter_errors(self, payload):
            observed.append(payload["proof_id"])
            return self.validator.iter_errors(payload)

    monkeypatch.setattr(MODULE.jsonschema, "Draft202012Validator", CountingValidator)
    assert (
        MODULE.check_manifest(
            payloads["root"][0],
            manifest_path=tmp_path / payloads["root"][1],
            proof=proofs["root"],
            schema=schema,
            root=tmp_path,
            bind_source=False,
            proof_by_id=proofs,
        )
        == []
    )
    assert len(observed) == 2 * depth + 2
    assert set(observed) == set(proofs)


@pytest.mark.parametrize("mutation", ["artifact", "binary", "archive", "symlink", "authority"])
def test_dependency_cache_rechecks_live_inputs(tmp_path: Path, monkeypatch, mutation: str) -> None:
    schema = json.loads(SCHEMA_PATH.read_text())
    proofs, payloads = _dependency_diamond(tmp_path)
    leaf, leaf_path, _ = payloads["leaf"]
    original = MODULE._payload_bytes
    reads = 0

    def read(root, value, *, label):
        nonlocal reads
        raw = original(root, value, label=label)
        if value == leaf_path:
            reads += 1
            if reads == 2:
                if mutation == "authority":
                    proofs["leaf"]["command"] = "wrong command"
                elif mutation == "archive":
                    (tmp_path / leaf_path).write_bytes(raw + b"\n")
                else:
                    binding = (
                        leaf["daemon_binary"] if mutation == "binary" else leaf["artifacts"][0]
                    )
                    target = tmp_path / binding["path"]
                    if mutation == "symlink":
                        saved = tmp_path / "substituted-evidence"
                        saved.write_bytes(target.read_bytes())
                        target.unlink()
                        target.symlink_to(saved)
                    else:
                        target.write_bytes(target.read_bytes() + b"corruption")
        return raw

    monkeypatch.setattr(MODULE, "_payload_bytes", read)
    messages = _messages(
        MODULE.check_manifest(
            payloads["root"][0],
            manifest_path=tmp_path / payloads["root"][1],
            proof=proofs["root"],
            schema=schema,
            root=tmp_path,
            bind_source=False,
            proof_by_id=proofs,
        )
    )
    assert reads == 2
    assert messages
    if mutation == "authority":
        assert "invocation.command differs from proof authority" in messages
    elif mutation == "symlink":
        assert any("regular archive" in message or "unreadable" in message for message in messages)
    else:
        assert any("digest mismatch" in message for message in messages)


def test_dependency_cache_does_not_survive_an_invocation(tmp_path: Path) -> None:
    schema = json.loads(SCHEMA_PATH.read_text())
    proofs, payloads = _dependency_diamond(tmp_path)
    kwargs = dict(
        manifest_path=tmp_path / payloads["root"][1],
        proof=proofs["root"],
        schema=schema,
        root=tmp_path,
        bind_source=False,
        proof_by_id=proofs,
    )
    assert MODULE.check_manifest(payloads["root"][0], **kwargs) == []
    (tmp_path / payloads["leaf"][1]).unlink()
    assert MODULE.check_manifest(payloads["root"][0], **kwargs)


def test_dependency_cache_keeps_live_source_binding(tmp_path: Path) -> None:
    schema = json.loads(SCHEMA_PATH.read_text())
    proofs, payloads = _dependency_diamond(tmp_path)
    payload = payloads["root"][0]
    kwargs = dict(
        manifest_path=tmp_path / payloads["root"][1],
        proof=proofs["root"],
        schema=schema,
        root=tmp_path,
        bind_source=True,
        proof_by_id=proofs,
    )
    assert MODULE.check_manifest(payload, bound_source=payload["source"], **kwargs) == []
    current_source = {**payload["source"], "head": "b" * 40}
    messages = _messages(MODULE.check_manifest(payload, bound_source=current_source, **kwargs))
    assert "source.head is not current HEAD" in messages


def test_registry_cycle_cannot_become_a_cache_hit() -> None:
    registry = copy.deepcopy(MODULE._read_toml(REGISTRY_PATH))
    registry["proofs"][0]["dependencies"] = [registry["proofs"][-1]["id"]]
    registry["proofs"][-1]["dependencies"] = [registry["proofs"][0]["id"]]
    messages = _messages(MODULE.check_registry(registry, root=REPO_ROOT, path=REGISTRY_PATH))
    assert any("dependency cycle" in message for message in messages)


def test_manifest_v0_is_refused(tmp_path: Path) -> None:
    (tmp_path / "owner.md").write_text("owner\n", encoding="utf-8")
    proof = _proof(tmp_path)
    payload = _manifest(tmp_path, proof)
    payload["schema_version"] = 0
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

    assert any("schema" in message for message in messages)


def test_legacy_verification_receipt_is_not_proof_manifest(tmp_path: Path) -> None:
    (tmp_path / "owner.md").write_text("owner\n", encoding="utf-8")
    proof = _proof(tmp_path)
    legacy_receipt = {
        "schema_version": 1,
        "revision": "a" * 40,
        "evidence_digest": "b" * 64,
        "test_count": 1,
    }
    schema = json.loads(SCHEMA_PATH.read_text(encoding="utf-8"))

    messages = _messages(
        MODULE.check_manifest(
            legacy_receipt,
            manifest_path=tmp_path / "proof.json",
            proof=proof,
            schema=schema,
            root=tmp_path,
            bind_source=False,
        )
    )

    assert any("schema" in message for message in messages)


def test_nonpassing_terminal_manifest_is_not_authoritative(tmp_path: Path) -> None:
    proof = _proof(tmp_path)
    schema = json.loads(SCHEMA_PATH.read_text(encoding="utf-8"))
    for status in ("failed", "blocked", "not_run"):
        payload = _manifest(tmp_path, proof)
        payload["status"] = status
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
        assert f"authoritative proof status must be 'passed', got {status!r}" in messages

    payload = _manifest(tmp_path, proof)
    payload["status"] = "blocked"
    messages = _messages(
        MODULE.check_manifest(
            payload,
            manifest_path=tmp_path / "proof.json",
            proof=proof,
            schema=schema,
            root=tmp_path,
            bind_source=False,
            allow_non_passed=True,
        )
    )
    assert not any("authoritative proof status" in message for message in messages)


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
    proof["binary_binding"] = "release-daemon"
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
    assert "dependency receipts are not the ordered one-to-one proof authority edges" in messages


def test_manifest_refuses_dependency_receipt_path_not_owned_by_registry(tmp_path: Path) -> None:
    proof = _proof(tmp_path)
    proof["dependencies"] = ["parent-proof"]
    parent = _proof(tmp_path)
    parent["id"] = "parent-proof"
    parent["artifact"] = "proofs/parent-proof.json"
    registered_receipt = tmp_path / parent["artifact"]
    registered_receipt.parent.mkdir(parents=True)
    registered_receipt.write_text('{"proof_id":"parent-proof"}\n', encoding="utf-8")
    substitute = tmp_path / "proofs/substitute.json"
    substitute.write_bytes(registered_receipt.read_bytes())

    payload = _manifest(tmp_path, proof)
    payload["dependency_receipts"] = [
        {
            "proof_id": "parent-proof",
            "path": "proofs/substitute.json",
            "sha256": _sha(substitute),
        }
    ]
    schema = json.loads(SCHEMA_PATH.read_text(encoding="utf-8"))
    messages = _messages(
        MODULE.check_manifest(
            payload,
            manifest_path=tmp_path / "proof.json",
            proof=proof,
            schema=schema,
            root=tmp_path,
            bind_source=False,
            proof_by_id={proof["id"]: proof, parent["id"]: parent},
        )
    )
    assert any("does not match" in message for message in messages)


def test_historical_dependency_archive_survives_current_alias_move(tmp_path: Path) -> None:
    parent = _proof(tmp_path)
    parent["id"] = "parent-proof"
    parent["artifact"] = "artifacts/proof-authority/parent-proof.json"
    child = _proof(tmp_path)
    child["id"] = "child-proof"
    child["artifact"] = "artifacts/proof-authority/child-proof.json"
    child["dependencies"] = [parent["id"]]
    schema = json.loads(SCHEMA_PATH.read_text(encoding="utf-8"))

    parent_payload = _manifest(tmp_path, parent)
    parent_bytes = (json.dumps(parent_payload, sort_keys=True, indent=2) + "\n").encode()
    parent_digest = hashlib.sha256(parent_bytes).hexdigest()
    parent_archive_relative = MODULE.proof_archive_relative_path(parent_payload, parent_digest)
    parent_archive = tmp_path / parent_archive_relative
    parent_archive.parent.mkdir(parents=True)
    parent_archive.write_bytes(parent_bytes)

    child_payload = _manifest(tmp_path, child)
    child_payload["dependency_receipts"] = [
        {
            "proof_id": parent["id"],
            "path": parent_archive_relative,
            "sha256": parent_digest,
        }
    ]

    current_alias = tmp_path / parent["artifact"]
    current_alias.parent.mkdir(parents=True, exist_ok=True)
    newer_payload = copy.deepcopy(parent_payload)
    newer_payload["source"]["head"] = "c" * 40
    current_alias.write_text(json.dumps(newer_payload), encoding="utf-8")

    assert (
        MODULE.check_manifest(
            child_payload,
            manifest_path=tmp_path / child["artifact"],
            proof=child,
            schema=schema,
            root=tmp_path,
            bind_source=False,
            proof_by_id={parent["id"]: parent, child["id"]: child},
        )
        == []
    )

    parent_archive.write_bytes(parent_bytes + b"tampered\n")
    messages = _messages(
        MODULE.check_manifest(
            child_payload,
            manifest_path=tmp_path / child["artifact"],
            proof=child,
            schema=schema,
            root=tmp_path,
            bind_source=False,
            proof_by_id={parent["id"]: parent, child["id"]: child},
        )
    )
    assert any("dependency receipt digest mismatch" in message for message in messages)


def test_manifest_refuses_duplicate_retry_receipts_for_one_dependency(tmp_path: Path) -> None:
    parent = _proof(tmp_path)
    parent["id"] = "parent-proof"
    child = _proof(tmp_path)
    child["id"] = "child-proof"
    child["dependencies"] = [parent["id"]]
    schema = json.loads(SCHEMA_PATH.read_text(encoding="utf-8"))

    receipts = []
    for offset in (1, 2):
        payload = _manifest(tmp_path, parent)
        payload["ended_at"] = f"2026-09-21T00:00:0{offset}Z"
        raw = (json.dumps(payload, sort_keys=True, indent=2) + "\n").encode()
        digest = hashlib.sha256(raw).hexdigest()
        relative = MODULE.proof_archive_relative_path(payload, digest)
        archive = tmp_path / relative
        archive.parent.mkdir(parents=True, exist_ok=True)
        archive.write_bytes(raw)
        receipts.append({"proof_id": parent["id"], "path": relative, "sha256": digest})

    payload = _manifest(tmp_path, child)
    payload["dependency_receipts"] = receipts
    messages = _messages(
        MODULE.check_manifest(
            payload,
            manifest_path=tmp_path / "child.json",
            proof=child,
            schema=schema,
            root=tmp_path,
            bind_source=False,
            proof_by_id={parent["id"]: parent, child["id"]: child},
        )
    )

    assert "dependency receipts are not the ordered one-to-one proof authority edges" in messages


def test_source_binding_digest_includes_pair_and_manifest_digest_allows_retries() -> None:
    proof = _proof(Path("."))
    first = {
        "proof_id": proof["id"],
        "source": {"head": "a" * 40},
        "source_pair": None,
    }
    paired = copy.deepcopy(first)
    paired["source_pair"] = {"repository": "github:example/pair", "source": {"head": "b" * 40}}
    assert MODULE.source_binding_digest(first) != MODULE.source_binding_digest(paired)
    first_leaf = MODULE.proof_archive_relative_path(first, "1" * 64)
    retry_leaf = MODULE.proof_archive_relative_path(first, "2" * 64)
    assert first_leaf != retry_leaf
    assert first_leaf.split("/")[-2] == retry_leaf.split("/")[-2]


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


def test_linux_production_host_label_cannot_spoof_non_linux_os(tmp_path: Path) -> None:
    proof = _proof(tmp_path)
    proof["required_host"] = "linux-production-like"
    payload = _manifest(tmp_path, proof)
    payload["environment"]["host"]["profile"] = "linux-production-like"
    payload["environment"]["os"] = "darwin"
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
    assert "linux-production-like proof requires environment.os='linux'" in messages


def test_p00_bound_inventory_rejects_forged_discovery(tmp_path: Path) -> None:
    _init_repo(tmp_path)
    tools = tmp_path / "tools/ci"
    tools.mkdir(parents=True)
    writer_path = tools / "write-error-authority-inventory.py"
    writer_path.write_bytes(
        (REPO_ROOT / "tools/ci/write-error-authority-inventory.py").read_bytes()
    )
    (tools / "error-authority-inventory.schema.json").write_bytes(
        (REPO_ROOT / "tools/ci/error-authority-inventory.schema.json").read_bytes()
    )
    spec = importlib.util.spec_from_file_location("p00_inventory_fixture", writer_path)
    assert spec and spec.loader
    writer = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = writer
    spec.loader.exec_module(writer)
    inventory = writer.build_inventory(tmp_path)
    archive = tmp_path / "artifacts/inventory.json"
    archive.parent.mkdir()
    archive.write_text(json.dumps(inventory), encoding="utf-8")
    artifacts = [{"source_path": MODULE.ERROR_INVENTORY_PATH, "path": "artifacts/inventory.json"}]

    MODULE._check_p00_current_inventory(tmp_path, artifacts)

    inventory["source_digest"] = "sha256:" + "0" * 64
    archive.write_text(json.dumps(inventory), encoding="utf-8")
    with pytest.raises(ValueError, match="differs from current source-bound discovery"):
        MODULE._check_p00_current_inventory(tmp_path, artifacts)
    with pytest.raises(ValueError, match="exactly one"):
        MODULE._check_p00_current_inventory(tmp_path, artifacts * 2)


def test_bind_source_refuses_stale_head_and_dirty_digest(tmp_path: Path) -> None:
    _init_repo(tmp_path)
    proof = _proof(tmp_path)
    payload = _manifest(tmp_path, proof)
    payload["source"]["dirty_digest"] = "sha256:" + "1" * 64
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


def test_bind_source_accepts_checkout_metadata_drift_for_same_bytes(tmp_path: Path) -> None:
    _init_repo(tmp_path)
    proof = _proof(tmp_path)
    payload = _manifest(tmp_path, proof)
    current = MODULE.proof_source_snapshot(
        tmp_path,
        manifest_path=tmp_path / "proof.json",
        proof=proof,
        excluded_paths=[tmp_path / "artifacts/raw.jsonl"],
    )
    payload["source"].update(
        head=current["head"],
        dirty_digest=current["dirty_digest"],
        branch="producer-branch",
        upstream="origin/producer-branch",
        merge_base="a" * 40,
    )
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
    assert not any(message.startswith("source.") for message in messages), messages


def test_aggregate_compares_source_content_and_pair_lock() -> None:
    source = {
        "head": "a" * 40,
        "dirty_digest": MODULE.CLEAN_DIRTY_DIGEST,
        "branch": "producer",
        "upstream": "origin/main",
        "merge_base": "b" * 40,
    }
    checkout = {**source, "branch": None, "upstream": None, "merge_base": None}
    assert MODULE.source_content_identity(source) == MODULE.source_content_identity(checkout)
    pair = {
        "repository": MODULE.PAIRED_REPOSITORY,
        "remote_identity_digest": "sha256:" + "1" * 64,
        "source": source,
        "dependency_lock": {"path": "Cargo.lock", "sha256": "2" * 64},
    }
    other = {**pair, "source": checkout}
    assert MODULE.paired_content_identity(pair) == MODULE.paired_content_identity(other)
    other["dependency_lock"] = {"path": "Cargo.lock", "sha256": "3" * 64}
    assert MODULE.paired_content_identity(pair) != MODULE.paired_content_identity(other)


def test_dirty_digest_binds_staged_unstaged_and_scoped_untracked_bytes(
    tmp_path: Path,
) -> None:
    _init_repo(tmp_path)
    excluded = [Path("artifacts/proof-authority")]
    clean = MODULE.dirty_digest(tmp_path, excluded_paths=excluded)

    (tmp_path / "tracked").write_bytes(b"staged\n")
    subprocess.run(["git", "-C", str(tmp_path), "add", "tracked"], check=True)
    staged = MODULE.dirty_digest(tmp_path, excluded_paths=excluded)
    assert staged != clean

    (tmp_path / "tracked").write_bytes(b"unstaged-after-index\n")
    staged_and_unstaged = MODULE.dirty_digest(tmp_path, excluded_paths=excluded)
    assert staged_and_unstaged not in {clean, staged}

    (tmp_path / "source-new").write_bytes(b"untracked-source\x00bytes")
    with_untracked = MODULE.dirty_digest(tmp_path, excluded_paths=excluded)
    assert with_untracked not in {clean, staged, staged_and_unstaged}

    proof_root = tmp_path / "artifacts/proof-authority"
    proof_root.mkdir(parents=True)
    (proof_root / "proof.json").write_bytes(b"self-referential-output-v1")
    assert MODULE.dirty_digest(tmp_path, excluded_paths=excluded) == with_untracked
    (proof_root / "proof.json").write_bytes(b"self-referential-output-v2")
    assert MODULE.dirty_digest(tmp_path, excluded_paths=excluded) == with_untracked


def test_dirty_digest_batches_staged_index_reads(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    _init_repo(tmp_path)
    contents = {f"staged-{index}".encode(): f"payload-{index}\n".encode() for index in range(32)}
    for raw_path in contents:
        (tmp_path / raw_path.decode()).write_bytes(b"base\n")
    subprocess.run(["git", "-C", str(tmp_path), "add", "."], check=True)
    subprocess.run(["git", "-C", str(tmp_path), "commit", "-qm", "batch fixtures"], check=True)
    for raw_path, content in contents.items():
        (tmp_path / raw_path.decode()).write_bytes(content)
    subprocess.run(["git", "-C", str(tmp_path), "add", "."], check=True)

    calls: list[list[str]] = []
    original_run = MODULE.subprocess.run
    original_popen = MODULE.subprocess.Popen

    def record_run(*args, **kwargs):
        calls.append(args[0])
        return original_run(*args, **kwargs)

    def record_popen(*args, **kwargs):
        if "cat-file" in args[0]:
            calls.append(args[0])
        return original_popen(*args, **kwargs)

    monkeypatch.setattr(MODULE.subprocess, "run", record_run)
    monkeypatch.setattr(MODULE.subprocess, "Popen", record_popen)
    monkeypatch.setattr(MODULE.os, "fpathconf", lambda *_args: 82)
    actual = MODULE.dirty_digest(tmp_path)

    expected = hashlib.sha256()
    expected.update(b"quanta-index-dirty-v2\0")
    for raw_path, content in sorted(contents.items()):
        MODULE._digest_record(expected, b"index", raw_path, b"mode=100644;stage=0", content)
    assert actual == f"sha256:{expected.hexdigest()}"
    assert sum("ls-files" in command for command in calls) == 1
    assert sum("cat-file" in command for command in calls) == 1
    assert len(calls) == 3


def test_digest_stream_field_is_bounded_and_byte_compatible() -> None:
    content = b"x" * (MODULE.STREAM_CHUNK_SIZE * 2 + 17)

    class RecordingDigest:
        def __init__(self) -> None:
            self.delegate = hashlib.sha256()
            self.largest_update = 0

        def update(self, field: bytes) -> None:
            self.largest_update = max(self.largest_update, len(field))
            self.delegate.update(field)

        def hexdigest(self) -> str:
            return self.delegate.hexdigest()

    actual = RecordingDigest()
    MODULE._digest_stream_field(actual, io.BytesIO(content), len(content))

    expected = hashlib.sha256()
    MODULE._digest_field(expected, content)
    assert actual.hexdigest() == expected.hexdigest()
    assert actual.largest_update <= MODULE.STREAM_CHUNK_SIZE


def test_digest_stream_field_rejects_truncated_source() -> None:
    with pytest.raises(RuntimeError, match="truncated source while hashing"):
        MODULE._digest_stream_field(hashlib.sha256(), io.BytesIO(b"short"), 6)


def test_file_sha256_streams_large_artifacts(tmp_path: Path) -> None:
    content = b"artifact\x00" * (MODULE.STREAM_CHUNK_SIZE // 9 + 5)
    artifact = tmp_path / "artifact.bin"
    artifact.write_bytes(content)

    assert MODULE._sha256(artifact) == hashlib.sha256(content).hexdigest()


def test_digest_working_tree_entry_is_byte_compatible(tmp_path: Path) -> None:
    content = b"source\x00" * (MODULE.STREAM_CHUNK_SIZE // 7 + 3)
    source = tmp_path / "large-source"
    source.write_bytes(content)
    mode = f"{source.stat().st_mode & 0o7777:04o}".encode()

    actual = hashlib.sha256()
    MODULE._digest_working_tree_entry(actual, b"untracked", tmp_path, b"large-source")

    expected = hashlib.sha256()
    MODULE._digest_record(expected, b"untracked", b"large-source", b"file:" + mode, content)
    assert actual.hexdigest() == expected.hexdigest()


def test_porcelain_status_paths_match_existing_git_diff_domains(tmp_path: Path) -> None:
    _init_repo(tmp_path)
    (tmp_path / "rename-source").write_bytes(b"rename\n")
    (tmp_path / "unstaged-delete").write_bytes(b"delete\n")
    subprocess.run(
        ["git", "-C", str(tmp_path), "add", "rename-source", "unstaged-delete"],
        check=True,
    )
    subprocess.run(["git", "-C", str(tmp_path), "commit", "-qm", "more fixtures"], check=True)
    tracked = tmp_path / "tracked"
    tracked.write_bytes(b"staged\n")
    subprocess.run(["git", "-C", str(tmp_path), "add", "tracked"], check=True)
    tracked.write_bytes(b"unstaged-after-index\n")
    (tmp_path / "untracked with space").write_bytes(b"new\n")
    (tmp_path / "intent-to-add").write_bytes(b"intent\n")
    subprocess.run(["git", "-C", str(tmp_path), "add", "-N", "intent-to-add"], check=True)
    subprocess.run(
        ["git", "-C", str(tmp_path), "mv", "rename-source", "renamed target"], check=True
    )
    (tmp_path / "unstaged-delete").unlink()

    staged, unstaged, untracked = MODULE._dirty_paths_from_status(
        MODULE._git_status_snapshot(tmp_path)
    )

    def git_paths(*args: str) -> list[bytes]:
        output = subprocess.run(
            ["git", "-C", str(tmp_path), *args],
            check=True,
            capture_output=True,
        ).stdout
        return sorted(path for path in output.split(b"\0") if path)

    assert staged == git_paths(
        "diff",
        "--cached",
        "--name-only",
        "--no-renames",
        "--ignore-submodules=none",
        "-z",
        "HEAD",
        "--",
    )
    assert unstaged == git_paths(
        "diff",
        "--name-only",
        "--no-renames",
        "--ignore-submodules=none",
        "-z",
        "--",
    )
    assert untracked == git_paths("ls-files", "--others", "--exclude-standard", "-z")


def test_proof_source_snapshot_excludes_manifest_artifact_root(tmp_path: Path) -> None:
    _init_repo(tmp_path)
    proof = _proof(tmp_path)
    proof["artifact"] = "artifacts/proof-authority/fixture-proof.json"
    manifest_path = tmp_path / proof["artifact"]
    before = MODULE.proof_source_snapshot(
        tmp_path,
        manifest_path=manifest_path,
        proof=proof,
    )
    manifest_path.parent.mkdir(parents=True)
    manifest_path.write_bytes(b"first-manifest")
    (manifest_path.parent / "raw.log").write_bytes(b"proof-output")
    after = MODULE.proof_source_snapshot(
        tmp_path,
        manifest_path=manifest_path,
        proof=proof,
    )
    assert after == before
    assert before["upstream"] is None
    assert before["merge_base"] is None


def test_source_snapshot_cache_ignores_only_out_of_repo_exclusions(tmp_path: Path) -> None:
    root = tmp_path / "primary"
    root.mkdir()
    _init_repo(root)
    proof = _proof(root)
    cache = {}
    manifest_path = root / proof["artifact"]

    baseline = MODULE._cached_proof_source_snapshot(
        cache,
        root,
        manifest_path=manifest_path,
        proof=proof,
    )
    external = MODULE._cached_proof_source_snapshot(
        cache,
        root,
        manifest_path=manifest_path,
        proof=proof,
        excluded_paths=(tmp_path / "paired",),
    )
    assert external is baseline
    assert len(cache) == 1

    MODULE._cached_proof_source_snapshot(
        cache,
        root,
        manifest_path=manifest_path,
        proof=proof,
        excluded_paths=(root / ".proof-pairs/paired",),
    )
    assert len(cache) == 2


def test_source_snapshot_tracks_upstream_merge_base_and_detached_head(tmp_path: Path) -> None:
    _init_repo(tmp_path)
    branch = subprocess.run(
        ["git", "-C", str(tmp_path), "branch", "--show-current"],
        check=True,
        capture_output=True,
        text=True,
    ).stdout.strip()
    subprocess.run(["git", "-C", str(tmp_path), "branch", "tracking-target"], check=True)
    subprocess.run(
        ["git", "-C", str(tmp_path), "branch", "--set-upstream-to", "tracking-target"],
        check=True,
        capture_output=True,
    )
    attached = MODULE.source_snapshot(tmp_path)
    assert attached["branch"] == branch
    assert attached["upstream"] == "tracking-target"
    assert attached["merge_base"] == attached["head"]

    subprocess.run(["git", "-C", str(tmp_path), "checkout", "--detach", "-q"], check=True)
    detached = MODULE.source_snapshot(tmp_path)
    assert detached["branch"] is None
    assert detached["upstream"] is None
    assert detached["merge_base"] is None

    subprocess.run(
        ["git", "-C", str(tmp_path), "checkout", "--orphan", "disconnected", "-q"],
        check=True,
    )
    subprocess.run(
        ["git", "-C", str(tmp_path), "commit", "--allow-empty", "-qm", "disconnected"],
        check=True,
    )
    subprocess.run(
        ["git", "-C", str(tmp_path), "branch", "--set-upstream-to", "tracking-target"],
        check=True,
        capture_output=True,
    )
    disconnected = MODULE.source_snapshot(tmp_path)
    assert disconnected["branch"] == "disconnected"
    assert disconnected["upstream"] == "tracking-target"
    assert disconnected["merge_base"] is None


def test_source_snapshot_rejects_source_change_during_capture(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    _init_repo(tmp_path)
    original = MODULE._git_status_snapshot(tmp_path)
    changed = original + b"? changed-during-capture\0"
    snapshots = iter((original, changed))
    monkeypatch.setattr(MODULE, "_git_status_snapshot", lambda _root: next(snapshots))

    with pytest.raises(RuntimeError, match="source changed while capturing proof snapshot"):
        MODULE.source_snapshot(tmp_path)


def test_merge_base_distinguishes_disconnected_history_from_git_failure(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    def completed(args: list[str], **_kwargs):
        if args[-1] == "disconnected":
            return subprocess.CompletedProcess(args, 1, stdout="", stderr="")
        return subprocess.CompletedProcess(args, 128, stdout="", stderr="fatal: broken ref")

    monkeypatch.setattr(MODULE.subprocess, "run", completed)

    assert MODULE._merge_base(tmp_path, "a" * 40, "disconnected") is None
    with pytest.raises(RuntimeError, match="fatal: broken ref"):
        MODULE._merge_base(tmp_path, "a" * 40, "broken")


def test_source_snapshot_rejects_upstream_move_during_capture(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    _init_repo(tmp_path)
    subprocess.run(["git", "-C", str(tmp_path), "branch", "tracking-target"], check=True)
    subprocess.run(
        ["git", "-C", str(tmp_path), "branch", "--set-upstream-to", "tracking-target"],
        check=True,
        capture_output=True,
    )
    merge_bases = iter(("a" * 40, "b" * 40))
    monkeypatch.setattr(MODULE, "_merge_base", lambda *_args: next(merge_bases))

    with pytest.raises(RuntimeError, match="upstream changed while capturing proof snapshot"):
        MODULE.source_snapshot(tmp_path)


def test_manifest_refuses_absolute_traversal_and_symlink_escape(tmp_path: Path) -> None:
    root = tmp_path / "root"
    root.mkdir()
    proof = _proof(root)
    proof["binary_binding"] = "release-daemon"
    schema = json.loads(SCHEMA_PATH.read_text(encoding="utf-8"))

    absolute_artifact = _manifest(root, proof)
    absolute_artifact["artifacts"][0]["path"] = str(
        (root / absolute_artifact["artifacts"][0]["path"]).resolve()
    )
    messages = _messages(
        MODULE.check_manifest(
            absolute_artifact,
            manifest_path=root / "proof.json",
            proof=proof,
            schema=schema,
            root=root,
            bind_source=False,
        )
    )
    assert any("schema artifacts.0.path" in item for item in messages)

    traversal = _manifest(root, proof)
    proof["dependencies"] = ["parent-proof"]
    traversal["dependency_receipts"] = [
        {"proof_id": "parent-proof", "path": "../outside.json", "sha256": "0" * 64}
    ]
    messages = _messages(
        MODULE.check_manifest(
            traversal,
            manifest_path=root / "proof.json",
            proof=proof,
            schema=schema,
            root=root,
            bind_source=False,
        )
    )
    assert any("does not match" in item for item in messages)

    proof["dependencies"] = []
    outside_binary = tmp_path / "outside-searchd"
    outside_binary.write_bytes(b"outside")
    symlink_escape = _manifest(root, proof)
    daemon_archive = root / symlink_escape["daemon_binary"]["path"]
    daemon_archive.unlink()
    daemon_archive.symlink_to(outside_binary)
    messages = _messages(
        MODULE.check_manifest(
            symlink_escape,
            manifest_path=root / "proof.json",
            proof=proof,
            schema=schema,
            root=root,
            bind_source=False,
        )
    )
    assert any("daemon binary is not a regular archive" in item for item in messages)


def test_manifest_refuses_in_repo_symlinked_evidence_and_binary(
    tmp_path: Path,
) -> None:
    proof = _proof(tmp_path)
    proof["binary_binding"] = "release-daemon"
    payload = _manifest(tmp_path, proof)
    schema = json.loads(SCHEMA_PATH.read_text(encoding="utf-8"))
    for field, expected in (
        (payload["artifacts"][0], "proof artifact is not a regular archive"),
        (payload["daemon_binary"], "daemon binary is not a regular archive"),
    ):
        archive = tmp_path / field["path"]
        mutable_copy = tmp_path / f"mutable-{archive.name}"
        mutable_copy.write_bytes(archive.read_bytes())
        archive.unlink()
        archive.symlink_to(mutable_copy)
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
        assert any(expected in message for message in messages), messages


def test_manifest_refuses_in_repo_symlinked_dependency_archive(
    tmp_path: Path,
) -> None:
    parent = _proof(tmp_path)
    parent["id"] = "parent-proof"
    child = _proof(tmp_path)
    child["id"] = "child-proof"
    child["dependencies"] = [parent["id"]]
    schema = json.loads(SCHEMA_PATH.read_text(encoding="utf-8"))
    parent_payload = _manifest(tmp_path, parent)
    parent_bytes = (json.dumps(parent_payload, sort_keys=True) + "\n").encode()
    digest = hashlib.sha256(parent_bytes).hexdigest()
    relative = MODULE.proof_archive_relative_path(parent_payload, digest)
    archive = tmp_path / relative
    archive.parent.mkdir(parents=True)
    mutable_copy = tmp_path / "mutable-parent.json"
    mutable_copy.write_bytes(parent_bytes)
    archive.symlink_to(mutable_copy)
    child_payload = _manifest(tmp_path, child)
    child_payload["dependency_receipts"] = [
        {"proof_id": parent["id"], "path": relative, "sha256": digest}
    ]

    messages = _messages(
        MODULE.check_manifest(
            child_payload,
            manifest_path=tmp_path / "child.json",
            proof=child,
            schema=schema,
            root=tmp_path,
            bind_source=False,
            proof_by_id={parent["id"]: parent, child["id"]: child},
        )
    )
    assert any("dependency receipt is unreadable" in message for message in messages), messages


def test_aggregate_closure_is_registry_driven_and_includes_infrastructure() -> None:
    registry = MODULE._read_toml(REGISTRY_PATH)
    closure = MODULE.aggregate_proof_ids(registry)
    assert closure[-1] == "p12a-proof-infrastructure"
    assert "p11-deployment" in closure
    assert "p11-activation" in closure
    assert "p11-rollback" in closure
    assert len(closure) == len(set(closure))


def test_code_gate_selects_only_canonical_code_proofs_and_prerequisites() -> None:
    registry = MODULE._read_toml(REGISTRY_PATH)
    selected = MODULE.code_qualified_proof_ids(registry)

    assert selected == MODULE.EXPECTED_VERDICT_PROOFS["CODE_QUALIFIED"]
    assert len(selected) == 21
    assert not {
        "p11-deployment",
        "p11-activation",
        "p11-rollback",
    } & set(selected)

    registry["aggregate"]["verdicts"]["CODE_QUALIFIED"].remove("p11-cross-repo-cutover")
    with pytest.raises(ValueError, match="canonical authority"):
        MODULE.code_qualified_proof_ids(registry)


def test_code_gate_requires_source_binding_and_does_not_require_operations(capsys) -> None:
    assert MODULE.main(["--require-code-qualified"]) == 1
    assert "--require-code-qualified requires --bind-source" in capsys.readouterr().err

    assert MODULE.main(["--require-code-qualified", "--bind-source"]) == 1
    stderr = capsys.readouterr().err
    assert "p11-cross-repo-cutover" in stderr
    assert "p11-deployment" not in stderr
    assert "p11-activation" not in stderr
    assert "p11-rollback" not in stderr
    assert "registered aggregate artifact is missing" not in stderr


def test_registry_refuses_proof_set_or_edge_drift_from_canonical_graph() -> None:
    registry = MODULE._read_toml(REGISTRY_PATH)
    extra = copy.deepcopy(registry["proofs"][1])
    extra["id"] = "p01-shadow"
    extra["artifact"] = "artifacts/proof-authority/p01-shadow.json"
    registry["proofs"].append(extra)
    messages = _messages(MODULE.check_registry(registry, root=REPO_ROOT, path=REGISTRY_PATH))
    assert any("proof IDs differ from canonical SEP-21 graph" in message for message in messages)

    registry = MODULE._read_toml(REGISTRY_PATH)
    p10 = next(proof for proof in registry["proofs"] if proof["id"] == "p10-state-migration")
    p10["dependencies"].remove("p09-control-readiness")
    messages = _messages(MODULE.check_registry(registry, root=REPO_ROOT, path=REGISTRY_PATH))
    assert any("dependencies differ from canonical SEP-21 graph" in message for message in messages)


def test_registry_refuses_verdict_meaning_drift() -> None:
    registry = MODULE._read_toml(REGISTRY_PATH)
    registry["aggregate"]["verdicts"]["DEPLOYED"] = ["p00-authority-freeze"]

    messages = _messages(MODULE.check_registry(registry, root=REPO_ROOT, path=REGISTRY_PATH))

    assert any("verdict proof sets differ from canonical" in message for message in messages)


def test_registry_refuses_aggregate_prerequisite_drift() -> None:
    registry = MODULE._read_toml(REGISTRY_PATH)
    registry["aggregate"]["prerequisites"] = []

    messages = _messages(MODULE.check_registry(registry, root=REPO_ROOT, path=REGISTRY_PATH))

    assert any("aggregate.prerequisites must require P12A" in message for message in messages)


def test_executable_test_proof_requires_scope_that_selects_registered_targets() -> None:
    registry = MODULE._read_toml(REGISTRY_PATH)
    proof = next(proof for proof in registry["proofs"] if proof["id"] == "p02b-operation-journal")
    proof["test_authority_scopes"] = []

    messages = _messages(MODULE.check_registry(registry, root=REPO_ROOT, path=REGISTRY_PATH))
    assert any("requires non-empty test_authority_scopes" in message for message in messages)

    proof["test_authority_scopes"] = ["p02b-operation-journal"]
    messages = _messages(MODULE.check_registry(registry, root=REPO_ROOT, path=REGISTRY_PATH))
    assert not any("test_authority_scopes" in message for message in messages)
    assert not any("targets are not selected" in message for message in messages)
    assert not any("command/profile binding differs" in message for message in messages)


def test_executable_dedicated_proof_requires_existing_scope_bound_recipe() -> None:
    registry = MODULE._read_toml(REGISTRY_PATH)
    proof = next(proof for proof in registry["proofs"] if proof["id"] == "p02b-operation-journal")
    proof["command"] = "just proof-does-not-exist"

    messages = _messages(MODULE.check_registry(registry, root=REPO_ROOT, path=REGISTRY_PATH))
    assert any("dedicated proof recipe does not exist" in message for message in messages)


def test_p12a_python_owner_is_registered_and_source_bound() -> None:
    registry = MODULE._read_toml(REGISTRY_PATH)
    proof = next(
        proof for proof in registry["proofs"] if proof["id"] == "p12a-proof-infrastructure"
    )
    assert proof["authority_state"] == "executable"
    assert proof["source_binding"] == "exact"
    assert proof["dependencies"] == []
    assert proof["test_authority_targets"] == MODULE.EXPECTED_P12A_TEST_TARGETS
    assert not MODULE.check_registry(registry, root=REPO_ROOT, path=REGISTRY_PATH)


def test_static_registry_lint_does_not_claim_execution_proof(capsys) -> None:
    assert MODULE._main_locked([]) == 0
    assert "REGISTRY_ONLY:" in capsys.readouterr().out


def test_p12a_refuses_a_shrunk_infrastructure_test_set() -> None:
    registry = MODULE._read_toml(REGISTRY_PATH)
    proof = next(
        proof for proof in registry["proofs"] if proof["id"] == "p12a-proof-infrastructure"
    )
    proof["test_authority_targets"] = ["proof-aggregate-python-owner"]
    messages = _messages(MODULE.check_registry(registry, root=REPO_ROOT, path=REGISTRY_PATH))
    assert any("P12A test authority targets differ" in message for message in messages)


def test_p12a_python_owner_refuses_missing_scope_target_and_recipe(monkeypatch) -> None:
    registry = MODULE._read_toml(REGISTRY_PATH)
    proof = next(
        proof for proof in registry["proofs"] if proof["id"] == "p12a-proof-infrastructure"
    )
    proof["test_authority_targets"] = []
    messages = _messages(MODULE.check_registry(registry, root=REPO_ROOT, path=REGISTRY_PATH))
    assert any("requires at least one target" in message for message in messages)

    proof["test_authority_targets"] = ["proof-aggregate-python-owner"]
    proof["test_authority_scopes"] = ["missing-python-scope"]
    messages = _messages(MODULE.check_registry(registry, root=REPO_ROOT, path=REGISTRY_PATH))
    assert any("unknown python scopes" in message for message in messages)

    proof["test_authority_scopes"] = ["p12a-proof-infrastructure"]
    original_body = MODULE._just_recipe_body
    monkeypatch.setattr(
        MODULE,
        "_just_recipe_body",
        lambda root, recipe: (
            "python3 -m pytest tools/ci/tests/test_write_proof_manifest.py -q"
            if recipe == "proof-p12a-proof-infrastructure"
            else original_body(root, recipe)
        ),
    )
    messages = _messages(MODULE.check_registry(registry, root=REPO_ROOT, path=REGISTRY_PATH))
    assert any("does not execute Python targets" in message for message in messages)

    monkeypatch.setattr(
        MODULE,
        "_just_recipe_body",
        lambda root, recipe: (
            "python3 -m pytest tools/ci/tests/test_write_proof_aggregate.py -k selected -q"
            if recipe == "proof-p12a-proof-infrastructure"
            else original_body(root, recipe)
        ),
    )
    messages = _messages(MODULE.check_registry(registry, root=REPO_ROOT, path=REGISTRY_PATH))
    assert any("does not execute Python targets" in message for message in messages)


def test_p12a_refuses_unneeded_pair_binding() -> None:
    registry = MODULE._read_toml(REGISTRY_PATH)
    proof = next(
        proof for proof in registry["proofs"] if proof["id"] == "p12a-proof-infrastructure"
    )
    proof["source_binding"] = "exact-pair"
    proof["paired_repository"] = MODULE.PAIRED_REPOSITORY
    proof["paired_dependency_lock"] = MODULE.PAIRED_DEPENDENCY_LOCK
    messages = _messages(MODULE.check_registry(registry, root=REPO_ROOT, path=REGISTRY_PATH))
    assert any("P12A requires exact source binding" in message for message in messages)


def test_manual_paired_checkout_is_outside_primary_source() -> None:
    result = subprocess.run(
        [
            "git",
            "-C",
            str(REPO_ROOT),
            "check-ignore",
            "-q",
            ".proof-pairs/semantica-codegraph-v2/Cargo.lock",
        ],
        check=False,
    )
    assert result.returncode == 0


@pytest.mark.parametrize(
    "prefix",
    ("python3 -m pytest", "python3 tools/ci/proof_execution_result.py run-p12a"),
)
@pytest.mark.parametrize(
    "option",
    ("--lf", "--last-failed", "--stepwise", "-kselected", "--ignore-glob=*.py"),
)
def test_python_proof_recipe_refuses_selection_narrowing(prefix: str, option: str) -> None:
    path = "tools/ci/tests/test_write_proof_manifest.py"
    valid = f"{prefix} {path}" + (" -q" if prefix == "python3 -m pytest" else "")
    assert MODULE._recipe_selects_python_target(valid, path)
    assert not MODULE._recipe_selects_python_target(f"{valid} {option}", path)
    if prefix != "python3 -m pytest":
        assert not MODULE._recipe_selects_python_target(f"{valid} -q", path)


def test_python_target_requires_exact_junit_module_path() -> None:
    path = "tools/ci/tests/test_proof_execution_result.py"
    module = "tools.ci.tests.test_proof_execution_result"
    assert MODULE._python_target_has_passed_case(path, {("pytest-junit", f"{module}.test_case")})
    for case in (
        "unrelated.test_proof_execution_result.test_case",
        f"other.{module}.test_case",
        f"{module}_copy.test_case",
        f"{module}",
    ):
        assert not MODULE._python_target_has_passed_case(path, {("pytest-junit", case)})
    assert not MODULE._python_target_has_passed_case(
        path, {("nextest-jsonl", f"{module}.test_case")}
    )


def test_p12a_registry_refuses_last_failed_recipe(monkeypatch) -> None:
    registry = MODULE._read_toml(REGISTRY_PATH)
    authority = MODULE._read_toml(REPO_ROOT / "tools/ci/test-authority.toml")
    paths = {item["id"]: item["path"] for item in authority["python_targets"]}
    selectors = " ".join(paths[target] for target in MODULE.EXPECTED_P12A_TEST_TARGETS)
    original_body = MODULE._just_recipe_body
    monkeypatch.setattr(
        MODULE,
        "_just_recipe_body",
        lambda root, recipe: (
            f"python3 tools/ci/proof_execution_result.py run-p12a {selectors} --lf"
            if recipe == "proof-p12a-proof-infrastructure"
            else original_body(root, recipe)
        ),
    )
    messages = _messages(MODULE.check_registry(registry, root=REPO_ROOT, path=REGISTRY_PATH))
    assert any("does not execute Python targets" in message for message in messages)


def test_python_proof_recipes_refuse_addopts_before_running() -> None:
    for recipe in ("proof-p00-authority-freeze", "proof-p12a-proof-infrastructure"):
        body = MODULE._just_recipe_body(REPO_ROOT, recipe)
        assert body is not None
        assert body.splitlines()[0].startswith('@test -z "${PYTEST_ADDOPTS:-}"')


def test_aggregate_refuses_source_and_release_daemon_identity_drift(tmp_path: Path) -> None:
    proof_a = _proof(tmp_path)
    proof_a["id"] = "proof-a"
    proof_a["binary_binding"] = "release-daemon"
    proof_b = copy.deepcopy(proof_a)
    proof_b["id"] = "proof-b"
    payload_a = _manifest(tmp_path / "a", proof_a)
    payload_b = copy.deepcopy(payload_a)
    payload_b["proof_id"] = "proof-b"
    payload_b["source"]["head"] = "b" * 40
    payload_b["daemon_binary"] = {
        "source_path": "bin/other",
        "path": "artifacts/proof-authority/binaries/" + "2" * 64,
        "sha256": "2" * 64,
    }
    messages = _messages(
        MODULE.check_aggregate(
            {"proof-a": payload_a, "proof-b": payload_b},
            proof_by_id={"proof-a": proof_a, "proof-b": proof_b},
            path=tmp_path / "registry.toml",
        )
    )
    assert "aggregate manifests do not share one primary source identity" in messages
    assert "aggregate release-daemon proofs do not share one daemon path and digest" in messages


def test_aggregate_refuses_primary_source_drift_from_exact_pair(tmp_path: Path) -> None:
    proof_a = _proof(tmp_path)
    proof_a["id"] = "proof-a"
    proof_b = copy.deepcopy(proof_a)
    proof_b["id"] = "proof-b"
    proof_b["source_binding"] = "exact-pair"
    payload_a = _manifest(tmp_path / "a", proof_a)
    payload_b = copy.deepcopy(payload_a)
    payload_b["proof_id"] = "proof-b"
    payload_b["source"]["head"] = "b" * 40
    payload_b["source_pair"] = {
        "repository": MODULE.PAIRED_REPOSITORY,
        "remote_identity_digest": "sha256:" + "1" * 64,
        "source": copy.deepcopy(payload_a["source"]),
        "dependency_lock": {"path": "Cargo.lock", "sha256": "2" * 64},
    }
    messages = _messages(
        MODULE.check_aggregate(
            {"proof-a": payload_a, "proof-b": payload_b},
            proof_by_id={"proof-a": proof_a, "proof-b": proof_b},
            path=tmp_path / "registry.toml",
        )
    )
    assert "aggregate manifests do not share one primary source identity" in messages


def test_exact_pair_live_binding_and_nested_checkout_exclusion(tmp_path: Path) -> None:
    root = tmp_path / "primary"
    root.mkdir()
    _init_repo(root)
    paired = root / ".proof-pairs/semantica-codegraph-v2"
    paired.mkdir(parents=True)
    _init_repo(paired)
    resolver_lock = paired / MODULE.PAIRED_DEPENDENCY_LOCK
    resolver_lock.parent.mkdir(parents=True)
    resolver_lock.write_bytes(b"lock-v1\n")
    subprocess.run(["git", "-C", str(paired), "add", MODULE.PAIRED_DEPENDENCY_LOCK], check=True)
    subprocess.run(["git", "-C", str(paired), "commit", "-qm", "lock"], check=True)
    subprocess.run(
        [
            "git",
            "-C",
            str(paired),
            "remote",
            "add",
            "origin",
            "git@github-personal:josongmin/semantica-codegraph-v2.git",
        ],
        check=True,
    )

    proof = _proof(root)
    proof.update(
        {
            "source_binding": "exact-pair",
            "paired_repository": MODULE.PAIRED_REPOSITORY,
            "paired_dependency_lock": MODULE.PAIRED_DEPENDENCY_LOCK,
        }
    )
    payload = _manifest(root, proof)
    payload["source_pair"] = MODULE.paired_source_snapshot(
        paired,
        repository=proof["paired_repository"],
        dependency_lock=Path(proof["paired_dependency_lock"]),
    )
    manifest_path = root / proof["artifact"]
    payload["source"] = MODULE.proof_source_snapshot(
        root,
        manifest_path=manifest_path,
        proof=proof,
        excluded_paths=[paired],
    )
    schema = json.loads(SCHEMA_PATH.read_text(encoding="utf-8"))
    assert (
        MODULE.check_manifest(
            payload,
            manifest_path=manifest_path,
            proof=proof,
            schema=schema,
            root=root,
            bind_source=True,
            paired_checkouts={proof["paired_repository"]: paired},
        )
        == []
    )
    payload["source_pair"]["source"].update(
        branch="producer-branch",
        upstream="origin/producer-branch",
        merge_base="a" * 40,
    )
    assert (
        MODULE.check_manifest(
            payload,
            manifest_path=manifest_path,
            proof=proof,
            schema=schema,
            root=root,
            bind_source=True,
            paired_checkouts={proof["paired_repository"]: paired},
        )
        == []
    )

    resolver_lock.write_bytes(b"lock-v2\n")
    messages = _messages(
        MODULE.check_manifest(
            payload,
            manifest_path=manifest_path,
            proof=proof,
            schema=schema,
            root=root,
            bind_source=True,
            paired_checkouts={proof["paired_repository"]: paired},
        )
    )
    assert "source_pair is not current paired source" in messages


def test_paired_remote_identity_is_transport_independent(tmp_path: Path) -> None:
    paired = tmp_path / "semantica-codegraph-v2"
    paired.mkdir()
    _init_repo(paired)
    resolver_lock = paired / MODULE.PAIRED_DEPENDENCY_LOCK
    resolver_lock.parent.mkdir(parents=True)
    resolver_lock.write_bytes(b"lock-v1\n")
    subprocess.run(["git", "-C", str(paired), "add", MODULE.PAIRED_DEPENDENCY_LOCK], check=True)
    subprocess.run(["git", "-C", str(paired), "commit", "-qm", "lock"], check=True)
    subprocess.run(
        [
            "git",
            "-C",
            str(paired),
            "remote",
            "add",
            "origin",
            "git@github-personal:josongmin/semantica-codegraph-v2.git",
        ],
        check=True,
    )
    ssh_snapshot = MODULE.paired_source_snapshot(
        paired,
        repository=MODULE.PAIRED_REPOSITORY,
        dependency_lock=Path(MODULE.PAIRED_DEPENDENCY_LOCK),
    )
    subprocess.run(
        [
            "git",
            "-C",
            str(paired),
            "remote",
            "set-url",
            "origin",
            "https://github.com/josongmin/semantica-codegraph-v2.git",
        ],
        check=True,
    )
    https_snapshot = MODULE.paired_source_snapshot(
        paired,
        repository=MODULE.PAIRED_REPOSITORY,
        dependency_lock=Path(MODULE.PAIRED_DEPENDENCY_LOCK),
    )

    assert https_snapshot == ssh_snapshot


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


def test_require_all_refuses_absent_manifests(tmp_path: Path, capsys) -> None:
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
    stderr = capsys.readouterr().err
    assert "required proof 'p03-candidate-activation' is staged" in stderr
    assert "p03-candidate-activation.json: required proof manifest is missing" not in stderr
