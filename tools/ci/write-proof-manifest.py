#!/usr/bin/env python3
"""Atomically publish a registry-bound, terminal SEP-21 proof manifest."""

from __future__ import annotations

import argparse
import fcntl
import hashlib
import importlib.util
import json
import os
import platform
import subprocess
import sys
from pathlib import Path
from types import ModuleType
from typing import Any

import jsonschema

try:
    import tomllib
except ModuleNotFoundError:  # pragma: no cover - Python 3.10 compatibility
    import tomli as tomllib


ROOT = Path(__file__).resolve().parents[2]
if str(ROOT) not in sys.path:
    sys.path.insert(0, str(ROOT))
from tools.ci.proof_json import parse_proof_json  # noqa: E402

CHECKER_PATH = ROOT / "tools/ci/lint/check-proof-authority.py"
REGISTRY_PATH = ROOT / "tools/ci/proof-authority.toml"
SCHEMA_PATH = ROOT / "tools/ci/proof-manifest.schema.json"
INPUT_NAMES = frozenset(("fixture", "corpus", "config", "model", "provider"))
ERROR_INVENTORY_PATH = "artifacts/sep-21/p00/error-authority-inventory.json"
ERROR_INVENTORY_WRITER = ROOT / "tools/ci/write-error-authority-inventory.py"
ERROR_INVENTORY_SCHEMA = ROOT / "tools/ci/error-authority-inventory.schema.json"


class ManifestRefused(ValueError):
    """The requested receipt cannot be published as authoritative proof."""


def _load_checker(path: Path = CHECKER_PATH) -> ModuleType:
    spec = importlib.util.spec_from_file_location("quanta_check_proof_authority", path)
    if spec is None or spec.loader is None:
        raise ManifestRefused(f"cannot load semantic validator: {path}")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def _load_module(name: str, path: Path) -> ModuleType:
    spec = importlib.util.spec_from_file_location(name, path)
    if spec is None or spec.loader is None:
        raise ManifestRefused(f"cannot load authority module: {path}")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def _json_object_bytes(content: bytes, *, label: str) -> dict[str, Any]:
    try:
        value = parse_proof_json(content)
    except ValueError as error:
        raise ManifestRefused(f"{label} is not valid JSON: {error}") from error
    if not isinstance(value, dict):
        raise ManifestRefused(f"{label} root must be an object")
    return value


def _external_regular_bytes(path: Path, *, label: str, checker: ModuleType) -> bytes:
    absolute = Path(os.path.abspath(path))
    try:
        return checker.HANDOFF_VALIDATION._read_repo_regular_bytes(
            Path("/"), absolute.relative_to(Path("/")).as_posix(), label=label
        )
    except (OSError, ValueError) as error:
        raise ManifestRefused(f"{label} must be a regular non-symlink file: {error}") from error


def _require_exact_keys(value: dict[str, Any], expected: set[str], *, label: str) -> None:
    actual = set(value)
    if actual != expected:
        missing = sorted(expected - actual)
        extra = sorted(actual - expected)
        raise ManifestRefused(f"{label} keys differ: missing={missing} extra={extra}")


def _repo_bytes(root: Path, value: Any, *, label: str, checker: ModuleType) -> tuple[str, bytes]:
    if not isinstance(value, str) or not value:
        raise ManifestRefused(f"{label} must be a non-empty repo-relative path")
    try:
        checker.HANDOFF_VALIDATION._repo_path(root, value, label=label)
        content = checker.HANDOFF_VALIDATION._read_repo_regular_bytes(root, value, label=label)
    except (OSError, ValueError) as error:
        raise ManifestRefused(
            f"{label} must be a regular non-symlink repo file: {error}"
        ) from error
    return value, content


def _digest_input(
    root: Path, value: Any, *, label: str, checker: ModuleType | None = None
) -> str | None:
    if value is None:
        return None
    if not isinstance(value, dict):
        raise ManifestRefused(f"{label} must be null, {{path}}, or {{value}}")
    if set(value) == {"path"}:
        _, content = _repo_bytes(
            root,
            value["path"],
            label=f"{label}.path",
            checker=checker if checker is not None else _load_checker(),
        )
        return f"sha256:{hashlib.sha256(content).hexdigest()}"
    if set(value) == {"value"} and isinstance(value["value"], str) and value["value"]:
        digest = hashlib.sha256(value["value"].encode()).hexdigest()
        return f"sha256:{digest}"
    raise ManifestRefused(f"{label} must contain exactly one valid path or non-empty value")


