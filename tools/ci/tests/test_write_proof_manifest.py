"""Owner-local tests for the registry-driven ProofManifestV1 writer."""

from __future__ import annotations

import importlib.util
import json
import platform
import shutil
import subprocess
import sys
from pathlib import Path

import jsonschema
import pytest

REPO_ROOT = Path(__file__).resolve().parents[3]
WRITER_PATH = REPO_ROOT / "tools/ci/write-proof-manifest.py"
CHECKER_PATH = REPO_ROOT / "tools/ci/lint/check-proof-authority.py"
REGISTRY_PATH = REPO_ROOT / "tools/ci/proof-authority.toml"
SCHEMA_PATH = REPO_ROOT / "tools/ci/proof-manifest.schema.json"
AGGREGATE_SCHEMA_PATH = REPO_ROOT / "tools/ci/proof-aggregate.schema.json"
ERROR_INVENTORY_WRITER_PATH = REPO_ROOT / "tools/ci/write-error-authority-inventory.py"


def _load_module(name: str, path: Path):
    spec = importlib.util.spec_from_file_location(name, path)
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


WRITER = _load_module("write_proof_manifest", WRITER_PATH)
CHECKER = _load_module("write_proof_manifest_checker", CHECKER_PATH)
ERROR_INVENTORY_WRITER = _load_module(
    "write_proof_manifest_error_inventory", ERROR_INVENTORY_WRITER_PATH
)


def _run(root: Path, *args: str) -> None:
    subprocess.run([*args], cwd=root, check=True, capture_output=True, text=True)


def _fixture_root(tmp_path: Path) -> tuple[Path, dict]:
    root = tmp_path / "repo"
    root.mkdir()
    registry = CHECKER._read_toml(REGISTRY_PATH)
    registry_path = root / "tools/ci/proof-authority.toml"
    registry_path.parent.mkdir(parents=True)
    shutil.copyfile(REGISTRY_PATH, registry_path)
    shutil.copyfile(SCHEMA_PATH, root / "tools/ci/proof-manifest.schema.json")
    shutil.copyfile(AGGREGATE_SCHEMA_PATH, root / "tools/ci/proof-aggregate.schema.json")

    target_ids = sorted(
        {target for proof in registry["proofs"] for target in proof["test_authority_targets"]}
    )
    test_authority = root / "tools/ci/test-authority.toml"
    test_authority.write_text(
        '[local_scopes.integration-fast]\ntargets = ["catalog-idempotency"]\n\n'
        + "".join(f'[[integration_targets]]\nid = "{target}"\n' for target in target_ids),
        encoding="utf-8",
    )
    for proof in registry["proofs"]:
        owner = root / proof["owner"]
        owner.parent.mkdir(parents=True, exist_ok=True)
        owner.touch(exist_ok=True)

    _run(root, "git", "init", "-q")
    _run(root, "git", "config", "user.name", "Proof Fixture")
    _run(root, "git", "config", "user.email", "proof@example.invalid")
    _run(root, "git", "add", ".")
    _run(root, "git", "commit", "-qm", "fixture")
    return root, registry


