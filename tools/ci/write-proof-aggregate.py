#!/usr/bin/env python3
"""Atomically publish the registry-derived SEP-21 aggregate qualification receipt."""

from __future__ import annotations

import argparse
import fcntl
import hashlib
import importlib.util
import json
import os
import subprocess
import sys
import tempfile
from datetime import datetime, timezone
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


class AggregateRefused(ValueError):
    """The aggregate cannot be published from the registered live evidence."""


def _load_checker(path: Path = CHECKER_PATH) -> ModuleType:
    spec = importlib.util.spec_from_file_location("quanta_check_proof_aggregate", path)
    if spec is None or spec.loader is None:
        raise AggregateRefused(f"cannot load semantic validator: {path}")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def _read_toml(path: Path) -> dict[str, Any]:
    with path.open("rb") as handle:
        value = tomllib.load(handle)
    if not isinstance(value, dict):
        raise AggregateRefused("proof registry root must be a table")
    return value


def _read_json(path: Path) -> Any:
    return json.loads(path.read_text(encoding="utf-8"))


def _sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def _manifest_status(
    *,
    root: Path,
    proof: dict[str, Any],
    proof_by_id: dict[str, dict[str, Any]],
    checker: ModuleType,
    paired_checkouts: dict[str, Path],
) -> tuple[str, str | None, dict[str, Any] | None]:
    if proof.get("authority_state") != "executable":
        return "BLOCKED", None, None
    manifest_path = root / proof["artifact"]
    if not manifest_path.is_file():
        return "NOT_RUN", None, None
    digest = _sha256(manifest_path)
    try:
        payload = _read_json(manifest_path)
        schema = _read_json(root / proof["artifact_schema"])
    except (OSError, json.JSONDecodeError):
        return "FAILED", digest, None
    findings = checker.check_manifest(
        payload,
        manifest_path=manifest_path,
        proof=proof,
        schema=schema,
        root=root,
        bind_source=True,
        allow_non_passed=True,
        paired_checkouts=paired_checkouts,
        proof_by_id=proof_by_id,
    )
    if findings or not isinstance(payload, dict):
        return "FAILED", digest, None
    status = {
        "passed": "PASSED",
        "failed": "FAILED",
        "blocked": "BLOCKED",
        "not_run": "NOT_RUN",
    }[payload["status"]]
    return status, digest, payload if status == "PASSED" else None


def _derived_verdict(statuses: list[str], *, consistency_failed: bool) -> str:
    if consistency_failed and all(status == "PASSED" for status in statuses):
        return "FAILED"
    if "FAILED" in statuses:
        return "FAILED"
    if "BLOCKED" in statuses:
        return "BLOCKED"
    if "NOT_RUN" in statuses:
        return "NOT_RUN"
    return "PASSED"


