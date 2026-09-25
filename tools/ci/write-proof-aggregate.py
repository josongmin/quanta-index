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


def _manifest_status(
    *,
    root: Path,
    proof: dict[str, Any],
    proof_by_id: dict[str, dict[str, Any]],
    checker: ModuleType,
    paired_checkouts: dict[str, Path],
    source_cache: dict[tuple[Path, Path | None, tuple[Path, ...]], dict[str, Any]],
    pair_cache: dict[tuple[Path, str, Path], dict[str, Any]],
) -> tuple[str, str | None, dict[str, Any] | None]:
    if proof.get("authority_state") != "executable":
        return "BLOCKED", None, None
    manifest_path = root / proof["artifact"]
    try:
        present = checker.HANDOFF_VALIDATION._repo_entry_present_no_follow(
            root, proof["artifact"], label="proof manifest"
        )
    except (OSError, ValueError):
        return "FAILED", None, None
    if not present:
        return "NOT_RUN", None, None
    try:
        manifest_bytes = checker._payload_bytes(root, proof["artifact"], label="proof manifest")
        digest = hashlib.sha256(manifest_bytes).hexdigest()
        payload = json.loads(manifest_bytes)
        schema = checker._payload_json(root, proof["artifact_schema"], label="proof schema")
    except (OSError, ValueError, UnicodeDecodeError, json.JSONDecodeError):
        return "FAILED", None, None
    repository = proof.get("paired_repository")
    paired_checkout = paired_checkouts.get(repository) if isinstance(repository, str) else None
    bound_source = checker._cached_proof_source_snapshot(
        source_cache,
        root,
        manifest_path=manifest_path,
        proof=proof,
        excluded_paths=(() if paired_checkout is None else (paired_checkout,)),
    )
    bound_source_pair = None
    if paired_checkout is not None:
        try:
            bound_source_pair = checker._cached_paired_source_snapshot(
                pair_cache,
                paired_checkout,
                repository=repository,
                dependency_lock=Path(proof["paired_dependency_lock"]),
            )
        except (OSError, RuntimeError, ValueError):
            # Keep check_manifest's proof-specific paired-source finding.
            bound_source_pair = None
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
        bound_source=bound_source,
        bound_source_pair=bound_source_pair,
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
    dependency_ids = checker.aggregate_proof_ids(registry)
    repository = target["paired_repository"]
    paired_checkouts = {repository: paired_checkout.resolve()}
    source_cache: dict[tuple[Path, Path | None, tuple[Path, ...]], dict[str, Any]] = {}
    pair_cache: dict[tuple[Path, str, Path], dict[str, Any]] = {}
    output_path, path_error = checker._payload_repo_file(
        root, aggregate["artifact"], label="registered aggregate artifact"
    )
    if path_error is not None or output_path is None:
        raise AggregateRefused(path_error or "registered aggregate artifact is invalid")

    source = checker._cached_proof_source_snapshot(
        source_cache,
        root,
        manifest_path=output_path,
        proof=target,
        excluded_paths=(paired_checkout.resolve(),),
    )
    try:
        source_pair = checker._cached_paired_source_snapshot(
            pair_cache,
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
            source_cache=source_cache,
            pair_cache=pair_cache,
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
    handoff_ledger, _handoff_findings = checker.HANDOFF_VALIDATION.inspect_handoff_ledger(
        root=root, proof_checker=checker
    )
    handoffs_ready = (
        handoff_ledger["product_chain_status"] == "VERIFIED"
        and handoff_ledger["infrastructure_handoff"]["status"] == "VERIFIED"
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
        host_values = checker.operational_host_identities(payload_by_id)
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
        verdict_findings = checker.verdict_consistency_findings(
            required_proofs,
            payload_by_id=payload_by_id,
            proof_by_id=proof_by_id,
            path=registry_path,
        )
        verdicts[verdict] = {
            "status": _derived_verdict(
                [dependency_statuses.get(proof_id, "FAILED") for proof_id in required_proofs],
                consistency_failed=bool(verdict_findings),
            ),
            "required_proofs": required_proofs,
        }
    production_ready = (
        release_ready_inputs
        and all(verdict["status"] == "PASSED" for verdict in verdicts.values())
        and handoffs_ready
    )
    return (
        {
            "schema_version": 1,
            "aggregate_id": aggregate["id"],
            "target_proof_id": target["id"],
            "registry_sha256": checker._payload_sha256(
                root, registry_path.relative_to(root).as_posix(), label="proof registry"
            ),
            "source": source,
            "source_pair": source_pair,
            "daemon_binary": daemon_binary,
            "release_host": release_host,
            "state_root_format": state_root_format,
            "generated_at": datetime.now(timezone.utc).isoformat().replace("+00:00", "Z"),
            "dependency_receipts": dependency_receipts,
            **handoff_ledger,
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


def _open_output_parent(root: Path, relative: str, *, checker: ModuleType) -> int:
    try:
        return checker.HANDOFF_VALIDATION._open_repo_output_parent(root, relative)
    except (OSError, ValueError) as error:
        raise AggregateRefused(f"aggregate output parent is unsafe: {error}") from error


def _require_parent_identity(
    root: Path, relative: str, parent_fd: int, *, checker: ModuleType
) -> None:
    try:
        checker.HANDOFF_VALIDATION._require_output_parent_identity(root, relative, parent_fd)
    except (OSError, ValueError) as error:
        raise AggregateRefused(f"aggregate output parent changed or is unsafe: {error}") from error


def _publish_aggregate_locked(
    *,
    root: Path,
    registry_path: Path,
    paired_checkout: Path,
) -> tuple[Path, str, bool]:
    root = root.resolve()
    registry_path = registry_path if registry_path.is_absolute() else Path.cwd() / registry_path
    expected_registry = root / "tools/ci/proof-authority.toml"
    if registry_path != expected_registry:
        raise AggregateRefused(f"registry override is forbidden: expected {expected_registry}")
    checker = _load_checker()
    try:
        registry = tomllib.loads(
            checker._payload_bytes(
                root, registry_path.relative_to(root).as_posix(), label="proof registry"
            ).decode("utf-8")
        )
    except (OSError, ValueError, UnicodeDecodeError, tomllib.TOMLDecodeError) as error:
        raise AggregateRefused(f"registered proof registry is unreadable: {error}") from error
    registry_findings = checker.check_registry(registry, root=root, path=registry_path)
    if registry_findings:
        rendered = "; ".join(finding.render() for finding in registry_findings)
        raise AggregateRefused(f"proof registry is invalid: {rendered}")
    output_relative = registry["aggregate"]["artifact"]
    output_path, path_error = checker._payload_repo_file(
        root, output_relative, label="registered aggregate artifact"
    )
    if path_error is not None or output_path is None:
        raise AggregateRefused(path_error or "registered aggregate artifact is invalid")
    parent_fd = _open_output_parent(root, output_relative, checker=checker)
    try:
        payload, observed_output_path = build_aggregate(
            root=root,
            registry=registry,
            registry_path=registry_path,
            checker=checker,
            paired_checkout=paired_checkout.resolve(),
        )
        if observed_output_path != output_path:
            raise AggregateRefused("registered aggregate output identity changed")
        try:
            schema = checker._payload_json(
                root, registry["aggregate"]["schema"], label="aggregate schema"
            )
        except (OSError, ValueError, UnicodeDecodeError, json.JSONDecodeError) as error:
            raise AggregateRefused(f"registered aggregate schema is unreadable: {error}") from error
        serialized = (json.dumps(payload, sort_keys=True, indent=2) + "\n").encode()
        temporary_name: str | None = None
        try:
            temporary_name = checker.HANDOFF_VALIDATION._write_output_temporary(
                parent_fd, output_path.name, serialized
            )
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
            try:
                prior_bytes = checker.HANDOFF_VALIDATION._read_output_regular_bytes(
                    parent_fd, output_path.name
                )
            except (OSError, ValueError) as error:
                raise AggregateRefused(f"existing aggregate output is unsafe: {error}") from error
            _require_parent_identity(root, output_relative, parent_fd, checker=checker)
            os.replace(
                temporary_name,
                output_path.name,
                src_dir_fd=parent_fd,
                dst_dir_fd=parent_fd,
            )
            temporary_name = None
            try:
                installed_bytes = checker._payload_bytes(
                    root, output_relative, label="published aggregate"
                )
                if installed_bytes != serialized:
                    raise AggregateRefused(
                        "published aggregate bytes differ from validated payload"
                    )
                _require_parent_identity(root, output_relative, parent_fd, checker=checker)
                installed_payload = json.loads(installed_bytes)
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
                    raise AggregateRefused(
                        f"proof inputs changed at aggregate publication: {rendered}"
                    )
                os.fsync(parent_fd)
                _require_parent_identity(root, output_relative, parent_fd, checker=checker)
            except BaseException:
                checker.HANDOFF_VALIDATION._restore_prior_output(
                    parent_fd, output_path.name, prior_bytes
                )
                raise
        finally:
            if temporary_name is not None:
                os.unlink(temporary_name, dir_fd=parent_fd)
    finally:
        os.close(parent_fd)
    return output_path, hashlib.sha256(serialized).hexdigest(), bool(payload["production_ready"])


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