def _terminal(root: Path) -> tuple[Path, dict]:
    proof_dir = root / "artifacts/proof-authority/raw"
    proof_dir.mkdir(parents=True, exist_ok=True)
    evidence = proof_dir / "p00.log"
    evidence.write_text("proof authority: passed\n", encoding="utf-8")
    inventory = root / "artifacts/sep-21/p00/error-authority-inventory.json"
    inventory.parent.mkdir(parents=True, exist_ok=True)
    inventory.write_text(
        json.dumps(ERROR_INVENTORY_WRITER.build_inventory(root), sort_keys=True, indent=2) + "\n",
        encoding="utf-8",
    )
    terminal = {
        "status": "passed",
        "counts": {"selected": 1, "executed": 1, "passed": 1, "failed": 0, "ignored": 0},
        "environment": {
            "toolchain": "python fixture",
            "features": [],
            "os": platform.system().lower(),
            "arch": platform.machine(),
            "host": {
                "profile": "fixture",
                "cpu_count": 1,
                "memory_bytes": 1024,
                "identity": "fixture-host",
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
        "artifacts": [
            "artifacts/proof-authority/raw/p00.log",
            "artifacts/sep-21/p00/error-authority-inventory.json",
        ],
    }
    terminal_path = proof_dir / "terminal.json"
    terminal_path.write_text(json.dumps(terminal), encoding="utf-8")
    return terminal_path, terminal


def _publish(root: Path, terminal_path: Path) -> tuple[Path, str, str]:
    return WRITER.publish_manifest(
        root=root,
        registry_path=root / "tools/ci/proof-authority.toml",
        schema_path=root / "tools/ci/proof-manifest.schema.json",
        proof_id="p00-authority-freeze",
        terminal_input_path=terminal_path,
    )


def _paired_checkout(parent: Path) -> Path:
    checkout = parent / "arbitrary-local-directory"
    checkout.mkdir()
    (checkout / "Cargo.lock").write_text("version = 4\n", encoding="utf-8")
    _run(checkout, "git", "init", "-q")
    _run(checkout, "git", "config", "user.name", "Pair Fixture")
    _run(checkout, "git", "config", "user.email", "pair@example.invalid")
    _run(
        checkout,
        "git",
        "remote",
        "add",
        "origin",
        "git@github-personal:josongmin/semantica-codegraph-v2.git",
    )
    _run(checkout, "git", "add", "Cargo.lock")
    _run(checkout, "git", "commit", "-qm", "fixture")
    return checkout


def test_writer_resolves_registry_source_and_null_binary_then_semantically_validates(
    tmp_path: Path,
) -> None:
    root, registry = _fixture_root(tmp_path)
    terminal_path, _ = _terminal(root)

    output, digest, status = _publish(root, terminal_path)

    payload = json.loads(output.read_text(encoding="utf-8"))
    proof = next(proof for proof in registry["proofs"] if proof["id"] == payload["proof_id"])
    schema = json.loads((root / "tools/ci/proof-manifest.schema.json").read_text())
    assert payload["invocation"] == {
        "command": proof["command"],
        "profile": proof["profile"],
        "target": proof["target"],
        "filter": proof["filter"],
    }
    assert payload["source"]["upstream"] is None
    assert payload["source"]["merge_base"] is None
    assert payload["daemon_binary"] is None
    assert digest == WRITER._sha256(output)
    assert status == "passed"
    assert (
        CHECKER.check_manifest(
            payload,
            manifest_path=output,
            proof=proof,
            schema=schema,
            root=root,
            bind_source=True,
        )
        == []
    )


def test_p00_manifest_refuses_missing_error_inventory_attestation(tmp_path: Path) -> None:
    root, _ = _fixture_root(tmp_path)
    terminal_path, terminal = _terminal(root)
    terminal["artifacts"] = ["artifacts/proof-authority/raw/p00.log"]
    terminal_path.write_text(json.dumps(terminal), encoding="utf-8")

    with pytest.raises(WRITER.ManifestRefused, match="must attest.*error-authority inventory"):
        _publish(root, terminal_path)


@pytest.mark.parametrize("terminal_status", ["failed", "blocked", "not_run"])
def test_non_passed_terminal_state_is_preserved_but_cli_returns_nonzero(
    tmp_path: Path,
    terminal_status: str,
) -> None:
    root, _ = _fixture_root(tmp_path)
    terminal_path, terminal = _terminal(root)
    output = root / "artifacts/proof-authority/p00-authority-freeze.json"
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_bytes(b"existing-authoritative-receipt\n")
    terminal["status"] = terminal_status
    terminal["counts"] = (
        {"selected": 1, "executed": 1, "passed": 0, "failed": 1, "ignored": 0}
        if terminal_status == "failed"
        else {"selected": 0, "executed": 0, "passed": 0, "failed": 0, "ignored": 0}
    )
    terminal_path.write_text(json.dumps(terminal), encoding="utf-8")

    exit_code = WRITER.main(
        [
            "--root",
            str(root),
            "--registry",
            str(root / "tools/ci/proof-authority.toml"),
            "--schema",
            str(root / "tools/ci/proof-manifest.schema.json"),
            "--proof-id",
            "p00-authority-freeze",
            "--terminal-input",
            str(terminal_path),
        ]
    )

    assert exit_code == 1
    assert json.loads(output.read_text(encoding="utf-8"))["status"] == terminal_status


def test_unknown_terminal_state_is_refused_without_replacing_receipt(tmp_path: Path) -> None:
    root, _ = _fixture_root(tmp_path)
    terminal_path, terminal = _terminal(root)
    output = root / "artifacts/proof-authority/p00-authority-freeze.json"
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_bytes(b"existing-authoritative-receipt\n")
    terminal["status"] = "NOT_RUN"
    terminal_path.write_text(json.dumps(terminal), encoding="utf-8")

    with pytest.raises(WRITER.ManifestRefused, match="terminal status is not registered"):
        _publish(root, terminal_path)

    assert output.read_bytes() == b"existing-authoritative-receipt\n"


def test_free_form_invocation_is_refused_instead_of_overriding_registry(tmp_path: Path) -> None:
    root, _ = _fixture_root(tmp_path)
    terminal_path, terminal = _terminal(root)
    terminal["invocation"] = {"command": "printf fake-green"}
    terminal_path.write_text(json.dumps(terminal), encoding="utf-8")

    with pytest.raises(WRITER.ManifestRefused, match=r"extra=\['invocation'\]"):
        _publish(root, terminal_path)

    assert not (root / "artifacts/proof-authority/p00-authority-freeze.json").exists()


def test_semantic_failure_is_validated_before_atomic_replace(tmp_path: Path) -> None:
    root, _ = _fixture_root(tmp_path)
    terminal_path, terminal = _terminal(root)
    output = root / "artifacts/proof-authority/p00-authority-freeze.json"
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_bytes(b"existing-authoritative-receipt\n")
    terminal["counts"] = {
        "selected": 0,
        "executed": 0,
        "passed": 0,
        "failed": 0,
        "ignored": 0,
    }
    terminal_path.write_text(json.dumps(terminal), encoding="utf-8")

    with pytest.raises(WRITER.ManifestRefused, match="semantic manifest validation failed"):
        _publish(root, terminal_path)

    assert output.read_bytes() == b"existing-authoritative-receipt\n"
    assert list(output.parent.glob(f".{output.name}.*.tmp")) == []


def test_binary_binding_is_derived_and_none_rejects_a_daemon_path(tmp_path: Path) -> None:
    root, registry = _fixture_root(tmp_path)
    daemon = root / "bin/searchd"
    daemon.parent.mkdir()
    daemon.write_bytes(b"release-daemon")
    proof_none = next(proof for proof in registry["proofs"] if proof["binary_binding"] == "none")
    proof_release = next(
        proof for proof in registry["proofs"] if proof["binary_binding"] == "release-daemon"
    )

    with pytest.raises(WRITER.ManifestRefused, match="requires terminal.daemon_binary=null"):
        WRITER._resolve_binary(root, proof_none, "bin/searchd")
    assert WRITER._resolve_binary(root, proof_release, "bin/searchd") == {
        "path": "bin/searchd",
        "sha256": WRITER._sha256(daemon),
    }


def test_exact_pair_is_derived_from_live_external_checkout_without_persisting_path(
    tmp_path: Path,
) -> None:
    checkout = _paired_checkout(tmp_path)
    proof = {
        "source_binding": "exact-pair",
        "paired_repository": "github:josongmin/semantica-codegraph-v2",
        "paired_dependency_lock": "Cargo.lock",
    }

    source_pair = WRITER._resolve_source_pair(proof, str(checkout), checker=CHECKER)

    assert source_pair["repository"] == "github:josongmin/semantica-codegraph-v2"
    assert (
        source_pair["source"]["head"]
        == subprocess.check_output(
            ["git", "-C", str(checkout), "rev-parse", "HEAD"],
            text=True,
        ).strip()
    )
    assert source_pair["dependency_lock"] == {
        "path": "Cargo.lock",
        "sha256": WRITER._sha256(checkout / "Cargo.lock"),
    }
    assert str(checkout) not in json.dumps(source_pair, sort_keys=True)
    schema = json.loads(SCHEMA_PATH.read_text(encoding="utf-8"))
    jsonschema.Draft202012Validator(schema["properties"]["source_pair"]).validate(source_pair)


def test_exact_pair_manifest_is_live_bound_through_atomic_writer(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    root, registry = _fixture_root(tmp_path)
    registry_path = root / "tools/ci/proof-authority.toml"
    registry_text = (
        registry_path.read_text(encoding="utf-8")
        .replace(
            'id = "p11-cross-repo-cutover"\nauthority_state = "staged"\nexecution_mode = "test-authority"\nstaged_reason = "P11 must land the exact-pair producer protocol and cross-repository terminal receipt rail."',
            'id = "p11-cross-repo-cutover"\nauthority_state = "executable"\nexecution_mode = "test-authority"',
        )
        .replace(
            'target = "semantica-terminal-receipt"\nfilter = "none"\nsource_binding = "exact-pair"',
            'target = "semantica-terminal-receipt"\nfilter = "none"\nsource_binding = "exact-pair"',
        )
        .replace(
            'test_authority_targets = []\ndependencies = ["p03-candidate-activation", "p02b-operation-journal", "p06-sdk-binding", "p10-state-migration"]',
            'test_authority_targets = ["catalog-idempotency"]\ntest_authority_scopes = ["integration-fast"]\ndependencies = ["p03-candidate-activation", "p02b-operation-journal", "p06-sdk-binding", "p10-state-migration"]',
        )
        .replace(
            'command = "just rust-verify-hellgate-cross-repo"',
            'command = "just proof-p11-cross-repo-cutover"',
        )
    )
    registry_path.write_text(registry_text, encoding="utf-8")
    (root / "Justfile").write_text(
        "proof-p11-cross-repo-cutover:\n    @just rust-profile test-integration-fast\n",
        encoding="utf-8",
    )
    registry = CHECKER._read_toml(registry_path)
    checkout = _paired_checkout(tmp_path)
    terminal_path, terminal = _terminal(root)
    proof = next(proof for proof in registry["proofs"] if proof["id"] == "p11-cross-repo-cutover")
    daemon = root / "bin/searchd"
    daemon.parent.mkdir()
    daemon.write_bytes(b"release-daemon")
    for dependency_id in proof["dependencies"]:
        dependency = next(item for item in registry["proofs"] if item["id"] == dependency_id)
        dependency_path = root / dependency["artifact"]
        dependency_path.parent.mkdir(parents=True, exist_ok=True)
        dependency_path.write_text(f'{{"proof_id":"{dependency_id}"}}\n', encoding="utf-8")
    terminal["environment"]["host"]["profile"] = "linux-production-like"
    terminal["environment"]["os"] = "linux"
    terminal["daemon_binary"] = "bin/searchd"
    terminal_path.write_text(json.dumps(terminal), encoding="utf-8")
    monkeypatch.setattr(WRITER.platform, "system", lambda: "Linux")

    output, _, status = WRITER.publish_manifest(
        root=root,
        registry_path=root / "tools/ci/proof-authority.toml",
        schema_path=root / "tools/ci/proof-manifest.schema.json",
        proof_id=proof["id"],
        terminal_input_path=terminal_path,
        paired_checkout=checkout,
    )

    payload = json.loads(output.read_text(encoding="utf-8"))
    schema = json.loads((root / "tools/ci/proof-manifest.schema.json").read_text())
    assert status == "passed"
    assert payload["source_pair"]["repository"] == proof["paired_repository"]
    assert (
        CHECKER.check_manifest(
            payload,
            manifest_path=output,
            proof=proof,
            schema=schema,
            root=root,
            bind_source=True,
            paired_checkouts={proof["paired_repository"]: checkout},
        )
        == []
    )


def test_exact_binding_refuses_a_free_paired_checkout(tmp_path: Path) -> None:
    proof = {"source_binding": "exact"}
    with pytest.raises(WRITER.ManifestRefused, match="forbids --paired-checkout"):
        WRITER._resolve_source_pair(proof, str(tmp_path), checker=CHECKER)


def test_linux_production_profile_cannot_be_spoofed_on_non_linux_host(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    root, registry = _fixture_root(tmp_path)
    _, terminal = _terminal(root)
    proof = dict(registry["proofs"][0])
    proof["required_host"] = "linux-production-like"
    terminal["environment"]["os"] = "darwin"
    terminal["environment"]["host"]["profile"] = "linux-production-like"
    monkeypatch.setattr(WRITER.platform, "system", lambda: "Darwin")

    with pytest.raises(WRITER.ManifestRefused, match="cannot be published from a non-Linux"):
        WRITER.build_manifest(
            root=root,
            proof=proof,
            proof_by_id={proof["id"]: proof},
            terminal=terminal,
            checker=CHECKER,
            paired_checkout=None,
        )


def test_input_digests_are_computed_from_file_or_literal_identity(tmp_path: Path) -> None:
    root = tmp_path.resolve()
    fixture = root / "fixture.json"
    fixture.write_bytes(b'{"fixture":true}\n')

    assert WRITER._digest_input(root, {"path": "fixture.json"}, label="fixture") == (
        f"sha256:{WRITER._sha256(fixture)}"
    )
    assert WRITER._digest_input(root, {"value": "model-v1"}, label="model") == (
        "sha256:1a1f4502024df8a68d12e64bb2364ad6308d04ed0a7d5e8300a676ec70867140"
    )
    with pytest.raises(WRITER.ManifestRefused, match="valid path or non-empty value"):
        WRITER._digest_input(root, {"sha256": "sha256:" + "0" * 64}, label="provider")


def test_schema_allows_upstreamless_or_detached_source_but_keeps_binary_field() -> None:
    schema = json.loads(SCHEMA_PATH.read_text(encoding="utf-8"))
    source = {
        "head": "a" * 40,
        "dirty_digest": "sha256:" + "b" * 64,
        "branch": None,
        "upstream": None,
        "merge_base": None,
    }
    jsonschema.Draft202012Validator(schema).validate(
        {
            "schema_version": 1,
            "proof_id": "fixture-proof",
            "family": "S",
            "status": "passed",
            "source": source,
            "source_pair": None,
            "invocation": {
                "command": "check",
                "profile": None,
                "target": None,
                "filter": None,
            },
            "counts": {
                "selected": 1,
                "executed": 1,
                "passed": 1,
                "failed": 0,
                "ignored": 0,
            },
            "environment": {
                "toolchain": "fixture",
                "features": [],
                "os": "linux",
                "arch": "x86_64",
                "host": {
                    "profile": "fixture",
                    "cpu_count": 1,
                    "memory_bytes": 1,
                    "identity_digest": "sha256:" + "c" * 64,
                },
            },
            "daemon_binary": None,
            "state_root_format": "not-applicable",
            "inputs": {name: None for name in WRITER.INPUT_NAMES},
            "started_at": "2026-09-21T00:00:00Z",
            "ended_at": "2026-09-21T00:00:01Z",
            "dependency_receipts": [],
            "artifacts": [{"path": "raw.log", "sha256": "d" * 64}],
        }
    )