def build_aggregate(
    *,
    root: Path,
    registry: dict[str, Any],
    registry_path: Path,
    checker: ModuleType,
    paired_checkout: Path,
) -> tuple[dict[str, Any], Path]:
    aggregate = registry["aggregate"]
    proof_by_id = {proof["id"]: proof for proof in registry["proofs"]}
    target = proof_by_id[aggregate["target_proof"]]
    dependency_ids = checker.dependency_closure(proof_by_id, target["id"])
    repository = target["paired_repository"]
    paired_checkouts = {repository: paired_checkout.resolve()}
    output_path = (root / aggregate["artifact"]).resolve()
    try:
        output_path.relative_to(root)
    except ValueError as error:
        raise AggregateRefused("registered aggregate artifact escapes repository root") from error

    source = checker.proof_source_snapshot(
        root,
        manifest_path=output_path,
        proof=target,
        excluded_paths=(paired_checkout.resolve(),),
    )
    try:
        source_pair = checker.paired_source_snapshot(
            paired_checkout,
            repository=repository,
            dependency_lock=Path(target["paired_dependency_lock"]),
        )
    except (OSError, RuntimeError, ValueError) as error:
        raise AggregateRefused(f"cannot bind paired checkout: {error}") from error

    dependency_receipts: list[dict[str, Any]] = []
    dependency_statuses: dict[str, str] = {}
    payload_by_id: dict[str, dict[str, Any]] = {}
    for proof_id in dependency_ids:
        proof = proof_by_id[proof_id]
        status, digest, manifest = _manifest_status(
            root=root,
            proof=proof,
            proof_by_id=proof_by_id,
            checker=checker,
            paired_checkouts=paired_checkouts,
        )
        dependency_statuses[proof_id] = status
        dependency_receipts.append(
            {
                "proof_id": proof_id,
                "path": proof["artifact"],
                "sha256": digest,
                "status": status,
            }
        )
        if manifest is not None:
            payload_by_id[proof_id] = manifest

    consistency_findings = checker.check_aggregate(
        payload_by_id,
        proof_by_id=proof_by_id,
        path=registry_path,
    )
    all_dependencies_passed = all(
        dependency_statuses.get(proof_id) == "PASSED" for proof_id in dependency_ids
    )
    release_ready_inputs = all_dependencies_passed and not consistency_findings
    daemon_binary: dict[str, str] | None = None
    release_host: dict[str, str] | None = None
    state_root_format: str | None = None
    if release_ready_inputs:
        daemon_values = {
            (manifest["daemon_binary"]["path"], manifest["daemon_binary"]["sha256"])
            for proof_id, manifest in payload_by_id.items()
            if proof_by_id[proof_id]["binary_binding"] == "release-daemon"
        }
        host_values = {
            (
                manifest["environment"]["host"]["profile"],
                manifest["environment"]["host"]["identity_digest"],
            )
            for proof_id, manifest in payload_by_id.items()
            if proof_by_id[proof_id]["binary_binding"] == "release-daemon"
        }
        root_values = {
            manifest["state_root_format"]
            for proof_id, manifest in payload_by_id.items()
            if proof_by_id[proof_id]["binary_binding"] == "release-daemon"
        }
        if len(daemon_values) == len(host_values) == len(root_values) == 1:
            daemon_path, daemon_digest = next(iter(daemon_values))
            host_profile, host_digest = next(iter(host_values))
            daemon_binary = {"path": daemon_path, "sha256": daemon_digest}
            release_host = {"profile": host_profile, "identity_digest": host_digest}
            state_root_format = next(iter(root_values))
        else:
            release_ready_inputs = False

    verdicts: dict[str, dict[str, Any]] = {}
    for verdict, required_proofs in aggregate["verdicts"].items():
        verdicts[verdict] = {
            "status": _derived_verdict(
                [dependency_statuses.get(proof_id, "FAILED") for proof_id in required_proofs],
                consistency_failed=bool(consistency_findings),
            ),
            "required_proofs": required_proofs,
        }
    production_ready = release_ready_inputs and all(
        verdict["status"] == "PASSED" for verdict in verdicts.values()
    )
    return (
        {
            "schema_version": 1,
            "aggregate_id": aggregate["id"],
            "target_proof_id": target["id"],
            "registry_sha256": _sha256(registry_path),
            "source": source,
            "source_pair": source_pair,
            "daemon_binary": daemon_binary,
            "release_host": release_host,
            "state_root_format": state_root_format,
            "generated_at": datetime.now(timezone.utc).isoformat().replace("+00:00", "Z"),
            "dependency_receipts": dependency_receipts,
            "verdicts": verdicts,
            "production_ready": production_ready,
        },
        output_path,
    )


def _proof_lock_path(root: Path) -> Path:
    completed = subprocess.run(
        ["git", "-C", str(root), "rev-parse", "--git-path", "quanta-proof-authority.lock"],
        check=True,
        capture_output=True,
        text=True,
    )
    path = Path(completed.stdout.strip())
    return path if path.is_absolute() else root / path


def _restore_prior_aggregate(output_path: Path, prior_bytes: bytes | None) -> None:
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
        # Keep the complete backup on disk if replacement itself fails.
        os.replace(restore_path, output_path)
    directory_fd = os.open(output_path.parent, os.O_RDONLY)
    try:
        os.fsync(directory_fd)
    finally:
        os.close(directory_fd)


