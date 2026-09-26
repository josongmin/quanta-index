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
import re
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
    validate_payload,
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
        module._git(
            repo_root,
            "status",
            "--porcelain=v1",
            "--untracked-files=normal",
            "--",
            *manifest["roots"],
        )  # noqa: SLF001
    )
    return {
        "revision": manifest["revision"],
        "dirty": dirty,
        "dirty_paths_digest": (
            digest_bytes(
                module._git(  # noqa: SLF001
                    repo_root,
                    "status",
                    "--porcelain=v1",
                    "--untracked-files=normal",
                    "--",
                    *manifest["roots"],
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
    if (
        type(artifact.get("schema_version")) is not int
        or artifact["schema_version"] != ARTIFACT_SCHEMA_VERSION
    ):
        raise EvidenceError(
            f"native artifact schema_version {artifact.get('schema_version')!r} is not "
            f"{ARTIFACT_SCHEMA_VERSION}"
        )
    rows = artifact.get("rows")
    if not isinstance(rows, list) or not rows:
        raise EvidenceError("native artifact has no measured rows to promote")
    payload_rows: list[dict[str, Any]] = []
    for index, row in enumerate(rows):
        if not isinstance(row, dict):
            raise EvidenceError(f"native artifact rows[{index}] is not an object")
        try:
            latency = row["latency"]
            reason = row["early_stop_reason"]
            if latency is None:
                if reason is None:
                    raise EvidenceError(f"native artifact rows[{index}] has no latency or reason")
                samples = 0
                percentiles = [None, None, None]
            elif isinstance(latency, dict):
                samples = latency["samples"]
                percentiles = [latency["p50_ms"], latency["p95_ms"], latency["p99_ms"]]
            else:
                raise EvidenceError(f"native artifact rows[{index}].latency is not an object")
            case_id = row["scenario_id"]
            error_count = row["error_count"]
            timeout_count = row["timeout_count"]
        except KeyError as exc:
            raise EvidenceError(f"native artifact rows[{index}] is missing {exc}") from exc
        payload_rows.append(
            {
                "case_id": case_id,
                "metric": "p50",
                "unit": "ms",
                "samples": samples,
                "p50": percentiles[0],
                "p95": percentiles[1],
                "p99": percentiles[2],
                "error_count": error_count,
                "timeout_count": timeout_count,
                "early_stop_reason": reason,
            }
        )
    # Validate original values before arithmetic: coercion would manufacture
    # valid facts from null, bool, string or absent native measurements.
    payload = {"kind": "latency", "rows": payload_rows, "errors": 0, "timeouts": 0, "drops": 0}
    validate_payload(payload)
    payload["errors"] = sum(row["error_count"] for row in payload_rows)
    payload["timeouts"] = sum(row["timeout_count"] for row in payload_rows)
    return payload


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
    payload = {
        "kind": "micro",
        "bench_id": bench_id,
        "metric": statistic,
        "unit": unit,
        "instrumentation": instrumentation,
        "statistic": statistic,
        "value": value_ns,
        "iterations": iterations,
        "samples": samples,
    }
    validate_payload(payload)
    return payload


def native_payload_from_artifacts(artifacts: list[dict[str, Any]], kind: str) -> dict[str, Any]:
    """Re-derive typed facts from the complete, ordered native artifact set.

    Callers must also run the independent native envelope checker. Counts are
    never coerced and closed-loop aggregate/route subsets are not double counted.
    """
    if not artifacts:
        raise EvidenceError("native artifact set is empty")

    def count(value: Any) -> int:
        if type(value) is not int or value < 0:
            raise EvidenceError("native count is not an unsigned integer")
        return value

    try:
        # Reuse the strict row validation even when projecting another kind.
        latencies = [latency_payload_from_artifact(a) for a in artifacts]
        if kind == "latency":
            payload = {
                "kind": kind,
                "rows": [row for p in latencies for row in p["rows"]],
                "errors": sum(p["errors"] for p in latencies),
                "timeouts": sum(p["timeouts"] for p in latencies),
                "drops": 0,
            }
        elif kind == "freshness":
            if len(artifacts) != 1:
                raise EvidenceError("freshness requires one complete artifact")
            detail = artifacts[0]["detail"]
            samples = detail["samples"]
            if (
                not isinstance(samples, list)
                or not samples
                or count(detail["sample_count"]) != len(samples)
            ):
                raise EvidenceError("freshness sample inventory mismatch")
            phases = []
            for index, sample in enumerate(samples):
                phases.append(
                    {
                        "name": f"sample-{index}.base_build",
                        "ms": sample["base_build_ms"],
                        "samples": 1,
                    }
                )
                for transition in ("update", "delete", "rename"):
                    measured = sample[transition]
                    for metric in (
                        "mutation_to_visible_ms",
                        "receipt_to_visible_ms",
                        "ingest_ms",
                        "ingest_through_seal_ms",
                        "seal_ms",
                        "activation_ms",
                        "first_query_ms",
                    ):
                        phases.append(
                            {
                                "name": f"sample-{index}.{transition}.{metric}",
                                "ms": measured[metric],
                                "samples": 1,
                            }
                        )
                    count(measured["generation"])
            payload = {
                "kind": kind,
                "phases": phases,
                "stale_hits": count(detail["stale_hits"]),
                "generation": str(samples[-1]["rename"]["generation"]),
            }
        elif kind == "load":
            dimensions = {a["dimension"] for a in artifacts}
            if len(dimensions) != 1 or not dimensions <= {"open-loop", "concurrency"}:
                raise EvidenceError("load native dimensions are ambiguous or unsupported")
            opened = dimensions == {"open-loop"}
            if opened and len(artifacts) != 1:
                raise EvidenceError("open-loop requires one complete artifact")
            points, errors, scheduler_drops, clients = [], 0, 0, set()
            for artifact in artifacts:
                detail = artifact["detail"]
                if opened:
                    for point in detail["points"]:
                        dropped = sum(
                            count(point[key])
                            for key in (
                                "dropped_queue_full",
                                "dropped_scheduler_late",
                                "dropped_deadline",
                            )
                        )
                        failed = sum(
                            count(point[key])
                            for key in ("typed_errors", "transport_errors", "invalid_results")
                        )
                        timeouts = count(point["timeouts"])
                        if (
                            count(point["offered"])
                            != count(point["served"]) + failed + timeouts + dropped
                        ):
                            raise EvidenceError("open-loop offered request accounting mismatch")
                        scheduler_drops += count(point["dropped_scheduler_late"])
                        errors += failed
                        points.append(
                            {
                                "label": f"qps-{count(point['target_qps'])}",
                                "offered_rate": point["offered_qps"],
                                "completed_rate": point["achieved_qps"],
                                "dropped": dropped,
                                "timeouts": timeouts,
                            }
                        )
                else:
                    client = concurrency_clients_from_artifact(artifact)
                    if client in clients:
                        raise EvidenceError("duplicate concurrency artifact")
                    clients.add(client)
                    matching = [m for m in detail["measurements"] if m["clients"] == client]
                    if len(matching) != 1:
                        raise EvidenceError("concurrency measurement inventory mismatch")
                    measurement = matching[0]
                    groups = [measurement["fast"]]
                    if measurement["slow"] is not None:
                        groups.append(measurement["slow"])
                    for group in groups:
                        failed, timeouts = (
                            count(group["error_count"]),
                            count(group["timeout_count"]),
                        )
                        if count(group["requests"]) != count(group["served"]) + failed + timeouts:
                            raise EvidenceError("closed-loop request accounting mismatch")
                        errors += failed
                        points.append(
                            {
                                "label": f"c{client}.{group['label']}",
                                "offered_rate": None,
                                "completed_rate": group["qps"],
                                "dropped": 0,
                                "timeouts": timeouts,
                            }
                        )
            # For this open-loop producer, scheduler deadline misses are the
            # observable generator saturation signal, not SUT queue backpressure.
            payload = {
                "kind": kind,
                "arrival": "open_loop" if opened else "closed_loop",
                "generator_saturated": scheduler_drops > 0,
                "points": points,
                "errors": errors,
            }
        else:
            raise EvidenceError(f"no native payload oracle for {kind!r}")
        if kind == "load" and len({point["label"] for point in payload["points"]}) != len(
            payload["points"]
        ):
            raise EvidenceError("duplicate load point label")
        validate_payload(payload)
        return payload
    except (KeyError, TypeError, AttributeError) as exc:
        raise EvidenceError(f"incomplete native {kind} measurement: {exc}") from exc


def concurrency_clients_from_artifact(artifact: dict[str, Any]) -> int:
    """Rows identify FAST clients; envelope concurrency includes the slow client."""
    try:
        counts = set()
        for row in artifact["rows"]:
            match = re.fullmatch(r"concurrency\.c([1-9][0-9]*)\..+", row["scenario_id"])
            if match is None:
                raise EvidenceError("concurrency row has no client identity")
            counts.add(int(match.group(1)))
        if len(counts) != 1:
            raise EvidenceError("concurrency rows mix client identities")
        client = counts.pop()
        matching = [
            m
            for m in artifact["detail"]["measurements"]
            if type(m["clients"]) is int and m["clients"] == client
        ]
        if len(matching) != 1:
            raise EvidenceError("concurrency measurement inventory mismatch")
        expected = client + int(matching[0]["slow"] is not None)
        if type(artifact["concurrency"]) is not int or artifact["concurrency"] != expected:
            raise EvidenceError("concurrency envelope disagrees with fast/slow client inventory")
        return client
    except (KeyError, TypeError, AttributeError) as exc:
        raise EvidenceError(f"incomplete concurrency identity: {exc}") from exc


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
    additional_native: list[tuple[Path, bytes]] | None = None,
) -> dict[str, Any]:
    """Stage, verify and atomically promote one immutable run."""
    store = RunStore(evidence_root)
    staged = store.stage(run_id)
    try:
        relative = f"raw/{native_path.name}"
        references = [staged.write_raw(relative, native_bytes)]
        for extra_path, extra_bytes in additional_native or []:
            references.append(staged.write_raw(f"raw/{extra_path.name}", extra_bytes))
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
                "raw": references,
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
