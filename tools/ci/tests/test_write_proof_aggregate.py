"""Owner-local tests for the registry-derived P12 aggregate writer."""

from __future__ import annotations

import copy
import hashlib
import importlib.util
import json
import re
import shutil
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path

import pytest

REPO_ROOT = Path(__file__).resolve().parents[3]
WRITER_PATH = REPO_ROOT / "tools/ci/write-proof-aggregate.py"
CHECKER_PATH = REPO_ROOT / "tools/ci/lint/check-proof-authority.py"
REGISTRY_PATH = REPO_ROOT / "tools/ci/proof-authority.toml"
MANIFEST_SCHEMA_PATH = REPO_ROOT / "tools/ci/proof-manifest.schema.json"
AGGREGATE_SCHEMA_PATH = REPO_ROOT / "tools/ci/proof-aggregate.schema.json"
MANIFEST_WRITER_PATH = REPO_ROOT / "tools/ci/write-proof-manifest.py"


def _load_module(name: str, path: Path):
    spec = importlib.util.spec_from_file_location(name, path)
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


WRITER = _load_module("write_proof_aggregate", WRITER_PATH)
CHECKER = _load_module("write_proof_aggregate_checker", CHECKER_PATH)
MANIFEST_WRITER = _load_module("aggregate_test_manifest_writer", MANIFEST_WRITER_PATH)

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
        r'authority_state = "staged"\nexecution_mode = "test-authority"\n'
        r'staged_reason = "[^"]*"',
        'authority_state = "executable"\nexecution_mode = "test-authority"',
        text,
    )
    text = re.sub(
        r'authority_state = "staged"\nexecution_mode = "aggregate"\n'
        r'staged_reason = "[^"]*"',
        'authority_state = "executable"\nexecution_mode = "aggregate"',
        text,
    )
    sections = text.split("[[proofs]]")
    for index in range(1, len(sections)):
        if 'execution_mode = "test-authority"' in sections[index]:
            proof_id = re.search(r'^\nid = "([^"]+)"', sections[index])
            assert proof_id is not None
            sections[index] = re.sub(
                r'command = "[^"]+"',
                f'command = "just proof-fixture-{proof_id.group(1)}"',
                sections[index],
                count=1,
            )
            sections[index] = sections[index].replace(
                "test_authority_targets = []",
                'test_authority_targets = ["catalog-idempotency"]',
            )
            if "test_authority_scopes = " in sections[index]:
                sections[index] = re.sub(
                    r"test_authority_scopes = \[[^\]]*\]",
                    'test_authority_scopes = ["fixture-all"]',
                    sections[index],
                    count=1,
                )
            else:
                sections[index] = sections[index].replace(
                    "test_authority_targets = ",
                    'test_authority_scopes = ["fixture-all"]\ntest_authority_targets = ',
                    1,
                )
    return "[[proofs]]".join(sections)


def _build_paired_checkout(tmp_path: Path) -> Path:
    checkout = tmp_path / "semantica"
    checkout.mkdir(parents=True)
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