def _host_environment(value: Any) -> dict[str, Any]:
    if not isinstance(value, dict):
        raise ManifestRefused("terminal.environment must be an object")
    _require_exact_keys(
        value,
        {"toolchain", "features", "os", "arch", "host"},
        label="terminal.environment",
    )
    if not isinstance(value["toolchain"], str) or not value["toolchain"]:
        raise ManifestRefused("terminal.environment.toolchain must be non-empty")
    features = value["features"]
    if not isinstance(features, list) or any(
        not isinstance(feature, str) or not feature for feature in features
    ):
        raise ManifestRefused("terminal.environment.features must be non-empty strings")
    if len(features) != len(set(features)):
        raise ManifestRefused("terminal.environment.features must be unique")
    actual_os = platform.system().lower()
    actual_arch = platform.machine()
    if value["os"] != actual_os:
        raise ManifestRefused(
            f"terminal.environment.os is {value['os']!r}, current host is {actual_os!r}"
        )
    if value["arch"] != actual_arch:
        raise ManifestRefused(
            f"terminal.environment.arch is {value['arch']!r}, current host is {actual_arch!r}"
        )
    host = value["host"]
    if not isinstance(host, dict):
        raise ManifestRefused("terminal.environment.host must be an object")
    _require_exact_keys(
        host,
        {"profile", "cpu_count", "memory_bytes", "identity"},
        label="terminal.environment.host",
    )
    if not isinstance(host["profile"], str) or not host["profile"]:
        raise ManifestRefused("terminal.environment.host.profile must be non-empty")
    if not isinstance(host["cpu_count"], int) or isinstance(host["cpu_count"], bool):
        raise ManifestRefused("terminal.environment.host.cpu_count must be an integer")
    if host["cpu_count"] < 1:
        raise ManifestRefused("terminal.environment.host.cpu_count must be positive")
    if not isinstance(host["memory_bytes"], int) or isinstance(host["memory_bytes"], bool):
        raise ManifestRefused("terminal.environment.host.memory_bytes must be an integer")
    if host["memory_bytes"] < 1:
        raise ManifestRefused("terminal.environment.host.memory_bytes must be positive")
    if not isinstance(host["identity"], str) or not host["identity"]:
        raise ManifestRefused("terminal.environment.host.identity must be non-empty")
    identity_source = json.dumps(
        {
            "arch": actual_arch,
            "cpu_count": host["cpu_count"],
            "identity": host["identity"],
            "memory_bytes": host["memory_bytes"],
            "os": actual_os,
            "profile": host["profile"],
        },
        sort_keys=True,
        separators=(",", ":"),
    ).encode()
    return {
        "toolchain": value["toolchain"],
        "features": sorted(features),
        "os": actual_os,
        "arch": actual_arch,
        "host": {
            "profile": host["profile"],
            "cpu_count": host["cpu_count"],
            "memory_bytes": host["memory_bytes"],
            "identity_digest": f"sha256:{hashlib.sha256(identity_source).hexdigest()}",
        },
    }


def _publish_content_archive(
    *, root: Path, checker: ModuleType, kind: str, content: bytes, digest: str
) -> str:
    relative = checker.content_archive_relative_path(kind, digest)
    parent_fd = _open_output_parent(root, relative, checker=checker)
    name = Path(relative).name
    temporary_name: str | None = None
    try:
        existing = _read_output(parent_fd, name, checker=checker)
        if existing is not None:
            if hashlib.sha256(existing).hexdigest() != digest:
                raise ManifestRefused(f"immutable {kind} archive collision: {relative}")
            _require_parent_identity(root, relative, parent_fd, checker=checker)
            return relative
        if hashlib.sha256(content).hexdigest() != digest:
            raise ManifestRefused(f"{kind} source changed during archival")
        temporary_name = checker.HANDOFF_VALIDATION._write_output_temporary(
            parent_fd, name, content
        )
        _require_parent_identity(root, relative, parent_fd, checker=checker)
        try:
            os.link(
                temporary_name,
                name,
                src_dir_fd=parent_fd,
                dst_dir_fd=parent_fd,
                follow_symlinks=False,
            )
        except FileExistsError as error:
            existing = _read_output(parent_fd, name, checker=checker)
            if existing is None or hashlib.sha256(existing).hexdigest() != digest:
                raise ManifestRefused(f"immutable {kind} archive collision: {relative}") from error
        os.fsync(parent_fd)
        _require_parent_identity(root, relative, parent_fd, checker=checker)
    finally:
        if temporary_name is not None:
            os.unlink(temporary_name, dir_fd=parent_fd)
        os.close(parent_fd)
    return relative