def _publish_aggregate_locked(
    *,
    root: Path,
    registry_path: Path,
    paired_checkout: Path,
) -> tuple[Path, str, bool]:
    root = root.resolve()
    registry_path = registry_path.resolve()
    expected_registry = (root / "tools/ci/proof-authority.toml").resolve()
    if registry_path != expected_registry:
        raise AggregateRefused(f"registry override is forbidden: expected {expected_registry}")
    checker = _load_checker()
    registry = _read_toml(registry_path)
    registry_findings = checker.check_registry(registry, root=root, path=registry_path)
    if registry_findings:
        rendered = "; ".join(finding.render() for finding in registry_findings)
        raise AggregateRefused(f"proof registry is invalid: {rendered}")
    payload, output_path = build_aggregate(
        root=root,
        registry=registry,
        registry_path=registry_path,
        checker=checker,
        paired_checkout=paired_checkout.resolve(),
    )
    schema = _read_json(root / registry["aggregate"]["schema"])
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
        proof_by_id = {proof["id"]: proof for proof in registry["proofs"]}
        target = proof_by_id[registry["aggregate"]["target_proof"]]
        findings = checker.check_aggregate_receipt(
            payload,
            receipt_path=output_path,
            registry=registry,
            registry_path=registry_path,
            schema=schema,
            root=root,
            bind_source=True,
            paired_checkouts={target["paired_repository"]: paired_checkout},
        )
        if findings:
            rendered = "; ".join(finding.render() for finding in findings)
            raise AggregateRefused(f"semantic aggregate validation failed: {rendered}")
        prior_bytes = output_path.read_bytes() if output_path.exists() else None
        os.replace(temporary_path, output_path)
        temporary_path = None
        try:
            installed_payload = _read_json(output_path)
            if installed_payload != payload:
                raise AggregateRefused("published aggregate bytes differ from validated payload")
            published_findings = checker.check_aggregate_receipt(
                installed_payload,
                receipt_path=output_path,
                registry=registry,
                registry_path=registry_path,
                schema=schema,
                root=root,
                bind_source=True,
                paired_checkouts={target["paired_repository"]: paired_checkout},
            )
            if published_findings:
                rendered = "; ".join(finding.render() for finding in published_findings)
                raise AggregateRefused(f"proof inputs changed at aggregate publication: {rendered}")
        except BaseException:
            _restore_prior_aggregate(output_path, prior_bytes)
            raise
        directory_fd = os.open(output_path.parent, os.O_RDONLY)
        try:
            os.fsync(directory_fd)
        finally:
            os.close(directory_fd)
    finally:
        if temporary_path is not None:
            temporary_path.unlink(missing_ok=True)
    return output_path, _sha256(output_path), bool(payload["production_ready"])


def publish_aggregate(
    *,
    root: Path,
    registry_path: Path,
    paired_checkout: Path,
) -> tuple[Path, str, bool]:
    root = root.resolve()
    lock_path = _proof_lock_path(root)
    lock_path.parent.mkdir(parents=True, exist_ok=True)
    with lock_path.open("a+b") as lock_handle:
        fcntl.flock(lock_handle.fileno(), fcntl.LOCK_EX)
        return _publish_aggregate_locked(
            root=root,
            registry_path=registry_path,
            paired_checkout=paired_checkout,
        )


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, default=ROOT)
    parser.add_argument("--registry", type=Path, default=None)
    parser.add_argument("--paired-checkout", type=Path, required=True)
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    args = parse_args(argv)
    root = args.root.resolve()
    registry_path = args.registry or root / "tools/ci/proof-authority.toml"
    try:
        output_path, digest, production_ready = publish_aggregate(
            root=root,
            registry_path=registry_path,
            paired_checkout=args.paired_checkout,
        )
    except (AggregateRefused, OSError, json.JSONDecodeError, tomllib.TOMLDecodeError) as error:
        print(f"REFUSED: {error}", file=sys.stderr)
        return 1
    state = "READY" if production_ready else "NOT_READY"
    print(f"WROTE {output_path.relative_to(root)} sha256:{digest} state={state}")
    return 0 if production_ready else 1


if __name__ == "__main__":
    raise SystemExit(main())
