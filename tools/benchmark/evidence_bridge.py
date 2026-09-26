#!/usr/bin/env python3
"""Bridge native producer artifacts into immutable `BenchmarkEvidenceV1` runs.

Producers keep writing their native artifacts (the `BenchArtifactV1` envelope
for the DSL/quality/system rails, Criterion stdout for the crate-local
microbenches, retrieval run manifests, recorded JSONL). This module captures
that native output *verbatim* as the run's raw evidence and writes the typed
common envelope beside it. It never rewrites a measured number and never
invents a metric the producer did not emit.

The bridge is deliberately narrow: it promotes what the registry declares and
refuses everything else. A native artifact with no measured rows is refused
rather than converted into an empty or synthetic payload.
"""

from __future__ import annotations

import hashlib
import importlib.util
import json
import sys
from pathlib import Path
from typing import Any

SCRIPT_DIR = Path(__file__).resolve().parent
if str(SCRIPT_DIR) not in sys.path:
    sys.path.insert(0, str(SCRIPT_DIR))

from evidence import (  # noqa: E402
    EvidenceError,
    RunStore,
    digest_bytes,
    seal,
)

ROOT = SCRIPT_DIR.parents[1]
ARTIFACT_SCHEMA_VERSION = 2


def _load_module(name: str, path: Path):
    spec = importlib.util.spec_from_file_location(name, path)
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def source_closure_module():
    return _load_module("benchmark_source_closure", ROOT / "tools" / "ci" / "source_closure.py")


def _relative(path: Path, repo_root: Path) -> str:
    try:
        return path.resolve().relative_to(repo_root.resolve()).as_posix()
    except ValueError as exc:
        raise EvidenceError(f"path escapes the repository: {path}") from exc


def source_identity(
    repo_root: Path,
    closure_profile: str,
    *,
    closure_manifest: dict[str, Any] | None = None,
) -> dict[str, Any]:
    """Exact Git revision plus the closure digest for `closure_profile`."""
    module = source_closure_module()
    manifest = closure_manifest
    if manifest is None:
        try:
            manifest = module.build_manifest(repo_root, closure_profile)
        except module.ClosureError as exc:
            raise EvidenceError(f"cannot capture source closure: {exc}") from exc
    dirty = bool(
        module._git(repo_root, "status", "--porcelain=v1", "--untracked-files=normal", "--", *manifest["roots"])  # noqa: SLF001
    )
    return {
        "revision": manifest["revision"],
        "dirty": dirty,
        "dirty_paths_digest": (
            digest_bytes(
                module._git(  # noqa: SLF001
                    repo_root, "status", "--porcelain=v1", "--untracked-files=normal", "--", *manifest["roots"]
                ).encode()
            )
            if dirty
            else None
        ),
        "closure_profile": closure_profile,
        "closure_digest": manifest["digest"],
    }


def host_identity(
    *,
    policy: str,
    os_name: str,
    arch: str,
    cpu_count: int,
    hostname: str,
    lease_mode: str,
    lease_samples: int,
    extra_identity: str = "",
) -> dict[str, Any]:
    """Host identity with a hashed hostname and an explicit lease observation."""
    hostname_hash = digest_bytes(hostname.encode("utf-8"))
    identity = digest_bytes(
        f"{policy}|{os_name}|{arch}|{cpu_count}|{hostname_hash}|{lease_mode}|{extra_identity}".encode()
    )
    return {
        "policy": policy,
        "os": os_name,
        "arch": arch,
        "cpu_count": cpu_count,
        "hostname_hash": hostname_hash,
        "identity_digest": identity,
        "lease": {"mode": lease_mode, "observed_samples": lease_samples},
    }