def _resolve_binary(
    root: Path, proof: dict[str, Any], value: Any, *, checker: ModuleType
) -> dict[str, str] | None:
    binding = proof["binary_binding"]
    if binding == "none":
        if value is not None:
            raise ManifestRefused("binary_binding=none requires terminal.daemon_binary=null")
        return None
    if binding != "release-daemon":
        raise ManifestRefused(f"unknown binary binding: {binding!r}")
    canonical, content = _repo_bytes(root, value, label="terminal.daemon_binary", checker=checker)
    digest = hashlib.sha256(content).hexdigest()
    return {
        "source_path": canonical,
        "path": _publish_content_archive(
            root=root, checker=checker, kind="binary", content=content, digest=digest
        ),
        "sha256": digest,
    }


def _resolve_source_pair(
    proof: dict[str, Any],
    value: Any,
    *,
    checker: ModuleType,
) -> dict[str, Any] | None:
    binding = proof["source_binding"]
    if binding == "exact":
        if value is not None:
            raise ManifestRefused("source_binding=exact forbids --paired-checkout")
        return None
    if binding != "exact-pair":
        raise ManifestRefused(f"unknown source binding: {binding!r}")
    if not isinstance(value, str) or not value:
        raise ManifestRefused(
            "source_binding=exact-pair requires a non-empty --paired-checkout path"
        )
    repository = proof.get("paired_repository")
    dependency_lock = proof.get("paired_dependency_lock")
    if not isinstance(repository, str) or not repository:
        raise ManifestRefused("exact-pair proof has no paired_repository authority")
    if not isinstance(dependency_lock, str) or not dependency_lock:
        raise ManifestRefused("exact-pair proof has no paired_dependency_lock authority")
    checkout = Path(value).expanduser().resolve()
    if not checkout.is_dir():
        raise ManifestRefused(f"paired checkout is missing: {checkout}")
    try:
        return checker.paired_source_snapshot(
            checkout,
            repository=repository,
            dependency_lock=Path(dependency_lock),
        )
    except (OSError, RuntimeError, ValueError) as error:
        raise ManifestRefused(f"cannot bind paired checkout: {error}") from error


def _resolve_artifacts(
    root: Path, value: Any, *, output_path: Path, checker: ModuleType
) -> list[dict[str, str]]:
    if not isinstance(value, list) or not value:
        raise ManifestRefused("terminal.artifacts must contain at least one artifact path")
    artifacts: list[dict[str, str]] = []
    seen: set[str] = set()
    for index, item in enumerate(value):
        canonical, content = _repo_bytes(
            root, item, label=f"terminal.artifacts[{index}]", checker=checker
        )
        if root / canonical == output_path:
            raise ManifestRefused("proof manifest cannot attest its own digest")
        if canonical in seen:
            raise ManifestRefused(f"duplicate terminal artifact: {canonical}")
        seen.add(canonical)
        digest = hashlib.sha256(content).hexdigest()
        artifacts.append(
            {
                "source_path": canonical,
                "path": _publish_content_archive(
                    root=root,
                    checker=checker,
                    kind="evidence",
                    content=content,
                    digest=digest,
                ),
                "sha256": digest,
            }
        )
    return artifacts


