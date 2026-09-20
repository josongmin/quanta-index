#!/usr/bin/env python3
"""Atomically publish a registry-bound, terminal SEP-21 proof manifest."""

from __future__ import annotations

import argparse
import hashlib
import importlib.util
import json
import os
import platform
import sys
import tempfile
from pathlib import Path
from types import ModuleType
from typing import Any

try:
    import tomllib
except ModuleNotFoundError:  # pragma: no cover - Python 3.10 compatibility
    import tomli as tomllib


ROOT = Path(__file__).resolve().parents[2]
CHECKER_PATH = ROOT / "tools/ci/lint/check-proof-authority.py"
REGISTRY_PATH = ROOT / "tools/ci/proof-authority.toml"
SCHEMA_PATH = ROOT / "tools/ci/proof-manifest.schema.json"
INPUT_NAMES = frozenset(("fixture", "corpus", "config", "model", "provider"))


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
    return hashlib.sha256(path.read_bytes()).hexdigest()


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


def _resolve_binary(root: Path, proof: dict[str, Any], value: Any) -> dict[str, str] | None:
    binding = proof["binary_binding"]
    if binding == "none":
        if value is not None:
            raise ManifestRefused("binary_binding=none requires terminal.daemon_binary=null")
        return None
    if binding != "release-daemon":
        raise ManifestRefused(f"unknown binary binding: {binding!r}")
    canonical, path = _repo_file(root, value, label="terminal.daemon_binary")
    return {"path": canonical, "sha256": _sha256(path)}


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


def _resolve_artifacts(root: Path, value: Any, *, output_path: Path) -> list[dict[str, str]]:
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
        artifacts.append({"path": canonical, "sha256": _sha256(path)})
    return artifacts


def _resolve_dependencies(
    root: Path,
    proof: dict[str, Any],
    proof_by_id: dict[str, dict[str, Any]],
) -> list[dict[str, str]]:
    receipts: list[dict[str, str]] = []
    for proof_id in proof["dependencies"]:
        dependency = proof_by_id.get(proof_id)
        if dependency is None:
            raise ManifestRefused(f"unknown dependency proof: {proof_id}")
        canonical, path = _repo_file(
            root,
            dependency["artifact"],
            label=f"dependency {proof_id}",
        )
        receipts.append({"proof_id": proof_id, "path": canonical, "sha256": _sha256(path)})
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
    payload = {
        "schema_version": 1,
        "proof_id": proof["id"],
        "family": proof["family"],
        "status": status,
        "source": checker.proof_source_snapshot(
            root,
            manifest_path=output_path,
            proof=proof,
            excluded_paths=(paired_checkout,) if paired_checkout is not None else (),
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
        "daemon_binary": _resolve_binary(root, proof, terminal["daemon_binary"]),
        "state_root_format": terminal["state_root_format"],
        "inputs": {
            name: _digest_input(root, inputs[name], label=f"terminal.inputs.{name}")
            for name in sorted(INPUT_NAMES)
        },
        "started_at": terminal["started_at"],
        "ended_at": terminal["ended_at"],
        "dependency_receipts": _resolve_dependencies(root, proof, proof_by_id),
        "artifacts": _resolve_artifacts(root, terminal["artifacts"], output_path=output_path),
    }
    return payload, output_path


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
        if payload["source"] != checker.proof_source_snapshot(
            root,
            manifest_path=output_path,
            proof=proof,
            excluded_paths=(paired_checkout,) if paired_checkout is not None else (),
        ):
            raise ManifestRefused("source changed while proof manifest was being prepared")
        os.replace(temporary_path, output_path)
        temporary_path = None
        directory_fd = os.open(output_path.parent, os.O_RDONLY)
        try:
            os.fsync(directory_fd)
        finally:
            os.close(directory_fd)
    finally:
        if temporary_path is not None:
            temporary_path.unlink(missing_ok=True)
    return output_path, _sha256(output_path), payload["status"]


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
