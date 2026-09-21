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
import shutil
import stat
import subprocess
import sys
import tempfile
from pathlib import Path
from types import ModuleType
from typing import Any

import jsonschema

try:
    import tomllib
except ModuleNotFoundError:  # pragma: no cover - Python 3.10 compatibility
    import tomli as tomllib


ROOT = Path(__file__).resolve().parents[2]
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


def _read_toml(path: Path) -> dict[str, Any]:
    with path.open("rb") as handle:
        value = tomllib.load(handle)
    if not isinstance(value, dict):
        raise ManifestRefused("proof registry root must be a table")
    return value


def _read_json_object(path: Path, *, label: str) -> dict[str, Any]:
    value = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(value, dict):
        raise ManifestRefused(f"{label} root must be an object")
    return value


def _require_exact_keys(value: dict[str, Any], expected: set[str], *, label: str) -> None:
    actual = set(value)
    if actual != expected:
        missing = sorted(expected - actual)
        extra = sorted(actual - expected)
        raise ManifestRefused(f"{label} keys differ: missing={missing} extra={extra}")


def _sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def _repo_file(root: Path, value: Any, *, label: str) -> tuple[str, Path]:
    if not isinstance(value, str) or not value:
        raise ManifestRefused(f"{label} must be a non-empty repo-relative path")
    relative = Path(value)
    if relative.is_absolute() or ".." in relative.parts:
        raise ManifestRefused(f"{label} must be a repo-relative path: {value!r}")
    path = (root / relative).resolve()
    try:
        canonical = path.relative_to(root).as_posix()
    except ValueError as error:
        raise ManifestRefused(f"{label} escapes repository root: {value!r}") from error
    if not path.is_file():
        raise ManifestRefused(f"{label} is missing: {canonical}")
    return canonical, path


def _digest_input(root: Path, value: Any, *, label: str) -> str | None:
    if value is None:
        return None
    if not isinstance(value, dict):
        raise ManifestRefused(f"{label} must be null, {{path}}, or {{value}}")
    if set(value) == {"path"}:
        _, path = _repo_file(root, value["path"], label=f"{label}.path")
        return f"sha256:{_sha256(path)}"
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


def _ensure_safe_directory(root: Path, relative: Path) -> Path:
    """Create a repo-contained directory tree without traversing symlinks."""

    if relative.is_absolute() or ".." in relative.parts:
        raise ManifestRefused(f"archive directory must be repo-relative: {relative}")
    current = root.resolve()
    for part in relative.parts:
        current = current / part
        try:
            metadata = current.lstat()
        except FileNotFoundError:
            current.mkdir()
            metadata = current.lstat()
        if stat.S_ISLNK(metadata.st_mode) or not stat.S_ISDIR(metadata.st_mode):
            raise ManifestRefused(f"archive directory is not a real directory: {current}")
    try:
        current.resolve().relative_to(root.resolve())
    except ValueError as error:
        raise ManifestRefused(f"archive directory escapes repository root: {relative}") from error
    return current