def _resolve_dependencies(
    root: Path,
    proof: dict[str, Any],
    proof_by_id: dict[str, dict[str, Any]],
    checker: ModuleType,
) -> list[dict[str, str]]:
    receipts: list[dict[str, str]] = []
    for proof_id in proof["dependencies"]:
        dependency = proof_by_id.get(proof_id)
        if dependency is None:
            raise ManifestRefused(f"unknown dependency proof: {proof_id}")
        _, content = _repo_bytes(
            root,
            dependency["artifact"],
            label=f"dependency {proof_id}",
            checker=checker,
        )
        payload = _json_object_bytes(content, label=f"dependency {proof_id}")
        if payload.get("proof_id") != proof_id:
            raise ManifestRefused(f"dependency {proof_id} current alias has the wrong proof_id")
        manifest_digest = hashlib.sha256(content).hexdigest()
        try:
            archive_relative = checker.proof_archive_relative_path(payload, manifest_digest)
        except (KeyError, TypeError, ValueError) as error:
            raise ManifestRefused(
                f"dependency {proof_id} archive identity is invalid: {error}"
            ) from error
        _, archive_content = _repo_bytes(
            root,
            archive_relative,
            label=f"dependency {proof_id} immutable archive",
            checker=checker,
        )
        if archive_content != content:
            raise ManifestRefused(
                f"dependency {proof_id} current alias differs from its immutable archive"
            )
        receipts.append({"proof_id": proof_id, "path": archive_relative, "sha256": manifest_digest})
    return receipts


def build_manifest(
    *,
    root: Path,
    proof: dict[str, Any],
    proof_by_id: dict[str, dict[str, Any]],
    terminal: dict[str, Any],
    checker: ModuleType,
    paired_checkout: Path | None,
) -> tuple[dict[str, Any], Path]:
    required_terminal_keys = {
        "status",
        "counts",
        "environment",
        "daemon_binary",
        "state_root_format",
        "inputs",
        "started_at",
        "ended_at",
        "artifacts",
    }
    missing = required_terminal_keys - set(terminal)
    extra = set(terminal) - required_terminal_keys - {"execution_result"}
    if missing or extra:
        raise ManifestRefused(
            f"terminal input keys differ: missing={sorted(missing)} extra={sorted(extra)}"
        )
    status = terminal["status"]
    counts = terminal["counts"]
    if not isinstance(status, str) or status not in {"passed", "failed", "blocked", "not_run"}:
        raise ManifestRefused(f"terminal status is not registered: {status!r}")
    if not isinstance(counts, dict):
        raise ManifestRefused("terminal.counts must be an object")
    failed_count = counts.get("failed")
    if status == "failed" and (
        not isinstance(failed_count, int) or isinstance(failed_count, bool) or failed_count < 1
    ):
        raise ManifestRefused("terminal status 'failed' requires counts.failed > 0")
    if status in {"blocked", "not_run"} and any(value != 0 for value in counts.values()):
        raise ManifestRefused(f"terminal status {status!r} requires zero execution counts")
    try:
        output_path = checker.HANDOFF_VALIDATION._repo_path(
            root, proof["artifact"], label="registered proof artifact"
        )
    except ValueError as error:
        raise ManifestRefused(f"registered proof artifact is invalid: {error}") from error
    inputs = terminal["inputs"]
    if not isinstance(inputs, dict):
        raise ManifestRefused("terminal.inputs must be an object")
    _require_exact_keys(inputs, set(INPUT_NAMES), label="terminal.inputs")
    if not isinstance(terminal["state_root_format"], str) or not terminal["state_root_format"]:
        raise ManifestRefused("terminal.state_root_format must be non-empty")
    environment = _host_environment(terminal["environment"])
    required_host = proof["required_host"]
    if required_host != "any" and environment["host"]["profile"] != required_host:
        raise ManifestRefused(f"proof requires host profile {required_host!r}")
    if required_host == "linux-production-like" and environment["os"] != "linux":
        raise ManifestRefused(
            "linux-production-like proof cannot be published from a non-Linux host"
        )
    resolved_binary = _resolve_binary(root, proof, terminal["daemon_binary"], checker=checker)
    resolved_artifacts = _resolve_artifacts(
        root, terminal["artifacts"], output_path=output_path, checker=checker
    )
    source_exclusions = [root / item["source_path"] for item in resolved_artifacts]
    if resolved_binary is not None:
        source_exclusions.append(root / resolved_binary["source_path"])
    if paired_checkout is not None:
        source_exclusions.append(paired_checkout)
    payload = {
        "schema_version": 1,
        "proof_id": proof["id"],
        "family": proof["family"],
        "status": status,
        "source": checker.proof_source_snapshot(
            root,
            manifest_path=output_path,
            proof=proof,
            excluded_paths=source_exclusions,
        ),
        "source_pair": _resolve_source_pair(
            proof,
            str(paired_checkout) if paired_checkout is not None else None,
            checker=checker,
        ),
        "invocation": {
            "command": proof["command"],
            "profile": proof["profile"],
            "target": proof["target"],
            "filter": proof["filter"],
        },
        "counts": counts,
        "environment": environment,
        "daemon_binary": resolved_binary,
        "state_root_format": terminal["state_root_format"],
        "inputs": {
            name: _digest_input(
                root, inputs[name], label=f"terminal.inputs.{name}", checker=checker
            )
            for name in sorted(INPUT_NAMES)
        },
        "started_at": terminal["started_at"],
        "ended_at": terminal["ended_at"],
        "dependency_receipts": _resolve_dependencies(root, proof, proof_by_id, checker),
        "artifacts": resolved_artifacts,
    }
    if "execution_result" in terminal:
        payload["execution_result"] = terminal["execution_result"]
    return payload, output_path