def latency_payload_from_artifact(artifact: dict[str, Any]) -> dict[str, Any]:
    """Map a `BenchArtifactV1` row set into the typed latency payload.

    p50/p95/p99 stay milliseconds; a row the producer marked unmeasured keeps
    `early_stop_reason` and carries no percentile. Error/timeout counts are
    summed, never dropped.
    """
    if artifact.get("schema_version") != ARTIFACT_SCHEMA_VERSION:
        raise EvidenceError(
            f"native artifact schema_version {artifact.get('schema_version')!r} is not "
            f"{ARTIFACT_SCHEMA_VERSION}"
        )
    rows = artifact.get("rows")
    if not isinstance(rows, list) or not rows:
        raise EvidenceError("native artifact has no measured rows to promote")
    payload_rows: list[dict[str, Any]] = []
    errors = 0
    timeouts = 0
    for index, row in enumerate(rows):
        if not isinstance(row, dict):
            raise EvidenceError(f"native artifact rows[{index}] is not an object")
        latency = row.get("latency")
        samples = 0
        percentiles: list[float | None] = [None, None, None]
        if isinstance(latency, dict):
            samples = int(latency.get("samples") or 0)
            percentiles = [latency.get("p50_ms"), latency.get("p95_ms"), latency.get("p99_ms")]
        reason = row.get("early_stop_reason")
        payload_rows.append(
            {
                "case_id": str(row.get("scenario_id")),
                "metric": "p50",
                "unit": "ms",
                "samples": samples,
                "p50": percentiles[0],
                "p95": percentiles[1],
                "p99": percentiles[2],
                "error_count": int(row.get("error_count") or 0),
                "timeout_count": int(row.get("timeout_count") or 0),
                "early_stop_reason": None if reason is None else str(reason),
            }
        )
        errors += int(row.get("error_count") or 0)
        timeouts += int(row.get("timeout_count") or 0)
    return {"kind": "latency", "rows": payload_rows, "errors": errors, "timeouts": timeouts, "drops": 0}


def micro_payload_from_criterion(
    *,
    bench_id: str,
    statistic: str,
    value_ns: float,
    iterations: int,
    samples: int,
    instrumentation: str = "wall",
) -> dict[str, Any]:
    """Typed micro payload; instruction counts never become wall latency."""
    unit = "ns" if instrumentation == "wall" else "instructions"
    return {
        "kind": "micro",
        "bench_id": bench_id,
        "metric": statistic,
        "unit": unit,
        "instrumentation": instrumentation,
        "statistic": statistic,
        "value": float(value_ns),
        "iterations": iterations,
        "samples": samples,
    }


def recorded_experiment_payload(
    *, experiment_id: str, points: list[dict[str, Any]], source_digest: str
) -> dict[str, Any]:
    """Recorded experiments stay diagnostic-only and never become a gate."""
    return {
        "kind": "recorded_experiment",
        "experiment_id": experiment_id,
        "diagnostic_only": True,
        "points": points,
        "source_digest": source_digest,
    }


def promote_native_run(
    *,
    evidence_root: Path,
    run_id: str,
    family: str,
    profile: str,
    created_utc: str,
    native_path: Path,
    native_bytes: bytes,
    payload: dict[str, Any],
    source: dict[str, Any],
    build: dict[str, Any],
    inputs: list[dict[str, Any]],
    host: dict[str, Any],
    command: dict[str, Any],
    boundary: dict[str, Any],
    verdict: dict[str, Any],
    case_id: str | None = None,
) -> dict[str, Any]:
    """Stage, verify and atomically promote one immutable run."""
    store = RunStore(evidence_root)
    staged = store.stage(run_id)
    try:
        relative = f"raw/{native_path.name}"
        reference = staged.write_raw(relative, native_bytes)
        evidence = seal(
            {
                "protocol": "BenchmarkEvidenceV1",
                "protocol_version": 1,
                "run_id": run_id,
                "family": family,
                "profile": profile,
                "case_id": case_id,
                "created_utc": created_utc,
                "source": source,
                "build": build,
                "inputs": inputs,
                "host": host,
                "command": command,
                "boundary": boundary,
                "payload": payload,
                "raw": [reference],
                "output_digest": digest_bytes(json.dumps(payload, sort_keys=True).encode("utf-8")),
                "verdict": verdict,
                "digest": None,
            }
        )
        staged.write_evidence(evidence)
        promotion = store.promote(staged)
    except Exception:
        if staged.path.exists():
            staged.abort()
        raise
    return {**promotion, "evidence": evidence}


def sha256_file(path: Path) -> str:
    return digest_bytes(path.read_bytes())


def sha256_hex_file(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()
