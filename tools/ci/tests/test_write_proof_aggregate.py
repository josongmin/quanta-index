"""Owner-local tests for the registry-derived P12 aggregate writer."""

from __future__ import annotations

import errno
import hashlib
import importlib.util
import json
import os
import re
import shutil
import subprocess
import sys
from copy import deepcopy
from dataclasses import dataclass
from datetime import datetime, timedelta, timezone
from pathlib import Path

import pytest

from tools.ci.daemon_execution_evidence import (
    NATIVE_RELEASE_SCOPES,
    P07_PROOF,
    P10_PROOF,
    daemon_witness_specs,
)
from tools.ci.rust_test_mount import mounted_test_module
from tools.ci.tests.p11_test_issuer import (
    bind_run,
    keypair,
    nextest_bytes,
    paired_evidence,
    pytest_bytes,
)

REPO_ROOT = Path(__file__).resolve().parents[3]
WRITER_PATH = REPO_ROOT / "tools/ci/write-proof-aggregate.py"
CHECKER_PATH = REPO_ROOT / "tools/ci/lint/check-proof-authority.py"
REGISTRY_PATH = REPO_ROOT / "tools/ci/proof-authority.toml"
MANIFEST_SCHEMA_PATH = REPO_ROOT / "tools/ci/proof-manifest.schema.json"
AGGREGATE_SCHEMA_PATH = REPO_ROOT / "tools/ci/proof-aggregate.schema.json"
INVENTORY_WRITER_PATH = REPO_ROOT / "tools/ci/write-error-authority-inventory.py"
NATIVE_CATALOG_PROOF_SCOPES = {
    "p02b-operation-journal": ("p02b-operation-journal", "p02b-native-journal-lib"),
    "p03-candidate-activation-owner": ("candidate-activation-owner",),
    "p04-read-view-lifetime-owner": ("read-view-lifetime-owner",),
}


def _load_module(name: str, path: Path):
    spec = importlib.util.spec_from_file_location(name, path)
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


WRITER = _load_module("write_proof_aggregate", WRITER_PATH)
CHECKER = _load_module("write_proof_aggregate_checker", CHECKER_PATH)
INVENTORY_WRITER = _load_module("aggregate_test_inventory_writer", INVENTORY_WRITER_PATH)


@pytest.mark.parametrize(
    "raw",
    [
        b'{"status":"failed","status":"passed"}',
        b'{"status":"passed","counts":{"failed":1,"failed":0}}',
        b'{"status":"passed","value":1e9999}',
    ],
)
def test_aggregate_json_reader_refuses_ambiguous_bytes(
    tmp_path: Path, monkeypatch, raw: bytes
) -> None:
    # Isolate byte admission, not qualification: downstream validation must
    # never be reached for ambiguous input. A valid JSON control reaches it.
    observed = []
    monkeypatch.setattr(CHECKER, "_cached_proof_source_snapshot", lambda *args, **kwargs: {})
    monkeypatch.setattr(
        CHECKER, "check_manifest", lambda payload, **kwargs: observed.append(payload) or []
    )
    (tmp_path / "schema.json").write_bytes(b"{}")
    kwargs = dict(
        root=tmp_path,
        proof={
            "authority_state": "executable",
            "artifact": "proof.json",
            "artifact_schema": "schema.json",
        },
        proof_by_id={},
        checker=CHECKER,
        paired_checkouts={},
        source_cache={},
        pair_cache={},
    )
    path = tmp_path / "proof.json"
    path.write_bytes(b'{"status":"passed"}')
    assert WRITER._manifest_status(**kwargs)[0] == "PASSED"
    assert observed == [{"status": "passed"}]
    observed.clear()
    path.write_bytes(raw)
    status, digest, payload = WRITER._manifest_status(**kwargs)
    assert (status, digest, payload) == ("FAILED", None, None)
    assert observed == []


RELEASE_HOST_INPUT = {
    "profile": "linux-production-like",
    "cpu_count": 1,
    "memory_bytes": 1024,
    "identity": "fixture-host",
}
RELEASE_HOST_DIGEST = (
    "sha256:"
    + hashlib.sha256(
        json.dumps(
            {
                "arch": "x86_64",
                "cpu_count": 1,
                "identity": "fixture-host",
                "memory_bytes": 1024,
                "os": "linux",
                "profile": "linux-production-like",
            },
            sort_keys=True,
            separators=(",", ":"),
        ).encode()
    ).hexdigest()
)
PAIR_PROOF = "p11-cross-repo-cutover"
P10_LIB_SCOPE = "state-migration-owner-lib"
P12A_PROOF = "p12a-proof-infrastructure"


@dataclass(frozen=True)
class AggregateTemplates:
    staged_root: Path
    executable_root: Path
    paired_checkout: Path


def _run(root: Path, *args: str) -> None:
    subprocess.run([*args], cwd=root, check=True, capture_output=True, text=True)