def _validate_p00_issuance(
    *,
    root: Path,
    proof: dict[str, Any],
    payload: dict[str, Any],
    checker: ModuleType,
) -> None:
    if proof.get("id") != "p00-authority-freeze":
        return
    inventory_artifact = next(
        (item for item in payload["artifacts"] if item["source_path"] == ERROR_INVENTORY_PATH),
        None,
    )
    if inventory_artifact is None:
        raise ManifestRefused("P00 proof must attest the registered error-authority inventory")
    _, inventory_bytes = _repo_bytes(
        root, ERROR_INVENTORY_PATH, label="error-authority inventory", checker=checker
    )
    inventory = _json_object_bytes(inventory_bytes, label="error-authority inventory")
    schema_bytes = _external_regular_bytes(
        ERROR_INVENTORY_SCHEMA, label="error-authority schema", checker=checker
    )
    schema = _json_object_bytes(schema_bytes, label="error-authority schema")
    try:
        jsonschema.Draft202012Validator(schema).validate(inventory)
    except jsonschema.ValidationError as error:
        raise ManifestRefused(f"error-authority inventory is invalid: {error.message}") from error
    writer = _load_module("quanta_error_authority_inventory", ERROR_INVENTORY_WRITER)
    if inventory != writer.build_inventory(root):
        raise ManifestRefused(
            "error-authority inventory is not current source-bound discovery evidence"
        )
    if inventory.get("closed") is not False:
        raise ManifestRefused("P00 discovery inventory cannot claim semantic closure")
    if inventory_artifact["sha256"] != hashlib.sha256(inventory_bytes).hexdigest():
        raise ManifestRefused("error-authority inventory changed during P00 issuance")


def _proof_lock_path(root: Path) -> Path:
    completed = subprocess.run(
        ["git", "-C", str(root), "rev-parse", "--git-path", "quanta-proof-authority.lock"],
        check=True,
        capture_output=True,
        text=True,
    )
    path = Path(completed.stdout.strip())
    return path if path.is_absolute() else root / path


def _open_output_parent(root: Path, relative: str, *, checker: ModuleType) -> int:
    try:
        return checker.HANDOFF_VALIDATION._open_repo_output_parent(root, relative)
    except (OSError, ValueError) as error:
        raise ManifestRefused(f"proof output parent is unsafe: {error}") from error


def _read_output(parent_fd: int, name: str, *, checker: ModuleType) -> bytes | None:
    try:
        return checker.HANDOFF_VALIDATION._read_output_regular_bytes(parent_fd, name)
    except (OSError, ValueError) as error:
        raise ManifestRefused(f"existing proof output is unsafe: {error}") from error


def _require_parent_identity(
    root: Path, relative: str, parent_fd: int, *, checker: ModuleType
) -> None:
    try:
        checker.HANDOFF_VALIDATION._require_output_parent_identity(root, relative, parent_fd)
    except (OSError, ValueError) as error:
        raise ManifestRefused(f"proof output parent changed or is unsafe: {error}") from error