def _publish_content_archive(
    *, root: Path, checker: ModuleType, kind: str, source: Path, digest: str
) -> str:
    relative = checker.content_archive_relative_path(kind, digest)
    destination = root / relative
    _ensure_safe_directory(root, destination.parent.relative_to(root))
    if destination.exists() or destination.is_symlink():
        metadata = destination.lstat()
        if not stat.S_ISREG(metadata.st_mode) or _sha256(destination) != digest:
            raise ManifestRefused(f"immutable {kind} archive collision: {relative}")
        return relative
    temporary_path: Path | None = None
    try:
        with tempfile.NamedTemporaryFile(
            dir=destination.parent,
            prefix=f".{destination.name}.",
            suffix=".tmp",
            delete=False,
        ) as handle:
            with source.open("rb") as source_handle:
                shutil.copyfileobj(source_handle, handle)
            handle.flush()
            os.fsync(handle.fileno())
            temporary_path = Path(handle.name)
        if _sha256(temporary_path) != digest:
            raise ManifestRefused(f"{kind} source changed during archival")
        try:
            os.link(temporary_path, destination)
        except FileExistsError as error:
            if (
                destination.is_symlink()
                or not destination.is_file()
                or _sha256(destination) != digest
            ):
                raise ManifestRefused(f"immutable {kind} archive collision: {relative}") from error
        directory_fd = os.open(destination.parent, os.O_RDONLY)
        try:
            os.fsync(directory_fd)
        finally:
            os.close(directory_fd)
    finally:
        if temporary_path is not None:
            temporary_path.unlink(missing_ok=True)
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
    canonical, path = _repo_file(root, value, label="terminal.daemon_binary")
    digest = _sha256(path)
    return {
        "source_path": canonical,
        "path": _publish_content_archive(
            root=root, checker=checker, kind="binary", source=path, digest=digest
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
        canonical, path = _repo_file(root, item, label=f"terminal.artifacts[{index}]")
        if path == output_path:
            raise ManifestRefused("proof manifest cannot attest its own digest")
        if canonical in seen:
            raise ManifestRefused(f"duplicate terminal artifact: {canonical}")
        seen.add(canonical)
        digest = _sha256(path)
        artifacts.append(
            {
                "source_path": canonical,
                "path": _publish_content_archive(
                    root=root,
                    checker=checker,
                    kind="evidence",
                    source=path,
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
        _, path = _repo_file(
            root,
            dependency["artifact"],
            label=f"dependency {proof_id}",
        )
        payload = _read_json_object(path, label=f"dependency {proof_id}")
        if payload.get("proof_id") != proof_id:
            raise ManifestRefused(f"dependency {proof_id} current alias has the wrong proof_id")
        manifest_digest = _sha256(path)
        try:
            archive_relative = checker.proof_archive_relative_path(payload, manifest_digest)
        except (KeyError, TypeError, ValueError) as error:
            raise ManifestRefused(
                f"dependency {proof_id} archive identity is invalid: {error}"
            ) from error
        archive_path = root / archive_relative
        if not archive_path.is_file():
            raise ManifestRefused(
                f"dependency {proof_id} immutable archive is missing: {archive_relative}"
            )
        if archive_path.read_bytes() != path.read_bytes():
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
    _require_exact_keys(
        terminal,
        {
            "status",
            "counts",
            "environment",
            "daemon_binary",
            "state_root_format",
            "inputs",
            "started_at",
            "ended_at",
            "artifacts",
        },
        label="terminal input",
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
    output_relative = Path(proof["artifact"])
    if output_relative.is_absolute() or ".." in output_relative.parts:
        raise ManifestRefused("registry artifact must be repo-relative")
    output_path = (root / output_relative).resolve()
    try:
        output_path.relative_to(root)
    except ValueError as error:
        raise ManifestRefused("registry artifact escapes repository root") from error
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
            name: _digest_input(root, inputs[name], label=f"terminal.inputs.{name}")
            for name in sorted(INPUT_NAMES)
        },
        "started_at": terminal["started_at"],
        "ended_at": terminal["ended_at"],
        "dependency_receipts": _resolve_dependencies(root, proof, proof_by_id, checker),
        "artifacts": resolved_artifacts,
    }
    return payload, output_path


def _validate_aggregate_issuance(
    *,
    root: Path,
    registry: dict[str, Any],
    registry_path: Path,
    proof: dict[str, Any],
    payload: dict[str, Any],
    checker: ModuleType,
    paired_checkout: Path | None,
) -> None:
    if proof.get("execution_mode") != "aggregate":
        return
    aggregate = registry.get("aggregate")
    if not isinstance(aggregate, dict) or aggregate.get("target_proof") != proof["id"]:
        raise ManifestRefused("aggregate proof has no registered aggregate authority")
    if payload["status"] != "passed" or payload["counts"] != {
        "selected": 1,
        "executed": 1,
        "passed": 1,
        "failed": 0,
        "ignored": 0,
    }:
        raise ManifestRefused("aggregate proof requires derived passed counts 1/1/1/0/0")
    aggregate_path = aggregate.get("artifact")
    aggregate_artifact = next(
        (
            artifact
            for artifact in payload["artifacts"]
            if artifact["source_path"] == aggregate_path
        ),
        None,
    )
    if aggregate_artifact is None:
        raise ManifestRefused("aggregate proof must attest the registered aggregate artifact")
    if not isinstance(aggregate_path, str):
        raise ManifestRefused("registered aggregate artifact path is invalid")
    aggregate_file = root / aggregate_path
    try:
        aggregate_payload = _read_json_object(aggregate_file, label="aggregate receipt")
        aggregate_schema = _read_json_object(root / aggregate["schema"], label="aggregate schema")
    except (OSError, json.JSONDecodeError) as error:
        raise ManifestRefused(f"cannot load registered aggregate receipt: {error}") from error
    paired_checkouts = None
    if paired_checkout is not None:
        paired_checkouts = {proof["paired_repository"]: paired_checkout}
    findings = checker.check_aggregate_receipt(
        aggregate_payload,
        receipt_path=aggregate_file,
        registry=registry,
        registry_path=registry_path,
        schema=aggregate_schema,
        root=root,
        bind_source=True,
        paired_checkouts=paired_checkouts,
        require_ready=True,
    )
    if findings:
        rendered = "; ".join(finding.render() for finding in findings)
        raise ManifestRefused(f"aggregate receipt is not authoritative: {rendered}")
    if aggregate_artifact["sha256"] != _sha256(aggregate_file):
        raise ManifestRefused("aggregate artifact changed during P12 issuance")
    if payload["source"] != aggregate_payload["source"]:
        raise ManifestRefused("P12 source differs from aggregate source")
    if payload["source_pair"] != aggregate_payload["source_pair"]:
        raise ManifestRefused("P12 source pair differs from aggregate source pair")
    manifest_binary = payload["daemon_binary"]
    aggregate_binary = aggregate_payload["daemon_binary"]
    if (
        not isinstance(manifest_binary, dict)
        or not isinstance(aggregate_binary, dict)
        or manifest_binary["sha256"] != aggregate_binary["sha256"]
    ):
        raise ManifestRefused("P12 daemon binary differs from aggregate daemon binary")
    if payload["environment"]["host"]["profile"] != aggregate_payload["release_host"]["profile"]:
        raise ManifestRefused("P12 host profile differs from aggregate release host")
    if (
        payload["environment"]["host"]["identity_digest"]
        != aggregate_payload["release_host"]["identity_digest"]
    ):
        raise ManifestRefused("P12 host identity differs from aggregate release host")
    if payload["state_root_format"] != aggregate_payload["state_root_format"]:
        raise ManifestRefused("P12 state-root format differs from aggregate authority")


def _validate_p00_issuance(
    *,
    root: Path,
    proof: dict[str, Any],
    payload: dict[str, Any],
) -> None:
    if proof.get("id") != "p00-authority-freeze":
        return
    inventory_artifact = next(
        (item for item in payload["artifacts"] if item["source_path"] == ERROR_INVENTORY_PATH),
        None,
    )
    if inventory_artifact is None:
        raise ManifestRefused("P00 proof must attest the registered error-authority inventory")
    inventory_path = root / ERROR_INVENTORY_PATH
    inventory = _read_json_object(inventory_path, label="error-authority inventory")
    schema = _read_json_object(ERROR_INVENTORY_SCHEMA, label="error-authority schema")
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
    if inventory_artifact["sha256"] != _sha256(inventory_path):
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


def _restore_prior_manifest(output_path: Path, prior_bytes: bytes | None) -> None:
    if prior_bytes is None:
        output_path.unlink(missing_ok=True)
    else:
        restore_path: Path | None = None
        try:
            with tempfile.NamedTemporaryFile(
                dir=output_path.parent,
                prefix=f".{output_path.name}.",
                suffix=".restore",
                delete=False,
            ) as handle:
                restore_path = Path(handle.name)
                handle.write(prior_bytes)
                handle.flush()
                os.fsync(handle.fileno())
        except BaseException:
            if restore_path is not None:
                restore_path.unlink(missing_ok=True)
            raise
        os.replace(restore_path, output_path)
    directory_fd = os.open(output_path.parent, os.O_RDONLY)
    try:
        os.fsync(directory_fd)
    finally:
        os.close(directory_fd)


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
    archive_parent = _ensure_safe_directory(root, archive_path.parent.relative_to(root))
    index_path = archive_parent / "index.json"
    source_binding = archive_parent.name
    index_payload: dict[str, Any]
    if index_path.exists() or index_path.is_symlink():
        metadata = index_path.lstat()
        if not stat.S_ISREG(metadata.st_mode):
            raise ManifestRefused(
                f"immutable proof archive index is not a regular file: {index_path}"
            )
        try:
            index_payload = _read_json_object(index_path, label="proof archive index")
        except (OSError, json.JSONDecodeError) as error:
            raise ManifestRefused(f"immutable proof archive index is invalid: {error}") from error
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
            or len(index_payload["manifest_digests"]) != len(set(index_payload["manifest_digests"]))
        ):
            raise ManifestRefused("immutable proof archive index authority is invalid")
    else:
        index_payload = {
            "schema_version": 1,
            "proof_id": payload["proof_id"],
            "source_binding_digest": source_binding,
            "manifest_digests": [],
        }

    indexed_digests = index_payload["manifest_digests"]
    for indexed_digest in indexed_digests:
        indexed_path = archive_parent / f"{indexed_digest}.json"
        if indexed_path.is_symlink() or not indexed_path.is_file():
            raise ManifestRefused(
                f"immutable proof archive leaf was deleted: {indexed_path.relative_to(root)}"
            )
        if _sha256(indexed_path) != indexed_digest:
            raise ManifestRefused(
                f"immutable proof archive leaf was modified: {indexed_path.relative_to(root)}"
            )

    archive_exists = archive_path.exists() or archive_path.is_symlink()
    if archive_exists:
        metadata = archive_path.lstat()
        if not stat.S_ISREG(metadata.st_mode) or archive_path.read_bytes() != serialized:
            raise ManifestRefused(
                f"immutable proof archive collision or overwrite attempt: {relative}"
            )
        if manifest_digest not in indexed_digests:
            raise ManifestRefused(f"immutable proof archive leaf is not indexed: {relative}")
        return archive_path, manifest_digest
    if manifest_digest in indexed_digests:
        raise ManifestRefused(f"immutable proof archive leaf was deleted: {relative}")

    temporary_path: Path | None = None
    try:
        with tempfile.NamedTemporaryFile(
            dir=archive_path.parent,
            prefix=f".{archive_path.name}.",
            suffix=".tmp",
            delete=False,
        ) as handle:
            handle.write(serialized)
            handle.flush()
            os.fsync(handle.fileno())
            temporary_path = Path(handle.name)
        try:
            os.link(temporary_path, archive_path)
        except FileExistsError as error:
            if not archive_path.is_file() or archive_path.read_bytes() != serialized:
                raise ManifestRefused(
                    f"immutable proof archive collision or overwrite attempt: {relative}"
                ) from error
        next_index = dict(index_payload)
        next_index["manifest_digests"] = [*indexed_digests, manifest_digest]
        serialized_index = (json.dumps(next_index, sort_keys=True, indent=2) + "\n").encode()
        index_temporary: Path | None = None
        try:
            with tempfile.NamedTemporaryFile(
                dir=archive_parent,
                prefix=".index.json.",
                suffix=".tmp",
                delete=False,
            ) as handle:
                handle.write(serialized_index)
                handle.flush()
                os.fsync(handle.fileno())
                index_temporary = Path(handle.name)
            os.replace(index_temporary, index_path)
            index_temporary = None
        finally:
            if index_temporary is not None:
                index_temporary.unlink(missing_ok=True)
        directory_fd = os.open(archive_parent, os.O_RDONLY)
        try:
            os.fsync(directory_fd)
        finally:
            os.close(directory_fd)
    finally:
        if temporary_path is not None:
            temporary_path.unlink(missing_ok=True)
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
    registry_path = registry_path.resolve()
    schema_path = schema_path.resolve()
    expected_registry_path = (root / "tools/ci/proof-authority.toml").resolve()
    if registry_path != expected_registry_path:
        raise ManifestRefused(f"registry override is forbidden: expected {expected_registry_path}")
    checker = _load_checker()
    registry = _read_toml(registry_path)
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
    expected_schema_path = (root / proof["artifact_schema"]).resolve()
    if schema_path != expected_schema_path:
        raise ManifestRefused(f"schema override is forbidden: expected {expected_schema_path}")
    schema = _read_json_object(schema_path, label="manifest schema")
    terminal = _read_json_object(terminal_input_path.resolve(), label="terminal input")
    payload, output_path = build_manifest(
        root=root,
        proof=proof,
        proof_by_id=proof_by_id,
        terminal=terminal,
        checker=checker,
        paired_checkout=paired_checkout,
    )
    output_path.parent.mkdir(parents=True, exist_ok=True)
    serialized = (json.dumps(payload, sort_keys=True, indent=2) + "\n").encode()
    temporary_path: Path | None = None
    try:
        with tempfile.NamedTemporaryFile(
            dir=output_path.parent,
            prefix=f".{output_path.name}.",
            suffix=".tmp",
            delete=False,
        ) as handle:
            handle.write(serialized)
            handle.flush()
            os.fsync(handle.fileno())
            temporary_path = Path(handle.name)
        _validate_aggregate_issuance(
            root=root,
            registry=registry,
            registry_path=registry_path,
            proof=proof,
            payload=payload,
            checker=checker,
            paired_checkout=paired_checkout,
        )
        _validate_p00_issuance(root=root, proof=proof, payload=payload)
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
        archive_path, manifest_digest = _publish_immutable_archive(
            root=root,
            payload=payload,
            serialized=serialized,
            checker=checker,
        )
        prior_bytes = output_path.read_bytes() if output_path.exists() else None
        os.replace(temporary_path, output_path)
        temporary_path = None
        try:
            installed_payload = _read_json_object(output_path, label="published proof manifest")
            if installed_payload != payload:
                raise ManifestRefused(
                    "published proof manifest bytes differ from validated payload"
                )
            if output_path.read_bytes() != archive_path.read_bytes():
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
        except BaseException:
            _restore_prior_manifest(output_path, prior_bytes)
            raise
        directory_fd = os.open(output_path.parent, os.O_RDONLY)
        try:
            os.fsync(directory_fd)
        finally:
            os.close(directory_fd)
    finally:
        if temporary_path is not None:
            temporary_path.unlink(missing_ok=True)
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