def _digest(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def _make_all_proofs_executable(text: str) -> str:
    text = re.sub(
        r'authority_state = "staged"\nexecution_mode = "(test-authority|operational-action)"\n'
        r'staged_reason = "[^"]*"',
        r'authority_state = "executable"\nexecution_mode = "\1"',
        text,
    )
    sections = text.split("[[proofs]]")
    for index in range(1, len(sections)):
        if 'execution_mode = "test-authority"' in sections[index]:
            proof_id = re.search(r'^\nid = "([^"]+)"', sections[index])
            assert proof_id is not None
            if proof_id.group(1) == PAIR_PROOF:
                sections[index] += f'\nattestation_public_key = "{keypair()[0]}"\n'
                continue
            if proof_id.group(1) == P07_PROOF:
                sections[index] += f'\nprovider_approval_sha256 = "{"a" * 64}"\n'
            if proof_id.group(1) == P12A_PROOF or proof_id.group(1) in NATIVE_RELEASE_SCOPES.values():
                continue
            if proof_id.group(1) in NATIVE_CATALOG_PROOF_SCOPES:
                continue
            sections[index] = re.sub(
                r'command = "[^"]+"',
                f'command = "just proof-fixture-{proof_id.group(1)}"',
                sections[index],
                count=1,
            )
            sections[index] = sections[index].replace(
                "test_authority_targets = []",
                "test_authority_targets = " + json.dumps(["catalog-idempotency"]),
            )
            scopes = ["fixture-all"]
            if proof_id.group(1) == "p10-state-migration-owner":
                scopes.append(P10_LIB_SCOPE)
            scope_line = f"test_authority_scopes = {json.dumps(scopes)}"
            if "test_authority_scopes = " in sections[index]:
                sections[index] = re.sub(
                    r"test_authority_scopes = \[[^\]]*\]",
                    scope_line,
                    sections[index],
                    count=1,
                )
            else:
                sections[index] = sections[index].replace(
                    "test_authority_targets = ",
                    f"{scope_line}\ntest_authority_targets = ",
                    1,
                )
        elif 'execution_mode = "operational-action"' in sections[index]:
            proof_id = re.search(r'^\nid = "([^"]+)"', sections[index])
            assert proof_id is not None
            sections[index] += (
                f'\nattestation_public_key = "{keypair()[0]}"\n'
                f'operational_contract = "tools/ci/fixtures/{proof_id.group(1)}.json"\n'
            )
    return "[[proofs]]".join(sections)


def _build_paired_checkout(tmp_path: Path) -> Path:
    checkout = tmp_path / "semantica"
    checkout.mkdir(parents=True)
    (checkout / "Cargo.lock").write_text("version = 4\n", encoding="utf-8")
    resolver_lock = checkout / CHECKER.PAIRED_DEPENDENCY_LOCK
    resolver_lock.parent.mkdir(parents=True)
    resolver_lock.write_text("version = 4\n# active resolver lock\n", encoding="utf-8")
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
    _run(checkout, "git", "add", "Cargo.lock", CHECKER.PAIRED_DEPENDENCY_LOCK)
    _run(checkout, "git", "commit", "-qm", "fixture")
    return checkout


def _build_fixture_root(tmp_path: Path, *, executable: bool) -> tuple[Path, dict]:
    root = tmp_path / "repo"
    root.mkdir(parents=True)
    (root / "Cargo.lock").write_text("version = 4\n", encoding="utf-8")
    format_source = root / "crates/quanta-index-contract/src/index.rs"
    format_source.parent.mkdir(parents=True)
    format_source.write_text("native_index_state_format!(2);\n", encoding="utf-8")
    registry_text = REGISTRY_PATH.read_text(encoding="utf-8")
    if executable:
        registry_text = _make_all_proofs_executable(registry_text)
    registry_path = root / "tools/ci/proof-authority.toml"
    registry_path.parent.mkdir(parents=True)
    registry_path.write_text(registry_text, encoding="utf-8")
    shutil.copyfile(MANIFEST_SCHEMA_PATH, root / "tools/ci/proof-manifest.schema.json")
    shutil.copyfile(REPO_ROOT / "tools/ci/proof_json.py", root / "tools/ci/proof_json.py")
    shutil.copyfile(AGGREGATE_SCHEMA_PATH, root / "tools/ci/proof-aggregate.schema.json")
    shutil.copyfile(INVENTORY_WRITER_PATH, root / "tools/ci/write-error-authority-inventory.py")
    shutil.copyfile(
        REPO_ROOT / "tools/ci/error-authority-inventory.schema.json",
        root / "tools/ci/error-authority-inventory.schema.json",
    )
    registry = CHECKER._read_toml(registry_path)
    if executable:
        source_catalog = CHECKER._read_toml(REPO_ROOT / "tools/ci/test-authority.toml")
        source_targets = {
            row["id"]: row
            for category in ("integration_targets", "python_targets")
            for row in source_catalog.get(category, [])
        }
        python_ids = source_catalog["python_scopes"][P12A_PROOF]["targets"]
        target_ids = sorted(
            {target for proof in registry["proofs"] for target in proof["test_authority_targets"]}
            - set(python_ids)
        )
        mounted = [
            source_targets[t]
            for t in target_ids
            if source_targets[t].get("target") and "tests" in Path(source_targets[t]["path"]).parts
        ]
        for target in mounted:
            (launcher,) = [
                item
                for item in source_targets.values()
                if item.get("owner") == target["owner"]
                and Path(item["path"]).stem == target["target"]
            ]
            if launcher["id"] not in target_ids:
                target_ids.append(launcher["id"])
            (root / launcher["path"]).parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(REPO_ROOT / launcher["path"], root / launcher["path"])
            (root / target["path"]).parent.mkdir(parents=True, exist_ok=True)
            (root / target["path"]).touch()
        (root / "tools/ci/test-authority.toml").write_text(
            "[local_scopes.fixture-all]\n"
            f"targets = {json.dumps(target_ids)}\n"
            + "".join(
                f"[local_scopes.{scope}]\n"
                'targets = ["searchd-runtime-native-product-process"]\n'
                for scope in NATIVE_RELEASE_SCOPES
            )
            + f"[local_scopes.{P10_LIB_SCOPE}]\n"
            "lib = true\n"
            f"packages = {json.dumps(source_catalog['local_scopes'][P10_LIB_SCOPE]['packages'])}\n"
            + "".join(
                f"[local_scopes.{scope}]\n"
                + "".join(
                    f"{key} = {json.dumps(value)}\n"
                    for key, value in source_catalog["local_scopes"][scope].items()
                )
                for scopes in NATIVE_CATALOG_PROOF_SCOPES.values()
                for scope in scopes
            )
            + f"[python_scopes.{P12A_PROOF}]\n"
            f"targets = {json.dumps(python_ids)}\n"
            + "".join(
                "[[integration_targets]]\n"
                f'id = "{target}"\n'
                f'path = "{source_targets[target]["path"]}"\n'
                f'owner = "{source_targets[target]["owner"]}"\n'
                + (
                    f'target = "{source_targets[target]["target"]}"\n'
                    if "target" in source_targets[target]
                    else ""
                )
                for target in target_ids
            )
            + "".join(
                "[[python_targets]]\n"
                f'id = "{target}"\n'
                f'path = "{source_targets[target]["path"]}"\n'
                f'owner = "{source_targets[target]["owner"]}"\n'
                for target in python_ids
            ),
            encoding="utf-8",
        )
        proof_recipes = []
        for proof in registry["proofs"]:
            if (
                proof["authority_state"] != "executable"
                or proof["execution_mode"] != "test-authority"
            ):
                continue
            recipe = proof["command"].removeprefix("just ")
            if proof["id"] == PAIR_PROOF:
                proof_recipes.append(
                    '[positional-arguments]\n'
                    f'{recipe} semantica_root="fixture":\n'
                    '    #!/bin/bash -p\n'
                    '    exec ./scripts/verify-repomap-cross-repo.sh "$1"\n'
                )
                continue
            if proof["id"] == P12A_PROOF:
                selectors = " ".join(source_targets[target]["path"] for target in python_ids)
                proof_recipes.append(
                    f"{recipe}:\n    python3 tools/ci/proof_execution_result.py run-p12a {selectors}\n"
                )
                continue
            commands = "".join(
                f"    @just rust-profile test-{scope}\n" for scope in proof["test_authority_scopes"]
            )
            proof_recipes.append(f"{recipe}:\n{commands}")
        (root / "Justfile").write_text("\n".join(proof_recipes), encoding="utf-8")
    else:
        shutil.copyfile(
            REPO_ROOT / "tools/ci/test-authority.toml", root / "tools/ci/test-authority.toml"
        )
        shutil.copyfile(REPO_ROOT / "Justfile", root / "Justfile")
    for proof in registry["proofs"]:
        owner = root / proof["owner"]
        owner.parent.mkdir(parents=True, exist_ok=True)
        owner.touch(exist_ok=True)
    native_owner = Path("crates/quanta-index-catalog/src/native/authority.rs")
    fixture_native_owner = root / native_owner
    fixture_native_owner.parent.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(REPO_ROOT / native_owner, fixture_native_owner)
    binary = root / "bin/searchd"
    binary.parent.mkdir()
    binary.write_bytes(b"release-daemon")
    if executable:
        from tools.ci.tests.test_proof_operational_result import install_contract

        for proof in registry["proofs"]:
            if proof["execution_mode"] == "operational-action":
                install_contract(root, proof["id"], RELEASE_HOST_DIGEST)

    (root / ".gitignore").write_text("/artifacts/\n__pycache__/\n*.py[cod]\n", encoding="utf-8")
    _run(root, "git", "init", "-q")
    _run(root, "git", "config", "user.name", "Aggregate Fixture")
    _run(root, "git", "config", "user.email", "aggregate@example.invalid")
    _run(root, "git", "add", ".")
    _run(root, "git", "commit", "-qm", "fixture")
    return root, registry


@pytest.fixture(scope="module")
def aggregate_templates(tmp_path_factory: pytest.TempPathFactory) -> AggregateTemplates:
    base = tmp_path_factory.mktemp("proof-aggregate-templates")
    staged_root, _ = _build_fixture_root(base / "staged", executable=False)
    executable_root, _ = _build_fixture_root(base / "executable", executable=True)
    paired_checkout = _build_paired_checkout(base / "paired")
    return AggregateTemplates(
        staged_root=staged_root,
        executable_root=executable_root,
        paired_checkout=paired_checkout,
    )


def _fixture_root(
    tmp_path: Path,
    *,
    executable: bool,
    templates: AggregateTemplates,
) -> tuple[Path, dict]:
    source = templates.executable_root if executable else templates.staged_root
    root = tmp_path / "repo"
    shutil.copytree(source, root)
    registry = CHECKER._read_toml(root / "tools/ci/proof-authority.toml")
    return root, registry


def _paired_checkout(tmp_path: Path, templates: AggregateTemplates) -> Path:
    checkout = tmp_path / "semantica"
    shutil.copytree(templates.paired_checkout, checkout)
    return checkout


def _write_dependency_manifests(
    root: Path,
    registry: dict,
    paired: Path,
    *,
    release_host_digest_overrides: dict[str, str] | None = None,
    release_prerequisite_override: tuple[str, str, str] | None = None,
    wrong_paired_caller: bool = False,
) -> None:
    from tools.ci.tests.test_proof_operational_result import archive

    release_host_digest_overrides = release_host_digest_overrides or {}
    proof_by_id = {proof["id"]: proof for proof in registry["proofs"]}
    target_catalog = CHECKER._read_toml(root / "tools/ci/test-authority.toml")
    target_by_id = {row["id"]: row for row in target_catalog.get("integration_targets", [])}
    source_snapshots: dict[tuple[Path, tuple[Path, ...], Path | None], dict] = {}
    paired_snapshots: dict[tuple[str, str], dict] = {}
    for index, proof_id in enumerate(CHECKER.aggregate_proof_ids(registry)):
        proof = proof_by_id[proof_id]
        manifest_path = root / proof["artifact"]
        evidence = root / f"artifacts/proof-authority/raw/{proof_id}.log"
        evidence.parent.mkdir(parents=True, exist_ok=True)
        evidence.write_text(f"{proof_id}: passed\n", encoding="utf-8")
        source_pair = None
        excluded_paths: tuple[Path, ...] = ()
        if proof["source_binding"] == "exact-pair":
            pair_key = (proof["paired_repository"], proof["paired_dependency_lock"])
            if pair_key not in paired_snapshots:
                paired_snapshots[pair_key] = CHECKER.paired_source_snapshot(
                    paired,
                    repository=proof["paired_repository"],
                    dependency_lock=Path(proof["paired_dependency_lock"]),
                )
            source_pair = paired_snapshots[pair_key]
            excluded_paths = (paired,)
        artifact_parent = manifest_path.parent
        source_key = (
            artifact_parent,
            excluded_paths,
            manifest_path if artifact_parent == root else None,
        )
        if source_key not in source_snapshots:
            source_snapshots[source_key] = CHECKER.proof_source_snapshot(
                root,
                manifest_path=manifest_path,
                proof=proof,
                excluded_paths=excluded_paths,
            )
        daemon_binary = None
        if proof["binary_binding"] == "release-daemon":
            binary_digest = _digest(root / "bin/searchd")
            binary_archive = root / CHECKER.content_archive_relative_path("binary", binary_digest)
            binary_archive.parent.mkdir(parents=True, exist_ok=True)
            binary_archive.write_bytes((root / "bin/searchd").read_bytes())
            daemon_binary = {
                "source_path": "bin/searchd",
                "path": binary_archive.relative_to(root).as_posix(),
                "sha256": binary_digest,
            }
        dependencies = []
        for dependency_id in proof["dependencies"]:
            dependency_path = root / proof_by_id[dependency_id]["artifact"]
            dependency_payload = json.loads(dependency_path.read_text(encoding="utf-8"))
            dependency_digest = _digest(dependency_path)
            dependencies.append(
                {
                    "proof_id": dependency_id,
                    "path": CHECKER.proof_archive_relative_path(
                        dependency_payload,
                        dependency_digest,
                    ),
                    "sha256": dependency_digest,
                }
            )
        evidence_digest = _digest(evidence)
        evidence_archive = root / CHECKER.content_archive_relative_path("evidence", evidence_digest)
        evidence_archive.parent.mkdir(parents=True, exist_ok=True)
        evidence_archive.write_bytes(evidence.read_bytes())
        result_artifacts = []
        execution_result = None
        result_count = 1
        if proof_id == "p00-authority-freeze":
            inventory_source = root / CHECKER.ERROR_INVENTORY_PATH
            inventory_source.parent.mkdir(parents=True, exist_ok=True)
            inventory_source.write_text(
                json.dumps(INVENTORY_WRITER.build_inventory(root)), encoding="utf-8"
            )
            inventory_digest = _digest(inventory_source)
            inventory_archive = root / CHECKER.content_archive_relative_path(
                "evidence", inventory_digest
            )
            inventory_archive.parent.mkdir(parents=True, exist_ok=True)
            inventory_archive.write_bytes(inventory_source.read_bytes())
            result_artifacts.append(
                {
                    "source_path": CHECKER.ERROR_INVENTORY_PATH,
                    "path": inventory_archive.relative_to(root).as_posix(),
                    "sha256": inventory_digest,
                }
            )
        if proof_id == P12A_PROOF:
            python_paths = {row["id"]: row["path"] for row in target_catalog["python_targets"]}
            selectors = [python_paths[target] for target in proof["test_authority_targets"]]
            result_count = len(selectors)
            listed, events = pytest_bytes(selectors)
            run = {"format": "pytest-junit"}
            for key, raw in (("inventory", listed), ("events", events)):
                source = evidence.parent / f"{proof_id}-{key}"
                source.write_bytes(raw)
                run[key] = source.relative_to(root).as_posix()
                result_artifacts.append(archive(root, run[key]))
            execution_result = {"schema_version": 1, "runs": [run]}
        elif proof["execution_mode"] == "test-authority" and proof_id != PAIR_PROOF:
            targets = [target_by_id[t] for t in proof["test_authority_targets"]]
            runs = []
            witnesses = daemon_witness_specs(proof)
            selections = []
            for target in targets:
                path = Path(target["path"])
                binary = target.get("target", path.stem)
                prefix = f"{target['owner']}::{binary}$"
                module_prefix = ""
                if "tests" in path.parts and path.stem != binary:
                    module = mounted_test_module(
                        root, root / path, target["owner"], binary, list(target_by_id.values())
                    )
                    module_prefix = module + "::"
                if witnesses:
                    cases = [
                        name.removeprefix(prefix)
                        for name in witnesses
                        if name.startswith(prefix + module_prefix)
                    ]
                else:
                    cases = [
                        module_prefix + case
                        for case in proof.get("required_nextest_cases", {}).get(
                            target["id"], ["passes"]
                        )
                    ]
                selections.extend((target["owner"], binary, "test", case) for case in cases)
            for package, cases in proof.get("required_nextest_lib_cases", {}).items():
                binary = package.replace("-", "_")
                selections.extend((package, binary, "lib", case) for case in cases)
            result_count = len(selections)
            for i, (owner, binary, kind, case) in enumerate(selections):
                name = f"{owner}::{binary}${case}"
                listed, events = nextest_bytes(owner, binary, kind, case)
                run = {"format": "nextest-jsonl"}
                for key, raw in (("inventory", listed), ("events", events)):
                    source = evidence.parent / f"{proof_id}-{i}-{key}.json"
                    source.write_bytes(raw)
                    run[key] = source.relative_to(root).as_posix()
                    result_artifacts.append(archive(root, run[key]))
                if name in witnesses:
                    nonce = f"{i:032x}"
                    observed = {
                        "schema_version": 1,
                        "test": name,
                        "nonce": nonce,
                        "executables": [daemon_binary["sha256"]] * witnesses[name][1],
                    }
                    if proof_id == P07_PROOF:
                        observed["provider_approval_sha256"] = proof["provider_approval_sha256"]
                    source = evidence.parent / f"{proof_id}-{i}-daemon-observations.json"
                    source.write_text(json.dumps({
                        "nonce": nonce,
                        "events_sha256": hashlib.sha256(events).hexdigest(),
                        "inventory_sha256": hashlib.sha256(listed).hexdigest(),
                        "observations": {name: observed},
                    }))
                    run["daemon_observations"] = source.relative_to(root).as_posix()
                    result_artifacts.append(archive(root, run["daemon_observations"]))
                runs.append(run)
            execution_result = {"schema_version": 1, "runs": runs}
        if proof.get("execution_source_binding"):
            for i, run in enumerate(execution_result["runs"]):
                source_path = f"artifacts/proof-authority/raw/{proof_id}-{i}-run-binding.json"
                bind_run(
                    root, proof, run, source_path,
                    daemon_sha256=daemon_binary["sha256"] if daemon_binary else None,
                )
                result_artifacts.append(archive(root, source_path))
        if proof_id == PAIR_PROOF:
            result_count = 2
        start = datetime(2026, 9, 21, tzinfo=timezone.utc) + timedelta(minutes=index)
        payload = {
            "schema_version": 1,
            "proof_id": proof_id,
            "family": proof["family"],
            "status": "passed",
            "source": source_snapshots[source_key],
            "source_pair": source_pair,
            "invocation": {
                "command": proof["command"],
                "profile": proof["profile"],
                "target": proof["target"],
                "filter": proof["filter"],
            },
            "counts": {
                "selected": result_count,
                "executed": result_count,
                "passed": result_count,
                "failed": 0,
                "ignored": 0,
            },
            "environment": {
                "toolchain": "fixture",
                "features": ["all-features"]
                if proof_id in CHECKER.BOUND_OWNER_SCOPES.values()
                else [],
                "os": "linux",
                "arch": "x86_64",
                "host": {
                    "profile": proof["required_host"]
                    if proof["required_host"] != "any"
                    else "fixture",
                    "cpu_count": 1,
                    "memory_bytes": 1024,
                    "identity_digest": (
                        release_host_digest_overrides.get(proof_id, RELEASE_HOST_DIGEST)
                        if proof["binary_binding"] == "release-daemon"
                        else "sha256:" + "1" * 64
                    ),
                },
            },
            "daemon_binary": daemon_binary,
            "state_root_format": "v2" if daemon_binary is not None else "not-applicable",
            "inputs": {
                "fixture": "sha256:" + "2" * 64,
                "corpus": None,
                "config": None,
                "model": None,
                "provider": f"sha256:{proof['provider_approval_sha256']}" if proof_id == P07_PROOF else None,
            },
            "started_at": start.isoformat(),
            "ended_at": (start + timedelta(seconds=1)).isoformat(),
            "dependency_receipts": dependencies,
            "artifacts": [
                {
                    "source_path": evidence.relative_to(root).as_posix(),
                    "path": evidence_archive.relative_to(root).as_posix(),
                    "sha256": evidence_digest,
                }
            ]
            + result_artifacts,
        }
        if release_prerequisite_override and proof_id == release_prerequisite_override[0]:
            _, field, value = release_prerequisite_override
            if field == "source":
                payload["source"] = {**payload["source"], "head": value}
            else:
                payload[field] = value
        if proof_id == PAIR_PROOF:
            payload["inputs"] = dict.fromkeys(payload["inputs"])
            execution_result, payload["artifacts"] = paired_evidence(
                root, proof, payload, wrong_paired_caller
            )
        if proof["execution_mode"] == "operational-action":
            from tools.ci.proof_operational_result import manifest_binding
            from tools.ci.tests.test_proof_operational_result import fixture_evidence

            payload["inputs"] = dict.fromkeys(payload["inputs"])
            payload["inputs"]["config"] = "sha256:" + _digest(root / proof["operational_contract"])
            execution_result, operational_artifacts = fixture_evidence(
                root, proof, manifest_binding(payload), started_at=start
            )
            payload["artifacts"] = operational_artifacts
            payload["ended_at"] = (start + timedelta(seconds=5)).isoformat()
        if execution_result is not None:
            payload["execution_result"] = execution_result
        manifest_path.parent.mkdir(parents=True, exist_ok=True)
        manifest_bytes = (json.dumps(payload, sort_keys=True) + "\n").encode()
        manifest_path.write_bytes(manifest_bytes)
        manifest_digest = hashlib.sha256(manifest_bytes).hexdigest()
        archive_path = root / CHECKER.proof_archive_relative_path(payload, manifest_digest)
        archive_path.parent.mkdir(parents=True, exist_ok=True)
        archive_path.write_bytes(manifest_bytes)
        if proof_id in CHECKER.P11_RELEASE_PROOFS:
            _index_fixture_release(archive_path, proof_id)


def _index_fixture_release(archive: Path, proof_id: str) -> None:
    path = archive.parent / "index.json"
    prior = json.loads(path.read_bytes())["manifest_digests"] if path.exists() else []
    path.write_text(
        json.dumps(
            {
                "schema_version": 1,
                "proof_id": proof_id,
                "source_binding_digest": archive.parent.name,
                "manifest_digests": [*prior, archive.stem],
            }
        )
    )


def _share_checker_modules(monkeypatch: pytest.MonkeyPatch) -> None:
    """Use the same checker module when tests patch its source validation."""
    monkeypatch.setattr(WRITER, "_load_checker", lambda: CHECKER)


def test_full_dependency_graph_qualifies_without_historical_handoffs(
    tmp_path: Path,
    aggregate_templates: AggregateTemplates,
) -> None:
    root, registry = _fixture_root(tmp_path, executable=True, templates=aggregate_templates)
    paired = _paired_checkout(tmp_path, aggregate_templates)
    _write_dependency_manifests(root, registry, paired)

    output, _, ready = WRITER.publish_aggregate(
        root=root,
        registry_path=root / "tools/ci/proof-authority.toml",
        paired_checkout=paired,
    )

    payload = json.loads(output.read_text(encoding="utf-8"))
    assert ready
    assert all(item["status"] == "PASSED" for item in payload["dependency_receipts"])
    assert payload["production_ready"] is True
    assert "product_handoffs" not in payload
    assert "product_chain_status" not in payload
    assert "infrastructure_handoff" not in payload


@pytest.mark.parametrize("proof_id", ("p11-activation", "p11-rollback"))
def test_operational_manifest_refuses_a_different_prerequisite_target(
    tmp_path: Path, aggregate_templates: AggregateTemplates, proof_id: str
) -> None:
    root, registry = _fixture_root(tmp_path, executable=True, templates=aggregate_templates)
    paired = _paired_checkout(tmp_path, aggregate_templates)
    proof_by_id = {proof["id"]: proof for proof in registry["proofs"]}
    proof = proof_by_id[proof_id]
    contract_path = root / proof["operational_contract"]
    contract = json.loads(contract_path.read_text())
    contract["target"]["state_root"] = "/different/live/state"
    contract_path.write_text(json.dumps(contract))
    _run(root, "git", "add", proof["operational_contract"])
    _run(root, "git", "commit", "-qm", "fixture with different operational target")
    _write_dependency_manifests(root, registry, paired)
    path = root / proof["artifact"]
    findings = CHECKER.check_manifest(
        json.loads(path.read_text()),
        proof=proof,
        root=root,
        manifest_path=path,
        bind_source=False,
        schema=json.loads((root / "tools/ci/proof-manifest.schema.json").read_text()),
        proof_by_id=proof_by_id,
    )
    assert any("different target or configuration" in item.message for item in findings)


@pytest.mark.parametrize(
    "predecessor,field,value,message",
    [
        ("p10-state-migration", "source", "e" * 40, "different source"),
        ("p10-state-migration", "state_root_format", "v1", "different daemon or state-root format"),
        ("p10-state-migration", "ended_at", "2099-09-21T00:00:01Z", "prerequisite ends after"),
        ("p09-control-readiness", "source", "e" * 40, "different source"),
        (
            "p09-control-readiness",
            "state_root_format",
            "v1",
            "different daemon or state-root format",
        ),
        ("p09-control-readiness-owner", "source", "e" * 40, "different source"),
    ],
)
def test_paired_manifest_refuses_mixed_release_prerequisite(
    tmp_path: Path, aggregate_templates: AggregateTemplates, predecessor, field, value, message
) -> None:
    root, registry = _fixture_root(tmp_path, executable=True, templates=aggregate_templates)
    paired = _paired_checkout(tmp_path, aggregate_templates)
    _write_dependency_manifests(
        root, registry, paired, release_prerequisite_override=(predecessor, field, value)
    )
    proof_by_id = {proof["id"]: proof for proof in registry["proofs"]}
    proof = proof_by_id[PAIR_PROOF]
    path = root / proof["artifact"]
    findings = CHECKER.check_manifest(
        json.loads(path.read_text()),
        proof=proof,
        root=root,
        manifest_path=path,
        bind_source=False,
        schema=json.loads((root / "tools/ci/proof-manifest.schema.json").read_text()),
        proof_by_id=proof_by_id,
    )
    assert any(message in item.message for item in findings), findings


@pytest.mark.parametrize("executable", [False, True], ids=["staged", "executable"])
def test_registered_paired_recipe_refuses_ambient_shell_launch(
    tmp_path: Path, aggregate_templates: AggregateTemplates, executable: bool,
) -> None:
    root, registry = _fixture_root(tmp_path, executable=executable, templates=aggregate_templates)
    path = root / "tools/ci/proof-authority.toml"
    assert not CHECKER.check_registry(registry, root=root, path=path)
    justfile = root / "Justfile"
    original = justfile.read_text()
    justfile.write_text(original.replace("    #!/bin/bash -p\n", "    #!/bin/bash\n", 1))
    findings = CHECKER.check_registry(registry, root=root, path=path)
    assert any(
        "P11 paired runner must use its registered cross-repo recipe" in item.message
        for item in findings
    ), findings


def test_paired_manifest_requires_both_exact_witnesses(
    tmp_path: Path, aggregate_templates: AggregateTemplates
) -> None:
    root, registry = _fixture_root(tmp_path, executable=True, templates=aggregate_templates)
    paired = _paired_checkout(tmp_path, aggregate_templates)
    _write_dependency_manifests(root, registry, paired, wrong_paired_caller=True)
    proof_by_id = {proof["id"]: proof for proof in registry["proofs"]}
    proof = proof_by_id[PAIR_PROOF]
    path = root / proof["artifact"]
    findings = CHECKER.check_manifest(
        json.loads(path.read_text()),
        proof=proof,
        root=root,
        manifest_path=path,
        bind_source=False,
        schema=json.loads((root / "tools/ci/proof-manifest.schema.json").read_text()),
        proof_by_id=proof_by_id,
    )
    assert any("exact P11 caller and kernel" in item.message for item in findings), findings


def test_archived_paired_manifest_uses_the_issuing_commit_selections(tmp_path, aggregate_templates):
    root, registry = _fixture_root(tmp_path, executable=True, templates=aggregate_templates)
    paired = _paired_checkout(tmp_path, aggregate_templates)
    _write_dependency_manifests(root, registry, paired)
    authorities = {row["id"]: row for row in registry["proofs"]}
    path = root / authorities[PAIR_PROOF]["artifact"]
    payload = json.loads(path.read_bytes())
    registry_path = root / "tools/ci/proof-authority.toml"
    original = registry_path.read_text()
    old_test = authorities[PAIR_PROOF]["paired_execution_cases"][1]["test"]
    registry_path.write_text(original.replace(old_test, "native_repomap::new_kernel_case"))
    _run(root, "git", "add", "tools/ci/proof-authority.toml")
    _run(root, "git", "commit", "-qm", "change current paired selection")
    live = CHECKER._read_toml(registry_path)
    live_by_id = {row["id"]: row for row in live["proofs"]}
    assert live_by_id[PAIR_PROOF]["paired_execution_cases"][1]["test"] != old_test
    assert CHECKER.check_manifest(
        payload,
        proof=live_by_id[PAIR_PROOF],
        root=root,
        manifest_path=path,
        bind_source=False,
        schema=json.loads((root / "tools/ci/proof-manifest.schema.json").read_text()),
        proof_by_id=live_by_id,
    ) == []


def test_writer_publishes_truthful_not_ready_diagnostic_for_staged_graph(
    tmp_path: Path,
    aggregate_templates: AggregateTemplates,
) -> None:
    root, registry = _fixture_root(tmp_path, executable=False, templates=aggregate_templates)
    paired = _paired_checkout(tmp_path, aggregate_templates)

    output, _, ready = WRITER.publish_aggregate(
        root=root,
        registry_path=root / "tools/ci/proof-authority.toml",
        paired_checkout=paired,
    )

    payload = json.loads(output.read_text(encoding="utf-8"))
    statuses = {item["proof_id"]: item["status"] for item in payload["dependency_receipts"]}
    assert not ready
    assert payload["production_ready"] is False
    assert statuses["p00-authority-freeze"] == "NOT_RUN"
    # P01A is now executable, so absent execution is NOT_RUN rather than a
    # staged-contract BLOCKED state. Neither state can qualify the aggregate.
    assert statuses["p01-canonical-identity"] == "NOT_RUN"
    assert statuses["p12a-proof-infrastructure"] == "NOT_RUN"
    assert payload["verdicts"]["DEPLOYED"]["status"] == "BLOCKED"
    assert payload["registry_sha256"] == _digest(root / "tools/ci/proof-authority.toml")
    assert registry["aggregate"]["artifact"] == output.relative_to(root).as_posix()


@pytest.mark.parametrize("prior_bytes", [None, b"prior-authoritative-aggregate\n"])
def test_writer_rebinds_source_after_publication(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    prior_bytes: bytes | None,
    aggregate_templates: AggregateTemplates,
) -> None:
    root, registry = _fixture_root(tmp_path, executable=False, templates=aggregate_templates)
    paired = _paired_checkout(tmp_path, aggregate_templates)
    output = root / registry["aggregate"]["artifact"]
    if prior_bytes is not None:
        output.parent.mkdir(parents=True, exist_ok=True)
        output.write_bytes(prior_bytes)
    check_receipt = CHECKER.check_aggregate_receipt
    calls = 0

    def mutate_after_prepublication_check(*args, **kwargs):
        nonlocal calls
        findings = check_receipt(*args, **kwargs)
        calls += 1
        if calls == 1:
            with (root / "Justfile").open("a", encoding="utf-8") as handle:
                handle.write("\n# source changed during publication\n")
        return findings

    monkeypatch.setattr(WRITER, "_load_checker", lambda: CHECKER)
    monkeypatch.setattr(CHECKER, "check_aggregate_receipt", mutate_after_prepublication_check)

    with pytest.raises(
        WRITER.AggregateRefused, match="proof inputs changed at aggregate publication"
    ):
        WRITER.publish_aggregate(
            root=root,
            registry_path=root / "tools/ci/proof-authority.toml",
            paired_checkout=paired,
        )

    assert calls == 2
    if prior_bytes is None:
        assert not output.exists()
    else:
        assert output.read_bytes() == prior_bytes


def test_writer_derives_ready_receipt_from_verified_proof_input(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    aggregate_templates: AggregateTemplates,
) -> None:
    _share_checker_modules(monkeypatch)
    root, registry = _fixture_root(tmp_path, executable=True, templates=aggregate_templates)
    paired = _paired_checkout(tmp_path, aggregate_templates)
    _write_dependency_manifests(root, registry, paired)

    output, _, ready = WRITER.publish_aggregate(
        root=root,
        registry_path=root / "tools/ci/proof-authority.toml",
        paired_checkout=paired,
    )

    payload = json.loads(output.read_text(encoding="utf-8"))
    assert ready
    assert payload["production_ready"] is True
    assert all(item["status"] == "PASSED" for item in payload["dependency_receipts"])
    assert all(verdict["status"] == "PASSED" for verdict in payload["verdicts"].values())
    assert payload["daemon_binary"] == {
        "path": CHECKER.content_archive_relative_path("binary", _digest(root / "bin/searchd")),
        "sha256": _digest(root / "bin/searchd"),
    }
    assert payload["release_host"]["profile"] == "linux-production-like"
    assert payload["state_root_format"] == "v2"


@pytest.mark.parametrize("changed_component", ("daemon", "s_source", "q_source"))
def test_required_release_identity_refuses_swapped_candidate_without_replacing_aggregate(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    aggregate_templates: AggregateTemplates,
    changed_component: str,
) -> None:
    _share_checker_modules(monkeypatch)
    root, registry = _fixture_root(tmp_path, executable=True, templates=aggregate_templates)
    paired = _paired_checkout(tmp_path, aggregate_templates)
    _write_dependency_manifests(root, registry, paired)
    registry_path = root / "tools/ci/proof-authority.toml"
    payload, output = WRITER.build_aggregate(
        root=root,
        registry=registry,
        registry_path=registry_path,
        checker=CHECKER,
        paired_checkout=paired,
    )
    assert payload["production_ready"] is True
    expected = [
        payload["daemon_binary"]["sha256"],
        payload["source_pair"]["source"]["head"],
        payload["source"]["head"],
    ]
    expected[("daemon", "s_source", "q_source").index(changed_component)] = "0" * 40
    prior = b"prior-authoritative-aggregate\n"
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_bytes(prior)

    with pytest.raises(WRITER.AggregateRefused, match="required Q/S release identity"):
        WRITER.publish_aggregate(
            root=root,
            registry_path=registry_path,
            paired_checkout=paired,
            required_release_identity=tuple(expected),
        )

    assert output.read_bytes() == prior


@pytest.mark.parametrize("mismatch", ("config", "manifest"))
def test_required_release_installation_refuses_foreign_selection_without_replacing_aggregate(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    aggregate_templates: AggregateTemplates,
    mismatch: str,
) -> None:
    _share_checker_modules(monkeypatch)
    root, registry = _fixture_root(tmp_path, executable=True, templates=aggregate_templates)
    paired = _paired_checkout(tmp_path, aggregate_templates)
    _write_dependency_manifests(root, registry, paired)
    registry_path = root / "tools/ci/proof-authority.toml"
    output, _, ready = WRITER.publish_aggregate(root=root, registry_path=registry_path, paired_checkout=paired)
    assert ready
    prior = output.read_bytes()
    payload = json.loads(prior)
    operation = next(proof for proof in registry["proofs"] if proof["id"] == "p11-rollback")
    target = json.loads((root / operation["artifact"]).read_bytes())["execution_result"]["target"]
    identity = (payload["daemon_binary"]["sha256"], payload["source_pair"]["source"]["head"], payload["source"]["head"])

    with pytest.raises(WRITER.AggregateRefused, match="signed installation target"):
        WRITER.publish_aggregate(
            root=root, registry_path=registry_path, paired_checkout=paired,
            required_release_identity=identity,
            required_installation=(
                target["state_root"],
                "0" * 64 if mismatch == "config" else target["config_sha256"],
                "0" * 64 if mismatch == "manifest" else target["release_manifest_sha256"],
            ),
        )
    assert output.read_bytes() == prior


def test_writer_never_promotes_symlinked_registered_manifest(
    tmp_path: Path,
    aggregate_templates: AggregateTemplates,
) -> None:
    root, registry = _fixture_root(tmp_path, executable=True, templates=aggregate_templates)
    paired = _paired_checkout(tmp_path, aggregate_templates)
    _write_dependency_manifests(root, registry, paired)
    proof = next(item for item in registry["proofs"] if item["id"] == "p00-authority-freeze")
    archive = root / proof["artifact"]
    mutable_copy = root / "artifacts/raw/mutable-p00-manifest.json"
    mutable_copy.parent.mkdir(parents=True, exist_ok=True)
    mutable_copy.write_bytes(archive.read_bytes())
    archive.unlink()
    archive.symlink_to(mutable_copy)

    output, _, ready = WRITER.publish_aggregate(
        root=root,
        registry_path=root / "tools/ci/proof-authority.toml",
        paired_checkout=paired,
    )
    payload = json.loads(output.read_text(encoding="utf-8"))
    assert not ready
    assert payload["production_ready"] is False
    status = next(
        item["status"]
        for item in payload["dependency_receipts"]
        if item["proof_id"] == "p00-authority-freeze"
    )
    assert status == "FAILED"


def test_writer_classifies_broken_manifest_symlink_as_failed(
    tmp_path: Path,
    aggregate_templates: AggregateTemplates,
) -> None:
    root, registry = _fixture_root(tmp_path, executable=True, templates=aggregate_templates)
    paired = _paired_checkout(tmp_path, aggregate_templates)
    _write_dependency_manifests(root, registry, paired)
    proof = next(item for item in registry["proofs"] if item["id"] == "p00-authority-freeze")
    alias = root / proof["artifact"]
    alias.unlink()
    alias.symlink_to("missing-manifest.json")

    output, _, ready = WRITER.publish_aggregate(
        root=root,
        registry_path=root / "tools/ci/proof-authority.toml",
        paired_checkout=paired,
    )
    payload = json.loads(output.read_text(encoding="utf-8"))
    assert not ready
    status = next(
        item["status"]
        for item in payload["dependency_receipts"]
        if item["proof_id"] == "p00-authority-freeze"
    )
    assert status == "FAILED"


def test_writer_refuses_symlinked_aggregate_parent_without_writing(
    tmp_path: Path,
    aggregate_templates: AggregateTemplates,
) -> None:
    root, registry = _fixture_root(tmp_path, executable=False, templates=aggregate_templates)
    paired = _paired_checkout(tmp_path, aggregate_templates)
    output = root / registry["aggregate"]["artifact"]
    redirected = root / "artifacts/mutable-output"
    redirected.mkdir(parents=True)
    output.parent.symlink_to(redirected, target_is_directory=True)

    with pytest.raises(WRITER.AggregateRefused, match="aggregate output parent is unsafe"):
        WRITER.publish_aggregate(
            root=root,
            registry_path=root / "tools/ci/proof-authority.toml",
            paired_checkout=paired,
        )
    assert not (redirected / output.name).exists()


def test_writer_refuses_symlinked_existing_aggregate_without_replacing(
    tmp_path: Path,
    aggregate_templates: AggregateTemplates,
) -> None:
    root, registry = _fixture_root(tmp_path, executable=False, templates=aggregate_templates)
    paired = _paired_checkout(tmp_path, aggregate_templates)
    output = root / registry["aggregate"]["artifact"]
    output.parent.mkdir(parents=True)
    redirected = root / "artifacts/mutable-aggregate.json"
    redirected.write_bytes(b"prior external bytes\n")
    output.symlink_to(redirected)

    with pytest.raises(WRITER.AggregateRefused, match="existing aggregate output is unsafe"):
        WRITER.publish_aggregate(
            root=root,
            registry_path=root / "tools/ci/proof-authority.toml",
            paired_checkout=paired,
        )
    assert output.is_symlink()
    assert redirected.read_bytes() == b"prior external bytes\n"


def test_ready_aggregate_rejects_injected_handoff_status(
    tmp_path: Path,
    aggregate_templates: AggregateTemplates,
) -> None:
    root, registry = _fixture_root(tmp_path, executable=True, templates=aggregate_templates)
    paired = _paired_checkout(tmp_path, aggregate_templates)
    _write_dependency_manifests(root, registry, paired)

    output, _, ready = WRITER.publish_aggregate(
        root=root,
        registry_path=root / "tools/ci/proof-authority.toml",
        paired_checkout=paired,
    )
    payload = json.loads(output.read_text(encoding="utf-8"))
    assert ready
    assert all(item["status"] == "PASSED" for item in payload["dependency_receipts"])
    assert all(item["status"] == "PASSED" for item in payload["verdicts"].values())
    assert payload["production_ready"] is True

    forged = dict(payload, product_chain_status="VERIFIED")
    findings = CHECKER.check_aggregate_receipt(
        forged,
        receipt_path=output,
        registry=registry,
        registry_path=root / "tools/ci/proof-authority.toml",
        schema=json.loads((root / "tools/ci/proof-aggregate.schema.json").read_text()),
        root=root,
        bind_source=True,
        paired_checkouts={"github:josongmin/semantica-codegraph-v2": paired},
    )
    assert any("product_chain_status" in item.message for item in findings)


@pytest.mark.parametrize("proof_id", ["p11-cross-repo-cutover", "p12a-proof-infrastructure"])
def test_complete_proof_verdicts_cannot_bypass_missing_dependency(
    tmp_path: Path,
    aggregate_templates: AggregateTemplates,
    proof_id: str,
) -> None:
    root, registry = _fixture_root(tmp_path, executable=True, templates=aggregate_templates)
    paired = _paired_checkout(tmp_path, aggregate_templates)
    _write_dependency_manifests(root, registry, paired)
    proof = next(item for item in registry["proofs"] if item["id"] == proof_id)
    (root / proof["artifact"]).unlink()

    output, _, ready = WRITER.publish_aggregate(
        root=root,
        registry_path=root / "tools/ci/proof-authority.toml",
        paired_checkout=paired,
    )
    payload = json.loads(output.read_text(encoding="utf-8"))
    assert not ready
    assert (
        next(
            item["status"]
            for item in payload["dependency_receipts"]
            if item["proof_id"] == proof_id
        )
        == "NOT_RUN"
    )
    assert payload["production_ready"] is False


def test_writer_keeps_p12a_source_proof_when_paired_source_changes(
    tmp_path: Path,
    aggregate_templates: AggregateTemplates,
) -> None:
    root, registry = _fixture_root(tmp_path, executable=True, templates=aggregate_templates)
    paired = _paired_checkout(tmp_path, aggregate_templates)
    _write_dependency_manifests(root, registry, paired)
    (paired / "Cargo.lock").write_text(
        "version = 4\n# changed after P12A proof\n", encoding="utf-8"
    )

    output, _, ready = WRITER.publish_aggregate(
        root=root,
        registry_path=root / "tools/ci/proof-authority.toml",
        paired_checkout=paired,
    )

    payload = json.loads(output.read_text(encoding="utf-8"))
    statuses = {item["proof_id"]: item["status"] for item in payload["dependency_receipts"]}
    assert not ready
    assert statuses["p12a-proof-infrastructure"] == "PASSED"
    assert statuses["p11-cross-repo-cutover"] == "FAILED"
    assert payload["production_ready"] is False


def test_aggregate_validation_refuses_source_change_during_cached_pass(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    aggregate_templates: AggregateTemplates,
) -> None:
    _share_checker_modules(monkeypatch)
    root, registry = _fixture_root(tmp_path, executable=True, templates=aggregate_templates)
    paired = _paired_checkout(tmp_path, aggregate_templates)
    _write_dependency_manifests(root, registry, paired)
    output, _, ready = WRITER.publish_aggregate(
        root=root,
        registry_path=root / "tools/ci/proof-authority.toml",
        paired_checkout=paired,
    )
    assert ready
    payload = json.loads(output.read_text(encoding="utf-8"))
    original_check_manifest = CHECKER.check_manifest
    mutated = False

    def mutate_after_first_manifest(*args, **kwargs):
        nonlocal mutated
        findings = original_check_manifest(*args, **kwargs)
        if not mutated:
            with (root / "Justfile").open("a", encoding="utf-8") as handle:
                handle.write("\n# changed during aggregate validation\n")
            mutated = True
        return findings

    monkeypatch.setattr(CHECKER, "check_manifest", mutate_after_first_manifest)
    findings = CHECKER.check_aggregate_receipt(
        payload,
        receipt_path=output,
        registry=registry,
        registry_path=root / "tools/ci/proof-authority.toml",
        schema=json.loads((root / "tools/ci/proof-aggregate.schema.json").read_text()),
        root=root,
        bind_source=True,
        paired_checkouts={"github:josongmin/semantica-codegraph-v2": paired},
        require_ready=True,
    )

    assert mutated
    assert any(
        finding.message == "source changed during aggregate validation" for finding in findings
    )


def test_writer_accepts_distinct_code_proof_host_identity(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    aggregate_templates: AggregateTemplates,
) -> None:
    _share_checker_modules(monkeypatch)
    root, registry = _fixture_root(tmp_path, executable=True, templates=aggregate_templates)
    paired = _paired_checkout(tmp_path, aggregate_templates)
    _write_dependency_manifests(
        root,
        registry,
        paired,
        release_host_digest_overrides={"p08-runtime-supervisor": "sha256:" + "3" * 64},
    )

    output, _, ready = WRITER.publish_aggregate(
        root=root,
        registry_path=root / "tools/ci/proof-authority.toml",
        paired_checkout=paired,
    )

    payload = json.loads(output.read_text(encoding="utf-8"))
    assert ready
    assert payload["production_ready"] is True
    assert payload["release_host"]["identity_digest"] == RELEASE_HOST_DIGEST


def test_writer_refuses_operational_host_drift(
    tmp_path: Path,
    aggregate_templates: AggregateTemplates,
) -> None:
    root, registry = _fixture_root(tmp_path, executable=True, templates=aggregate_templates)
    paired = _paired_checkout(tmp_path, aggregate_templates)
    _write_dependency_manifests(
        root,
        registry,
        paired,
        release_host_digest_overrides={"p11-activation": "sha256:" + "3" * 64},
    )

    output, _, ready = WRITER.publish_aggregate(
        root=root,
        registry_path=root / "tools/ci/proof-authority.toml",
        paired_checkout=paired,
    )

    payload = json.loads(output.read_text(encoding="utf-8"))
    assert not ready
    assert payload["production_ready"] is False
    assert payload["release_host"] is None
    assert payload["verdicts"]["CODE_QUALIFIED"]["status"] == "PASSED"
    assert payload["verdicts"]["DEPLOYED"]["status"] == "PASSED"
    assert payload["verdicts"]["ACTIVATED"]["status"] == "FAILED"
    assert payload["verdicts"]["ROLLBACK_PROVEN"]["status"] == "FAILED"


def test_ready_aggregate_is_final_release_receipt(
    tmp_path: Path,
    aggregate_templates: AggregateTemplates,
) -> None:
    root, registry = _fixture_root(tmp_path, executable=True, templates=aggregate_templates)
    paired = _paired_checkout(tmp_path, aggregate_templates)
    _write_dependency_manifests(root, registry, paired)
    aggregate_path, _, ready = WRITER.publish_aggregate(
        root=root,
        registry_path=root / "tools/ci/proof-authority.toml",
        paired_checkout=paired,
    )
    assert ready
    payload = json.loads(aggregate_path.read_text(encoding="utf-8"))
    assert payload["schema_version"] == 2
    findings = CHECKER.check_aggregate_receipt(
        payload,
        receipt_path=aggregate_path,
        registry=registry,
        registry_path=root / "tools/ci/proof-authority.toml",
        schema=json.loads((root / "tools/ci/proof-aggregate.schema.json").read_text()),
        root=root,
        bind_source=True,
        paired_checkouts={registry["aggregate"]["paired_repository"]: paired},
        require_ready=True,
    )
    assert findings == []
    assert "target_proof_id" not in payload

    stale = dict(payload, schema_version=1)
    findings = CHECKER.check_aggregate_receipt(
        stale,
        receipt_path=aggregate_path,
        registry=registry,
        registry_path=root / "tools/ci/proof-authority.toml",
        schema=json.loads((root / "tools/ci/proof-aggregate.schema.json").read_text()),
        root=root,
        bind_source=True,
        paired_checkouts={registry["aggregate"]["paired_repository"]: paired},
        require_ready=True,
    )
    assert any("schema schema_version" in item.message for item in findings)


def test_failure_preserves_prior_aggregate(
    tmp_path: Path,
    aggregate_templates: AggregateTemplates,
) -> None:
    root, _ = _fixture_root(tmp_path, executable=False, templates=aggregate_templates)
    paired = _paired_checkout(tmp_path, aggregate_templates)
    output = root / "artifacts/proof-authority/p12-release-aggregate.json"
    output.parent.mkdir(parents=True)
    output.write_bytes(b"prior-authoritative-bytes\n")
    _run(paired, "git", "remote", "set-url", "origin", "https://example.invalid/wrong.git")

    try:
        WRITER.publish_aggregate(
            root=root,
            registry_path=root / "tools/ci/proof-authority.toml",
            paired_checkout=paired,
        )
    except WRITER.AggregateRefused:
        pass
    else:
        raise AssertionError("invalid paired source must be refused")

    assert output.read_bytes() == b"prior-authoritative-bytes\n"


def test_current_aggregate_rejects_activation_after_deployment_is_reissued(
    tmp_path, aggregate_templates
):
    from tools.ci.proof_operational_result import manifest_binding
    from tools.ci.tests.test_proof_operational_result import fixture_evidence

    root, registry = _fixture_root(tmp_path, executable=True, templates=aggregate_templates)
    paired = _paired_checkout(tmp_path, aggregate_templates)
    _write_dependency_manifests(root, registry, paired)
    authorities = {p["id"]: p for p in registry["proofs"]}
    deployment = authorities["p11-deployment"]
    path = root / deployment["artifact"]
    current = json.loads(path.read_bytes())
    start = datetime.fromisoformat(current["started_at"]) + timedelta(hours=1)
    current["started_at"] = start.isoformat()
    current["ended_at"] = (start + timedelta(seconds=5)).isoformat()
    current["execution_result"], current["artifacts"] = fixture_evidence(
        root, deployment, manifest_binding(current), started_at=start
    )
    data = json.dumps(current).encode()
    path.write_bytes(data)
    archived = root / CHECKER.proof_archive_relative_path(current, hashlib.sha256(data).hexdigest())
    archived.parent.mkdir(parents=True, exist_ok=True)
    archived.write_bytes(data)
    _index_fixture_release(archived, deployment["id"])

    activation = authorities["p11-activation"]
    activation_path = root / activation["artifact"]
    activation_raw = activation_path.read_bytes()
    original = json.loads(activation_raw)
    archived_activation = root / CHECKER.proof_archive_relative_path(
        original, hashlib.sha256(activation_raw).hexdigest()
    )
    args = dict(
        proof=activation,
        manifest_path=archived_activation,
        root=root,
        bind_source=False,
        schema=json.loads((root / activation["artifact_schema"]).read_bytes()),
        proof_by_id=authorities,
    )
    assert CHECKER.check_manifest(original, **args) == []
    assert any(
        "selected receipt for p11-deployment" in f.message
        for f in CHECKER.check_manifest(original, **{**args, "manifest_path": activation_path})
    )
    registry_path = root / "tools/ci/proof-authority.toml"
    output, _, ready = WRITER.publish_aggregate(
        root=root, registry_path=registry_path, paired_checkout=paired
    )
    payload = json.loads(output.read_bytes())
    assert not ready and not payload["production_ready"]
    assert {k: v["status"] for k, v in payload["verdicts"].items()} == {
        "CODE_QUALIFIED": "PASSED",
        "DEPLOYED": "PASSED",
        "ACTIVATED": "FAILED",
        "ROLLBACK_PROVEN": "FAILED",
    }
    payload["production_ready"] = True
    for verdict in payload["verdicts"].values():
        verdict["status"] = "PASSED"
    findings = CHECKER.check_aggregate_receipt(
        payload,
        receipt_path=output,
        registry=registry,
        registry_path=registry_path,
        schema=json.loads((root / registry["aggregate"]["schema"]).read_bytes()),
        root=root,
        bind_source=True,
        paired_checkouts={registry["aggregate"]["paired_repository"]: paired},
        require_ready=True,
    )
    assert any("selected receipt for p11-deployment" in f.message for f in findings)


def _refresh_run_binding(root: Path, proof: dict, run: dict, payload: dict) -> None:
    from tools.ci.tests.test_proof_operational_result import archive

    binding = run["run_binding"]
    (root / binding).unlink()
    bind_run(root, proof, run, binding, daemon_sha256=payload["daemon_binary"]["sha256"])
    index = next(i for i, item in enumerate(payload["artifacts"]) if item["source_path"] == binding)
    payload["artifacts"][index] = archive(root, binding)


def test_p07_archive_requires_the_same_approved_provider_in_manifest_and_witness(
    tmp_path, aggregate_templates,
):
    from tools.ci.tests.test_proof_operational_result import archive

    root, registry = _fixture_root(tmp_path, executable=True, templates=aggregate_templates)
    paired = _paired_checkout(tmp_path, aggregate_templates)
    _write_dependency_manifests(root, registry, paired)
    authorities = {proof["id"]: proof for proof in registry["proofs"]}
    proof = authorities[P07_PROOF]
    path = root / proof["artifact"]
    payload = json.loads(path.read_bytes())
    args = dict(
        proof=proof,
        manifest_path=path,
        root=root,
        bind_source=False,
        schema=json.loads((root / proof["artifact_schema"]).read_bytes()),
        proof_by_id=authorities,
    )
    assert CHECKER.check_manifest(payload, **args) == []
    provider_digest = payload["inputs"]["provider"]
    payload["inputs"]["provider"] = "sha256:" + "b" * 64
    assert any(
        "provider input differs from the approved config" in finding.message
        for finding in CHECKER.check_manifest(payload, **args)
    )
    payload["inputs"]["provider"] = provider_digest
    (run,) = payload["execution_result"]["runs"]
    observed = run["daemon_observations"]
    batch = json.loads((root / observed).read_bytes())
    (witness,) = batch["observations"].values()
    witness["provider_approval_sha256"] = "b" * 64
    (root / observed).write_text(json.dumps(batch))
    index = next(i for i, item in enumerate(payload["artifacts"]) if item["source_path"] == observed)
    payload["artifacts"][index] = archive(root, observed)
    _refresh_run_binding(root, proof, run, payload)
    assert any(
        "daemon witness differs" in finding.message
        for finding in CHECKER.check_manifest(payload, **args)
    )


@pytest.mark.parametrize("missing", [0, 1])
def test_release_read_view_requires_both_registered_observations(tmp_path, aggregate_templates, missing):
    root, registry = _fixture_root(tmp_path, executable=True, templates=aggregate_templates)
    paired = _paired_checkout(tmp_path, aggregate_templates)
    _write_dependency_manifests(root, registry, paired)
    authorities = {p["id"]: p for p in registry["proofs"]}
    proof = authorities["p04-read-view-lifetime"]
    path = root / proof["artifact"]
    payload = json.loads(path.read_bytes())
    args = dict(
        proof=proof,
        manifest_path=path,
        root=root,
        bind_source=False,
        schema=json.loads((root / proof["artifact_schema"]).read_bytes()),
        proof_by_id=authorities,
    )
    assert CHECKER.check_manifest(payload, **args) == []
    from tools.ci.tests.test_proof_operational_result import archive

    run = payload["execution_result"]["runs"][missing]
    observations_path = run.pop("daemon_observations")
    _refresh_run_binding(root, proof, run, payload)
    assert any(
        "P04 lacks a daemon observation" in f.message for f in CHECKER.check_manifest(payload, **args)
    )
    run["daemon_observations"] = observations_path
    observation = root / observations_path
    original = observation.read_bytes()
    batch = json.loads(original)
    (row,) = batch["observations"].values()
    row["executables"] = ["e" * 64] * len(row["executables"])
    observation.write_text(json.dumps(batch))
    artifact_index = next(
        i for i, item in enumerate(payload["artifacts"]) if item["source_path"] == observations_path
    )
    payload["artifacts"][artifact_index] = archive(root, observations_path)
    _refresh_run_binding(root, proof, run, payload)
    assert any(
        "P04 executed daemon differs" in f.message for f in CHECKER.check_manifest(payload, **args)
    )
    observation.write_bytes(original)
    payload["artifacts"][artifact_index] = archive(root, observations_path)
    _refresh_run_binding(root, proof, run, payload)
    # Replay uses the target and observation contract of the issuing commit.
    catalog = root / "tools/ci/test-authority.toml"
    catalog.write_text(
        catalog.read_text().replace(
            'id = "searchd-runtime-native-product-process"',
            'id = "read-view-renamed-in-a-later-checkout"',
        )
    )
    active_proof = deepcopy(proof)
    active_proof["daemon_observation_tests"][0]["event"] += "_renamed"
    args["proof"] = active_proof
    assert CHECKER.check_manifest(payload, **args) == []
    payload["execution_result"]["runs"].pop(missing)
    payload["counts"].update(selected=1, executed=1, passed=1)
    messages = [f.message for f in CHECKER.check_manifest(payload, **args)]
    assert any("P04 lacks a daemon observation" in msg for msg in messages)


def test_p04_archive_refuses_an_extra_passing_body(tmp_path, aggregate_templates):
    from tools.ci.tests.test_proof_operational_result import archive

    root, registry = _fixture_root(tmp_path, executable=True, templates=aggregate_templates)
    paired = _paired_checkout(tmp_path, aggregate_templates)
    _write_dependency_manifests(root, registry, paired)
    authorities = {proof["id"]: proof for proof in registry["proofs"]}
    proof = authorities["p04-read-view-lifetime"]
    path = root / proof["artifact"]
    payload = json.loads(path.read_bytes())
    args = dict(
        proof=proof, manifest_path=path, root=root, bind_source=False,
        schema=json.loads((root / proof["artifact_schema"]).read_bytes()),
        proof_by_id=authorities,
    )
    assert CHECKER.check_manifest(payload, **args) == []
    event = proof["daemon_observation_tests"][0]["event"]
    package, binary = event.split("$")[0].split("::")
    listed, events = nextest_bytes(package, binary, "test", "unregistered_release_case")
    run = {"format": "nextest-jsonl"}
    for key, raw in (("inventory", listed), ("events", events)):
        source = f"artifacts/proof-authority/raw/p04-extra-{key}.json"
        (root / source).write_bytes(raw)
        run[key] = source
        payload["artifacts"].append(archive(root, source))
    binding = "artifacts/proof-authority/raw/p04-extra-run-binding.json"
    bind_run(root, proof, run, binding, daemon_sha256=payload["daemon_binary"]["sha256"])
    payload["artifacts"].append(archive(root, binding))
    payload["execution_result"]["runs"].append(run)
    payload["counts"].update(selected=3, executed=3, passed=3)

    findings = CHECKER.check_manifest(payload, **args)
    assert any("P04 requires only the registered" in item.message for item in findings)


def test_p10_archive_requires_all_four_observations_of_the_release_elf(
    tmp_path, aggregate_templates,
):
    from tools.ci.tests.test_proof_operational_result import archive

    root, registry = _fixture_root(tmp_path, executable=True, templates=aggregate_templates)
    paired = _paired_checkout(tmp_path, aggregate_templates)
    _write_dependency_manifests(root, registry, paired)
    authorities = {proof["id"]: proof for proof in registry["proofs"]}
    proof = authorities[P10_PROOF]
    path = root / proof["artifact"]
    payload = json.loads(path.read_bytes())
    args = dict(
        proof=proof,
        manifest_path=path,
        root=root,
        bind_source=False,
        schema=json.loads((root / proof["artifact_schema"]).read_bytes()),
        proof_by_id=authorities,
    )
    assert CHECKER.check_manifest(payload, **args) == []
    (run,) = payload["execution_result"]["runs"]
    observed = run.pop("daemon_observations")
    _refresh_run_binding(root, proof, run, payload)
    assert any(
        "P10 lacks a daemon observation" in finding.message
        for finding in CHECKER.check_manifest(payload, **args)
    )
    run["daemon_observations"] = observed
    batch = json.loads((root / observed).read_bytes())
    (row,) = batch["observations"].values()
    digest = payload["daemon_binary"]["sha256"]
    index = next(i for i, item in enumerate(payload["artifacts"]) if item["source_path"] == observed)
    for digests, reason in (
        ([digest] * 3, "must bind 4 launch"),
        (["e" * 64] * 4, "P10 executed daemon differs from the release binary"),
    ):
        row["executables"] = digests
        (root / observed).write_text(json.dumps(batch))
        payload["artifacts"][index] = archive(root, observed)
        _refresh_run_binding(root, proof, run, payload)
        assert any(
            reason in finding.message for finding in CHECKER.check_manifest(payload, **args)
        )


def test_p10_archive_refuses_a_format_different_from_the_issuing_source(
    tmp_path, aggregate_templates,
):
    root, registry = _fixture_root(tmp_path, executable=True, templates=aggregate_templates)
    paired = _paired_checkout(tmp_path, aggregate_templates)
    format_source = root / "crates/quanta-index-contract/src/index.rs"
    format_source.write_text("native_index_state_format!(3);\n", encoding="utf-8")
    _run(root, "git", "add", format_source.relative_to(root).as_posix())
    _run(root, "git", "commit", "-qm", "fixture with a newer Native state format")
    _write_dependency_manifests(root, registry, paired)
    authorities = {proof["id"]: proof for proof in registry["proofs"]}
    proof = authorities[P10_PROOF]
    path = root / proof["artifact"]
    payload = json.loads(path.read_bytes())
    assert payload["state_root_format"] == "v2"
    format_source.write_text("native_index_state_format!(2);\n", encoding="utf-8")
    findings = CHECKER.check_manifest(
        payload,
        proof=proof,
        manifest_path=path,
        root=root,
        bind_source=False,
        schema=json.loads((root / proof["artifact_schema"]).read_bytes()),
        proof_by_id=authorities,
    )
    assert any("P10 state-root format differs from its issuing source" in f.message for f in findings)


def test_interrupted_deployment_invalidates_current_success_and_refuses_republication(
    tmp_path, aggregate_templates, monkeypatch, capsys
):
    from tools.ci import operational_proof as cli
    from tools.ci import proof_operational_result as operation
    from tools.ci.tests.p11_test_issuer import sign_statement
    from tools.ci.tests.test_proof_operational_result import fixture_states

    root, registry = _fixture_root(tmp_path, executable=True, templates=aggregate_templates)
    pair = _paired_checkout(tmp_path, aggregate_templates)
    proof = next(p for p in registry["proofs"] if p["id"] == "p11-deployment")
    contract = json.loads((root / proof["operational_contract"]).read_bytes())
    state = root / "artifacts/fixture-state.json"
    state.parent.mkdir()
    pre, post = fixture_states(contract)
    state.write_text(json.dumps(pre))
    actor_header = (
        "import json,sys\nfrom pathlib import Path\n"
        "request=json.loads(Path(sys.argv[2]).read_bytes())\n"
        f"state=Path({str(state)!r})\n"
    )
    observer = (
        actor_header
        + "request['observed']=json.loads(state.read_bytes())\nprint(json.dumps(request))\n"
    )
    action = actor_header + (
        "request.pop('expected')\n"
        f"state.write_text({json.dumps(post)!r})\n"
        "request['observed']={'action':'deployment','completed':True}\nprint(json.dumps(request))\n"
    )
    for phase, source in contract["actors"].items():
        script = action if phase == "action" else observer
        if phase == "post":
            script += "if state.with_suffix('.interrupt').exists(): raise SystemExit('fixture post observation interrupted')\n"
        (root / source).write_text(script)
    _run(root, "git", "add", *contract["actors"].values())
    _run(root, "git", "commit", "-qm", "Local state transition fixture")
    _write_dependency_manifests(root, registry, pair)
    writer = operation._writer()
    monkeypatch.setattr(cli, "ROOT", root)
    monkeypatch.setattr(writer.platform, "system", lambda: "Linux")
    monkeypatch.setattr(writer.platform, "machine", lambda: "x86_64")
    monkeypatch.setattr(operation.p11_attestation, "sign", sign_statement)
    current = root / proof["artifact"]
    env = json.loads(current.read_bytes())["environment"]
    monkeypatch.setattr(
        operation, "observed_host_environment", lambda: {**env, "host": RELEASE_HOST_INPUT}
    )

    def run(name):
        output = current.parent / name
        return cli.main([proof["id"], "--output", str(output), "--paired-checkout", str(pair)])

    real_replace, real_sync = os.replace, os.fsync
    unsynced_bucket = None

    def record_index_rename(src, dst, **kwargs):
        nonlocal unsynced_bucket
        result = real_replace(src, dst, **kwargs)
        if dst == "index.json" and json.loads(current.read_bytes())["status"] == "passed":
            unsynced_bucket = os.fstat(kwargs["dst_dir_fd"])
        return result

    def fail_bucket_sync(fd):
        if unsynced_bucket is not None and os.path.samestat(os.fstat(fd), unsynced_bucket):
            raise OSError(errno.EIO, "fixture release index directory sync failed")
        return real_sync(fd)

    with monkeypatch.context() as fault:
        fault.setattr(os, "replace", record_index_rename)
        fault.setattr(os, "fsync", fail_bucket_sync)
        assert run("complete") == 2
        assert "fixture release index directory sync failed" in capsys.readouterr().err
        pending_sync = current.read_bytes()
        assert json.loads(pending_sync)["status"] == "passed"
        with pytest.raises(OSError, match="fixture release index directory sync failed"):
            CHECKER.require_current_release(
                root, json.loads(pending_sync), hashlib.sha256(pending_sync).hexdigest()
            )
        aggregate, _, ready = WRITER.publish_aggregate(
            root=root, registry_path=root / "tools/ci/proof-authority.toml", paired_checkout=pair
        )
        assert not ready
        assert json.loads(aggregate.read_bytes())["verdicts"]["DEPLOYED"]["status"] == "FAILED"
    passed = current.read_bytes()
    args = dict(
        root=root,
        proof_id=proof["id"],
        registry_path=root / "tools/ci/proof-authority.toml",
        schema_path=root / proof["artifact_schema"],
        paired_checkout=pair,
        terminal_input_path=current.parent / "complete/terminal.json",
    )
    # Resume from the signed terminal without performing the action again.
    _run(root, "git", "switch", "--detach", "HEAD")
    archived, _, status = writer.publish_manifest(**args)
    assert status == "passed" and archived.read_bytes() == passed
    _run(pair, "git", "switch", "--detach", "HEAD")
    assert writer.publish_manifest(**args)[0] == archived
    prior_terminal = args["terminal_input_path"].with_name("legacy-terminal.json")
    record = json.loads(args["terminal_input_path"].read_bytes())
    del record["source_binding"]
    prior_terminal.write_text(json.dumps(record))
    assert writer.publish_manifest(**{**args, "terminal_input_path": prior_terminal})[0] == archived
    state.write_text(json.dumps(pre))
    state.with_suffix(".interrupt").touch()
    assert run("interrupted") == 2
    assert "fixture post observation interrupted" in capsys.readouterr().err
    assert json.loads(state.read_bytes()) == post
    blocked = current.read_bytes()
    assert json.loads(blocked)["status"] == "blocked" and archived.read_bytes() == passed
    stale = CHECKER.check_manifest(
        json.loads(passed),
        proof=proof,
        schema=json.loads(args["schema_path"].read_bytes()),
        root=root,
        manifest_path=current,
        bind_source=True,
        proof_by_id={p["id"]: p for p in registry["proofs"]},
        paired_checkouts={proof["paired_repository"]: pair},
    )
    assert any("indexed release receipt differs from current alias" in f.message for f in stale)
    # Replaying the old input with its original checkout provenance cannot make it current again.
    _run(root, "git", "switch", "-")
    _run(pair, "git", "switch", "-")
    for have_alias in (True, False):
        if not have_alias:
            current.unlink()
        with pytest.raises(writer.ManifestRefused, match="advance the current execution window"):
            writer.publish_manifest(**args)
        if have_alias:
            assert current.read_bytes() == blocked
        else:
            assert not current.exists()
    current.write_bytes(passed)
    with pytest.raises(ValueError, match="latest indexed release"):
        CHECKER.require_current_release(
            root, json.loads(passed), hashlib.sha256(passed).hexdigest()
        )
    assert CHECKER.main(["--root", str(root), "--manifest", str(current)]) == 1
    assert CHECKER.main(["--root", str(root), "--manifest", str(archived)]) == 0
    stale_aggregate, _, ready = WRITER.publish_aggregate(
        root=root, registry_path=args["registry_path"], paired_checkout=pair
    )
    assert not ready
    assert json.loads(stale_aggregate.read_bytes())["verdicts"]["DEPLOYED"]["status"] == "FAILED"
    current.write_bytes(blocked)
    aggregate, _, ready = WRITER.publish_aggregate(
        root=root, registry_path=args["registry_path"], paired_checkout=pair
    )
    assert (
        not ready
        and json.loads(aggregate.read_bytes())["verdicts"]["DEPLOYED"]["status"] != "PASSED"
    )


@pytest.mark.parametrize(
    ("destination", "abrupt"), [("alias", False), ("index.json", False), ("index.json", True)]
)
def test_release_receipt_interruption_keeps_prior_selection_and_allows_retry(
    tmp_path, aggregate_templates, monkeypatch, destination, abrupt
):
    from tools.ci.proof_operational_result import _writer

    root, registry = _fixture_root(tmp_path, executable=True, templates=aggregate_templates)
    pair = _paired_checkout(tmp_path, aggregate_templates)
    _write_dependency_manifests(root, registry, pair)
    proof = next(p for p in registry["proofs"] if p["id"] == "p11-deployment")
    writer = _writer()
    current = root / proof["artifact"]
    previous = current.read_bytes()
    payload = json.loads(previous)
    now = datetime.now(timezone.utc).isoformat()
    terminal = current.parent / "receipt-retry-input.json"
    terminal.write_text(
        json.dumps(
            {
                "status": "blocked",
                "counts": dict.fromkeys(payload["counts"], 0),
                "environment": {**payload["environment"], "host": RELEASE_HOST_INPUT},
                "daemon_binary": payload["daemon_binary"]["source_path"],
                "state_root_format": payload["state_root_format"],
                "inputs": {
                    **dict.fromkeys(writer.INPUT_NAMES),
                    "config": {"path": proof["operational_contract"]},
                },
                "started_at": now,
                "ended_at": now,
                "artifacts": [proof["operational_contract"]],
            }
        )
    )
    real_replace = os.replace
    matched_name = current.name if destination == "alias" else destination

    def fail_replace(src, dst, **kwargs):
        if dst == matched_name:
            raise OSError(errno.ENOSPC, "fixture disk full")
        return real_replace(src, dst, **kwargs)

    monkeypatch.setattr(writer.platform, "system", lambda: "Linux")
    monkeypatch.setattr(writer.platform, "machine", lambda: "x86_64")
    args = dict(
        root=root,
        proof_id=proof["id"],
        registry_path=root / "tools/ci/proof-authority.toml",
        schema_path=root / proof["artifact_schema"],
        terminal_input_path=terminal,
        paired_checkout=pair,
    )
    if abrupt:
        child = subprocess.run(
            [
                sys.executable, "-B", "-c", '''
import json, os, sys
from pathlib import Path
from unittest.mock import patch
from tools.ci.proof_operational_result import _writer

arguments = json.loads(sys.argv[1])
arguments = {key: value if key == "proof_id" else Path(value) for key, value in arguments.items()}
replace = os.replace

def interrupt(src, dst, **kwargs):
    if dst == "index.json":
        os._exit(75)
    return replace(src, dst, **kwargs)

writer = _writer()
with patch.object(writer.platform, "system", return_value="Linux"), \\
     patch.object(writer.platform, "machine", return_value="x86_64"), \\
     patch.object(os, "replace", side_effect=interrupt):
    writer.publish_manifest(**arguments)
''',
                json.dumps({key: str(value) for key, value in args.items()}),
            ],
            cwd=REPO_ROOT,
            capture_output=True,
            text=True,
            check=False,
        )
        assert child.returncode == 75, child.stderr
        staged = current.read_bytes()
        assert json.loads(staged)["status"] == "blocked"
        with pytest.raises(ValueError, match="latest indexed release"):
            CHECKER.require_current_release(root, json.loads(staged), hashlib.sha256(staged).hexdigest())
    else:
        with monkeypatch.context() as fault:
            fault.setattr(os, "replace", fail_replace)
            with pytest.raises(OSError, match="fixture disk full"):
                writer.publish_manifest(**args)
        assert current.read_bytes() == previous
    CHECKER.require_current_release(root, payload, hashlib.sha256(previous).hexdigest())
    archive, digest, status = writer.publish_manifest(**args)
    assert status == "blocked" and current.read_bytes() == archive.read_bytes()
    CHECKER.require_current_release(root, json.loads(current.read_bytes()), digest)


def test_replay_uses_original_release_contract_after_valid_registry_change(
    tmp_path, aggregate_templates
):
    root, registry = _fixture_root(tmp_path, executable=True, templates=aggregate_templates)
    pair = _paired_checkout(tmp_path, aggregate_templates)
    _write_dependency_manifests(root, registry, pair)
    args = ["--root", str(root)]
    for proof in registry["proofs"]:
        if proof["execution_mode"] == "operational-action":
            raw = (root / proof["artifact"]).read_bytes()
            archived = CHECKER.proof_archive_relative_path(
                json.loads(raw), hashlib.sha256(raw).hexdigest()
            )
            args.extend(("--manifest", archived))
    assert CHECKER.main(args) == 0

    deployment = next(p for p in registry["proofs"] if p["id"] == "p11-deployment")
    old_path = root / deployment["operational_contract"]
    new_path = old_path.with_suffix(".relocated.json")
    old_path.rename(new_path)
    old_schema = root / deployment["artifact_schema"]
    new_schema = old_schema.with_name("relocated-proof.schema.json")
    old_schema.rename(new_schema)
    new_schema.write_text('{"not": {}}\n')
    registry_path = root / "tools/ci/proof-authority.toml"
    registry_path.write_text(
        registry_path.read_text().replace(
            f'"{old_path.relative_to(root)}"', f'"{new_path.relative_to(root)}"'
        ).replace(
            f'"{old_schema.relative_to(root)}"', f'"{new_schema.relative_to(root)}"'
        )
    )
    _run(root, "git", "add", "-A", "tools/ci")
    _run(root, "git", "commit", "-qm", "Relocate the fixture deployment contract")
    assert (
        CHECKER.check_registry(CHECKER._read_toml(registry_path), root=root, path=registry_path) == []
    )
    assert CHECKER.main(args) == 0
    assert CHECKER.main([*args, "--schema", str(new_schema)]) == 0
    assert CHECKER.main(["--root", str(root), "--manifest", deployment["artifact"]]) == 1