def _publish_immutable_archive(
    *,
    root: Path,
    payload: dict[str, Any],
    serialized: bytes,
    checker: ModuleType,
) -> tuple[Path, str]:
    manifest_digest = hashlib.sha256(serialized).hexdigest()
    try:
        relative = checker.proof_archive_relative_path(payload, manifest_digest)
    except (KeyError, TypeError, ValueError) as error:
        raise ManifestRefused(f"cannot derive immutable archive path: {error}") from error
    archive_path = root / relative
    parent_fd = _open_output_parent(root, relative, checker=checker)
    leaf_name = archive_path.name
    try:
        index_bytes = _read_output(parent_fd, "index.json", checker=checker)
        source_binding = archive_path.parent.name
        if index_bytes is None:
            index_payload = {
                "schema_version": 1,
                "proof_id": payload["proof_id"],
                "source_binding_digest": source_binding,
                "manifest_digests": [],
            }
        else:
            index_payload = _json_object_bytes(index_bytes, label="proof archive index")
            expected_index_keys = {
                "schema_version",
                "proof_id",
                "source_binding_digest",
                "manifest_digests",
            }
            _require_exact_keys(index_payload, expected_index_keys, label="proof archive index")
            if (
                index_payload["schema_version"] != 1
                or index_payload["proof_id"] != payload["proof_id"]
                or index_payload["source_binding_digest"] != source_binding
                or not isinstance(index_payload["manifest_digests"], list)
                or any(
                    not isinstance(item, str) or len(item) != 64
                    for item in index_payload["manifest_digests"]
                )
                or len(index_payload["manifest_digests"])
                != len(set(index_payload["manifest_digests"]))
            ):
                raise ManifestRefused("immutable proof archive index authority is invalid")

        indexed_digests = index_payload["manifest_digests"]
        for indexed_digest in indexed_digests:
            indexed_name = f"{indexed_digest}.json"
            indexed_bytes = _read_output(parent_fd, indexed_name, checker=checker)
            if indexed_bytes is None:
                raise ManifestRefused(
                    f"immutable proof archive leaf was deleted: {archive_path.parent / indexed_name}"
                )
            if hashlib.sha256(indexed_bytes).hexdigest() != indexed_digest:
                raise ManifestRefused(
                    f"immutable proof archive leaf was modified: {archive_path.parent / indexed_name}"
                )

        existing = _read_output(parent_fd, leaf_name, checker=checker)
        if existing is not None:
            if existing != serialized:
                raise ManifestRefused(
                    f"immutable proof archive collision or overwrite attempt: {relative}"
                )
            if manifest_digest not in indexed_digests:
                raise ManifestRefused(f"immutable proof archive leaf is not indexed: {relative}")
            _require_parent_identity(root, relative, parent_fd, checker=checker)
            return archive_path, manifest_digest
        if manifest_digest in indexed_digests:
            raise ManifestRefused(f"immutable proof archive leaf was deleted: {relative}")

        temporary_name = checker.HANDOFF_VALIDATION._write_output_temporary(
            parent_fd, leaf_name, serialized
        )
        linked = False
        index_published = False
        try:
            _require_parent_identity(root, relative, parent_fd, checker=checker)
            try:
                os.link(
                    temporary_name,
                    leaf_name,
                    src_dir_fd=parent_fd,
                    dst_dir_fd=parent_fd,
                    follow_symlinks=False,
                )
            except FileExistsError as error:
                raise ManifestRefused(
                    f"immutable proof archive collision or overwrite attempt: {relative}"
                ) from error
            linked = True
            next_index = dict(index_payload)
            next_index["manifest_digests"] = [*indexed_digests, manifest_digest]
            serialized_index = (json.dumps(next_index, sort_keys=True, indent=2) + "\n").encode()
            index_temporary = checker.HANDOFF_VALIDATION._write_output_temporary(
                parent_fd, "index.json", serialized_index
            )
            try:
                os.replace(
                    index_temporary,
                    "index.json",
                    src_dir_fd=parent_fd,
                    dst_dir_fd=parent_fd,
                )
                index_published = True
            finally:
                if not index_published:
                    os.unlink(index_temporary, dir_fd=parent_fd)
            os.fsync(parent_fd)
            _require_parent_identity(root, relative, parent_fd, checker=checker)
        except BaseException:
            if linked and not index_published:
                os.unlink(leaf_name, dir_fd=parent_fd)
                os.fsync(parent_fd)
            raise
        finally:
            os.unlink(temporary_name, dir_fd=parent_fd)
    finally:
        os.close(parent_fd)
    return archive_path, manifest_digest