def _build_fixture_root(tmp_path: Path, *, executable: bool) -> tuple[Path, dict]:
    root = tmp_path / "repo"
    root.mkdir(parents=True)
    registry_text = REGISTRY_PATH.read_text(encoding="utf-8")
    if executable:
        registry_text = _make_all_proofs_executable(registry_text)
    registry_path = root / "tools/ci/proof-authority.toml"
    registry_path.parent.mkdir(parents=True)
    registry_path.write_text(registry_text, encoding="utf-8")
    shutil.copyfile(MANIFEST_SCHEMA_PATH, root / "tools/ci/proof-manifest.schema.json")
    shutil.copyfile(AGGREGATE_SCHEMA_PATH, root / "tools/ci/proof-aggregate.schema.json")
    registry = CHECKER._read_toml(registry_path)
    if executable:
        source_catalog = CHECKER._read_toml(REPO_ROOT / "tools/ci/test-authority.toml")
        source_targets = {
            row["id"]: row
            for category in ("integration_targets", "python_targets")
            for row in source_catalog.get(category, [])
        }
        target_ids = sorted(
            {target for proof in registry["proofs"] for target in proof["test_authority_targets"]}
        )
        (root / "tools/ci/test-authority.toml").write_text(
            "[local_scopes.fixture-all]\n"
            f"targets = {json.dumps(target_ids)}\n"
            + "".join(
                "[[integration_targets]]\n"
                f'id = "{target}"\n'
                f'path = "{source_targets[target]["path"]}"\n'
                f'owner = "{source_targets[target]["owner"]}"\n'
                for target in target_ids
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
            proof_recipes.append(f"{recipe}:\n    @just rust-profile test-fixture-all\n")
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
    binary = root / "bin/searchd"
    binary.parent.mkdir()
    binary.write_bytes(b"release-daemon")
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


def _stub_verified_handoffs(monkeypatch: pytest.MonkeyPatch) -> dict:
    """Isolate aggregate derivation; real handoff validation has owner tests."""
    lanes = CHECKER.HANDOFF_VALIDATION.PRODUCT_LANES
    ledger = {
        "product_handoffs": [
            {
                "lane": lane,
                "path": f"artifacts/sep-21/handoffs/{lane}.json",
                "sha256": "a" * 64,
                "status": "VERIFIED",
            }
            for lane in lanes
        ],
        "product_chain_status": "VERIFIED",
        "infrastructure_handoff": {
            "lane": "P12A",
            "path": "artifacts/sep-21/handoffs/P12A.json",
            "sha256": "b" * 64,
            "status": "VERIFIED",
        },
    }
    monkeypatch.setattr(WRITER, "_load_checker", lambda: CHECKER)
    monkeypatch.setattr(MANIFEST_WRITER, "_load_checker", lambda: CHECKER)
    monkeypatch.setattr(
        CHECKER.HANDOFF_VALIDATION,
        "inspect_handoff_ledger",
        lambda **_kwargs: (ledger, []),
    )
    return ledger


def _write_dependency_manifests(
    root: Path,
    registry: dict,
    paired: Path,
    *,
    release_host_digest_overrides: dict[str, str] | None = None,
) -> None:
    release_host_digest_overrides = release_host_digest_overrides or {}
    proof_by_id = {proof["id"]: proof for proof in registry["proofs"]}
    target_catalog = CHECKER._read_toml(root / "tools/ci/test-authority.toml")
    target_by_id = {row["id"]: row for row in target_catalog.get("integration_targets", [])}
    source_snapshots: dict[tuple[Path, tuple[Path, ...], Path | None], dict] = {}
    paired_snapshots: dict[tuple[str, str], dict] = {}
    for proof_id in CHECKER.aggregate_proof_ids(registry):
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
        if proof["execution_mode"] == "test-authority":
            inventory_suites = {}
            events = []
            targets = [target_by_id[target_id] for target_id in proof["test_authority_targets"]]
            result_count = len(targets)
            for target in targets:
                package = target["owner"]
                binary = Path(target["path"]).stem
                metadata = {"crate": package, "test_binary": binary, "kind": "test"}
                name = f"{package}::{binary}$passes"
                inventory_suites[f"{package}::{binary}"] = {
                    "package-name": package,
                    "binary-name": binary,
                    "kind": "test",
                    "status": "listed",
                    "testcases": {
                        "passes": {"ignored": False, "filter-match": {"status": "matches"}}
                    },
                }
                events.extend(
                    [
                        {"type": "suite", "event": "started", "test_count": 1, "nextest": metadata},
                        {"type": "test", "event": "started", "name": name},
                        {"type": "test", "event": "ok", "name": name},
                        {
                            "type": "suite",
                            "event": "ok",
                            "passed": 1,
                            "failed": 0,
                            "ignored": 0,
                            "nextest": metadata,
                        },
                    ]
                )
            raw_root = evidence.parent
            sources = {
                "events": raw_root / f"{proof_id}.jsonl",
                "inventory": raw_root / f"{proof_id}-inventory.json",
            }
            sources["events"].write_text(
                "".join(json.dumps(event) + "\n" for event in events), encoding="utf-8"
            )
            sources["inventory"].write_text(
                json.dumps({"test-count": result_count, "rust-suites": inventory_suites}),
                encoding="utf-8",
            )
            for source in sources.values():
                digest = _digest(source)
                archive = root / CHECKER.content_archive_relative_path("evidence", digest)
                archive.parent.mkdir(parents=True, exist_ok=True)
                archive.write_bytes(source.read_bytes())
                result_artifacts.append(
                    {
                        "source_path": source.relative_to(root).as_posix(),
                        "path": archive.relative_to(root).as_posix(),
                        "sha256": digest,
                    }
                )
            execution_result = {
                "schema_version": 1,
                "runs": [
                    {
                        "format": "nextest-jsonl",
                        "events": sources["events"].relative_to(root).as_posix(),
                        "inventory": sources["inventory"].relative_to(root).as_posix(),
                    }
                ],
            }
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
                "features": [],
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
                "provider": None,
            },
            "started_at": "2026-09-21T00:00:00Z",
            "ended_at": "2026-09-21T00:00:01Z",
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
        if execution_result is not None:
            payload["execution_result"] = execution_result
        manifest_path.parent.mkdir(parents=True, exist_ok=True)
        manifest_bytes = (json.dumps(payload, sort_keys=True) + "\n").encode()
        manifest_path.write_bytes(manifest_bytes)
        manifest_digest = hashlib.sha256(manifest_bytes).hexdigest()
        archive_path = root / CHECKER.proof_archive_relative_path(payload, manifest_digest)
        archive_path.parent.mkdir(parents=True, exist_ok=True)
        archive_path.write_bytes(manifest_bytes)


def _inject_verified_handoff_ledger(monkeypatch: pytest.MonkeyPatch) -> None:
    """Isolate aggregate composition from the separately tested handoff validator."""
    lanes = (
        "P00",
        "P01",
        "P02A",
        "P02B",
        "P02I",
        "P03",
        "P04",
        "P05",
        "P06",
        "P07",
        "P08",
        "P09",
        "P10",
        "P11",
    )

    def reference(lane: str) -> dict[str, str]:
        return {
            "lane": lane,
            "path": f"artifacts/sep-21/handoffs/{lane}.json",
            "sha256": hashlib.sha256(f"fixture-{lane}".encode()).hexdigest(),
            "status": "VERIFIED",
        }

    ledger = {
        "product_handoffs": [reference(lane) for lane in lanes],
        "product_chain_status": "VERIFIED",
        "infrastructure_handoff": reference("P12A"),
    }
    monkeypatch.setattr(WRITER, "_load_checker", lambda: CHECKER)
    monkeypatch.setattr(MANIFEST_WRITER, "_load_checker", lambda: CHECKER)
    monkeypatch.setattr(
        CHECKER.HANDOFF_VALIDATION,
        "inspect_handoff_ledger",
        lambda **_kwargs: (copy.deepcopy(ledger), []),
    )


def test_full_dependency_graph_without_handoffs_is_not_ready(
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
    assert not ready
    assert all(item["status"] == "PASSED" for item in payload["dependency_receipts"])
    assert payload["product_chain_status"] == "NOT_RUN"
    assert payload["infrastructure_handoff"]["status"] == "NOT_RUN"
    assert payload["production_ready"] is False


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


def test_writer_derives_ready_receipt_from_verified_handoff_input(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    aggregate_templates: AggregateTemplates,
) -> None:
    _inject_verified_handoff_ledger(monkeypatch)
    root, registry = _fixture_root(tmp_path, executable=True, templates=aggregate_templates)
    paired = _paired_checkout(tmp_path, aggregate_templates)
    _write_dependency_manifests(root, registry, paired)
    _stub_verified_handoffs(monkeypatch)

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


def test_full_proof_closure_without_historical_handoffs_is_not_ready(
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
    assert not ready
    assert all(item["status"] == "PASSED" for item in payload["dependency_receipts"])
    assert all(item["status"] == "PASSED" for item in payload["verdicts"].values())
    assert payload["product_chain_status"] == "NOT_RUN"
    assert payload["infrastructure_handoff"]["status"] == "NOT_RUN"
    assert payload["production_ready"] is False

    forged = dict(payload, product_chain_status="VERIFIED", production_ready=True)
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
    assert any("product_chain_status is not derived" in item.message for item in findings)
    assert any("production_ready is not derived" in item.message for item in findings)


@pytest.mark.parametrize("missing_axis", ["product", "infrastructure"])
def test_complete_proof_verdicts_cannot_bypass_either_handoff_ledger(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    aggregate_templates: AggregateTemplates,
    missing_axis: str,
) -> None:
    root, registry = _fixture_root(tmp_path, executable=True, templates=aggregate_templates)
    paired = _paired_checkout(tmp_path, aggregate_templates)
    _write_dependency_manifests(root, registry, paired)
    ledger = _stub_verified_handoffs(monkeypatch)
    if missing_axis == "product":
        ledger["product_chain_status"] = "FAILED"
    else:
        ledger["infrastructure_handoff"]["status"] = "NOT_RUN"
        ledger["infrastructure_handoff"]["sha256"] = None

    output, _, ready = WRITER.publish_aggregate(
        root=root,
        registry_path=root / "tools/ci/proof-authority.toml",
        paired_checkout=paired,
    )
    payload = json.loads(output.read_text(encoding="utf-8"))
    assert not ready
    assert all(item["status"] == "PASSED" for item in payload["verdicts"].values())
    assert payload["production_ready"] is False


def test_writer_refuses_stale_p12a_exact_pair_after_paired_source_change(
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
    assert statuses["p12a-proof-infrastructure"] == "FAILED"
    assert payload["production_ready"] is False


def test_aggregate_validation_refuses_source_change_during_cached_pass(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    aggregate_templates: AggregateTemplates,
) -> None:
    _inject_verified_handoff_ledger(monkeypatch)
    root, registry = _fixture_root(tmp_path, executable=True, templates=aggregate_templates)
    paired = _paired_checkout(tmp_path, aggregate_templates)
    _write_dependency_manifests(root, registry, paired)
    _stub_verified_handoffs(monkeypatch)
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


def test_writer_publishes_failed_diagnostic_for_cross_manifest_host_drift(
    tmp_path: Path,
    aggregate_templates: AggregateTemplates,
) -> None:
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
    assert not ready
    assert payload["production_ready"] is False
    assert payload["release_host"] is None
    assert payload["verdicts"]["CODE_QUALIFIED"]["status"] == "FAILED"


def test_ready_aggregate_is_mandatory_and_sufficient_for_p12_issuance(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    aggregate_templates: AggregateTemplates,
) -> None:
    _inject_verified_handoff_ledger(monkeypatch)
    root, registry = _fixture_root(tmp_path, executable=True, templates=aggregate_templates)
    paired = _paired_checkout(tmp_path, aggregate_templates)
    _write_dependency_manifests(root, registry, paired)
    _stub_verified_handoffs(monkeypatch)
    aggregate_path, _, ready = WRITER.publish_aggregate(
        root=root,
        registry_path=root / "tools/ci/proof-authority.toml",
        paired_checkout=paired,
    )
    assert ready
    terminal = {
        "status": "passed",
        "counts": {"selected": 1, "executed": 1, "passed": 1, "failed": 0, "ignored": 0},
        "environment": {
            "toolchain": "fixture",
            "features": [],
            "os": "linux",
            "arch": "x86_64",
            "host": RELEASE_HOST_INPUT,
        },
        "daemon_binary": "bin/searchd",
        "state_root_format": "v2",
        "inputs": {
            "fixture": None,
            "corpus": None,
            "config": None,
            "model": None,
            "provider": None,
        },
        "started_at": "2026-09-21T00:00:02Z",
        "ended_at": "2026-09-21T00:00:03Z",
        "artifacts": [aggregate_path.relative_to(root).as_posix()],
    }
    terminal_path = root / "artifacts/proof-authority/raw/p12-terminal.json"
    terminal_path.write_text(json.dumps(terminal), encoding="utf-8")
    monkeypatch.setattr(MANIFEST_WRITER.platform, "system", lambda: "Linux")
    monkeypatch.setattr(MANIFEST_WRITER.platform, "machine", lambda: "x86_64")

    output, _, status = MANIFEST_WRITER.publish_manifest(
        root=root,
        registry_path=root / "tools/ci/proof-authority.toml",
        schema_path=root / "tools/ci/proof-manifest.schema.json",
        proof_id="p12-final-qualification",
        terminal_input_path=terminal_path,
        paired_checkout=paired,
    )

    payload = json.loads(output.read_text(encoding="utf-8"))
    assert status == "passed"
    assert payload["artifacts"] == [
        {
            "source_path": registry["aggregate"]["artifact"],
            "path": CHECKER.content_archive_relative_path("evidence", _digest(aggregate_path)),
            "sha256": _digest(aggregate_path),
        }
    ]


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


def test_p12_guard_refuses_an_unregistered_terminal_artifact(
    tmp_path: Path,
    aggregate_templates: AggregateTemplates,
) -> None:
    root, registry = _fixture_root(tmp_path, executable=False, templates=aggregate_templates)
    proof = next(proof for proof in registry["proofs"] if proof["id"] == "p12-final-qualification")
    payload = {
        "status": "passed",
        "counts": {"selected": 1, "executed": 1, "passed": 1, "failed": 0, "ignored": 0},
        "artifacts": [
            {
                "source_path": "artifacts/proof-authority/raw/not-aggregate.log",
                "path": "artifacts/proof-authority/evidence/" + "0" * 64,
                "sha256": "0" * 64,
            }
        ],
    }

    with pytest.raises(
        MANIFEST_WRITER.ManifestRefused,
        match="must attest the registered aggregate artifact",
    ):
        MANIFEST_WRITER._validate_aggregate_issuance(
            root=root,
            registry=registry,
            registry_path=root / "tools/ci/proof-authority.toml",
            proof=proof,
            payload=payload,
            checker=CHECKER,
            paired_checkout=None,
        )


def test_p12_guard_refuses_registered_not_ready_aggregate(
    tmp_path: Path,
    aggregate_templates: AggregateTemplates,
) -> None:
    root, registry = _fixture_root(tmp_path, executable=False, templates=aggregate_templates)
    paired = _paired_checkout(tmp_path, aggregate_templates)
    aggregate_path, aggregate_digest, ready = WRITER.publish_aggregate(
        root=root,
        registry_path=root / "tools/ci/proof-authority.toml",
        paired_checkout=paired,
    )
    assert not ready
    proof = next(proof for proof in registry["proofs"] if proof["id"] == "p12-final-qualification")
    payload = {
        "status": "passed",
        "counts": {"selected": 1, "executed": 1, "passed": 1, "failed": 0, "ignored": 0},
        "artifacts": [
            {
                "source_path": aggregate_path.relative_to(root).as_posix(),
                "path": "artifacts/proof-authority/evidence/" + aggregate_digest,
                "sha256": aggregate_digest,
            }
        ],
    }

    with pytest.raises(MANIFEST_WRITER.ManifestRefused, match="not authoritative"):
        MANIFEST_WRITER._validate_aggregate_issuance(
            root=root,
            registry=registry,
            registry_path=root / "tools/ci/proof-authority.toml",
            proof=proof,
            payload=payload,
            checker=CHECKER,
            paired_checkout=paired,
        )