def _publish_manifest_locked(
    *,
    root: Path,
    registry_path: Path,
    schema_path: Path,
    proof_id: str,
    terminal_input_path: Path,
    paired_checkout: Path | None = None,
) -> tuple[Path, str, str]:
    root = root.resolve()
    registry_path = Path(os.path.abspath(registry_path))
    schema_path = Path(os.path.abspath(schema_path))
    expected_registry_path = root / "tools/ci/proof-authority.toml"
    if registry_path != expected_registry_path:
        raise ManifestRefused(f"registry override is forbidden: expected {expected_registry_path}")
    checker = _load_checker()
    _, registry_bytes = _repo_bytes(
        root, "tools/ci/proof-authority.toml", label="proof registry", checker=checker
    )
    registry = tomllib.loads(registry_bytes.decode("utf-8"))
    registry_findings = checker.check_registry(registry, root=root, path=registry_path)
    if registry_findings:
        rendered = "; ".join(finding.render() for finding in registry_findings)
        raise ManifestRefused(f"proof registry is invalid: {rendered}")
    proof_by_id = {
        proof["id"]: proof
        for proof in registry.get("proofs", [])
        if isinstance(proof, dict) and isinstance(proof.get("id"), str)
    }
    proof = proof_by_id.get(proof_id)
    if proof is None:
        raise ManifestRefused(f"proof_id {proof_id!r} is not registered")
    if proof.get("authority_state") != "executable":
        raise ManifestRefused(
            f"proof_id {proof_id!r} is staged and cannot issue an authoritative manifest"
        )
    expected_schema_path = root / proof["artifact_schema"]
    if schema_path != expected_schema_path:
        raise ManifestRefused(f"schema override is forbidden: expected {expected_schema_path}")
    _, schema_bytes = _repo_bytes(
        root, proof["artifact_schema"], label="manifest schema", checker=checker
    )
    schema = _json_object_bytes(schema_bytes, label="manifest schema")
    terminal = _json_object_bytes(
        _external_regular_bytes(terminal_input_path, label="terminal input", checker=checker),
        label="terminal input",
    )
    payload, output_path = build_manifest(
        root=root,
        proof=proof,
        proof_by_id=proof_by_id,
        terminal=terminal,
        checker=checker,
        paired_checkout=paired_checkout,
    )
    serialized = (json.dumps(payload, sort_keys=True, indent=2) + "\n").encode()
    output_relative = output_path.relative_to(root).as_posix()
    parent_fd = _open_output_parent(root, output_relative, checker=checker)
    temporary_name: str | None = None
    try:
        prior_bytes = _read_output(parent_fd, output_path.name, checker=checker)
        temporary_name = checker.HANDOFF_VALIDATION._write_output_temporary(
            parent_fd, output_path.name, serialized
        )
        temporary_path = output_path.parent / temporary_name
        _validate_p00_issuance(root=root, proof=proof, payload=payload, checker=checker)
        findings = checker.check_manifest(
            payload,
            manifest_path=temporary_path,
            proof=proof,
            schema=schema,
            root=root,
            bind_source=True,
            allow_non_passed=True,
            paired_checkouts=(
                {proof["paired_repository"]: paired_checkout}
                if paired_checkout is not None
                else None
            ),
            proof_by_id=proof_by_id,
        )
        if findings:
            rendered = "; ".join(finding.render() for finding in findings)
            raise ManifestRefused(f"semantic manifest validation failed: {rendered}")
        _require_parent_identity(root, output_relative, parent_fd, checker=checker)
        if _read_output(parent_fd, temporary_name, checker=checker) != serialized:
            raise ManifestRefused("validated proof manifest temporary changed")
        archive_path, manifest_digest = _publish_immutable_archive(
            root=root,
            payload=payload,
            serialized=serialized,
            checker=checker,
        )
        if _read_output(parent_fd, output_path.name, checker=checker) != prior_bytes:
            raise ManifestRefused("proof current alias changed before publication")
        _require_parent_identity(root, output_relative, parent_fd, checker=checker)
        os.replace(
            temporary_name,
            output_path.name,
            src_dir_fd=parent_fd,
            dst_dir_fd=parent_fd,
        )
        temporary_name = None
        try:
            installed_bytes = _read_output(parent_fd, output_path.name, checker=checker)
            if installed_bytes != serialized:
                raise ManifestRefused(
                    "published proof manifest bytes differ from validated payload"
                )
            if (
                _repo_bytes(
                    root, output_relative, label="published proof manifest", checker=checker
                )[1]
                != serialized
            ):
                raise ManifestRefused("published proof manifest path changed")
            installed_payload = _json_object_bytes(
                installed_bytes, label="published proof manifest"
            )
            if installed_payload != payload:
                raise ManifestRefused(
                    "published proof manifest bytes differ from validated payload"
                )
            if (
                installed_bytes
                != _repo_bytes(
                    root,
                    archive_path.relative_to(root).as_posix(),
                    label="immutable proof archive",
                    checker=checker,
                )[1]
            ):
                raise ManifestRefused("published current alias differs from immutable archive")
            published_findings = checker.check_manifest(
                installed_payload,
                manifest_path=output_path,
                proof=proof,
                schema=schema,
                root=root,
                bind_source=True,
                allow_non_passed=True,
                paired_checkouts=(
                    {proof["paired_repository"]: paired_checkout}
                    if paired_checkout is not None
                    else None
                ),
                proof_by_id=proof_by_id,
            )
            if published_findings:
                rendered = "; ".join(finding.render() for finding in published_findings)
                raise ManifestRefused(f"proof inputs changed at publication: {rendered}")
            _require_parent_identity(root, output_relative, parent_fd, checker=checker)
        except BaseException:
            checker.HANDOFF_VALIDATION._restore_prior_output(
                parent_fd, output_path.name, prior_bytes
            )
            raise
        os.fsync(parent_fd)
    finally:
        if temporary_name is not None:
            os.unlink(temporary_name, dir_fd=parent_fd)
        os.close(parent_fd)
    return archive_path, manifest_digest, payload["status"]


def publish_manifest(
    *,
    root: Path,
    registry_path: Path,
    schema_path: Path,
    proof_id: str,
    terminal_input_path: Path,
    paired_checkout: Path | None = None,
) -> tuple[Path, str, str]:
    root = root.resolve()
    lock_path = _proof_lock_path(root)
    lock_path.parent.mkdir(parents=True, exist_ok=True)
    with lock_path.open("a+b") as lock_handle:
        fcntl.flock(lock_handle.fileno(), fcntl.LOCK_EX)
        return _publish_manifest_locked(
            root=root,
            registry_path=registry_path,
            schema_path=schema_path,
            proof_id=proof_id,
            terminal_input_path=terminal_input_path,
            paired_checkout=paired_checkout,
        )


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, default=ROOT)
    parser.add_argument("--registry", type=Path, default=None)
    parser.add_argument("--schema", type=Path, default=None)
    parser.add_argument("--proof-id", required=True)
    parser.add_argument("--terminal-input", required=True, type=Path)
    parser.add_argument("--paired-checkout", type=Path, default=None)
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    args = parse_args(argv)
    root = args.root.resolve()
    registry_path = args.registry or root / "tools/ci/proof-authority.toml"
    schema_path = args.schema or root / "tools/ci/proof-manifest.schema.json"
    try:
        output_path, digest, status = publish_manifest(
            root=root,
            registry_path=registry_path,
            schema_path=schema_path,
            proof_id=args.proof_id,
            terminal_input_path=args.terminal_input,
            paired_checkout=args.paired_checkout,
        )
    except (ManifestRefused, OSError, json.JSONDecodeError, tomllib.TOMLDecodeError) as error:
        print(f"REFUSED: {error}", file=sys.stderr)
        return 1
    print(f"WROTE {output_path.relative_to(root)} sha256:{digest} status={status}")
    return 0 if status == "passed" else 1


if __name__ == "__main__":
    raise SystemExit(main())
