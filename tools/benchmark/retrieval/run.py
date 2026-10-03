#!/usr/bin/env python3
"""Paired quality/speed/resource measurement orchestration (RB-05).

Subcommands:
  quanta    run the Rust SDK runner per strategy from a pinned spec
  pair      quanta + Semble sequential capture, merge, score, verdict
  merge     deterministically merge per-system v3 records into one record
  verdict   re-score immutable records and emit the verdict artifact (T13)
  host-probe  emit the host check-record (identity, load, thermal/frequency)

RB-05 consumes `benchmarks/retrieval/src/record.rs` output read-only; it
never edits that owner file. All captures land under an explicit output
root outside the source checkout.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import os
import platform
import random
import re
import secrets
import shutil
import signal
import subprocess
import sys
import tempfile
import time
from collections.abc import Iterable
from contextlib import contextmanager, nullcontext
from pathlib import Path

try:
    from tools.benchmark.retrieval import (
        code_search_rank_study,
        linux_isolation,
        linux_process,
        portable_proof,
        source_oracle,
        symbol_coverage,
    )
    from tools.benchmark.retrieval import execution_batch as eb
    from tools.benchmark.retrieval import query_plan as qp
    from tools.benchmark.retrieval import semble as semble_adapter
    from tools.benchmark.retrieval.contract_proof import nextest_summary, pytest_summary
    from tools.benchmark.retrieval.evaluator import (
        CHUNK_STRATEGIES,
        COMPLETE_JUDGMENT_POLICY,
        RUNNER_SCHEMA_VERSION,
        TOKENIZER_BUDGET_VERSION,
        SourceSnapshot,
        canonical,
        digest,
        evaluate,
        evaluate_complete_scored_file_evidence,
        evaluate_paired_file_diagnostic,
        qualified_query_family_ci,
        validate_comparison_contract,
        validate_evidence_against_suite,
        validate_experiment_custody,
        validate_suite,
        verify_repo,
    )
    from tools.benchmark.retrieval.evaluator import (
        read_json as read_evidence_json,
    )
    from tools.benchmark.retrieval.finite_json import is_finite_json_number
    from tools.benchmark.retrieval.proof_inventory import verify_inventory_authority
    from tools.benchmark.retrieval.sdk_proof import build_summary_from_evidence
except ImportError:  # direct script invocation: import the sibling module
    sys.path.insert(0, str(Path(__file__).resolve().parent))
    import code_search_rank_study  # noqa: E402
    import execution_batch as eb  # noqa: E402
    import linux_isolation  # noqa: E402
    import linux_process  # noqa: E402
    import portable_proof  # noqa: E402
    import query_plan as qp  # noqa: E402
    import semble as semble_adapter  # noqa: E402
    import source_oracle  # noqa: E402
    import symbol_coverage  # noqa: E402
    from contract_proof import nextest_summary, pytest_summary  # noqa: E402
    from evaluator import (  # noqa: E402
        CHUNK_STRATEGIES,
        COMPLETE_JUDGMENT_POLICY,
        RUNNER_SCHEMA_VERSION,
        TOKENIZER_BUDGET_VERSION,
        SourceSnapshot,
        canonical,
        digest,
        evaluate,
        evaluate_complete_scored_file_evidence,
        evaluate_paired_file_diagnostic,
        qualified_query_family_ci,
        validate_comparison_contract,
        validate_evidence_against_suite,
        validate_experiment_custody,
        validate_suite,
        verify_repo,
    )
    from evaluator import (
        read_json as read_evidence_json,
    )
    from finite_json import is_finite_json_number  # noqa: E402
    from proof_inventory import verify_inventory_authority  # noqa: E402
    from sdk_proof import build_summary_from_evidence  # noqa: E402

from tools.benchmark import host_monitor, raw_archive
from tools.benchmark.evidence import (
    CONTROL_DOCUMENT_BYTES,
    RawFile,
    parse_json,
    read_control,
    write_raw_file,
)

VERDICT_VERSION = 2
MANIFEST_VERSION = 2
PILOT_OBSERVATIONS_FLOOR = 1000
FRESH_ROOTS_FLOOR = 5
FROZEN_TASKS_FLOOR = 20
HOST_SAMPLE_INTERVAL_NS = host_monitor.INTERVAL_NS
HOST_SAMPLE_MAX_GAP_NS = host_monitor.MAX_GAP_NS
RUNNABLE_STRATEGIES = tuple(s for s in CHUNK_STRATEGIES if s != "semble_native")
SEMBLE_PINNED_VERSION = "0.6.0"
SEMBLE_PROFILES = (
    "native-default",
    "hybrid-no-rerank",
    "lexical-only",
    "lexical-file",
    "semantic-only",
)
SEMBLE_ROUTE_BY_MODE = {
    "native-default": "semble-hybrid",
    "hybrid-no-rerank": "semble-hybrid",
    "lexical-only": "semble-lexical-only",
    "lexical-file": "semble-lexical-file",
    "semantic-only": "semble-semantic-only",
}
QUANTA_SYMBOL_PRODUCER_IDENTITY = "source-bound-symbols-v2"
QUANTA_SYMBOL_GRAMMARS = symbol_coverage.grammar_identity()
PAIR_QUANTA_POLICIES = frozenset((*qp.V4_SUPPORTED_POLICIES, *qp.FILE_PAIR_POLICIES))
PAIR_CONTEXT_QUALITY_POLICIES = frozenset(qp.V4_SUPPORTED_POLICIES)


def _validate_semble_profile(value: object, where: str) -> dict:
    if not isinstance(value, dict) or set(value) != {"profile_id", "mode", "alpha", "rerank"}:
        raise RunError(f"{where} fields are invalid")
    mode = value["mode"]
    if not isinstance(mode, str):
        raise RunError(f"{where}.mode must be a string")
    fixed = {
        "native-default": ("semble-native-default-v1", None, "upstream-content-default"),
        "lexical-only": ("semble-lexical-only-v1", None, "not_applicable"),
        "lexical-file": ("semble-lexical-file-v1", None, "not_applicable"),
        "semantic-only": ("semble-semantic-only-v1", None, "not_applicable"),
    }
    if mode == "hybrid-no-rerank":
        alpha = value["alpha"]
        if (
            value["profile_id"] != "semble-hybrid-no-rerank-v1"
            or value["rerank"] is not False
            or not is_finite_json_number(alpha)
            or not 0 <= alpha <= 1
        ):
            raise RunError(f"{where} hybrid profile is invalid")
    elif mode in fixed:
        profile_id, alpha, rerank = fixed[mode]
        if value != {"profile_id": profile_id, "mode": mode, "alpha": alpha, "rerank": rerank}:
            raise RunError(f"{where} differs from the frozen profile")
    else:
        raise RunError(f"{where}.mode is unknown")
    return value


def _validate_semble_route_binding(spec: dict) -> None:
    """Keep the public route labels aligned with the frozen execution mode."""
    profiles = spec.get("execution_profiles", {})
    semble = profiles.get("semble") if isinstance(profiles, dict) else None
    if semble is None:
        return
    expected = SEMBLE_ROUTE_BY_MODE[semble["mode"]]
    if spec.get("semble_route", expected) != expected:
        raise RunError(f"spec.semble_route must be {expected} for {semble['mode']} execution")
    if spec.get("baseline_route", expected) != expected:
        raise RunError(f"spec.baseline_route must match Semble route {expected}")
    routes = spec.get("routes", ["lexical", "semantic", "hybrid"])
    candidate = spec.get("candidate_route")
    if candidate is not None and candidate not in routes:
        raise RunError("spec.candidate_route must name a configured Quanta route")


def _validate_model_cache_manifest(value: object, where: str) -> dict:
    manifest = _exact_keys(
        value,
        {
            "schema_version",
            "model_id",
            "revision",
            "ref",
            "members",
            "model_asset_digest",
            "snapshot_digest",
        },
        where,
    )
    if manifest["schema_version"] != 1:
        raise RunError(f"{where}.schema_version must be 1")
    if not isinstance(manifest["model_id"], str) or not manifest["model_id"]:
        raise RunError(f"{where}.model_id is invalid")
    if not _is_hex(manifest["model_asset_digest"], 64):
        raise RunError(f"{where}.model_asset_digest is invalid")
    if not _is_hex(manifest["revision"], 40):
        raise RunError(f"{where}.revision is invalid")
    if manifest["ref"] != {"name": "main", "revision": manifest["revision"]}:
        raise RunError(f"{where}.ref does not bind the pinned revision")
    members = manifest["members"]
    if not isinstance(members, list) or not members:
        raise RunError(f"{where}.members must be nonempty")
    paths = []
    for index, member in enumerate(members):
        row = _exact_keys(member, {"path", "sha256", "size"}, f"{where}.members[{index}]")
        path = row["path"]
        if (
            not isinstance(path, str)
            or not path
            or Path(path).is_absolute()
            or ".." in Path(path).parts
            or not _is_hex(row["sha256"], 64)
            or type(row["size"]) is not int
            or row["size"] < 0
        ):
            raise RunError(f"{where}.members[{index}] is invalid")
        paths.append(path)
    if paths != sorted(set(paths)):
        raise RunError(f"{where}.members are not sorted and unique")
    core = {
        key: manifest[key]
        for key in (
            "schema_version",
            "model_id",
            "revision",
            "ref",
            "members",
            "model_asset_digest",
        )
    }
    if manifest["snapshot_digest"] != digest(canonical_bytes(core)):
        raise RunError(f"{where}.snapshot_digest mismatch")
    return manifest


SANDBOX_EXEC = Path("/usr/bin/sandbox-exec")
MACOS_ISOLATION_BACKEND = "macos-seatbelt-v1"
LINUX_ISOLATION_BACKEND = linux_isolation.BACKEND
ISOLATION_PROOF_VERSION = 3

RUNNER_BUNDLE_MEMBERS = (
    "semble.py",
    "retrieval_contract.py",
    "finite_json.py",
    "linux_isolation.py",
)
RUNNER_ENTRYPOINT = b"from semble import main\nraise SystemExit(main())\n"
RUNNER_BUNDLE_LIMITS = raw_archive.ArchiveLimits(
    max_bytes=16 * 1024 * 1024,
    max_entries=len(RUNNER_BUNDLE_MEMBERS) + 2,
    max_directory_bytes=16 * 1024,
)


def build_runner_bundle(destination: Path) -> dict:
    """Build a deterministic stdlib zipapp from the frozen source list."""
    source_root = Path(__file__).resolve().parent
    try:
        members = {name: RawFile.capture(source_root / name) for name in RUNNER_BUNDLE_MEMBERS}
        with tempfile.TemporaryDirectory(prefix="retrieval-runner-bundle-") as directory:
            root = Path(directory).resolve(strict=True)
            members["__main__.py"] = write_raw_file(root / "__main__.py", [RUNNER_ENTRYPOINT])
            manifest = {
                "schema_version": 1,
                "entrypoint": "semble:main",
                "members": [
                    {"path": name, "sha256": raw.sha256.removeprefix("sha256:"), "size": raw.size}
                    for name, raw in sorted(members.items())
                ],
            }
            manifest_bytes = canonical_bytes(manifest) + b"\n"
            members["bundle-manifest.json"] = write_raw_file(
                root / "bundle-manifest.json", [manifest_bytes]
            )
            bundled = raw_archive.pack(members, destination, limits=RUNNER_BUNDLE_LIMITS)
    except (OSError, ValueError) as exc:
        raise RunError(f"cannot build runner bundle: {exc}") from exc
    return {
        "path": destination.name,
        "sha256": bundled.sha256.removeprefix("sha256:"),
        "manifest_sha256": digest(manifest_bytes),
        "manifest": manifest,
    }


def validate_runner_bundle(path: Path, expected: dict) -> None:
    bundled = _proof_file(path)
    if bundled.sha256.removeprefix("sha256:") != expected.get("sha256"):
        raise RunError("runner bundle digest mismatch")
    names = {"bundle-manifest.json", "__main__.py", *RUNNER_BUNDLE_MEMBERS}

    def admit(observed):
        if set(observed) != names:
            raise RunError("runner bundle member set differs from frozen source")

    try:
        with tempfile.TemporaryDirectory(prefix="retrieval-runner-replay-") as directory:
            root = Path(directory).resolve(strict=True)
            raw_archive.unpack(bundled, root, limits=RUNNER_BUNDLE_LIMITS, admit_names=admit)
            observed = {name: RawFile.capture(root / name) for name in names}
            _validate_runner_bundle_members(observed, expected)
        if _proof_file(path) != bundled:
            raise RunError("runner bundle changed during validation")
    except (OSError, ValueError) as exc:
        raise RunError(f"runner bundle is unreadable: {exc}") from exc


def _validate_runner_bundle_members(observed: dict[str, RawFile], expected: dict) -> None:
    manifest_bytes = observed.pop("bundle-manifest.json").read_control()
    if digest(manifest_bytes) != expected.get("manifest_sha256"):
        raise RunError("runner bundle manifest digest mismatch")
    if set(observed) != {"__main__.py", *RUNNER_BUNDLE_MEMBERS}:
        raise RunError("runner bundle member set differs from frozen source")
    manifest = expected.get("manifest")
    if (
        not isinstance(manifest, dict)
        or set(manifest) != {"schema_version", "entrypoint", "members"}
        or type(manifest["schema_version"]) is not int
        or manifest["schema_version"] != 1
        or manifest["entrypoint"] != "semble:main"
    ):
        raise RunError("runner bundle manifest is malformed")
    members = manifest.get("members")
    if not isinstance(members, list) or any(
        not isinstance(row, dict)
        or set(row) != {"path", "sha256", "size"}
        or not isinstance(row["path"], str)
        or not _is_hex(row["sha256"], 64)
        or type(row["size"]) is not int
        or row["size"] < 0
        for row in members
    ):
        raise RunError("runner bundle member manifest is malformed")
    rows = {row["path"]: row for row in members}
    if len(rows) != len(members):
        raise RunError("runner bundle member manifest contains duplicated paths")
    if list(rows) != sorted(rows):
        raise RunError("runner bundle member manifest is unordered")
    if set(rows) != set(observed):
        raise RunError("runner bundle member manifest is incomplete")
    if canonical_bytes(manifest) + b"\n" != manifest_bytes:
        raise RunError("runner bundle declared manifest differs from embedded bytes")
    for name, raw in observed.items():
        if rows[name] != {
            "path": name,
            "sha256": raw.sha256.removeprefix("sha256:"),
            "size": raw.size,
        }:
            raise RunError("runner bundle member bytes differ from manifest")
        if name in RUNNER_BUNDLE_MEMBERS:
            source = _proof_file(Path(__file__).resolve().parent / name)
            if (raw.sha256, raw.size) != (source.sha256, source.size):
                raise RunError("runner bundle member differs from frozen source")
        elif raw.read_control() != RUNNER_ENTRYPOINT:
            raise RunError("runner bundle entrypoint differs from prescribed bootstrap")


class RunError(ValueError):
    """Paired-run evidence is absent, inconsistent or ineligible."""


class ProcessRootAbsent(RunError):
    """A ps snapshot has no live row for the monitored root PID."""


QUERY_PROTOCOL_VERSION = 1


def _protocol_digest(payload: dict) -> str:
    return hashlib.sha256(canonical_bytes(payload)).hexdigest()


def build_query_protocol(
    task_ids: list[str], seed: int, warmup_passes: int, repetitions: int
) -> dict:
    """Build one deterministic schedule consumed byte-for-byte by both runners."""
    if (
        not task_ids
        or len(set(task_ids)) != len(task_ids)
        or any(not isinstance(task_id, str) or not task_id for task_id in task_ids)
    ):
        raise RunError("query protocol requires nonempty unique task ids")
    if type(seed) is not int or seed < 0:
        raise RunError("query protocol seed must be a nonnegative integer")
    if type(warmup_passes) is not int or warmup_passes < 0:
        raise RunError("query protocol warmup passes must be nonnegative")
    if type(repetitions) is not int or repetitions < 1:
        raise RunError("query protocol requires at least one measurement repetition")
    rng = random.Random(seed)

    def shuffled() -> list[str]:
        schedule = list(task_ids)
        rng.shuffle(schedule)
        return schedule

    core = {
        "schema_version": QUERY_PROTOCOL_VERSION,
        "seed": seed,
        "task_ids": list(task_ids),
        "cold_probe_task_id": task_ids[seed % len(task_ids)],
        "warmup_schedules": [shuffled() for _ in range(warmup_passes)],
        "measurement_schedules": [shuffled() for _ in range(repetitions)],
    }
    return {**core, "sha256": _protocol_digest(core)}


def validate_query_protocol(payload: object, task_ids: list[str], where: str) -> dict:
    protocol = _exact_keys(
        payload,
        {
            "schema_version",
            "seed",
            "task_ids",
            "cold_probe_task_id",
            "warmup_schedules",
            "measurement_schedules",
            "sha256",
        },
        where,
    )
    if protocol["schema_version"] != QUERY_PROTOCOL_VERSION:
        raise RunError(f"{where} schema version mismatch")
    if type(protocol["seed"]) is not int or protocol["seed"] < 0:
        raise RunError(f"{where}.seed must be a nonnegative integer")
    if protocol["task_ids"] != task_ids or len(set(task_ids)) != len(task_ids):
        raise RunError(f"{where}.task_ids differ from the frozen pack order")
    if protocol["cold_probe_task_id"] not in task_ids:
        raise RunError(f"{where}.cold_probe_task_id is not a frozen task")

    expected = set(task_ids)
    for key in ("warmup_schedules", "measurement_schedules"):
        schedules = protocol[key]
        if not isinstance(schedules, list) or (key == "measurement_schedules" and not schedules):
            raise RunError(f"{where}.{key} has an invalid schedule list")
        for index, schedule in enumerate(schedules):
            if (
                not isinstance(schedule, list)
                or len(schedule) != len(task_ids)
                or any(not isinstance(task_id, str) for task_id in schedule)
                or set(schedule) != expected
            ):
                raise RunError(f"{where}.{key}[{index}] must be an exact task permutation")
    core = {key: value for key, value in protocol.items() if key != "sha256"}
    if not _is_hex(protocol["sha256"], 64) or protocol["sha256"] != _protocol_digest(core):
        raise RunError(f"{where}.sha256 mismatch")
    expected_protocol = build_query_protocol(
        task_ids,
        protocol["seed"],
        len(protocol["warmup_schedules"]),
        len(protocol["measurement_schedules"]),
    )
    if protocol != expected_protocol:
        raise RunError(f"{where} differs from the deterministic seeded schedule")
    return protocol


def validate_qualified_speed_spec(spec: dict, task_count: int) -> None:
    roots = _int(spec.get("repetitions", 1), "spec.repetitions")
    warmups = _int(spec.get("query_warmup_passes", 0), "spec.query_warmup_passes")
    measurements = _int(
        spec.get("query_repetitions_per_root", 0), "spec.query_repetitions_per_root"
    )
    routes = spec.get("routes", ["lexical", "semantic", "hybrid"])
    if roots < FRESH_ROOTS_FLOOR:
        raise RunError(f"qualified speed requires at least {FRESH_ROOTS_FLOOR} fresh roots")
    if task_count < FROZEN_TASKS_FLOOR:
        raise RunError(f"qualified speed requires at least {FROZEN_TASKS_FLOOR} frozen tasks")
    if warmups < 1:
        raise RunError("qualified speed requires at least one shared warmup pass")
    if task_count * measurements * roots < PILOT_OBSERVATIONS_FLOOR:
        raise RunError(
            f"qualified speed requires at least {PILOT_OBSERVATIONS_FLOOR} warm observations per route"
        )
    if not isinstance(routes, list) or len(routes) != 1:
        raise RunError("qualified speed requires exactly one Quanta route")


def _process_tree_sample(root_pid: int) -> list[dict]:
    """Return one owned-process-tree RSS/CPU sample.

    Ownership is transitive PPID reachability over every syntactically valid
    live (non-zombie) ps row, including live zero-RSS connector processes.
    A live zero-RSS parent therefore keeps its positive-RSS descendants in
    the owned set. Metric rows are emitted only for owned processes with
    positive RSS: the frozen resource artifact only accepts positive-RSS
    process rows, so connector nodes stay in the ownership graph without
    producing metric rows, and no descendant is dropped with them.
    """
    if type(root_pid) is not int or root_pid < 1:
        raise RunError("ps process root PID must be positive")
    output = subprocess.check_output(
        ["ps", "-axo", "pid=,ppid=,rss=,pcpu=,stat=,comm="],
        text=True,
        stderr=subprocess.STDOUT,
    )
    topology: dict[int, tuple[int, int, float, str, str]] = {}
    for line in output.splitlines():
        if not line.strip():
            continue
        fields = line.split(maxsplit=5)
        if len(fields) != 6:
            raise RunError(f"malformed ps process row: {line!r}")
        try:
            pid, ppid, rss_kib = (int(field) for field in fields[:3])
            cpu_percent = float(fields[3])
        except ValueError as exc:
            raise RunError(f"malformed ps process row: {line!r}") from exc
        state, command = fields[4], fields[5]
        if (
            pid < 1
            or ppid < 0
            or rss_kib < 0
            or not math.isfinite(cpu_percent)
            or cpu_percent < 0
            or not state
            or not command
        ):
            raise RunError(f"malformed ps process row: {line!r}")
        if pid in topology:
            raise RunError(f"duplicate ps process PID: {pid}")
        topology[pid] = (ppid, rss_kib, cpu_percent, state, command)
    if root_pid not in topology or topology[root_pid][3].startswith("Z"):
        raise ProcessRootAbsent(f"root process is absent from live ps snapshot: {root_pid}")
    # A zombie cannot bridge ownership to a live descendant.
    topology = {pid: row for pid, row in topology.items() if not row[3].startswith("Z")}
    owned = {root_pid}
    changed = True
    while changed:
        changed = False
        for pid, (ppid, *_tail) in topology.items():
            if pid not in owned and ppid in owned:
                owned.add(pid)
                changed = True
    return [
        {
            "pid": pid,
            "ppid": topology[pid][0],
            "rss_bytes": topology[pid][1] * 1024,
            "cpu_percent": topology[pid][2],
            "command": topology[pid][4],
        }
        for pid in sorted(owned)
        if pid in topology and topology[pid][1] > 0
    ]


def _cleanup_process_group(pgid: int, timeout_secs: float = 5.0) -> tuple[bool, bool, str | None]:
    """Terminate any surviving descendants in the owned process group."""
    try:
        os.killpg(pgid, 0)
    except ProcessLookupError:
        return True, False, None
    except OSError as exc:
        return False, False, str(exc)
    try:
        os.killpg(pgid, signal.SIGTERM)
    except ProcessLookupError:
        return True, False, None
    except OSError as exc:
        return False, False, str(exc)
    deadline = time.monotonic() + timeout_secs
    while time.monotonic() < deadline:
        try:
            os.killpg(pgid, 0)
        except ProcessLookupError:
            return True, False, None
        except OSError as exc:
            return False, False, str(exc)
        time.sleep(0.05)
    try:
        os.killpg(pgid, signal.SIGKILL)
    except ProcessLookupError:
        return True, True, None
    except OSError as exc:
        return False, True, str(exc)
    deadline = time.monotonic() + 2.0
    while time.monotonic() < deadline:
        try:
            os.killpg(pgid, 0)
        except ProcessLookupError:
            return True, True, None
        except OSError as exc:
            return False, True, str(exc)
        time.sleep(0.05)
    return False, True, "owned process group survived SIGKILL"


def _validate_cgroup_parent_identity(payload: object, where: str) -> dict:
    identity = _exact_keys(payload, {"path", "device", "inode"}, where)
    path = identity["path"]
    if (
        not isinstance(path, str)
        or not Path(path).is_absolute()
        or ".." in Path(path).parts
        or str(Path(path)) != path
    ):
        raise RunError(f"{where}.path must be absolute and normalized")
    for key in ("device", "inode"):
        if type(identity[key]) is not int or identity[key] < 1:
            raise RunError(f"{where}.{key} must be positive")
    return identity


def _linux_parent_identity(parent: str) -> dict:
    if not isinstance(parent, str) or not parent:
        raise RunError("qualified Linux requires an explicit delegated cgroup parent")
    path = Path(parent)
    try:
        resolved = path.resolve(strict=True)
        identity = path.stat()
    except OSError as exc:
        raise RunError(f"delegated cgroup parent unavailable: {exc}") from exc
    if not path.is_absolute() or resolved != path or not path.is_dir():
        raise RunError("delegated cgroup parent must be an absolute, non-symlink directory")
    return _validate_cgroup_parent_identity(
        {"path": str(path), "device": identity.st_dev, "inode": identity.st_ino},
        "delegated cgroup parent",
    )


def _finish_linux_attestation(
    attestation_pipe: tuple[int, int, str] | None,
    isolation: dict | None,
    command: list[str],
    split: int | None,
    exit_code: int | None,
) -> dict | None:
    recorded = dict(isolation) if isolation is not None else None
    if attestation_pipe is None:
        return recorded
    read_fd, _write_fd, nonce = attestation_pipe
    try:
        raw = os.read(read_fd, 4097)
    finally:
        os.close(read_fd)
    try:
        attestation = json.loads(raw)
    except (ValueError, UnicodeDecodeError):
        attestation = None
    expected_keys = {
        "nonce",
        "abi",
        "exec_sha256",
        "suite_read_denied",
        "query_pack_read_allowed",
        "proc_read_denied",
    }
    invalid = (
        not isinstance(attestation, dict)
        or set(attestation) != expected_keys
        or attestation.get("nonce") != nonce
        or split is None
        or attestation.get("exec_sha256") != digest(canonical(command[split + 1 :]))
        or type(attestation.get("abi")) is not int
        or attestation["abi"] < linux_isolation.MIN_ABI
        or any(
            attestation[key] is not True
            for key in ("suite_read_denied", "query_pack_read_allowed", "proc_read_denied")
        )
        or len(raw) > 4096
    )
    if invalid and exit_code == 0:
        raise RunError("Linux child did not attest deny/allow after Landlock enforcement")
    recorded.pop("_child_check")
    recorded["child_attestation"] = None if invalid else attestation
    return recorded


def _linux_owned_resource(
    command: list[str],
    actual_command: list[str],
    *,
    stdout_path: Path,
    stderr_path: Path,
    resource_path: Path,
    timeout_secs: int,
    subject_path: Path | None,
    env: dict[str, str] | None,
    cwd: str | None,
    sample_interval_ms: int,
    isolation: dict | None,
    attestation_pipe: tuple[int, int, str] | None,
    split: int | None,
    capture_scope: str,
    cgroup_parent: str | None,
    expected_cgroup_parent_identity: dict | None,
) -> dict:
    qualified = capture_scope == "qualified"
    try:
        parent_identity = _linux_parent_identity(cgroup_parent) if qualified else None
        if (
            qualified
            and expected_cgroup_parent_identity is not None
            and (parent_identity != expected_cgroup_parent_identity)
        ):
            raise RunError("delegated cgroup parent identity drifted before launch")
    except (RunError, OSError):
        if attestation_pipe:
            os.close(attestation_pipe[0])
            os.close(attestation_pipe[1])
        raise
    try:
        result = linux_process.run(
            actual_command,
            timeout_secs=timeout_secs,
            sample_interval_ms=sample_interval_ms,
            env=env,
            cwd=cwd,
            stdout_path=str(stdout_path),
            stderr_path=str(stderr_path),
            qualified=qualified,
            cgroup_parent=cgroup_parent if qualified else None,
            pass_fds=(attestation_pipe[1],) if attestation_pipe else (),
        )
    except Exception as exc:
        if attestation_pipe:
            os.close(attestation_pipe[0])
            os.close(attestation_pipe[1])
        raise RunError(f"Linux process ownership failed: {exc}") from exc
    if attestation_pipe:
        os.close(attestation_pipe[1])
    if qualified and _linux_parent_identity(cgroup_parent) != parent_identity:
        if attestation_pipe:
            os.close(attestation_pipe[0])
        raise RunError("delegated cgroup parent identity drifted during capture")
    if qualified and Path(result.cgroup_path or "").parent != Path(cgroup_parent):
        if attestation_pipe:
            os.close(attestation_pipe[0])
        raise RunError("owned cgroup is not below the requested delegated parent")
    recorded_isolation = _finish_linux_attestation(
        attestation_pipe, isolation, command, split, result.root_exit_code
    )
    if qualified and not result.ownership_complete:
        raise RunError("qualified Linux capture lacks complete cgroup-v2 ownership evidence")
    subject_sha256 = None
    if subject_path is not None and subject_path.is_file():
        subject_sha256 = sha_file(subject_path)
    payload = {
        "schema_version": 2,
        "sampler": "linux-process-owner-v1",
        "capture_scope": capture_scope,
        "owner_backend": result.backend,
        "sample_interval_ms": result.sample_interval_ms,
        "command_sha256": digest(canonical(actual_command)),
        **(
            {"exec_command_sha256": digest(canonical(command[split + 1 :]))}
            if attestation_pipe
            else {}
        ),
        "subject_sha256": subject_sha256,
        "root_pid": result.root.pid,
        "root_start_ticks": result.root.start_ticks,
        "exit_code": result.root_exit_code,
        "timed_out": result.timed_out,
        "elapsed_ms": result.elapsed_ms,
        "peak_rss_bytes": result.peak_tree_rss_bytes,
        "peak_cgroup_memory_bytes": result.peak_cgroup_memory_bytes,
        "total_user_cpu_ns": result.total_user_cpu_ns,
        "total_kernel_cpu_ns": result.total_kernel_cpu_ns,
        "cgroup_cpu_usage_ns": result.cgroup_cpu_usage_ns,
        "cgroup_path": result.cgroup_path,
        "delegated_cgroup_parent": parent_identity,
        "processes": [
            {
                "pid": item.identity.pid,
                "start_ticks": item.identity.start_ticks,
                "peak_rss_bytes": item.peak_rss_bytes,
                "user_cpu_ns": item.user_cpu_ns,
                "kernel_cpu_ns": item.kernel_cpu_ns,
            }
            for item in result.processes
        ],
        "escaped": [{"pid": item.pid, "start_ticks": item.start_ticks} for item in result.escaped],
        "samples": result.samples,
        "sampling_complete": result.sampling_complete,
        "cleanup_complete": result.cleanup_complete,
        "ownership_complete": result.ownership_complete,
        "isolation": recorded_isolation,
    }
    with resource_path.open("x", encoding="utf-8") as handle:
        handle.write(json.dumps(payload, indent=2, sort_keys=True) + "\n")
    return payload


def run_monitored_process(
    command: list[str],
    *,
    stdout_path: Path,
    stderr_path: Path,
    resource_path: Path,
    timeout_secs: int,
    subject_path: Path | None = None,
    env: dict[str, str] | None = None,
    cwd: str | None = None,
    sample_interval_ms: int = 50,
    isolation: dict | None = None,
    capture_scope: str = "exploratory",
    linux_cgroup_parent: str | None = None,
    linux_cgroup_parent_identity: dict | None = None,
) -> dict:
    """Run a platform owner and persist exact resource evidence."""
    if capture_scope not in ("exploratory", "qualified"):
        raise RunError("capture scope must be exploratory or qualified")
    linux = platform.system() == "Linux"
    if (
        linux
        and capture_scope == "qualified"
        and (
            not linux_cgroup_parent
            or not isinstance(isolation, dict)
            or isolation.get("backend") != LINUX_ISOLATION_BACKEND
        )
    ):
        raise RunError("qualified Linux capture requires delegated cgroup v2 and Landlock")
    if linux and capture_scope != "qualified" and linux_cgroup_parent is not None:
        raise RunError("exploratory Linux fallback must not claim a cgroup parent")
    if linux and capture_scope != "qualified" and linux_cgroup_parent_identity is not None:
        raise RunError("exploratory Linux fallback must not claim a cgroup identity")
    if not linux and linux_cgroup_parent is not None:
        raise RunError("linux_cgroup_parent requires Linux")
    if timeout_secs <= 0:
        raise RunError("monitored process timeout must be positive")
    if sample_interval_ms <= 0:
        raise RunError("resource sample interval must be positive")
    for path in (stdout_path, stderr_path, resource_path):
        if path.exists():
            raise RunError(f"refusing existing process evidence: {path}")
        path.parent.mkdir(parents=True, exist_ok=True)
    attestation_pipe = None
    split = None
    actual_command = command
    if isolation is not None and isolation.get("backend") == LINUX_ISOLATION_BACKEND:
        child_check = isolation.get("_child_check")
        if not isinstance(child_check, dict) or set(child_check) != {"pack_sha256"}:
            raise RunError("Linux capture lacks child check context")
        if "--" not in command:
            raise RunError("Linux capture wrapper lacks exec delimiter")
        nonce = secrets.token_hex(32)
        read_fd, write_fd = os.pipe()
        attestation_pipe = (read_fd, write_fd, nonce)
        split = command.index("--")
        actual_command = [
            *command[:split],
            "--attest-fd",
            str(write_fd),
            "--nonce",
            nonce,
            *command[split:],
        ]
    if linux:
        return _linux_owned_resource(
            command,
            actual_command,
            stdout_path=stdout_path,
            stderr_path=stderr_path,
            resource_path=resource_path,
            timeout_secs=timeout_secs,
            subject_path=subject_path,
            env=env,
            cwd=cwd,
            sample_interval_ms=sample_interval_ms,
            isolation=isolation,
            attestation_pipe=attestation_pipe,
            split=split,
            capture_scope=capture_scope,
            cgroup_parent=linux_cgroup_parent,
            expected_cgroup_parent_identity=linux_cgroup_parent_identity,
        )
    started = time.monotonic()
    peak_rss_bytes = 0
    peak_cpu_percent = 0.0
    process_peaks: dict[int, dict] = {}
    samples = 0
    sample_error: str | None = None
    timed_out = False
    with (
        stdout_path.open("x", encoding="utf-8") as stdout,
        stderr_path.open("x", encoding="utf-8") as stderr,
    ):
        try:
            process = subprocess.Popen(
                actual_command,
                stdin=subprocess.DEVNULL,
                stdout=stdout,
                stderr=stderr,
                text=True,
                env=env,
                cwd=cwd,
                start_new_session=True,
                pass_fds=(attestation_pipe[1],) if attestation_pipe else (),
            )
        except OSError as error:
            if attestation_pipe:
                os.close(attestation_pipe[0])
                os.close(attestation_pipe[1])
            raise RunError(f"cannot start monitored process: {error}") from error
        if attestation_pipe:
            os.close(attestation_pipe[1])
        while True:
            try:
                sample = _process_tree_sample(process.pid)
                rss = sum(row["rss_bytes"] for row in sample)
                cpu = sum(row["cpu_percent"] for row in sample)
                if sample and rss > 0:
                    peak_rss_bytes = max(peak_rss_bytes, rss)
                    peak_cpu_percent = max(peak_cpu_percent, cpu)
                    samples += 1
                    for row in sample:
                        peak = process_peaks.setdefault(
                            row["pid"],
                            {
                                "pid": row["pid"],
                                "command": row["command"],
                                "peak_rss_bytes": 0,
                                "peak_cpu_percent": 0.0,
                                "samples": 0,
                            },
                        )
                        peak["peak_rss_bytes"] = max(peak["peak_rss_bytes"], row["rss_bytes"])
                        peak["peak_cpu_percent"] = max(peak["peak_cpu_percent"], row["cpu_percent"])
                        peak["samples"] += 1
            except ProcessRootAbsent as error:
                # A completed root with no surviving process group is an
                # ordinary end-of-run race after prior valid samples. A
                # still-running root or surviving descendants make the
                # ownership snapshot incomplete instead.
                root_exit = process.poll()
                group_survives = root_exit is None
                if root_exit is not None:
                    try:
                        os.killpg(process.pid, 0)
                    except ProcessLookupError:
                        group_survives = False
                    except OSError as probe_error:
                        group_survives = True
                        error = RunError(f"{error}; process-group probe failed: {probe_error}")
                    else:
                        group_survives = True
                if group_survives and sample_error is None:
                    sample_error = str(error)
            except (OSError, subprocess.CalledProcessError, RunError) as error:
                if sample_error is None:
                    sample_error = str(error)
            exit_code = process.poll()
            if exit_code is not None:
                break
            if time.monotonic() - started >= timeout_secs:
                timed_out = True
                try:
                    os.killpg(process.pid, signal.SIGTERM)
                    process.wait(timeout=5)
                except (ProcessLookupError, subprocess.TimeoutExpired):
                    try:
                        os.killpg(process.pid, signal.SIGKILL)
                    except ProcessLookupError:
                        pass
                    process.wait()
                exit_code = process.returncode
                break
            time.sleep(sample_interval_ms / 1000.0)
    elapsed_ms = (time.monotonic() - started) * 1000.0
    cleanup_complete, cleanup_escalated, cleanup_error = _cleanup_process_group(process.pid)
    recorded_isolation = _finish_linux_attestation(
        attestation_pipe, isolation, command, split, exit_code
    )
    subject_sha256 = None
    if subject_path is not None and subject_path.is_file():
        subject_sha256 = sha_file(subject_path)
    payload = {
        "schema_version": 1,
        "sampler": "ps-process-tree-rss-cpu-v2",
        "sample_interval_ms": sample_interval_ms,
        "command_sha256": digest(canonical(actual_command)),
        **(
            {"exec_command_sha256": digest(canonical(command[split + 1 :]))}
            if attestation_pipe
            else {}
        ),
        "subject_sha256": subject_sha256,
        "root_pid": process.pid,
        "exit_code": exit_code,
        "timed_out": timed_out,
        "elapsed_ms": elapsed_ms,
        "peak_rss_bytes": peak_rss_bytes if samples else None,
        "peak_cpu_percent": peak_cpu_percent if samples else None,
        "processes": [process_peaks[pid] for pid in sorted(process_peaks)],
        "samples": samples,
        "complete": sample_error is None and samples > 0,
        "error": sample_error,
        "cleanup_complete": cleanup_complete,
        "cleanup_escalated": cleanup_escalated,
        "cleanup_error": cleanup_error,
        "isolation": recorded_isolation,
    }
    resource_path.write_text(json.dumps(payload, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    return payload


def write_process_failure(
    run_dir: Path,
    *,
    system: str,
    strategy: str,
    failure_type: str,
    resource_path: Path,
    stderr_path: Path,
    record_path: Path,
    refusal_path: Path | None = None,
) -> Path:
    """Freeze a typed non-scoreable process failure for forensic review."""
    failure_path = run_dir / "failure.json"
    payload = {
        "schema_version": 2,
        "phase": "query_plan" if refusal_path is not None and refusal_path.is_file() else "process",
        "system": system,
        "strategy": strategy,
        "failure_type": failure_type,
        "resource_sha256": sha_file(resource_path),
        "stderr_sha256": sha_file(stderr_path),
        "record_emitted": record_path.is_file(),
        "query_plan_refusal_sha256": (
            sha_file(refusal_path) if refusal_path is not None and refusal_path.is_file() else None
        ),
    }
    with failure_path.open("x", encoding="utf-8") as handle:
        handle.write(json.dumps(payload, indent=2, sort_keys=True) + "\n")
    return failure_path


def _int(value: object, label: str) -> int:
    try:
        return int(str(value))
    except (TypeError, ValueError) as exc:
        raise RunError(f"{label} must be an integer: {value!r}") from exc


def read_json(path: Path) -> object:
    try:
        return read_evidence_json(path)
    except ValueError as exc:
        raise RunError(f"cannot read JSON {path}: {exc}") from exc


def sha_file(path: Path) -> str:
    digestor = hashlib.sha256()
    with path.open("rb") as handle:
        for block in iter(lambda: handle.read(65536), b""):
            digestor.update(block)
    return digestor.hexdigest()


def _path_within(path: Path, root: Path) -> bool:
    resolved = path.resolve()
    boundary = root.resolve()
    return resolved == boundary or boundary in resolved.parents


def _seatbelt_profile(
    denied_roots: list[str],
    allowed_read_roots: list[str],
    allowed_write_roots: list[str],
) -> str:
    """Build a deterministic default-deny profile with explicit file authority."""
    system_read_roots = [
        "/System",
        "/usr",
        "/bin",
        "/sbin",
        "/Library/Apple",
        "/private/var/db/dyld",
        "/private/var/db/timezone",
        "/dev",
    ]

    def validated(values: list[str], label: str) -> list[str]:
        roots = sorted(set(values))
        for root in roots:
            if (
                not isinstance(root, str)
                or not Path(root).is_absolute()
                or "\n" in root
                or "\x00" in root
            ):
                raise RunError(f"invalid Seatbelt {label} root: {root!r}")
        return roots

    denied = validated(denied_roots, "denial")
    allowed_read = validated([*system_read_roots, *allowed_read_roots], "read allow")
    allowed_write = validated(allowed_write_roots, "write allow")
    rules = [
        "(version 1)",
        "(deny default)",
        '(import "system.sb")',
        "(allow process*)",
        "(allow signal (target self))",
        "(allow signal (target children))",
        "(allow sysctl-read)",
        "(allow mach-lookup)",
        "(allow ipc-posix-shm)",
        "(allow file-read-metadata)",
    ]
    for root in allowed_read:
        quoted = json.dumps(root)
        rules.append(f"(allow file-read* (literal {quoted}))")
        rules.append(f"(allow file-read* (subpath {quoted}))")
    for root in allowed_write:
        quoted = json.dumps(root)
        rules.append(f"(allow file-write* (literal {quoted}))")
        rules.append(f"(allow file-write* (subpath {quoted}))")
        # Unix-domain listener creation and client connects are governed by
        # Seatbelt's network operations as well as filesystem writes. Keep
        # that authority path-scoped to the same capture output roots.
        rules.append(f"(allow network-bind (prefix {quoted}))")
        rules.append(f"(allow network-outbound (prefix {quoted}))")
    for root in denied:
        quoted = json.dumps(root)
        rules.append(f"(deny file-read* (literal {quoted}))")
        rules.append(f"(deny file-read* (subpath {quoted}))")
        rules.append(f"(deny file-write* (literal {quoted}))")
        rules.append(f"(deny file-write* (subpath {quoted}))")
    return "\n".join(rules) + "\n"


def _probe_seatbelt(profile: str, suite_path: Path, pack_path: Path) -> dict:
    """Prove the same profile denies gold and permits the blind input."""
    try:
        denied = subprocess.run(
            [str(SANDBOX_EXEC), "-p", profile, "/bin/cat", str(suite_path)],
            capture_output=True,
            text=True,
            timeout=15,
        )
        allowed = subprocess.run(
            [str(SANDBOX_EXEC), "-p", profile, "/usr/bin/shasum", "-a", "256", str(pack_path)],
            capture_output=True,
            text=True,
            timeout=15,
        )
    except (OSError, subprocess.SubprocessError) as exc:
        raise RunError(f"Seatbelt isolation probe failed: {exc}") from exc
    observed_pack = allowed.stdout.split()[0] if allowed.stdout.split() else ""
    result = {
        "suite_read_denied": denied.returncode != 0 and denied.stdout == "",
        "query_pack_read_allowed": (
            allowed.returncode == 0 and observed_pack == sha_file(pack_path)
        ),
    }
    if not all(result.values()):
        raise RunError("Seatbelt isolation probe did not deny suite and allow the exact query pack")
    return result


def _manifest_rows(manifest_path: Path) -> tuple[str, list[tuple[str, str]]]:
    payload = read_json(manifest_path)
    if not isinstance(payload, dict):
        raise RunError("corpus manifest must be an object")
    commit = payload.get("repository_commit")
    files = payload.get("files")
    if not _is_hex(commit, 40) or not isinstance(files, list) or not files:
        raise RunError("corpus manifest lacks a commit or admitted files")
    rows: list[tuple[str, str]] = []
    for entry in files:
        if not isinstance(entry, dict):
            raise RunError("corpus manifest file entry must be an object")
        name, expected = entry.get("path"), entry.get("file_sha256")
        if (
            not isinstance(name, str)
            or not name
            or Path(name).is_absolute()
            or "\\" in name
            or any(part in ("", ".", "..") for part in name.split("/"))
            or not _is_hex(expected, 64)
        ):
            raise RunError(f"unsafe corpus manifest entry: {name!r}")
        rows.append((name, expected))
    if len({name for name, _ in rows}) != len(rows):
        raise RunError("corpus manifest holds duplicate paths")
    return commit, sorted(rows)


def _verify_materialized_corpus(
    root: Path, manifest_path: Path, expected_proof_sha256: str | None = None
) -> dict:
    """Verify an exact, Git-free admitted-file view used inside the sandbox."""
    commit, rows = _manifest_rows(manifest_path)
    root = root.resolve()
    if not root.is_dir() or (root / ".git").exists():
        raise RunError("materialized corpus must be a Git-free directory")
    observed: list[tuple[str, str]] = []
    for dirpath, dirnames, filenames in os.walk(root, followlinks=False):
        base = Path(dirpath)
        for dirname in dirnames:
            if (base / dirname).is_symlink():
                raise RunError("materialized corpus contains a symlinked directory")
        for filename in filenames:
            path = base / filename
            if path.is_symlink() or not path.is_file():
                raise RunError("materialized corpus contains a non-regular file")
            observed.append((path.relative_to(root).as_posix(), sha_file(path)))
    if sorted(observed) != rows:
        raise RunError("materialized corpus differs from the exact admitted universe")
    core = {
        "schema_version": 1,
        "repository_commit": commit,
        "manifest_sha256": sha_file(manifest_path),
        "files": [{"path": name, "file_sha256": sha} for name, sha in rows],
    }
    proof_sha256 = digest(canonical_bytes(core))
    if expected_proof_sha256 is not None and proof_sha256 != expected_proof_sha256:
        raise RunError("materialized corpus proof digest mismatch")
    return {**core, "proof_sha256": proof_sha256}


def materialize_corpus_view(spec: dict, stage: Path, source_repo: Path) -> dict:
    """Copy only manifest-admitted bytes into the runner-readable sandbox view."""
    manifest_path = Path(spec["manifest"])
    commit, rows = _manifest_rows(manifest_path)
    try:
        source_repo = verify_repo(source_repo, commit)
    except ValueError as exc:
        raise RunError(f"materialized corpus source proof failed: {exc}") from exc
    view = stage / "runner-corpus"
    if view.exists():
        raise RunError("materialized corpus view already exists")
    view.mkdir()
    for name, expected in rows:
        source = source_repo / name
        if (
            source.is_symlink()
            or not source.is_file()
            or source_repo not in source.resolve().parents
        ):
            raise RunError(f"admitted source is not a regular in-repository file: {name}")
        if sha_file(source) != expected:
            raise RunError(f"admitted source digest drifted during materialization: {name}")
        target = view / name
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(source, target)
    proof = _verify_materialized_corpus(view, manifest_path)
    updated = dict(spec)
    updated["repo"] = str(view.resolve())
    updated["_source_repo"] = str(source_repo.resolve())
    updated["_materialized_corpus"] = proof
    return updated


def _linux_policy(spec: dict, stage: Path, denied_roots: list[str]) -> dict:
    """Grant only runner inputs and per-repetition output trees, never stage."""
    runner_input = stage / "runner-input"
    runner_input.mkdir()
    cache = Path(spec["semble_cache_root"]).resolve()
    reps = [
        stage / f"rep-{rep:02d}"
        for rep in range(_int(spec.get("repetitions", 1), "spec.repetitions"))
    ]
    for rep in reps:
        rep.mkdir()
    runtime = [
        "/lib",
        "/lib64",
        "/usr/lib",
        "/usr/lib64",
        "/usr/local/lib",
        "/etc/ld.so.cache",
        "/etc/ssl",
        "/dev/urandom",
    ]
    read = [
        stage / "runner-tools.pyz",
        runner_input,
        Path(spec["repo"]),
        Path(spec["manifest"]),
        Path(spec["query_pack"]),
        Path(spec["runner_binary"]),
        Path(spec["searchd_binary"]),
        Path(spec["semble_lockfile"]),
        Path(spec["semble_python"]).absolute().parent.parent,
        Path(spec["semble_python"]).resolve().parent.parent,
        Path(sys.executable).resolve().parent.parent,
        Path("/bin/cat"),
    ]
    if "quanta_model_dir" in spec:
        read.append(Path(spec["quanta_model_dir"]))
    read.append(cache)
    read.extend(Path(path) for path in runtime if Path(path).exists())
    policy = {
        "readonly": sorted({str(path.resolve()) for path in read}),
        "writable": sorted({str(path.resolve()) for path in [*reps, Path("/dev/null")]}),
        "denied": denied_roots,
    }
    try:
        linux_isolation.validate_policy(policy)
    except (linux_isolation.IsolationError, OSError) as exc:
        raise RunError(f"Linux isolation policy is invalid: {exc}") from exc
    return policy


def _probe_linux(policy: dict, module: Path, python: Path, suite: Path, pack: Path) -> dict:
    """Observe deny/allow from fresh restricted children with this exact policy."""
    state = linux_isolation.probe()
    if state["state"] != "available":
        raise RunError(f"Linux Landlock unavailable: {state}")
    cat = str(Path("/bin/cat").resolve())
    try:
        control_suite = subprocess.run(
            [cat, str(suite)], stdin=subprocess.DEVNULL, capture_output=True, timeout=15
        )
        control_proc = subprocess.run(
            [cat, "/proc/self/environ"],
            stdin=subprocess.DEVNULL,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.PIPE,
            timeout=15,
        )
    except (OSError, subprocess.SubprocessError) as exc:
        raise RunError(f"Linux Landlock unrestricted control failed: {exc}") from exc
    if (
        control_suite.returncode != 0
        or hashlib.sha256(control_suite.stdout).hexdigest() != sha_file(suite)
        or control_proc.returncode != 0
    ):
        raise RunError("Linux Landlock unrestricted control cannot read suite and proc")
    with tempfile.TemporaryDirectory(prefix="retrieval-landlock-probe-") as temp:
        policy_path = Path(temp) / "policy.json"
        policy_path.write_text(json.dumps(policy, sort_keys=True) + "\n", encoding="utf-8")

        def read(path: Path) -> subprocess.CompletedProcess:
            try:
                return subprocess.run(
                    [
                        str(python),
                        str(module),
                        "--policy",
                        str(policy_path),
                        "--",
                        cat,
                        str(path),
                    ],
                    stdin=subprocess.DEVNULL,
                    capture_output=True,
                    timeout=15,
                    env={**os.environ, "LC_ALL": "C"},
                )
            except (OSError, subprocess.SubprocessError) as exc:
                raise RunError(f"Linux Landlock child probe failed: {exc}") from exc

        denied = read(suite)
        allowed = read(pack)
        proc = read(Path("/proc/self/environ"))
    probes = {
        "suite_read_denied": denied.returncode != 0
        and not denied.stdout
        and b"Permission denied" in denied.stderr,
        "query_pack_read_allowed": allowed.returncode == 0
        and hashlib.sha256(allowed.stdout).hexdigest() == sha_file(pack),
        "proc_read_denied": proc.returncode != 0
        and not proc.stdout
        and b"Permission denied" in proc.stderr,
    }
    if not all(probes.values()):
        raise RunError(f"Linux Landlock child deny/allow probe failed: {probes}")
    return {"abi": state["abi"], "probes": probes}


def prepare_isolation(spec: dict, stage: Path, original_suite: Path) -> dict:
    """Create and verify the evaluator-only denial boundary for paired capture."""
    if spec.get("blinding", "attested") != "isolated":
        return spec
    system = platform.system()
    if system == "Darwin" and not SANDBOX_EXEC.is_file():
        raise RunError("macOS Seatbelt executable is unavailable")
    if system not in ("Darwin", "Linux"):
        raise RunError("isolated blinding requires macOS Seatbelt or Linux Landlock")
    if system == "Linux":
        state = linux_isolation.probe()
        if state["state"] != "available":
            raise RunError(f"Linux Landlock unavailable: {state}")
    if "isolation_method" in spec or "access_block_log" in spec:
        raise RunError(
            "isolated blinding derives isolation_method/access_block_log; do not supply them"
        )
    secret_value = spec.get("suite_secret_root")
    if not isinstance(secret_value, str) or not secret_value:
        raise RunError("isolated blinding requires spec.suite_secret_root")
    secret_root = Path(secret_value).resolve()
    original_suite_input = original_suite
    original_suite = original_suite.resolve()
    repo = Path(spec["repo"]).resolve()
    source_repo = Path(spec.get("_source_repo", spec["repo"])).resolve()
    materialized = spec.get("_materialized_corpus")
    if not isinstance(materialized, dict):
        raise RunError("isolated blinding requires a verified materialized corpus view")
    _verify_materialized_corpus(repo, Path(spec["manifest"]), materialized.get("proof_sha256"))
    if repo == source_repo or _path_within(repo, source_repo):
        raise RunError("materialized corpus must be outside the denied source checkout")
    if not secret_root.is_dir() or not _path_within(original_suite, secret_root):
        raise RunError("suite must be a regular file inside suite_secret_root")
    if (
        not original_suite.is_file()
        or original_suite_input.is_symlink()
        or original_suite.stat().st_nlink != 1
    ):
        raise RunError("isolated suite must be a regular single-link non-symlink file")
    if _path_within(original_suite, repo):
        raise RunError(
            "isolated suite must be outside the runner-readable repository and its Git objects"
        )
    readable_inputs = [
        Path(spec[key]).resolve()
        for key in (
            "repo",
            "manifest",
            "query_pack",
            "runner_binary",
            "searchd_binary",
            "semble_python",
            "semble_lockfile",
        )
    ]
    readable_inputs.extend((stage.resolve(), Path(spec["output_root"]).resolve()))
    conflicts = sorted(str(path) for path in readable_inputs if _path_within(path, secret_root))
    if conflicts:
        raise RunError(
            "suite_secret_root also contains runner-readable inputs: " + ", ".join(conflicts)
        )
    evaluator_root = (stage / "evaluator-only").resolve()
    runner_bundle = (stage / "runner-tools.pyz").resolve()
    bundle_proof = build_runner_bundle(runner_bundle)
    suite_path = Path(spec["suite"]).resolve()
    pack_path = Path(spec["query_pack"]).resolve()
    if not _path_within(suite_path, evaluator_root):
        raise RunError("frozen suite must be under the evaluator-only stage root")
    denied_roots = sorted({str(secret_root), str(evaluator_root), str(source_repo)})
    semble_python = Path(spec["semble_python"]).absolute()
    semble_env_root = semble_python.parent.parent
    semble_interpreter_root = semble_python.resolve().parent.parent
    extra_read_roots = [
        str(repo),
        str(Path(spec["manifest"]).resolve()),
        str(pack_path),
        str(Path(spec["runner_binary"]).resolve()),
        str(Path(spec["searchd_binary"]).resolve()),
        str(Path(spec["semble_lockfile"]).resolve()),
        str(semble_env_root),
        str(semble_interpreter_root),
        str(Path(sys.executable).resolve().parent.parent),
        str(Path(spec["semble_cache_root"]).resolve()),
        str(stage.resolve()),
        str(Path(spec["output_root"]).resolve()),
    ]
    for optional in ("quanta_model_dir",):
        if optional in spec:
            extra_read_roots.append(str(Path(spec[optional]).resolve()))
    if system == "Darwin":
        backend = MACOS_ISOLATION_BACKEND
        allowed_read_roots = sorted(set(extra_read_roots))
        allowed_write_roots = sorted(
            {str(stage.resolve()), str(Path(spec["output_root"]).resolve())}
        )
        profile = _seatbelt_profile(denied_roots, allowed_read_roots, allowed_write_roots)
        policy_sha256 = hashlib.sha256(profile.encode("utf-8")).hexdigest()
        probes = _probe_seatbelt(profile, suite_path, pack_path)
        backend_proof = {
            "sandbox_exec": {"path": str(SANDBOX_EXEC), "sha256": sha_file(SANDBOX_EXEC)}
        }
        isolation = {"backend": backend, "profile": profile}
    else:
        backend = LINUX_ISOLATION_BACKEND
        platform_dir = (stage / "platform-tools").resolve()
        platform_dir.mkdir()
        module = platform_dir / "linux_isolation.py"
        shutil.copyfile(Path(linux_isolation.__file__), module)
        policy = _linux_policy(spec, stage, denied_roots)
        allowed_read_roots = policy["readonly"]
        allowed_write_roots = policy["writable"]
        policy_path = stage / "runner-input" / "landlock-policy.json"
        policy_path.write_text(json.dumps(policy, sort_keys=True) + "\n", encoding="utf-8")
        policy_sha256 = sha_file(policy_path)
        python = Path(sys.executable).resolve()
        observed = _probe_linux(policy, module, python, suite_path, pack_path)
        probes = observed["probes"]
        backend_proof = {
            "landlock": {
                "abi": observed["abi"],
                "threat_model": "filesystem-path-read-v1",
                "module": {
                    "path": module.relative_to(stage).as_posix(),
                    "sha256": sha_file(module),
                },
                "python": {"path": str(python), "sha256": sha_file(python)},
                "policy": {
                    "path": policy_path.relative_to(stage).as_posix(),
                    "sha256": policy_sha256,
                },
            }
        }
        isolation = {
            "backend": backend,
            "module": str(module),
            "python": str(python),
            "module_sha256": sha_file(module),
            "python_sha256": sha_file(python),
            "policy_path": str(policy_path),
            "suite_path": str(suite_path),
            "pack_path": str(pack_path),
            "suite_sha256": sha_file(suite_path),
            "pack_sha256": sha_file(pack_path),
        }
    proof = {
        "schema_version": ISOLATION_PROOF_VERSION,
        "backend": backend,
        **backend_proof,
        "policy_sha256": policy_sha256,
        "denied_roots": denied_roots,
        "allowed_read_roots": allowed_read_roots,
        "allowed_write_roots": allowed_write_roots,
        "suite": {
            "path": suite_path.relative_to(stage.resolve()).as_posix(),
            "capture_path": str(suite_path),
            "sha256": sha_file(suite_path),
        },
        "query_pack": {
            "path": pack_path.relative_to(stage.resolve()).as_posix(),
            "capture_path": str(pack_path),
            "sha256": sha_file(pack_path),
        },
        "corpus_view": {
            "path": repo.relative_to(stage.resolve()).as_posix(),
            "manifest_sha256": materialized["manifest_sha256"],
            "proof_sha256": materialized["proof_sha256"],
            "file_count": len(materialized["files"]),
        },
        "runner_bundle": {
            **bundle_proof,
            "path": runner_bundle.relative_to(stage.resolve()).as_posix(),
        },
        "platform_helpers": (
            [
                {
                    "path": module.relative_to(stage.resolve()).as_posix(),
                    "sha256": sha_file(module),
                    "source": "linux_isolation.py",
                }
            ]
            if system == "Linux"
            else []
        ),
        "probes": probes,
    }
    proof_path = stage / "isolation-proof.json"
    with proof_path.open("x", encoding="utf-8") as handle:
        handle.write(json.dumps(proof, indent=2, sort_keys=True) + "\n")
    proof_sha256 = sha_file(proof_path)
    updated = dict(spec)
    updated["isolation_method"] = backend
    updated["access_block_log"] = f"sha256:{proof_sha256}"
    updated["_isolation"] = {
        **isolation,
        "policy_sha256": policy_sha256,
        "proof_sha256": proof_sha256,
    }
    updated["_semble_adapter"] = str(runner_bundle)
    return updated


def sandbox_command(spec: dict, command: list[str]) -> tuple[list[str], dict | None]:
    if spec.get("blinding", "attested") != "isolated":
        return command, None
    isolation = spec.get("_isolation")
    if not isinstance(isolation, dict):
        raise RunError("isolated capture lacks the verified driver isolation context")
    backend = isolation.get("backend")
    if backend == MACOS_ISOLATION_BACKEND:
        if set(isolation) != {"backend", "profile", "policy_sha256", "proof_sha256"}:
            raise RunError("isolated macOS context is malformed")
        profile = isolation["profile"]
        if (
            not isinstance(profile, str)
            or hashlib.sha256(profile.encode()).hexdigest() != isolation["policy_sha256"]
        ):
            raise RunError("isolated capture policy digest mismatch")
        wrapped = [str(SANDBOX_EXEC), "-p", profile, *command]
    elif backend == LINUX_ISOLATION_BACKEND:
        if set(isolation) != {
            "backend",
            "module",
            "module_sha256",
            "python",
            "python_sha256",
            "policy_path",
            "suite_path",
            "suite_sha256",
            "pack_path",
            "pack_sha256",
            "policy_sha256",
            "proof_sha256",
        }:
            raise RunError("isolated Linux context is malformed")
        policy_path = Path(isolation["policy_path"])
        if sha_file(policy_path) != isolation["policy_sha256"]:
            raise RunError("isolated Linux policy digest drifted")
        for path_key, digest_key in (
            ("module", "module_sha256"),
            ("python", "python_sha256"),
            ("suite_path", "suite_sha256"),
            ("pack_path", "pack_sha256"),
        ):
            if sha_file(Path(isolation[path_key])) != isolation[digest_key]:
                raise RunError(f"isolated Linux {path_key} digest drifted")
        wrapped = [
            isolation["python"],
            isolation["module"],
            "--policy",
            str(policy_path),
            "--suite",
            isolation["suite_path"],
            "--query-pack",
            isolation["pack_path"],
            "--query-pack-sha256",
            isolation["pack_sha256"],
            "--",
            *command,
        ]
    else:
        raise RunError("unknown isolated capture backend")
    evidence = {
        "backend": backend,
        "policy_sha256": isolation["policy_sha256"],
        "proof_sha256": isolation["proof_sha256"],
    }
    if backend == LINUX_ISOLATION_BACKEND:
        evidence["_child_check"] = {"pack_sha256": isolation["pack_sha256"]}
    return wrapped, evidence


def capture_process_env(temp_root: Path) -> dict[str, str]:
    """Pin all conventional temporary directories inside capture authority."""
    temp_root.mkdir(mode=0o700, parents=True, exist_ok=False)
    value = str(temp_root.resolve())
    return {**os.environ, "TMPDIR": value, "TMP": value, "TEMP": value}


def tree_size(root: Path) -> int:
    """Sum regular-file bytes. Stat failures are typed errors, never skipped."""
    total = 0
    for dirpath, _dirnames, filenames in os.walk(root):
        for name in filenames:
            path = Path(dirpath) / name
            try:
                total += path.stat().st_size
            except OSError as exc:
                raise RunError(f"cannot stat resource-accounting path {path}: {exc}") from exc
    return total


def bind_storage_metrics(resource_path: Path, storage: dict) -> dict:
    """Atomically bind post-process storage/cache accounting to resource evidence."""
    expected = {
        "index_bytes",
        "model_cache_bytes",
        "parser_cache_bytes",
        "embedding_cache_bytes",
        "discovered_files",
        "indexed_chunks",
        "index_storage",
        "index_measurement",
    }
    if set(storage) != expected:
        raise RunError("storage metrics hold missing or unknown keys")
    payload = read_json(resource_path)
    if not isinstance(payload, dict) or "storage" in payload:
        raise RunError("resource evidence is not an unbound object")
    payload["storage"] = storage
    _validate_resource_metrics(payload, f"resource metrics {resource_path}")
    temporary = resource_path.with_suffix(resource_path.suffix + ".tmp")
    temporary.write_text(json.dumps(payload, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    os.replace(temporary, resource_path)
    return payload


def host_probe() -> dict:
    """Host check-record: identity, concurrent load, thermal/frequency state.

    Records observations only; PERF_QUALIFIED gating reads this artifact.
    Anything unmeasurable is explicit `unavailable`, never inferred.
    """
    record: dict = {
        "system": platform.system(),
        "release": platform.release(),
        "machine": platform.machine(),
        "processor": platform.processor() or "unavailable",
        "cpu_count": os.cpu_count(),
        "python": platform.python_version(),
    }
    try:
        completed = subprocess.run(
            ["rustc", "--version"], capture_output=True, text=True, timeout=30
        )
        record["rustc"] = completed.stdout.strip() if completed.returncode == 0 else "unavailable"
    except (OSError, subprocess.SubprocessError):
        record["rustc"] = "unavailable"
    record["concurrent_processes"] = find_competing_processes()
    record["thermal"] = read_thermal()
    power = read_power()
    record["power"] = power
    record["frequency"] = read_frequency(power)
    return record


def _host_dynamic_probe(
    identity: dict, override: bool, owned_semble_adapter: Path | None = None
) -> dict:
    """Refresh mutable state without repeatedly spawning rustc during timing."""
    power = read_power()
    return {
        **{
            key: identity[key]
            for key in ("system", "release", "machine", "processor", "cpu_count", "python", "rustc")
        },
        "concurrent_processes": find_competing_processes(owned_semble_adapter=owned_semble_adapter),
        "thermal": read_thermal(),
        "power": power,
        "frequency": read_frequency(power),
        "contention_override": override,
    }


class HostTimeline:
    """Qualified probe adapter over the existing host reservation and scheduler.

    Thermal/power/frequency probes extend cooperative host facts; the existing
    HostMonitor owns polling, resource bounds, reservation and worker teardown.
    This remains sampled evidence, not continuous OS attestation.
    """

    def __init__(
        self,
        path: Path,
        identity: dict,
        override: bool,
        *,
        owned_semble_adapter: Path | None = None,
    ):
        self.path, self.identity, self.override = path, identity, override
        self.owned_semble_adapter = owned_semble_adapter
        self.samples: list[dict] = []
        self.errors: list[str] = []
        self.sample_bytes = 0
        self.monitor = host_monitor.HostMonitor(
            path.with_suffix(".jsonl"),
            "retrieval-host",
            "qualified-speed",
            sample_observer=self._sample,
        )

    def _sample(self) -> None:
        started = time.monotonic_ns()
        # HostMonitor captures and propagates this exception; no clean default.
        probe = (
            _host_dynamic_probe(
                self.identity, self.override, owned_semble_adapter=self.owned_semble_adapter
            )
            if self.owned_semble_adapter is not None
            else _host_dynamic_probe(self.identity, self.override)
        )
        sample = {"started_ns": started, "finished_ns": time.monotonic_ns(), "probe": probe}
        # Include indentation inside the outer array and reserve the control
        # envelope; long runs refuse before accumulating an unreadable artifact.
        encoded = json.dumps(sample, indent=2, sort_keys=True).encode("utf-8")
        cost = len(encoded) + 4 * (encoded.count(b"\n") + 1) + 2
        if self.sample_bytes + cost > CONTROL_DOCUMENT_BYTES - 4096:
            raise RunError("host timeline exceeds the control-document byte budget")
        self.sample_bytes += cost
        self.samples.append(sample)

    def __enter__(self):
        self.started_ns = time.monotonic_ns()
        self.monitor.start()
        return self

    def __exit__(self, exc_type, exc, traceback):
        try:
            self.monitor.finish(failed=exc_type is not None)
        except Exception as error:
            self.errors.append(f"{type(error).__name__}: {error}")
        # A stuck worker owns mutable state and its reservation. Retain failed
        # evidence without racing that worker or declaring the epoch complete.
        live = self.monitor.thread is not None and self.monitor.thread.is_alive()
        payload = {
            "schema_version": 1,
            "interval_ns": HOST_SAMPLE_INTERVAL_NS,
            "started_ns": self.started_ns,
            "finished_ns": time.monotonic_ns(),
            "samples": [] if live else self.samples,
            "errors": self.errors,
            "reservation_id": self.monitor.reservation_id,
            "monitor_sha256": sha_file(self.path.with_suffix(".jsonl")),
        }
        self.path.write_text(json.dumps(payload, indent=2, sort_keys=True) + "\n", encoding="utf-8")


def validate_host_timeline(payload: object, profile: dict) -> None:
    """Re-derive interval coverage and host validity from every captured sample."""
    timeline = _exact_keys(
        payload,
        {
            "schema_version",
            "interval_ns",
            "started_ns",
            "finished_ns",
            "samples",
            "errors",
            "reservation_id",
            "monitor_sha256",
        },
        "host timeline",
    )
    if type(timeline["schema_version"]) is not int or timeline["schema_version"] != 1:
        raise RunError("host timeline schema is unsupported")
    if not _is_hex(timeline["reservation_id"], 32) or not _is_hex(timeline["monitor_sha256"], 64):
        raise RunError("host timeline monitor binding is malformed")
    if (
        type(timeline["interval_ns"]) is not int
        or timeline["interval_ns"] != HOST_SAMPLE_INTERVAL_NS
    ):
        raise RunError("host timeline interval differs from the capture contract")
    for key in ("started_ns", "finished_ns"):
        if type(timeline[key]) is not int or timeline[key] < 0:
            raise RunError(f"host timeline {key} is invalid")
    started, finished = timeline["started_ns"], timeline["finished_ns"]
    if finished <= started or timeline["errors"] != []:
        raise RunError("host timeline has invalid coverage or probe errors")
    samples = timeline["samples"]
    if not isinstance(samples, list) or not 2 <= len(samples) <= host_monitor.MAX_SAMPLES:
        raise RunError("host timeline lacks boundary observations")
    previous_start, previous_end = started, started
    for index, item in enumerate(samples):
        sample = _exact_keys(item, {"started_ns", "finished_ns", "probe"}, "host timeline sample")
        begin, end = sample["started_ns"], sample["finished_ns"]
        if (
            type(begin) is not int
            or type(end) is not int
            or not previous_end <= begin < end <= finished
            or begin - previous_start > HOST_SAMPLE_MAX_GAP_NS
            or end - begin > HOST_SAMPLE_MAX_GAP_NS
        ):
            raise RunError(f"host timeline coverage gap or invalid sample {index}")
        if not _probe_clean(sample["probe"], profile):
            raise RunError(f"host timeline sample {index} is unclean or has host identity drift")
        previous_start, previous_end = begin, end
    if finished - previous_start > HOST_SAMPLE_MAX_GAP_NS:
        raise RunError("host timeline trailing coverage gap")


def validate_host_timeline_monitor(payload: dict, path: Path) -> None:
    """Bind qualified samples to the canonical reservation transcript on replay."""
    raw = host_monitor.RawFile.capture(path)
    if raw.sha256 != "sha256:" + payload["monitor_sha256"]:
        raise RunError("host timeline monitor digest mismatch")
    host_monitor.validate(raw, capture_id="retrieval-host", profile="qualified-speed")

    def consume(lines):
        header = json.loads(next(lines))
        probe = payload["samples"][0]["probe"]
        if (
            header["reservation_id"] != payload["reservation_id"]
            or header["host"]["os"]
            != ("macos" if probe["system"] == "Darwin" else probe["system"].lower())
            or header["host"]["arch"] != probe["machine"]
            or header["host"]["cpu_count"] != probe["cpu_count"]
        ):
            raise RunError("host timeline monitor host or reservation differs")
        count = 0
        for line in lines:
            observation = json.loads(line)
            if count >= len(payload["samples"]):
                raise RunError("host timeline monitor sample count differs")
            sample = payload["samples"][count]
            count += 1
            if observation["facts"]["foreign_rust"]:
                raise RunError("host timeline monitor contains foreign Rust contention")
            if (
                not sample["finished_ns"] <= observation["monotonic_ns"] <= payload["finished_ns"]
                or observation["monotonic_ns"] - sample["started_ns"] > HOST_SAMPLE_MAX_GAP_NS
            ):
                raise RunError("host timeline monitor interval differs from qualified samples")
        if count != len(payload["samples"]):
            raise RunError("host timeline monitor sample count differs")

    raw.consume_lines(consume)


def _darwin_power_source(text: str) -> str | None:
    match = re.search(r"^Now drawing from '([^']+)'\s*$", text, re.MULTILINE)
    if match is None:
        return None
    source = match.group(1)
    return source if source in {"AC Power", "Battery Power", "UPS Power"} else None


def _darwin_power_settings(text: str, active_source: str) -> dict[str, str] | None:
    sections: dict[str, dict[str, str]] = {}
    current: str | None = None
    for raw in text.splitlines():
        line = raw.strip()
        if line.endswith(":") and line[:-1] in {"AC Power", "Battery Power", "UPS Power"}:
            current = line[:-1]
            sections[current] = {}
            continue
        if current is None or not line:
            continue
        parts = line.split(maxsplit=1)
        if len(parts) != 2:
            return None
        key, value = parts
        if key in sections[current]:
            return None
        sections[current][key] = value.strip()
    selected = sections.get(active_source)
    return selected if selected else None


def read_power() -> dict:
    if sys.platform == "darwin":
        try:
            settings = subprocess.run(
                ["pmset", "-g", "custom"], capture_output=True, text=True, timeout=15
            )
            source = subprocess.run(
                ["pmset", "-g", "ps"], capture_output=True, text=True, timeout=15
            )
        except (OSError, subprocess.SubprocessError):
            return {"status": "unavailable", "digest": None}
        settings_text = settings.stdout.strip()
        source_text = source.stdout.strip()
        if (
            settings.returncode != 0
            or source.returncode != 0
            or not settings_text
            or not source_text
        ):
            return {"status": "unavailable", "digest": None}
        active_source = _darwin_power_source(source_text)
        active_settings = (
            _darwin_power_settings(settings_text, active_source)
            if active_source is not None
            else None
        )
        if active_source is None or active_settings is None:
            return {"status": "unavailable", "digest": None}
        return {
            "status": "bounded",
            "digest": digest(
                canonical(
                    {
                        "active_source": active_source,
                        "settings": active_settings,
                    }
                )
            ),
            "active_source": active_source,
            "observation_digest": digest(source_text.encode("utf-8")),
        }
    if sys.platform.startswith("linux"):
        governors = {}
        minimums = {}
        maximums = {}
        drivers = {}
        for node in sorted(
            Path("/sys/devices/system/cpu").glob("cpu[0-9]*/cpufreq/scaling_governor")
        ):
            try:
                cpu = node.parent.parent.name
                governors[cpu] = node.read_text().strip()
                minimums[cpu] = int((node.parent / "scaling_min_freq").read_text().strip())
                maximums[cpu] = int((node.parent / "scaling_max_freq").read_text().strip())
                drivers[cpu] = (node.parent / "scaling_driver").read_text().strip()
            except (OSError, ValueError):
                return {"status": "unavailable", "digest": None}
        boost = {}
        for path in (
            "/sys/devices/system/cpu/intel_pstate/no_turbo",
            "/sys/devices/system/cpu/cpufreq/boost",
        ):
            node = Path(path)
            if node.exists():
                try:
                    value = node.read_text().strip()
                except OSError:
                    return {"status": "unavailable", "digest": None}
                if value not in {"0", "1"}:
                    return {"status": "unavailable", "digest": None}
                boost[path] = value
        settings = {
            "governors": governors,
            "minimum_khz": minimums,
            "maximum_khz": maximums,
            "drivers": drivers,
            "boost": boost,
        }
        complete = (
            os.cpu_count() is not None
            and len(governors) == os.cpu_count()
            and all(value == "performance" for value in governors.values())
            and all(drivers.values())
            and all(0 < minimums[cpu] <= maximums[cpu] for cpu in governors)
            and bool(boost)
            and all(
                (path.endswith("/no_turbo") and value == "1")
                or (path.endswith("/boost") and value == "0")
                for path, value in boost.items()
            )
        )
        return {
            "status": "bounded" if complete else "unavailable",
            "digest": digest(canonical(settings)) if complete else None,
            "governors": governors,
            "settings": settings,
        }
    return {"status": "unavailable", "digest": None}


def _ps_process_snapshot() -> dict[int, tuple[int, str, str]] | None:
    """Read one PID/parent/start/argv view for an owned adapter exception."""
    try:
        completed = subprocess.run(
            ["ps", "-ww", "-axo", "pid=,ppid=,lstart=,command="],
            capture_output=True,
            text=True,
            timeout=15,
        )
    except (OSError, subprocess.SubprocessError):
        return None
    if completed.returncode != 0 or len(completed.stdout) > 16 * 1024 * 1024:
        return None
    processes: dict[int, tuple[int, str, str]] = {}
    for line in completed.stdout.splitlines():
        fields = line.split(maxsplit=7)
        if len(fields) != 8:
            return None
        try:
            pid, ppid = int(fields[0]), int(fields[1])
        except ValueError:
            return None
        if pid < 1 or ppid < 0 or pid in processes:
            return None
        # lstart is five fields. Retain the process identity observed with
        # its parent and command in this single ps snapshot.
        started = " ".join(fields[2:7])
        argv = fields[7]
        if not started or not argv:
            return None
        processes[pid] = (ppid, started, argv)
    return processes


def _is_owned_semble_process(
    pid: int, processes: dict[int, tuple[int, str, str]], adapter: Path
) -> bool:
    """Only exempt this driver's frozen Semble adapter run, never its tools."""
    row = processes.get(pid)
    if row is None:
        return False
    argv = row[2]
    path = re.escape(str(adapter))
    if not (
        re.search(rf"(?<!\S){path}(?=\s|$)", argv)
        and re.search(r"(?<!\S)run(?=\s|$)", argv)
        and "--query-pack" in argv.split()
        and "--output-root" in argv.split()
    ):
        return False
    seen = {pid}
    parent = row[0]
    while parent != os.getpid():
        if parent < 1 or parent in seen or parent not in processes:
            return False
        seen.add(parent)
        parent = processes[parent][0]
    return True


def find_competing_processes(*, owned_semble_adapter: Path | None = None) -> dict:
    """Look for competing builds; exempt only an attested owned Semble run."""
    patterns = ["cargo", "rustc", "semble", "pytest", "run_benchmark", "speed_benchmark"]
    found: dict[str, list[int]] = {}
    if shutil.which("pgrep") is None:
        return {"pgrep": "unavailable"}
    own = os.getpid()
    process_snapshot = None
    for pattern in patterns:
        try:
            completed = subprocess.run(
                ["pgrep", "-f", pattern], capture_output=True, text=True, timeout=15
            )
        except (OSError, subprocess.SubprocessError):
            return {"pgrep": "unavailable"}
        if completed.returncode not in (0, 1):
            return {"pgrep": "unavailable"}
        if (completed.returncode == 0) != bool(completed.stdout.strip()):
            return {"pgrep": "unavailable"}
        pids = []
        for line in completed.stdout.splitlines():
            try:
                pid = int(line.strip())
            except ValueError:
                return {"pgrep": "unavailable"}
            if pid != own:
                if pattern == "semble" and owned_semble_adapter is not None:
                    if process_snapshot is None:
                        process_snapshot = _ps_process_snapshot()
                    if process_snapshot is None or pid not in process_snapshot:
                        return {"ps": "unavailable"}
                    if "semble" not in process_snapshot[pid][2]:
                        return {"ps": "unavailable"}
                    if _is_owned_semble_process(pid, process_snapshot, owned_semble_adapter):
                        continue
                pids.append(pid)
        if pattern == "semble" and owned_semble_adapter is not None and completed.stdout.strip():
            try:
                repeated = subprocess.run(
                    ["pgrep", "-f", pattern], capture_output=True, text=True, timeout=15
                )
            except (OSError, subprocess.SubprocessError):
                return {"pgrep": "unavailable"}
            if repeated.returncode != completed.returncode or repeated.stdout != completed.stdout:
                return {"pgrep": "unavailable"}
        if pids:
            found[pattern] = sorted(pids)
    return found if found else {"none": []}


def read_thermal() -> dict:
    if sys.platform == "darwin":
        try:
            completed = subprocess.run(
                ["pmset", "-g", "therm"], capture_output=True, text=True, timeout=15
            )
        except (OSError, subprocess.SubprocessError):
            return {"status": "unavailable", "evidence": {}}
        text = completed.stdout.strip()
        if completed.returncode != 0 or not text:
            return {"status": "unavailable", "evidence": text}
        lowered = text.lower()
        no_pressure = all(
            phrase in lowered
            for phrase in (
                "no thermal warning",
                "no performance warning",
                "no cpu power status",
            )
        )
        limits = {
            key: int(value)
            for key, value in re.findall(
                r"^(CPU_Scheduler_Limit|CPU_Available_CPUs|CPU_Speed_Limit)\s*=\s*(\d+)\s*$",
                text,
                re.MULTILINE,
            )
        }
        expected_cpus = os.cpu_count()
        full_limits = set(limits) == {
            "CPU_Scheduler_Limit",
            "CPU_Available_CPUs",
            "CPU_Speed_Limit",
        }
        limits_clean = (
            full_limits
            and limits["CPU_Scheduler_Limit"] == 100
            and limits["CPU_Speed_Limit"] == 100
            and expected_cpus is not None
            and limits["CPU_Available_CPUs"] == expected_cpus
        )
        return {
            "status": "clean" if no_pressure or limits_clean else "warning",
            "evidence": text,
        }
    if sys.platform.startswith("linux"):
        out: dict = {}
        for zone in sorted(Path("/sys/class/thermal").glob("thermal_zone*/temp")):
            try:
                temperature = int(zone.read_text().strip())
                sensor_type = (zone.parent / "type").read_text().strip()
            except (OSError, ValueError):
                return {"status": "unavailable", "evidence": out}
            if not sensor_type or not 0 <= temperature <= 150_000:
                return {"status": "unavailable", "evidence": out}
            out[zone.parent.name] = {
                "type": sensor_type,
                "temp_millidegrees": temperature,
            }
        return {"status": "observed" if out else "unavailable", "evidence": out}
    return {"status": "unavailable", "evidence": {}}


def read_frequency(power: dict | None = None) -> dict:
    if sys.platform == "darwin":
        evidence = read_sysctl(["hw.cpufrequency", "hw.cpufrequency_max"])
        try:
            current = int(evidence["hw.cpufrequency"])
            maximum = int(evidence["hw.cpufrequency_max"])
        except (KeyError, TypeError, ValueError):
            return {"status": "unavailable", "evidence": evidence}
        return {
            "status": "bounded" if 0 < current <= maximum else "warning",
            "evidence": evidence,
        }
    if sys.platform.startswith("linux"):
        out: dict = {}
        for node in sorted(
            Path("/sys/devices/system/cpu").glob("cpu[0-9]*/cpufreq/scaling_cur_freq")
        ):
            try:
                current = int(node.read_text().strip())
                maximum = int((node.parent / "cpuinfo_max_freq").read_text().strip())
            except (OSError, ValueError):
                return {"status": "unavailable", "evidence": out}
            if not 0 < current <= maximum:
                return {"status": "unavailable", "evidence": out}
            out[node.parent.parent.name] = {"current_khz": current, "maximum_khz": maximum}
        complete = os.cpu_count() is not None and len(out) == os.cpu_count()
        return {"status": "observed" if complete else "unavailable", "evidence": out}
    return {"status": "unavailable", "evidence": {}}


def read_sysctl(keys: list[str]) -> dict:
    out: dict = {}
    for key in keys:
        try:
            completed = subprocess.run(
                ["sysctl", "-n", key], capture_output=True, text=True, timeout=10
            )
        except (OSError, subprocess.SubprocessError):
            out[key] = "unavailable"
            continue
        out[key] = (
            completed.stdout.strip()
            if completed.returncode == 0 and completed.stdout.strip()
            else "unavailable"
        )
    return out


def project_pack_and_suite(pack: dict, suite: dict, routes: list[str]) -> tuple[dict, dict]:
    """Project a frozen pack+suite to one system's route set.

    Tasks, universe, commit and tokenizer stay identical; the route list
    narrows and the suite commitment rebinds to the projected suite so the
    evaluator's pack/suite binding holds per record. Deterministic.
    """
    narrowed = sorted(routes)
    projected_suite = dict(suite)
    projected_suite["routes"] = narrowed
    projected_pack = dict(pack)
    projected_pack["routes"] = narrowed
    projected_pack["suite_commitment_sha256"] = digest(canonical(projected_suite))
    return projected_pack, projected_suite


def canonical_bytes(payload: object) -> bytes:
    return json.dumps(payload, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode(
        "utf-8"
    )


def merge_records(
    repo: Path, suite_path: Path, record_paths: list[Path]
) -> tuple[dict, dict, dict]:
    """Validate each v3 record via the evaluator and merge disjoint routes.

    Each system consumes a projected pack (same tasks/universe/commit/
    contract, narrowed routes), so pack digests legitimately differ per
    record: the merge re-derives each projected pack from the frozen
    combined pack and requires an exact digest match, then validates the
    record against a projected suite with the evaluator itself. All
    records must carry byte-equal comparison contracts. Captures are
    preserved (capture_ids must be disjoint) and route entries keep their
    capture_id references. Returns (suite, pack, combined_record).
    Results concatenate in (task_id, route) order; the merged record is
    identical regardless of input record order.
    """
    if not record_paths:
        raise RunError("merge needs at least one record")
    suite_payload = read_json(suite_path)
    if not isinstance(suite_payload, dict):
        raise RunError("suite must be an object")
    if suite_payload.get("schema_version") != 3:
        raise RunError("merge requires a v3 suite")
    suite, pack, source = validate_suite(repo, suite_payload)
    return _merge_validated_records(repo, suite, pack, source, record_paths)


def _merge_validated_records(
    repo: Path,
    suite: dict,
    pack: dict,
    source: SourceSnapshot,
    record_paths: list[Path],
) -> tuple[dict, dict, dict]:
    """Merge records within one independently validated suite pass."""
    if not record_paths:
        raise RunError("merge needs at least one record")
    provenance: dict = {}
    captures: dict = {}
    capture_sources: dict = {}
    results: dict[tuple[str, str], dict] = {}
    runners: list[dict] = []
    contracts: list[dict] = []
    validated_runs: list[dict] = []
    for path in record_paths:
        run = _validate_single_record(repo, suite, pack, source, path)
        validated_runs.append(run)
        contracts.append(run["comparison_contract"])
        for capture_id, entry in run["captures"].items():
            if capture_id in captures:
                raise RunError(
                    f"capture_id {capture_id} is recorded twice "
                    f"({capture_sources[capture_id]} and {path}); refusing merge"
                )
            captures[capture_id] = entry
            capture_sources[capture_id] = str(path)
        for route, entry in run["route_provenance"].items():
            if route in provenance:
                raise RunError(f"route {route} is recorded twice; refusing merge")
            provenance[route] = entry
        for row in run["results"]:
            key = (row["task_id"], row["route"])
            if key in results:
                raise RunError(f"duplicate merged result: {key}")
            results[key] = row
        runners.append(
            {
                "record_sha256": digest(canonical_bytes(run)),
                "capture_ids": sorted(run["captures"]),
                "runner": run["runner"],
            }
        )
    first, *rest = contracts
    for other in rest:
        if other != first:
            differing = sorted(k for k in first if first[k] != other.get(k))
            raise RunError(
                f"merged records disagree on the comparison contract: {differing}; refusing merge"
            )
    versions = {run.get("schema_version") for run in validated_runs}
    if versions != {RUNNER_SCHEMA_VERSION}:
        raise RunError(
            "current merge requires only runner v5 records; historical v3/v4 records are inspection-only"
        )
    merged_blinding = (
        "isolated"
        if all(r["runner"].get("blinding") == "isolated" for r in runners)
        else "attested"
    )
    expected_routes = set(suite["routes"])
    if set(provenance) != expected_routes:
        missing = sorted(expected_routes - set(provenance))
        extra = sorted(set(provenance) - expected_routes)
        raise RunError(
            f"merged routes {sorted(provenance)} != suite routes (missing={missing} extra={extra})"
        )
    ordered = [results[key] for key in sorted(results)]
    runners.sort(key=lambda entry: (entry["record_sha256"], entry["capture_ids"]))
    content_digests = sorted(digest(canonical_bytes(read_json(path))) for path in record_paths)
    merge_id = digest(canonical_bytes(content_digests))[:16]
    combined = {
        "schema_version": RUNNER_SCHEMA_VERSION,
        "query_pack_sha256": digest(canonical_bytes(pack)),
        "comparison_contract": first,
        "runner": {
            "name": "retrieval-pair-merge",
            "revision": "merge-v2",
            "run_id": f"merge:{merge_id}",
            "tokenizer": "qi-regex-v1",
            "tokenizer_budget_version": TOKENIZER_BUDGET_VERSION,
            "gold_access": False,
            "blinding": merged_blinding,
            "isolation_method": "merge of independently blinded records (weakest blinding wins)",
            "access_block_log": json.dumps(runners, sort_keys=True),
        },
        "captures": captures,
        "route_provenance": provenance,
        "results": ordered,
    }
    if any(run.get("span_accounting_version") == 1 for run in validated_runs):
        combined["span_accounting_version"] = 1
    # Validate the merged rows and route coverage against the same source
    # snapshot. The verdict builds a fresh snapshot before replaying this.
    checked = validate_evidence_against_suite(repo, suite, pack, source, combined)
    if len(checked["results"]) != len(ordered):
        raise RunError("merged record failed evaluator re-validation")
    return suite, pack, combined


def percentile(sorted_samples: list[float], pct: float) -> float:
    if not sorted_samples:
        raise RunError("no latency samples for percentile")
    if not 0 <= pct <= 100:
        raise RunError("percentile out of range")
    rank = (len(sorted_samples) - 1) * pct / 100
    low = math.floor(rank)
    high = math.ceil(rank)
    if low == high:
        return sorted_samples[low]
    return sorted_samples[low] + (sorted_samples[high] - sorted_samples[low]) * (rank - low)


def latency_summary(samples: list[float]) -> dict:
    ordered = sorted(samples)
    return {
        "count": len(ordered),
        "p50_ms": percentile(ordered, 50),
        "p95_ms": percentile(ordered, 95),
        "p99_ms": percentile(ordered, 99),
        "mean_ms": sum(ordered) / len(ordered),
        "min_ms": ordered[0],
        "max_ms": ordered[-1],
    }


def cmd_merge(args: argparse.Namespace) -> int:
    try:
        _, _, combined = merge_records(
            Path(args.repo), Path(args.suite), [Path(p) for p in args.records]
        )
    except (RunError, ValueError) as exc:
        print(f"ERROR: {exc}", file=sys.stderr)
        return 2
    Path(args.out).write_text(
        json.dumps(combined, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    print(f"merged {len(args.records)} records into {args.out}")
    return 0


def cmd_host_probe(args: argparse.Namespace) -> int:
    record = host_probe()
    rendered = json.dumps(record, indent=2, sort_keys=True) + "\n"
    if args.out:
        Path(args.out).write_text(rendered, encoding="utf-8")
    else:
        sys.stdout.write(rendered)
    return 0


def _host_fingerprint(probe: dict) -> dict:
    power = probe.get("power")
    return {
        "system": probe.get("system"),
        "release": probe.get("release"),
        "machine": probe.get("machine"),
        "processor": probe.get("processor"),
        "cpu_count": probe.get("cpu_count"),
        "rustc": probe.get("rustc"),
        "power_digest": power.get("digest") if isinstance(power, dict) else None,
    }


def validate_host_profile(payload: object) -> dict:
    if not isinstance(payload, dict):
        raise RunError("host profile must be an object")
    fingerprint_input = payload.get("fingerprint")
    linux = isinstance(fingerprint_input, dict) and fingerprint_input.get("system") == "Linux"
    keys = {"schema_version", "profile_id", "fingerprint"}
    if linux:
        keys.add("linux_limits")
    profile = _exact_keys(payload, keys, "host profile")
    if profile["schema_version"] != 2:
        raise RunError("host profile schema version mismatch")
    if not isinstance(profile["profile_id"], str) or not profile["profile_id"]:
        raise RunError("host profile id must be nonempty")
    fingerprint = _exact_keys(
        profile["fingerprint"],
        {"system", "release", "machine", "processor", "cpu_count", "rustc", "power_digest"},
        "host profile fingerprint",
    )
    for key in ("system", "release", "machine", "processor", "rustc"):
        if not isinstance(fingerprint[key], str) or not fingerprint[key]:
            raise RunError(f"host profile fingerprint.{key} must be nonempty")
    if type(fingerprint["cpu_count"]) is not int or fingerprint["cpu_count"] < 1:
        raise RunError("host profile fingerprint.cpu_count must be positive")
    if not _is_hex(fingerprint["power_digest"], 64):
        raise RunError("host profile requires a measured power configuration digest")
    if linux:
        limits = _exact_keys(
            profile["linux_limits"],
            {"max_thermal_millidegrees", "min_frequency_percent", "thermal_zones", "cpu_max_khz"},
            "host profile linux_limits",
        )
        if (
            type(limits["max_thermal_millidegrees"]) is not int
            or not 1 <= limits["max_thermal_millidegrees"] <= 85_000
        ):
            raise RunError("Linux thermal limit must be at most 85000 millidegrees")
        if (
            type(limits["min_frequency_percent"]) is not int
            or not 80 <= limits["min_frequency_percent"] <= 100
        ):
            raise RunError("Linux frequency floor must be 80..100 percent")
        zones = limits["thermal_zones"]
        if (
            not isinstance(zones, dict)
            or not zones
            or any(
                not isinstance(name, str)
                or not re.fullmatch(r"thermal_zone[0-9]+", name)
                or not isinstance(sensor_type, str)
                or not sensor_type
                for name, sensor_type in zones.items()
            )
        ):
            raise RunError("Linux thermal zones must name measured sensor types")
        maximums = limits["cpu_max_khz"]
        if (
            not isinstance(maximums, dict)
            or len(maximums) != fingerprint["cpu_count"]
            or any(
                not isinstance(name, str)
                or not re.fullmatch(r"cpu[0-9]+", name)
                or type(value) is not int
                or value <= 0
                for name, value in maximums.items()
            )
        ):
            raise RunError("Linux maximum frequencies must cover every CPU")
    return profile


def cmd_host_profile(args: argparse.Namespace) -> int:
    probe = host_probe()
    linux = probe["system"] == "Linux"
    zones = args.linux_thermal_zone or []
    limits = None
    if linux:
        if (
            args.linux_max_thermal_millidegrees is None
            or args.linux_min_frequency_percent is None
            or not zones
        ):
            raise RunError(
                "Linux host profile requires thermal zones, temperature ceiling, and frequency floor"
            )
        thermal = probe["thermal"]
        frequency = probe["frequency"]
        if thermal.get("status") != "observed" or frequency.get("status") != "observed":
            raise RunError("Linux thermal/frequency telemetry is unavailable")
        observed_zones = thermal["evidence"]
        if len(set(zones)) != len(zones) or any(zone not in observed_zones for zone in zones):
            raise RunError("Linux thermal zones must exist uniquely in the host probe")
        limits = {
            "max_thermal_millidegrees": args.linux_max_thermal_millidegrees,
            "min_frequency_percent": args.linux_min_frequency_percent,
            "thermal_zones": {zone: observed_zones[zone]["type"] for zone in zones},
            "cpu_max_khz": {
                cpu: entry["maximum_khz"] for cpu, entry in frequency["evidence"].items()
            },
        }
    elif (
        zones
        or args.linux_max_thermal_millidegrees is not None
        or args.linux_min_frequency_percent is not None
    ):
        raise RunError("Linux host profile limits cannot be set on another OS")
    payload = {
        "schema_version": 2,
        "profile_id": args.profile_id,
        "fingerprint": _host_fingerprint(probe),
    }
    if linux:
        payload["linux_limits"] = limits
    profile = validate_host_profile(payload)
    Path(args.out).write_text(
        json.dumps(profile, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    return 0


SPEC_REQUIRED = (
    "spec_version",
    "repo",
    "manifest",
    "suite",
    "query_pack",
    "execution_profiles",
    "top_k",
    "output_root",
    "runner_binary",
    "strategies",
    "searchd_binary",
    "searchd_expected_sha256",
)
SPEC_OPTIONAL = (
    "source_closure_reuse",
    "symbol_coverage_policy",
    "symbol_total_timeout_ms",
    "routes",
    "blinding",
    "suite_secret_root",
    "isolation_method",
    "access_block_log",
    "scope",
    "run_id",
    "runner_name",
    "repo_id",
    "revision_id",
    "generation",
    "embedder",
    "cache_regime",
    "seed",
    "timeout_secs",
    "io_timeout_secs",
    "repetitions",
    "alternate_order",
    "order",
    "semble_route",
    "semble_python",
    "semble_lockfile",
    "semble_lockfile_sha256",
    "semble_cache_root",
    "semble_model_revision",
    "quanta_model_dir",
    "query_repetitions_per_root",
    "query_warmup_passes",
    "query_stage_observation",
    "code_search_rank_study",
    "experimental_hybrid_fetch_floor",
    "baseline_route",
    "candidate_route",
    "host_profile",
    "linux_cgroup_parent",
    "admission",
    "claims",
    "receipts",
    "contention_override",
)
RECEIPT_KEYS = (
    "contract_execution_context",
    "contract_execution_logs",
    "contract_source_closure",
    "contract_python_receipt",
    "contract_python_results",
    "contract_python_raw",
    "contract_python_inventory",
    "contract_rust_receipt",
    "contract_rust_results",
    "contract_rust_raw",
    "contract_rust_inventory",
    "sdk_execution_context",
    "sdk_execution_logs",
    "sdk_source_closure",
    "sdk_receipt",
    "sdk_results",
    "sdk_nextest_raw",
    "sdk_record_raw",
    "sdk_inventory",
    "model_parity_results",
    "incremental_results",
)
ADMISSION_COMMON_KEYS = (
    "manifest",
    "license_receipt",
    "annotation_receipts",
    "adjudication_receipt",
)
ADMISSION_LOCAL_KEYS = ADMISSION_COMMON_KEYS + ("experiment_custody", "development_suite")
ADMISSION_DISJOINT_KEYS = ADMISSION_COMMON_KEYS + ("split_manifest", "split_releases")


def _admission_keys(value: object) -> tuple[str, ...]:
    if not isinstance(value, dict):
        raise RunError("qualification admission path inventory is malformed")
    if set(value) == set(ADMISSION_LOCAL_KEYS):
        return ADMISSION_LOCAL_KEYS
    if set(value) == set(ADMISSION_DISJOINT_KEYS):
        return ADMISSION_DISJOINT_KEYS
    raise RunError("qualification admission path inventory is incomplete or mixed")


CONTRACT_EVIDENCE_KEYS = (
    "contract_execution_context",
    "contract_execution_logs",
    "contract_source_closure",
    "contract_python_receipt",
    "contract_python_results",
    "contract_python_raw",
    "contract_python_inventory",
    "contract_rust_receipt",
    "contract_rust_results",
    "contract_rust_raw",
    "contract_rust_inventory",
)
SDK_EVIDENCE_KEYS = (
    "sdk_execution_context",
    "sdk_execution_logs",
    "sdk_source_closure",
    "sdk_receipt",
    "sdk_results",
    "sdk_nextest_raw",
    "sdk_record_raw",
    "sdk_inventory",
)
CONTEXT_COMMAND_NAMES = {
    "contract": (
        "source-closure",
        "python-collection",
        "rust-collection",
        "rust-build",
        "metadata",
        "python-test",
        "rust-test",
    ),
    "sdk": (
        "source-closure",
        "build-searchd",
        "rust-collection",
        "rust-build",
        "metadata",
        "rust-test",
    ),
}
MAX_CONTEXT_LOG_BYTES = 64 * 1024 * 1024
# Payload admission is unchanged. ZIP envelope and central metadata have
# separate finite allowances, checked before the shared parser allocates them.
CONTEXT_ZIP_OVERHEAD_BYTES = 64 * 1024
CONTEXT_ZIP_DIRECTORY_BYTES = 32 * 1024


def _is_hex(value: object, length: int) -> bool:
    return (
        isinstance(value, str)
        and len(value) == length
        and all(c in "0123456789abcdef" for c in value)
    )


def validate_admission_manifest(payload: object) -> dict:
    """Validate the closed W0-B qualification authority packet."""
    version = payload.get("schema_version") if isinstance(payload, dict) else None
    if type(version) is not int or version not in (2, 3):
        raise RunError("qualification admission schema version mismatch")
    policy_keys = (
        {"decision_policy_sha256"}
        if isinstance(payload, dict) and "decision_policy_sha256" in payload
        else set()
    )
    if version == 3 and not policy_keys:
        raise RunError("repository-disjoint admission requires a frozen decision policy")
    custody_keys = (
        {"development_suite_sha256", "experiment_custody_sha256"}
        if version == 2
        else {"repository_disjoint"}
    )
    admission = _exact_keys(
        payload,
        {
            "schema_version",
            "admission_id",
            "issued_at",
            "source_revision",
            "repository_commit",
            "corpus_manifest_sha256",
            "suite_sha256",
            "query_pack_sha256",
            "license",
            "gold",
            "models",
            "semble_lockfile_sha256",
            "host_profile_sha256",
            "cache_regime",
            "verification",
        }
        | policy_keys
        | custody_keys,
        "qualification admission",
    )
    for key in ("admission_id", "issued_at"):
        if not isinstance(admission[key], str) or not admission[key]:
            raise RunError(f"qualification admission {key} must be nonempty")
    for key in ("source_revision", "repository_commit"):
        if not _is_hex(admission[key], 40):
            raise RunError(f"qualification admission {key} must be a full Git SHA")
    for key in (
        "corpus_manifest_sha256",
        "suite_sha256",
        "query_pack_sha256",
        "semble_lockfile_sha256",
        "host_profile_sha256",
    ):
        if not _is_hex(admission[key], 64):
            raise RunError(f"qualification admission {key} must be a sha256")
    if version == 2:
        for key in ("development_suite_sha256", "experiment_custody_sha256"):
            if not _is_hex(admission[key], 64):
                raise RunError(f"qualification admission {key} must be a sha256")
    else:
        disjoint = _exact_keys(
            admission["repository_disjoint"],
            {
                "repository",
                "release_digest",
                "split_manifest_sha256",
                "split_releases_sha256",
            },
            "qualification repository-disjoint custody",
        )
        if not isinstance(disjoint["repository"], str) or not disjoint["repository"]:
            raise RunError("qualification repository-disjoint name is invalid")
        if (
            not isinstance(disjoint["release_digest"], str)
            or not disjoint["release_digest"].startswith("sha256:")
            or not _is_hex(disjoint["release_digest"][7:], 64)
            or any(
                not _is_hex(disjoint[key], 64)
                for key in ("split_manifest_sha256", "split_releases_sha256")
            )
        ):
            raise RunError("qualification repository-disjoint digests are invalid")
    if policy_keys and not _is_hex(admission["decision_policy_sha256"], 64):
        raise RunError("qualification admission decision_policy_sha256 must be a sha256")
    if admission["cache_regime"] not in ("true_process_cold", "warm_cache"):
        raise RunError("qualified admission cannot use an undeclared cache regime")

    license_claim = _exact_keys(
        admission["license"],
        {"reviewer_id", "decision", "receipt_sha256"},
        "qualification admission license",
    )
    if not isinstance(license_claim["reviewer_id"], str) or not license_claim["reviewer_id"]:
        raise RunError("qualification admission license reviewer must be nonempty")
    if license_claim["decision"] != "approved":
        raise RunError("qualification admission license must be approved")
    if not _is_hex(license_claim["receipt_sha256"], 64):
        raise RunError("qualification admission license receipt digest is malformed")

    gold = _exact_keys(
        admission["gold"],
        {"frozen_before_results", "annotators", "adjudicator_id", "adjudication_receipt_sha256"},
        "qualification admission gold",
    )
    if gold["frozen_before_results"] is not True:
        raise RunError("qualification gold must be frozen before runner results")
    annotators = gold["annotators"]
    if not isinstance(annotators, list) or len(annotators) != 2:
        raise RunError("qualification admission requires exactly two annotators")
    identities = []
    receipt_digests = []
    for index, raw in enumerate(annotators):
        annotator = _exact_keys(
            raw,
            {"annotator_id", "receipt_sha256"},
            f"qualification admission annotators[{index}]",
        )
        if not isinstance(annotator["annotator_id"], str) or not annotator["annotator_id"]:
            raise RunError("qualification annotator id must be nonempty")
        if not _is_hex(annotator["receipt_sha256"], 64):
            raise RunError("qualification annotation receipt digest is malformed")
        identities.append(annotator["annotator_id"])
        receipt_digests.append(annotator["receipt_sha256"])
    if len(set(identities)) != 2 or len(set(receipt_digests)) != 2:
        raise RunError("qualification annotation authorities and receipts must be distinct")
    if not isinstance(gold["adjudicator_id"], str) or not gold["adjudicator_id"]:
        raise RunError("qualification adjudicator id must be nonempty")
    if gold["adjudicator_id"] in identities:
        raise RunError("qualification adjudicator must be distinct from annotators")
    if not _is_hex(gold["adjudication_receipt_sha256"], 64):
        raise RunError("qualification adjudication receipt digest is malformed")

    models = _exact_keys(
        admission["models"],
        {"quanta_model_revision", "semble_model_revision", "semble_model_asset_sha256"},
        "qualification admission models",
    )
    for key in ("quanta_model_revision", "semble_model_revision"):
        if not isinstance(models[key], str) or not models[key]:
            raise RunError(f"qualification admission models.{key} must be nonempty")
    if not _is_hex(models["semble_model_asset_sha256"], 64):
        raise RunError("qualification Semble model asset digest is malformed")

    verification = _exact_keys(
        admission["verification"],
        {"contract_python_receipt_sha256", "contract_rust_receipt_sha256", "sdk_receipt_sha256"},
        "qualification admission verification",
    )
    for key, value in verification.items():
        if not _is_hex(value, 64):
            raise RunError(f"qualification admission verification.{key} is malformed")
    return admission


GOLD_REVIEW_LABEL_KEYS = frozenset(
    {
        "answerable",
        "gold",
        "query_intent",
        "judgment_policy",
        "file_judgments",
        "declaration_judgments",
    }
)


def _gold_review_labels(task: dict) -> dict:
    return {key: task[key] for key in task if key in GOLD_REVIEW_LABEL_KEYS}


def _validate_license_receipt(payload: object, admission: dict) -> None:
    """Bind the legal-use decision to the admitted corpus and reviewer."""
    receipt = _exact_keys(
        payload,
        {
            "schema_version",
            "reviewer_id",
            "decision",
            "repository_commit",
            "corpus_manifest_sha256",
            "rationale",
        },
        "qualification license receipt",
    )
    if type(receipt["schema_version"]) is not int or receipt["schema_version"] != 1:
        raise RunError("qualification license receipt schema version mismatch")
    if receipt["reviewer_id"] != admission["license"]["reviewer_id"]:
        raise RunError("qualification license receipt reviewer mismatch")
    if receipt["decision"] != "approved":
        raise RunError("qualification license receipt is not approved")
    for key in ("repository_commit", "corpus_manifest_sha256"):
        if receipt[key] != admission[key]:
            raise RunError(f"qualification license receipt {key} mismatch")
    if not isinstance(receipt["rationale"], str) or not receipt["rationale"].strip():
        raise RunError("qualification license receipt rationale missing")


def _validate_gold_review_receipt(
    payload: object,
    *,
    role: str,
    reviewer_id: str,
    suite_sha256: str,
    suite: dict,
    repo: Path,
    annotation_digests: list[str] | None = None,
    allow_mixed_source_oracle: bool = False,
) -> None:
    """Require source-valid decisions for every human-reviewed task.

    Version 2 receipts cover only subjective tasks in a disjoint mixed suite.
    Mechanical tasks stay bound to the full suite hash and source validation.
    """
    keys = {"schema_version", "reviewer_id", "suite_sha256", "reviews"}
    if annotation_digests is not None:
        keys.add("annotation_receipt_sha256")
    receipt = _exact_keys(payload, keys, f"qualification {role} receipt")
    tasks = suite["tasks"]
    has_mechanical = any("source_oracle" in task for task in tasks)
    mixed = allow_mixed_source_oracle and has_mechanical
    expected_version = 2 if mixed else 1
    if type(receipt["schema_version"]) is not int or receipt["schema_version"] != expected_version:
        raise RunError(f"qualification {role} receipt schema version mismatch")
    if receipt["reviewer_id"] != reviewer_id:
        raise RunError(f"qualification {role} receipt identity mismatch")
    if receipt["suite_sha256"] != suite_sha256:
        raise RunError(f"qualification {role} receipt suite mismatch")
    if (
        annotation_digests is not None
        and receipt["annotation_receipt_sha256"] != annotation_digests
    ):
        raise RunError("qualification adjudication receipt annotation binding mismatch")
    receipt_tasks = [task for task in tasks if "source_oracle" not in task] if mixed else tasks
    if mixed and not receipt_tasks:
        raise RunError("qualification mixed suite lacks human-reviewed tasks")
    reviews = receipt["reviews"]
    if not isinstance(reviews, list) or len(reviews) != len(receipt_tasks):
        raise RunError(f"qualification {role} receipt task coverage mismatch")
    reviewed_tasks = []
    for index, (task, raw_review) in enumerate(zip(receipt_tasks, reviews, strict=True)):
        review = _exact_keys(
            raw_review,
            {"task_id", "query_sha256", "labels", "rationale"},
            f"qualification {role} reviews[{index}]",
        )
        if (review["task_id"], review["query_sha256"]) != (
            task["task_id"],
            task["query_sha256"],
        ):
            raise RunError(f"qualification {role} receipt task identity mismatch: {index}")
        if "source_oracle" in task:
            raise RunError(
                f"qualification {role} suite uses mechanical source-oracle labels: {index}"
            )
        label_review = task.get("label_review")
        if isinstance(label_review, dict) and label_review.get("assessment") == "unreviewed":
            raise RunError(f"qualification {role} suite label explicitly unreviewed: {index}")
        if not isinstance(review["rationale"], str) or not review["rationale"].strip():
            raise RunError(f"qualification {role} receipt rationale missing: {index}")
        expected_keys = set(_gold_review_labels(task))
        labels = _exact_keys(
            review["labels"], expected_keys, f"qualification {role} reviews[{index}].labels"
        )
        if annotation_digests is not None and labels != _gold_review_labels(task):
            raise RunError(f"qualification adjudication differs from suite gold: {index}")
        reviewed_task = dict(task)
        reviewed_task.update(labels)
        reviewed_tasks.append(reviewed_task)
    reviewed_suite = dict(suite)
    if mixed:
        reviewed_by_id = {task["task_id"]: task for task in reviewed_tasks}
        reviewed_suite["tasks"] = [
            task if "source_oracle" in task else reviewed_by_id[task["task_id"]] for task in tasks
        ]
    else:
        reviewed_suite["tasks"] = reviewed_tasks
    try:
        validate_suite(repo, reviewed_suite)
    except (ValueError, TypeError, KeyError) as exc:
        raise RunError(f"qualification {role} receipt source validation failed: {exc}") from exc


def _validate_disjoint_admission_source(
    admission: dict,
    suite: dict,
    repo: Path,
    split_manifest_path: Path,
    split_releases_path: Path,
) -> None:
    """Prove a holdout suite against the complete release and split authority."""
    benchmark_dir = str(Path(__file__).resolve().parents[1])
    if benchmark_dir not in sys.path:
        sys.path.insert(0, benchmark_dir)
    from tools.benchmark import corpus_binding

    claim = admission["repository_disjoint"]
    if claim["split_manifest_sha256"] != sha_file(split_manifest_path) or claim[
        "split_releases_sha256"
    ] != sha_file(split_releases_path):
        raise RunError("qualification repository-disjoint split bytes differ")
    release_paths = read_json(split_releases_path)
    try:
        releases = corpus_binding._split_releases(release_paths)
        split = corpus_binding.validate_split_manifest(split_manifest_path.read_bytes(), releases)
    except (ValueError, OSError) as exc:
        raise RunError(f"qualification repository-disjoint split is invalid: {exc}") from exc
    selected = [
        row
        for row in split["repositories"]
        if row["repository"] == claim["repository"]
        and row["release_digest"] == claim["release_digest"]
    ]
    if len(selected) != 1 or selected[0]["split"] != "holdout":
        raise RunError("qualification repository-disjoint holdout assignment differs")
    source = selected[0]
    if (
        source["repository_commit"] != admission["repository_commit"]
        or source["code_only_universe_digest"] != "sha256:" + suite["file_universe_digest"]
        or any(task.get("split") != "eval" for task in suite["tasks"])
        or sorted({task["query_family_id"] for task in suite["tasks"]})
        != source["query_family_ids"]
    ):
        raise RunError("qualification repository-disjoint suite differs from split source")
    try:
        validate_suite(repo, suite)
    except (ValueError, TypeError, KeyError) as exc:
        raise RunError(f"qualification repository-disjoint suite source is invalid: {exc}") from exc


def verify_admission_bundle(
    manifest_path: Path,
    license_path: Path,
    annotation_paths: list[Path],
    adjudication_path: Path,
    *,
    source_revision: str,
    corpus_manifest_path: Path,
    suite_path: Path,
    development_suite_path: Path | None,
    experiment_custody_path: Path | None,
    repo: Path,
    query_pack_path: Path,
    lockfile_path: Path,
    host_profile_path: Path,
    cache_regime: str,
    receipt_paths: dict[str, Path],
    quanta_model_revision: str | None = None,
    semble_model_revision: str | None = None,
    semble_model_asset_sha256: str | None = None,
    split_manifest_path: Path | None = None,
    split_releases_path: Path | None = None,
) -> dict:
    """Re-derive every authority digest in a qualified admission bundle."""
    admission = validate_admission_manifest(read_json(manifest_path))
    if admission["source_revision"] != source_revision:
        raise RunError("qualification admission source revision mismatch")
    corpus = read_json(corpus_manifest_path)
    if not isinstance(corpus, dict) or admission["repository_commit"] != corpus.get(
        "repository_commit"
    ):
        raise RunError("qualification admission repository commit mismatch")
    suite_payload = read_json(suite_path)
    tasks = suite_payload.get("tasks") if isinstance(suite_payload, dict) else None
    if not isinstance(tasks, list) or not tasks:
        raise RunError("qualification admission suite lacks tasks")
    uncategorized = [
        task.get("task_id")
        for task in tasks
        if isinstance(task, dict)
        and task.get("split") == "eval"
        and (not isinstance(task.get("category"), str) or not task.get("category"))
    ]
    if uncategorized:
        raise RunError("qualification admission requires a category for every eval task")
    for key, path in (
        ("corpus_manifest_sha256", corpus_manifest_path),
        ("suite_sha256", suite_path),
        ("query_pack_sha256", query_pack_path),
        ("semble_lockfile_sha256", lockfile_path),
        ("host_profile_sha256", host_profile_path),
    ):
        if admission[key] != sha_file(path):
            raise RunError(f"qualification admission {key} mismatch")
    if admission["schema_version"] == 2:
        if (
            development_suite_path is None
            or experiment_custody_path is None
            or split_manifest_path is not None
            or split_releases_path is not None
        ):
            raise RunError("qualification local custody paths are incomplete or mixed")
        for key, path in (
            ("development_suite_sha256", development_suite_path),
            ("experiment_custody_sha256", experiment_custody_path),
        ):
            if admission[key] != sha_file(path):
                raise RunError(f"qualification admission {key} mismatch")
        custody = validate_experiment_custody(
            repo,
            read_json(experiment_custody_path),
            read_json(development_suite_path),
            suite_payload,
        )
        if custody["source_revision"] != source_revision:
            raise RunError("qualification experiment source revision mismatch")
    else:
        if (
            split_manifest_path is None
            or split_releases_path is None
            or development_suite_path is not None
            or experiment_custody_path is not None
        ):
            raise RunError(
                "qualification repository-disjoint custody paths are incomplete or mixed"
            )
        _validate_disjoint_admission_source(
            admission, suite_payload, repo, split_manifest_path, split_releases_path
        )
    if admission["cache_regime"] != cache_regime:
        raise RunError("qualification admission cache regime mismatch")
    if admission["license"]["receipt_sha256"] != sha_file(license_path):
        raise RunError("qualification admission license receipt mismatch")
    _validate_license_receipt(read_json(license_path), admission)
    if len(annotation_paths) != 2:
        raise RunError("qualification admission requires two frozen annotation receipts")
    expected_annotations = [row["receipt_sha256"] for row in admission["gold"]["annotators"]]
    observed_annotations = [sha_file(path) for path in annotation_paths]
    if expected_annotations != observed_annotations:
        raise RunError("qualification admission annotation receipt mismatch")
    if admission["gold"]["adjudication_receipt_sha256"] != sha_file(adjudication_path):
        raise RunError("qualification admission adjudication receipt mismatch")
    for index, path in enumerate(annotation_paths):
        _validate_gold_review_receipt(
            read_json(path),
            role=f"annotation {index + 1}",
            reviewer_id=admission["gold"]["annotators"][index]["annotator_id"],
            suite_sha256=admission["suite_sha256"],
            suite=suite_payload,
            repo=repo,
            allow_mixed_source_oracle=admission["schema_version"] == 3,
        )
    _validate_gold_review_receipt(
        read_json(adjudication_path),
        role="adjudication",
        reviewer_id=admission["gold"]["adjudicator_id"],
        suite_sha256=admission["suite_sha256"],
        suite=suite_payload,
        repo=repo,
        annotation_digests=observed_annotations,
        allow_mixed_source_oracle=admission["schema_version"] == 3,
    )
    required_receipts = {
        "contract_python_receipt": "contract_python_receipt_sha256",
        "contract_rust_receipt": "contract_rust_receipt_sha256",
        "sdk_receipt": "sdk_receipt_sha256",
    }
    if set(receipt_paths) != set(required_receipts):
        raise RunError("qualification admission lacks the complete receipt authority set")
    for role, claim in required_receipts.items():
        if admission["verification"][claim] != sha_file(receipt_paths[role]):
            raise RunError(f"qualification admission {role} mismatch")
    models = admission["models"]
    for expected, observed, label in (
        (models["quanta_model_revision"], quanta_model_revision, "Quanta model revision"),
        (models["semble_model_revision"], semble_model_revision, "Semble model revision"),
        (
            models["semble_model_asset_sha256"],
            semble_model_asset_sha256,
            "Semble model asset digest",
        ),
    ):
        if observed is not None and expected != observed:
            raise RunError(f"qualification admission {label} mismatch")
    return admission


def _spec_int(spec: dict, key: str, minimum: int) -> int:
    value = spec.get(key)
    if type(value) is not int or isinstance(value, bool) or value < minimum:
        raise RunError(f"spec.{key} must be an integer >= {minimum}")
    return value


def _validate_file_pair_contract(spec: dict, *, paired: bool) -> bool:
    """One admission boundary for loading, direct capture and staged capture."""
    profiles = spec.get("execution_profiles")
    quanta = profiles.get("quanta") if isinstance(profiles, dict) else None
    if not isinstance(quanta, dict) or quanta.get("policy") not in qp.FILE_PAIR_POLICIES:
        return False
    if spec.get("scope", "exploratory") == "qualified":
        if (
            quanta["policy"] not in qp.QUALIFIED_FILE_PAIR_POLICIES
            or spec.get("claims", {}).get("quality") is not True
            or not isinstance(spec.get("admission"), dict)
            or _admission_keys(spec["admission"]) != ADMISSION_DISJOINT_KEYS
        ):
            raise RunError(
                "qualified file scoring requires code_search_file or natural_language_file, "
                "repository-disjoint admission and a quality claim"
            )
    elif any(spec.get("claims", {}).values()):
        raise RunError("exploratory code-search file pair cannot carry claims")
    if spec.get("routes", ["lexical", "semantic", "hybrid"]) != ["lexical"]:
        raise RunError("code-search file pair requires lexical-only Quanta route")
    semble = profiles.get("semble")
    if paired and (not isinstance(semble, dict) or semble.get("mode") != "lexical-file"):
        raise RunError("code-search file pair requires Semble lexical-file rank unit")
    return True


def load_spec(path: Path, *, standalone_quanta: bool = False) -> dict:
    """Load a capture spec with pair-only policies unless Quanta runs alone."""
    spec = read_json(path)
    if not isinstance(spec, dict):
        raise RunError("spec must be an object")
    for key in SPEC_REQUIRED:
        if key not in spec:
            raise RunError(f"spec lacks required key: {key}")
    if "runner_revision" in spec:
        raise RunError("runner_revision is derived from the Rust runner binary; do not supply it")
    if "evidence" in spec:
        raise RunError(
            "spec.evidence was removed: evidence content never travels through "
            "the spec; pass receipt artifact paths via spec.receipts"
        )
    unknown = sorted(set(spec) - set(SPEC_REQUIRED) - set(SPEC_OPTIONAL))
    if unknown:
        raise RunError(f"spec has unknown keys: {unknown}")
    for key in (
        "repo",
        "manifest",
        "suite",
        "query_pack",
        "output_root",
        "runner_binary",
        "searchd_binary",
    ):
        if not isinstance(spec[key], str) or not spec[key]:
            raise RunError(f"spec.{key} must be a nonempty string")
    if spec["spec_version"] != 2:
        raise RunError("spec.spec_version must be 2")
    if spec.get("symbol_coverage_policy", "require-complete") not in (
        "require-complete",
        "allow-incomplete",
    ):
        raise RunError("unknown symbol coverage policy")
    profiles = spec["execution_profiles"]
    if not isinstance(profiles, dict) or set(profiles) not in ({"quanta"}, {"quanta", "semble"}):
        raise RunError("spec.execution_profiles must contain quanta and optional semble")
    quanta_profile = profiles["quanta"]
    if (
        not isinstance(quanta_profile, dict)
        or quanta_profile.get("policy") not in qp.SUPPORTED_POLICIES
    ):
        raise RunError("spec.execution_profiles.quanta is invalid")
    policy = quanta_profile["policy"]
    try:
        expected_quanta_profile = qp.execution_profile(
            policy,
            quanta_profile.get("config")
            if policy in ("natural_language", "natural_language_file")
            else None,
        )
    except qp.QueryPlanError as exc:
        raise RunError(f"spec.execution_profiles.quanta config is invalid: {exc}") from exc
    if quanta_profile != expected_quanta_profile:
        raise RunError("spec.execution_profiles.quanta differs from the frozen profile")
    if (
        policy in ("natural_language", "natural_language_file")
        and quanta_profile["config"]["max_tokens"] != qp.DEFAULT_NL_CONFIG["max_tokens"]
        and (
            spec.get("scope", "exploratory") != "exploratory"
            or any(spec.get("claims", {}).values())
        )
    ):
        raise RunError(
            "custom natural-language token budget requires exploratory scope without claims"
        )
    if quanta_profile["policy"] not in PAIR_QUANTA_POLICIES:
        if not standalone_quanta:
            raise RunError(
                "spec.execution_profiles.quanta uses a diagnostic rank profile; "
                "run it as a standalone Quanta capture"
            )
        if "semble" in profiles:
            raise RunError("standalone diagnostic rank profile cannot include Semble")
        if spec.get("scope", "exploratory") != "exploratory" or any(
            spec.get("claims", {}).values()
        ):
            raise RunError("standalone diagnostic rank profile cannot carry qualified claims")
        required_routes = (
            ["symbol"] if quanta_profile["policy"] == "exact_symbol_name" else ["lexical"]
        )
        if spec.get("routes") != required_routes:
            raise RunError(
                f"{quanta_profile['policy']} standalone capture requires {required_routes} route"
            )
    if "semble" in profiles:
        _validate_semble_profile(profiles["semble"], "spec.execution_profiles.semble")
    _validate_file_pair_contract(spec, paired="semble" in profiles)
    _spec_int(spec, "top_k", 1)
    server_observation_configuration(spec.get("query_stage_observation", "enabled"))
    hybrid_fetch_policy_configuration(spec.get("experimental_hybrid_fetch_floor", "100"))
    if "code_search_rank_study" in spec:
        rank_study_configuration(
            spec["code_search_rank_study"],
            quanta_profile["policy"],
            spec.get("routes", []),
            speed_claim=spec.get("claims", {}).get("speed") is True,
        )
    if not _is_hex(spec["searchd_expected_sha256"], 64):
        raise RunError("spec.searchd_expected_sha256 must be a lowercase sha256")
    strategies = spec["strategies"]
    if not isinstance(strategies, list) or not strategies:
        raise RunError("spec.strategies must be a nonempty list")
    for entry in strategies:
        if not isinstance(entry, dict):
            raise RunError("spec.strategies entries must be objects")
        unknown_entry = sorted(
            set(entry) - {"name", "window_bytes", "overlap_bytes", "max_item_bytes"}
        )
        if unknown_entry:
            raise RunError(f"strategy has unknown keys: {unknown_entry}")
        if entry.get("name") not in RUNNABLE_STRATEGIES:
            raise RunError(f"unknown strategy: {entry.get('name')}")
        for key, minimum in (("window_bytes", 1), ("overlap_bytes", 0), ("max_item_bytes", 1)):
            if key in entry and (
                type(entry[key]) is not int or isinstance(entry[key], bool) or entry[key] < minimum
            ):
                raise RunError(f"strategy.{key} must be an integer >= {minimum}")
    if "routes" in spec:
        routes = spec["routes"]
        if not isinstance(routes, list) or not routes or len(set(routes)) != len(routes):
            raise RunError("spec.routes must be a nonempty unique list")
        for route in routes:
            if not isinstance(route, str) or not route:
                raise RunError("spec.routes entries must be nonempty strings")
    _validate_semble_route_binding(spec)
    if "blinding" in spec and spec["blinding"] not in ("isolated", "attested"):
        raise RunError("spec.blinding must be isolated or attested")
    if "scope" in spec and spec["scope"] not in ("exploratory", "qualified"):
        raise RunError("spec.scope must be exploratory or qualified")
    if "embedder" in spec and spec["embedder"] not in (
        "potion-code",
        "potion-code-full-v2",
        "hash-dev",
    ):
        raise RunError("spec.embedder must be potion-code, potion-code-full-v2 or hash-dev")
    if "cache_regime" in spec and spec["cache_regime"] not in (
        "true_process_cold",
        "warm_cache",
        "undeclared",
    ):
        raise RunError("spec.cache_regime must be true_process_cold, warm_cache or undeclared")
    for key, minimum in (
        ("generation", 0),
        ("seed", 0),
        ("timeout_secs", 1),
        ("io_timeout_secs", 1),
        ("symbol_total_timeout_ms", 1),
        ("repetitions", 1),
        ("query_repetitions_per_root", 1),
        ("query_warmup_passes", 0),
    ):
        if key in spec:
            _spec_int(spec, key, minimum)
    if spec.get("symbol_total_timeout_ms", 120_000) >= 2**64:
        raise RunError("spec.symbol_total_timeout_ms must fit u64")
    if "alternate_order" in spec and type(spec["alternate_order"]) is not bool:
        raise RunError("spec.alternate_order must be a boolean")
    for key in (
        "suite_secret_root",
        "isolation_method",
        "access_block_log",
        "run_id",
        "runner_name",
        "repo_id",
        "revision_id",
        "semble_route",
        "semble_python",
        "semble_cache_root",
        "quanta_model_dir",
        "semble_lockfile",
        "baseline_route",
        "candidate_route",
        "host_profile",
        "linux_cgroup_parent",
    ):
        if key in spec and (not isinstance(spec[key], str) or not spec[key]):
            raise RunError(f"spec.{key} must be a nonempty string")
    if "linux_cgroup_parent" in spec:
        parent = Path(spec["linux_cgroup_parent"])
        if not parent.is_absolute() or ".." in parent.parts:
            raise RunError("spec.linux_cgroup_parent must be an absolute delegated path")
        if spec.get("scope", "exploratory") != "qualified":
            raise RunError("spec.linux_cgroup_parent is valid only for qualified Linux capture")
    if "semble_lockfile_sha256" in spec and not _is_hex(spec["semble_lockfile_sha256"], 64):
        raise RunError("spec.semble_lockfile_sha256 must be a lowercase sha256")
    if "semble_model_revision" in spec and not _is_hex(spec["semble_model_revision"], 40):
        raise RunError("spec.semble_model_revision must be a lowercase 40-hex revision")
    if "claims" in spec:
        claims = spec["claims"]
        if not isinstance(claims, dict):
            raise RunError("spec.claims must be an object")
        unknown_claims = sorted(set(claims) - {"quality", "speed", "same_model", "incremental"})
        if unknown_claims:
            raise RunError(f"spec.claims has unknown keys: {unknown_claims}")
        for key, value in claims.items():
            if type(value) is not bool:
                raise RunError(f"spec.claims.{key} must be a strict boolean")
    if "source_closure_reuse" in spec:
        value = spec["source_closure_reuse"]
        if standalone_quanta:
            raise RunError("spec.source_closure_reuse is supported only by pair capture")
        if not isinstance(value, str) or not Path(value).is_absolute() or not value:
            raise RunError("spec.source_closure_reuse must be an absolute path")
        if spec.get("scope", "exploratory") != "exploratory" or any(
            spec.get("claims", {}).values()
        ):
            raise RunError(
                "source closure reuse is only valid for exploratory captures without claims"
            )
    if spec.get("embedder") == "potion-code-full-v2" and (
        spec.get("scope", "exploratory") != "exploratory" or any(spec.get("claims", {}).values())
    ):
        raise RunError("potion-code-full-v2 is exploratory diagnostic only; claims must be false")
    if "receipts" in spec:
        receipts = spec["receipts"]
        if not isinstance(receipts, dict):
            raise RunError("spec.receipts must be an object")
        unknown_receipts = sorted(
            set(receipts) - (set(RECEIPT_KEYS) - {"contract_execution_logs", "sdk_execution_logs"})
        )
        if unknown_receipts:
            raise RunError(f"spec.receipts has unknown keys: {unknown_receipts}")
        for key, value in receipts.items():
            if not isinstance(value, str) or not value:
                raise RunError(f"spec.receipts.{key} must be a nonempty path")
    if "admission" in spec:
        keys = _admission_keys(spec["admission"])
        admission = _exact_keys(spec["admission"], set(keys), "spec.admission")
        for key in set(keys) - {"annotation_receipts"}:
            if not isinstance(admission[key], str) or not admission[key]:
                raise RunError(f"spec.admission.{key} must be a nonempty path")
        annotation_receipts = admission["annotation_receipts"]
        if (
            not isinstance(annotation_receipts, list)
            or len(annotation_receipts) != 2
            or any(not isinstance(path, str) or not path for path in annotation_receipts)
            or len(set(annotation_receipts)) != 2
        ):
            raise RunError("spec.admission.annotation_receipts must hold two distinct paths")
    scope = spec.get("scope", "exploratory")
    if scope == "qualified" and "admission" not in spec:
        raise RunError("qualified capture requires spec.admission")
    if scope != "qualified" and "admission" in spec:
        raise RunError("spec.admission is valid only for a qualified capture")
    if "contention_override" in spec and type(spec["contention_override"]) is not bool:
        raise RunError("spec.contention_override must be a boolean")
    return spec


def require_reviewed_file_labels(suite: dict, policy: str) -> None:
    """Bind qualified file quality to its request mode and independent labels."""
    if policy not in qp.QUALIFIED_FILE_PAIR_POLICIES:
        raise RunError("unsupported qualified file policy")
    natural_language = policy == "natural_language_file"
    mode = qp.NATURAL_LANGUAGE_FILE_SEARCH if natural_language else qp.DEFAULT_FILE_SEARCH
    tasks = suite.get("tasks")
    if not isinstance(tasks, list):
        raise RunError("qualified file suite lacks tasks")
    for task in tasks:
        if not isinstance(task, dict):
            raise RunError("qualified file suite task is malformed")
        if task.get("split") != "eval":
            continue
        contract = task.get("evaluation_contract")
        if not isinstance(contract, dict) or contract.get("request_mode") != mode:
            raise RunError("qualified file suite requires a declared file request mode")
        if (
            contract.get("gold_unit") != "distinct_file"
            or contract.get("result_unit") != "distinct_file"
        ):
            raise RunError("qualified file suite requires distinct_file gold and results")
        if natural_language and task.get("query_intent") != "semantic_intent":
            raise RunError("qualified natural-language file suite requires semantic_intent")
        if not natural_language and task.get("answerable") is not True:
            continue
        if "source_oracle" in task or task.get("judgment_policy") != COMPLETE_JUDGMENT_POLICY:
            raise RunError(
                "qualified file labels require independently reviewed "
                f"complete relevance labels: {task.get('task_id', '<unknown>')}"
            )


def preflight_capture(spec: dict) -> Path:
    """Refuse dirty/wrong-HEAD inputs, contract drift and unpinned binaries."""
    manifest = read_json(Path(spec["manifest"]))
    if not isinstance(manifest, dict) or not isinstance(manifest.get("repository_commit"), str):
        raise RunError("manifest must pin repository_commit")
    materialized = spec.get("_materialized_corpus")
    if materialized is not None:
        if not isinstance(materialized, dict):
            raise RunError("materialized corpus context must be an object")
        repo = Path(spec["repo"]).resolve()
        _verify_materialized_corpus(repo, Path(spec["manifest"]), materialized.get("proof_sha256"))
    else:
        try:
            repo = verify_repo(Path(spec["repo"]), manifest["repository_commit"])
        except ValueError as exc:
            raise RunError(f"pinned repository proof failed: {exc}") from exc
    suite_payload = read_json(Path(spec["suite"]))
    if not isinstance(suite_payload, dict) or suite_payload.get("schema_version") != 3:
        raise RunError("capture requires a v3 suite")
    profiles = spec.get("execution_profiles")
    quanta_profile = profiles.get("quanta") if isinstance(profiles, dict) else None
    if (
        spec.get("scope") == "qualified"
        and isinstance(quanta_profile, dict)
        and quanta_profile.get("policy") in qp.QUALIFIED_FILE_PAIR_POLICIES
    ):
        require_reviewed_file_labels(suite_payload, quanta_profile["policy"])
    try:
        contract = validate_comparison_contract(
            suite_payload.get("comparison_contract"), "suite.comparison_contract"
        )
    except ValueError as exc:
        raise RunError(f"suite comparison contract invalid: {exc}") from exc
    if contract["top_k"] != spec["top_k"]:
        raise RunError("spec top_k differs from the suite comparison contract")
    try:
        searchd_digest = sha_file(Path(spec["searchd_binary"]))
    except OSError as exc:
        raise RunError(f"cannot hash the pinned searchd binary: {exc}") from exc
    if searchd_digest != spec["searchd_expected_sha256"]:
        raise RunError("searchd binary digest differs from the pinned preflight digest")
    out_root = Path(spec["output_root"]).resolve()
    if out_root == repo or repo in out_root.parents:
        raise RunError("output root must be outside the frozen repository")
    return out_root


def _unix_socket_path_limit() -> int | None:
    # sockaddr_un.sun_path includes the trailing NUL for pathname sockets.
    # Keep this check before creating a stage: a daemon failure after indexing
    # is both expensive and leaves a partial forensic tree.
    if sys.platform == "darwin":
        return 103
    if sys.platform.startswith("linux"):
        return 107
    return None


def _strategy_run_directory(index: int, name: str) -> str:
    """Keep fixed-window state paths within the pathname Unix socket limit."""
    if name not in RUNNABLE_STRATEGIES:
        raise RunError(f"unknown strategy: {name}")
    # The strategy name remains in the record; this is only an artifact path.
    path_name = "fw_strict" if name == "fixed_window_strict" else name
    return f"strategy-{index:02d}-{path_name}"


def preflight_daemon_socket_paths(
    output_root: Path, strategies: list[dict], *, repetitions: int = 1, paired: bool = False
) -> None:
    """Refuse any state root whose searchd UDS path cannot fit sun_path."""
    limit = _unix_socket_path_limit()
    if limit is None:
        return
    if repetitions < 1:
        raise RunError("pair repetitions must be positive")
    for rep in range(repetitions):
        root = output_root / f"rep-{rep:02d}" / "quanta" if paired else output_root
        for index, strategy in enumerate(strategies):
            name = strategy.get("name")
            socket = root / _strategy_run_directory(index, name) / "state/search-plane/control.sock"
            length = len(os.fsencode(socket.resolve()))
            if length > limit:
                raise RunError(
                    f"searchd Unix socket path is {length} bytes (limit {limit}): "
                    f"{socket}; choose a shorter output_root"
                )


def cmd_quanta(args: argparse.Namespace) -> int:
    try:
        return run_quanta(
            load_spec(Path(args.spec), standalone_quanta=True), Path(args.spec).parent
        )
    except (RunError, ValueError) as exc:
        print(f"ERROR: {exc}", file=sys.stderr)
        return 2


def write_projected_pack(pack_path: Path, suite_path: Path, routes: list[str], out: Path) -> Path:
    """Freeze-consumer projection: pack+suite narrowed to one system's routes."""
    pack = read_json(pack_path)
    suite = read_json(suite_path)
    if not isinstance(pack, dict):
        raise RunError(f"query pack is not an object: {pack_path}")
    if not isinstance(suite, dict):
        raise RunError(f"suite is not an object: {suite_path}")
    projected_pack, _ = project_pack_and_suite(pack, suite, routes)
    out.write_text(
        json.dumps(projected_pack, indent=2, sort_keys=True) + "\n",
        encoding="utf-8",
    )
    return out


def _diagnostic_count(value: object, where: str) -> dict:
    if not isinstance(value, dict):
        raise RunError(f"{where} count is malformed")
    count = _exact_keys(value, {"kind", "count"}, where)
    if (
        count["kind"] not in ("exact", "at_least")
        or type(count["count"]) is not int
        or count["count"] < 0
    ):
        raise RunError(f"{where} count is invalid")
    return count


def _validate_diagnostic_response_v2(
    response: object, error_code: object, key: tuple[str, str]
) -> None:
    """Validate one preserved SDK response detail (RBR-01).

    Unknown observations stay explicit nulls; executed and contributed
    lanes are independent facts. Failure rows carry no response object.
    """
    where = f"retrieval diagnostic response for {key}"
    if error_code is not None:
        if response is not None:
            raise RunError(f"{where} must be null for failed results")
        return
    if not isinstance(response, dict):
        raise RunError(f"{where} is malformed")
    detail = _exact_keys(
        response,
        {
            "request_id",
            "early_stop_reason",
            "engines_executed",
            "engines_touched",
            "strategy",
            "window_returned",
            "window_candidate_count",
            "lane_traces",
        },
        where,
    )
    if detail["request_id"] is not None and (
        type(detail["request_id"]) is not int or detail["request_id"] < 0
    ):
        raise RunError(f"{where}.request_id is invalid")
    if detail["early_stop_reason"] is not None and not isinstance(detail["early_stop_reason"], str):
        raise RunError(f"{where}.early_stop_reason is invalid")
    for field in ("engines_executed", "engines_touched"):
        engines = detail[field]
        if engines is None:
            continue
        if (
            not isinstance(engines, list)
            or any(not isinstance(engine, str) or not engine for engine in engines)
            or len(set(engines)) != len(engines)
        ):
            raise RunError(f"{where}.{field} is invalid")
    if detail["strategy"] is not None and not isinstance(detail["strategy"], str):
        raise RunError(f"{where}.strategy is invalid")
    if detail["window_returned"] is not None and (
        type(detail["window_returned"]) is not int or detail["window_returned"] < 0
    ):
        raise RunError(f"{where}.window_returned is invalid")
    if detail["window_candidate_count"] is not None:
        _diagnostic_count(detail["window_candidate_count"], f"{where}.window")
    if not isinstance(detail["lane_traces"], list):
        raise RunError(f"{where}.lane_traces is malformed")
    for lane in detail["lane_traces"]:
        if not isinstance(lane, dict):
            raise RunError(f"{where}.lane_traces entry is malformed")
        trace = _exact_keys(
            lane,
            {
                "lane",
                "executed",
                "contributed",
                "candidates",
                "filtered_out",
                "cost",
                "profile",
            },
            f"{where}.lane_traces entry",
        )
        if (
            not isinstance(trace["lane"], str)
            or not trace["lane"]
            or type(trace["executed"]) is not bool
            or type(trace["contributed"]) is not bool
            or trace["contributed"]
            and not trace["executed"]
        ):
            raise RunError(f"{where}.lane_traces entry is invalid")
        if trace["candidates"] is not None:
            _diagnostic_count(trace["candidates"], f"{where}.lane_traces entry")
        for field in ("filtered_out", "cost"):
            if trace[field] is not None and (type(trace[field]) is not int or trace[field] < 0):
                raise RunError(f"{where}.lane_traces entry.{field} is invalid")
        if trace["profile"] is not None and not isinstance(trace["profile"], str):
            raise RunError(f"{where}.lane_traces entry.profile is invalid")


def _typed_count(value: object, where: str) -> tuple[str, int]:
    count = _exact_keys(value, {"kind", "value"}, where)
    if (
        count["kind"] not in ("exact", "at_least")
        or type(count["value"]) is not int
        or count["value"] < 0
    ):
        raise RunError(f"{where} is invalid")
    return count["kind"], count["value"]


def _typed_window(value: object, where: str) -> tuple[int, bool, dict[str, bool]]:
    if not isinstance(value, dict):
        raise RunError(f"{where} is malformed")
    required = {"returned", "candidate_count", "outcome", "coverage"}
    if set(value) not in (required, required | {"empty_provenance"}):
        raise RunError(f"{where} fields are invalid")
    returned = value["returned"]
    if type(returned) is not int or returned < 0:
        raise RunError(f"{where}.returned is invalid")
    count_kind, count_value = _typed_count(value["candidate_count"], f"{where}.candidate_count")
    if count_value < returned:
        raise RunError(f"{where} candidate count is below returned")
    outcome = value["outcome"]
    if not isinstance(outcome, dict) or not isinstance(outcome.get("kind"), str):
        raise RunError(f"{where}.outcome is malformed")
    outcome_kind = outcome["kind"]
    expected_outcome_fields = {
        "exact_exhausted": {"kind"},
        "lower_bound": {"kind", "continuation"},
        "capped_unknown": {"kind", "cap"},
        "interrupted_partial": {"kind", "reason"},
        "approximate": {"kind", "method", "quality_contract"},
    }
    if (
        outcome_kind not in expected_outcome_fields
        or set(outcome) != expected_outcome_fields[outcome_kind]
    ):
        raise RunError(f"{where}.outcome fields are invalid")
    coverage = _exact_keys(
        value["coverage"],
        {"examined", "lanes"}
        | (
            {"exhaustion_proof"}
            if isinstance(value["coverage"], dict) and "exhaustion_proof" in value["coverage"]
            else set()
        ),
        f"{where}.coverage",
    )
    examined = coverage["examined"]
    if not isinstance(examined, dict) or examined.get("kind") not in (
        "exact",
        "at_least",
        "unknown",
    ):
        raise RunError(f"{where}.coverage.examined is invalid")
    if examined["kind"] == "unknown":
        if set(examined) != {"kind"}:
            raise RunError(f"{where}.coverage.examined fields are invalid")
    elif (
        set(examined) != {"kind", "value"}
        or type(examined["value"]) is not int
        or examined["value"] < 0
    ):
        raise RunError(f"{where}.coverage.examined fields are invalid")
    proof = coverage.get("exhaustion_proof")
    if outcome_kind == "exact_exhausted":
        if count_kind != "exact" or count_value != returned or not isinstance(proof, dict):
            raise RunError(f"{where} exact exhaustion evidence is invalid")
        proof_kind = proof.get("kind")
        proof_field = {
            "probe_exhausted": "fetched",
            "exact_count": "total",
            "universe_scanned": "scanned",
        }.get(proof_kind)
        if (
            proof_field is None
            or set(proof) != {"kind", proof_field}
            or proof[proof_field] != returned
        ):
            raise RunError(f"{where}.coverage.exhaustion_proof is invalid")
    elif proof is not None:
        raise RunError(f"{where} non-exhausted outcome carries an exhaustion proof")
    if outcome_kind == "lower_bound":
        if type(outcome["continuation"]) is not bool or (
            outcome["continuation"] and count_value <= returned
        ):
            raise RunError(f"{where} lower-bound outcome is invalid")
    elif outcome_kind == "capped_unknown":
        if type(outcome["cap"]) is not int or outcome["cap"] <= 0:
            raise RunError(f"{where} capped outcome is invalid")
    elif outcome_kind == "interrupted_partial":
        if outcome["reason"] not in ("deadline", "cancelled", "examined_budget"):
            raise RunError(f"{where} interrupted outcome is invalid")
    elif outcome_kind == "approximate":
        quality = outcome["quality_contract"]
        if (
            outcome["method"] not in ("ann_search", "filtered_refill")
            or not isinstance(quality, dict)
            or set(quality) != {"examined_lower_bound"}
            or type(quality["examined_lower_bound"]) is not int
            or quality["examined_lower_bound"] < 0
        ):
            raise RunError(f"{where} approximate outcome is invalid")
    empty = value.get("empty_provenance")
    if (returned == 0) != (empty in ("available_empty", "filtered_empty", "zero_hit_executed")):
        raise RunError(f"{where}.empty_provenance contradicts returned")
    lanes = coverage["lanes"]
    if not isinstance(lanes, list):
        raise RunError(f"{where}.coverage.lanes is malformed")
    lane_execution: dict[str, bool] = {}
    for lane in lanes:
        if (
            not isinstance(lane, dict)
            or set(lane)
            - {"lane", "executed", "contributed", "filtered_out", "candidates", "cost", "profile"}
            or not {"lane", "executed", "contributed", "filtered_out", "candidates"}.issubset(lane)
        ):
            raise RunError(f"{where}.coverage lane fields are invalid")
        name = lane["lane"]
        if (
            not isinstance(name, str)
            or not name
            or name in lane_execution
            or type(lane["executed"]) is not bool
            or type(lane["contributed"]) is not bool
            or lane["contributed"]
            and not lane["executed"]
        ):
            raise RunError(f"{where}.coverage lane is invalid")
        if type(lane["filtered_out"]) is not int or lane["filtered_out"] < 0:
            raise RunError(f"{where}.coverage lane filtered_out is invalid")
        _typed_count(lane["candidates"], f"{where}.coverage lane candidates")
        if "cost" in lane and (type(lane["cost"]) is not int or lane["cost"] < 0):
            raise RunError(f"{where}.coverage lane cost is invalid")
        if "profile" in lane and (not isinstance(lane["profile"], str) or not lane["profile"]):
            raise RunError(f"{where}.coverage lane profile is invalid")
        lane_execution[name] = lane["executed"]
    exhausted = outcome_kind == "exact_exhausted"
    return returned, exhausted, lane_execution


def _validate_explanation(
    value: object,
    where: str,
    route: str,
    version: int,
    returned: int,
    observation_policy: str = "enabled",
) -> None:
    if value is None:
        if version in (4, 5, 6, 7, 8) and route in ("lexical", "semantic", "hybrid"):
            raise RunError(f"{where} is missing measured stage timings")
        return
    fields = {"request_id", "early_stop_reason", "engines_executed", "engines_touched", "strategy"}
    if version in (6, 7, 8):
        fields.add("planner_trace")
    if version in (4, 5, 6, 7, 8):
        fields.add("stage_timings")
    detail = _exact_keys(value, fields, where)
    if detail["request_id"] is not None and (
        type(detail["request_id"]) is not int or detail["request_id"] < 0
    ):
        raise RunError(f"{where}.request_id is invalid")
    if detail["early_stop_reason"] is not None and not isinstance(detail["early_stop_reason"], str):
        raise RunError(f"{where}.early_stop_reason is invalid")
    for field in ("engines_executed", "engines_touched"):
        engines = detail[field]
        if engines is not None and (
            not isinstance(engines, list)
            or any(not isinstance(item, str) or not item for item in engines)
            or len(engines) != len(set(engines))
        ):
            raise RunError(f"{where}.{field} is invalid")
    if detail["strategy"] is not None and not isinstance(detail["strategy"], str):
        raise RunError(f"{where}.strategy is invalid")
    if version in (6, 7, 8):
        trace = detail["planner_trace"]
        if not isinstance(trace, list) or any(
            not isinstance(entry, dict)
            or set(entry) != {"stage", "detail"}
            or not isinstance(entry["stage"], str)
            or not isinstance(entry["detail"], str)
            for entry in trace
        ):
            raise RunError(f"{where}.planner_trace is missing or malformed")
    if version not in (4, 5, 6, 7, 8):
        return
    timings = detail["stage_timings"]
    if version in (5, 6, 7, 8) and observation_policy == "disabled":
        if timings is not None:
            raise RunError(f"{where} disabled observation must be unmeasured, not zero/empty")
        if route in ("lexical", "semantic", "hybrid") and (
            type(detail["request_id"]) is not int or detail["request_id"] <= 0
        ):
            raise RunError(f"{where} unmeasured response still requires a transport request id")
        return
    stages = {
        "lexical": ("prepare", "read_view", "search", "project"),
        "semantic": (
            "prepare",
            "read_view",
            "lexical_scope",
            "embedding",
            "dense_search",
            "project",
        ),
        "hybrid": (
            "prepare",
            "read_view",
            "lexical_search",
            "embedding",
            "dense_fetch",
            "dense_admission",
            "fusion",
        ),
    }.get(route)
    if stages is None or not isinstance(timings, list) or not timings:
        raise RunError(f"{where} stage timings are missing or route is unknown")
    if type(detail["request_id"]) is not int or detail["request_id"] <= 0:
        raise RunError(f"{where} measured timings require a transport request id")
    observed = []
    for index, item in enumerate(timings):
        timing = _exact_keys(
            item,
            {"stage", "elapsed_ns", "calls", "returned_candidates"},
            f"{where}.stage_timings[{index}]",
        )
        stage = timing["stage"]
        if (
            not isinstance(stage, str)
            or stage not in {f"{route}.{name}" for name in stages}
            or stage in observed
            or type(timing["elapsed_ns"]) is not int
            or timing["elapsed_ns"] < 0
            or type(timing["calls"]) is not int
            or timing["calls"] < 1
            or (
                timing["returned_candidates"] is not None
                and (
                    type(timing["returned_candidates"]) is not int
                    or timing["returned_candidates"] < 0
                )
            )
        ):
            raise RunError(f"{where} stage timing is invalid")
        observed.append(stage)
    if observed != [f"{route}.{name}" for name in stages if f"{route}.{name}" in observed]:
        raise RunError(f"{where} stage order is invalid")
    required = {
        "lexical": {"lexical.prepare", "lexical.project"},
        "semantic": {
            "semantic.prepare",
            "semantic.read_view",
            "semantic.embedding",
            "semantic.dense_search",
            "semantic.project",
        },
        "hybrid": {
            "hybrid.prepare",
            "hybrid.read_view",
            "hybrid.embedding",
            "hybrid.dense_admission",
            "hybrid.fusion",
        },
    }[route]
    if not required.issubset(observed):
        raise RunError(f"{where} required stages are absent")
    counts = {item["stage"]: item["returned_candidates"] for item in timings}
    candidate_stages = (
        {"lexical.search", "lexical.project"}
        if route == "lexical"
        else {"semantic.lexical_scope", "semantic.dense_search", "semantic.project"}
        if route == "semantic"
        else {
            "hybrid.lexical_search",
            "hybrid.dense_fetch",
            "hybrid.dense_admission",
            "hybrid.fusion",
        }
    )
    for item in timings:
        if (item["stage"] in candidate_stages) != (item["returned_candidates"] is not None):
            raise RunError(f"{where} stage candidate count is missing or misplaced")
        if item["stage"] != "hybrid.dense_fetch" and item["calls"] != 1:
            raise RunError(f"{where} single-pass stage claims multiple calls")
    if route == "lexical":
        searched = "lexical.search" in counts
        if (
            ("lexical.read_view" in observed) != searched
            or (
                searched
                and (
                    counts["lexical.project"] > counts["lexical.search"]
                    or detail["engines_executed"] != ["lexical"]
                )
            )
            or (not searched and (returned != 0 or detail["engines_executed"] != []))
            or detail["engines_touched"] != (["lexical"] if returned else [])
            or detail["strategy"] != "lexical"
        ):
            raise RunError(f"{where} lexical stage execution contradicts response")
    if route == "semantic":
        scoped = "semantic.lexical_scope" in counts
        expected_executed = (["lexical"] if scoped else []) + ["semantic"]
        expected_touched = (["lexical"] if counts.get("semantic.lexical_scope", 0) else []) + (
            ["semantic"] if counts["semantic.project"] else []
        )
        expected_strategy = (
            "empty" if returned == 0 else "semantic_scoped" if scoped else "semantic"
        )
        if (
            counts["semantic.project"] > counts["semantic.dense_search"]
            or detail["engines_executed"] != expected_executed
            or detail["engines_touched"] != expected_touched
            or detail["strategy"] != expected_strategy
        ):
            raise RunError(f"{where} semantic stage execution contradicts response")
    if route == "hybrid":
        lexical = counts.get("hybrid.lexical_search", 0)
        dense = counts["hybrid.dense_admission"]
        expected_executed = (["lexical"] if "hybrid.lexical_search" in counts else []) + (
            ["semantic"] if "hybrid.dense_fetch" in counts else []
        )
        expected_touched = (["lexical"] if lexical else []) + (["semantic"] if dense else [])
        expected_strategy = (
            "rrf"
            if lexical and dense
            else "lexical_only"
            if lexical
            else "semantic_only"
            if dense
            else "empty"
        )
        if (
            ("hybrid.dense_fetch" in counts) != ("hybrid.lexical_search" in counts)
            or counts["hybrid.fusion"] > lexical + dense
            or ("hybrid.dense_fetch" in counts and dense > counts["hybrid.dense_fetch"])
            or detail["engines_executed"] != expected_executed
            or detail["engines_touched"] != expected_touched
            or detail["strategy"] != expected_strategy
        ):
            raise RunError(f"{where} hybrid stage counts contradict lane execution")
    if timings[-1]["returned_candidates"] != returned:
        raise RunError(f"{where} final stage count differs from returned window")


def _validate_native_span_projection(
    payload: object,
    candidates: list,
    scored: list,
    returned: int,
    where: str,
    *,
    rank_unit: str | None = None,
) -> None:
    projection = _exact_keys(payload, {"policy", "hits"}, f"{where}.native_projection")
    symbol_units = rank_unit == "symbol"
    expected_policy = "symbol-unit-v1" if symbol_units else "first-source-span-v1"
    if (
        projection["policy"] != expected_policy
        or not isinstance(projection["hits"], list)
        or len(projection["hits"]) != returned
        or len(candidates) != len(scored)
        or any(not isinstance(candidate, dict) for candidate in candidates)
    ):
        raise RunError(f"{where} native projection count or policy is invalid")
    spans = {}
    for rank, candidate in enumerate(scored, 1):
        if not isinstance(candidate, dict):
            raise RunError(f"{where} scored source span is malformed")
        span = (candidate.get("path"), candidate.get("start_byte"), candidate.get("end_byte"))
        accounting = candidate.get("span_accounting")
        unit = accounting.get("unit_id") if isinstance(accounting, dict) else None
        if symbol_units and (
            not isinstance(unit, str) or not unit or accounting.get("unit_kind") != "symbol"
        ):
            raise RunError(f"{where} native projection lacks a bound symbol unit")
        identity = (unit, *span) if symbol_units else span
        if (
            not isinstance(span[0], str)
            or type(span[1]) is not int
            or type(span[2]) is not int
            or not 0 <= span[1] < span[2]
            or identity in spans
        ):
            raise RunError(f"{where} scored source span is invalid or duplicated")
        spans[identity] = rank
    seen_units = set()
    first_spans = set()
    for value in projection["hits"]:
        hit = _exact_keys(
            value,
            {"candidate_id", "path", "start_byte", "end_byte", "scored_rank"},
            f"{where} native projection hit",
        )
        unit = hit["candidate_id"]
        if not isinstance(unit, str) or not unit or unit in seen_units:
            raise RunError(f"{where} native projection unit is invalid or duplicated")
        seen_units.add(unit)
        span = (hit["path"], hit["start_byte"], hit["end_byte"])
        identity = (unit, *span) if symbol_units else span
        if (
            not isinstance(span[0], str)
            or type(span[1]) is not int
            or type(span[2]) is not int
            or not 0 <= span[1] < span[2]
            or type(hit["scored_rank"]) is not int
            or spans.get(identity) != hit["scored_rank"]
        ):
            raise RunError(f"{where} native projection omitted or substituted a source span")
        rank = hit["scored_rank"]
        if identity not in first_spans:
            accounting = scored[rank - 1].get("span_accounting")
            if (
                rank != len(first_spans) + 1
                or not isinstance(accounting, dict)
                or accounting.get("unit_id") != unit
                or candidates[rank - 1].get("candidate_id") != unit
            ):
                raise RunError(f"{where} native projection changed first-hit order or identity")
            first_spans.add(identity)
    if len(first_spans) != len(scored):
        raise RunError(f"{where} native projection lacks a scored source span")


def _validate_diagnostic_response_v3(
    row: dict,
    key: tuple[str, str],
    version: int,
    observation_policy: str = "enabled",
    reference: dict | None = None,
    top_k: int | None = None,
) -> dict[str, bool]:
    where = f"retrieval diagnostic response for {key}"
    kind = row["response_kind"]
    response = row["response"]
    if kind == "sdk_failure":
        if (
            response is not None
            or row["error_code"] is None
            or row["error_code"] in {"stale_generation", "empty_non_exhausted_window"}
        ):
            raise RunError(f"{where} SDK failure shape is invalid")
        return {}
    if not isinstance(response, dict):
        raise RunError(f"{where} is malformed")
    if kind == "returned_window":
        fields = {"window", "explanation"}
        projected = version in (6, 7, 8) and "native_projection" in response
        if projected:
            fields.add("native_projection")
        detail = _exact_keys(response, fields, where)
        returned, exhausted, lanes = _typed_window(detail["window"], f"{where}.window")
        if top_k is not None and returned > top_k:
            raise RunError(f"{where} returned count exceeds top_k")
        _validate_explanation(
            detail["explanation"],
            f"{where}.explanation",
            key[1],
            version,
            returned,
            observation_policy,
        )
        if projected:
            if reference is None:
                raise RunError(f"{where} native projection lacks its bound record")
            rank_unit = reference.get("rank_unit")
            if rank_unit == "symbol" and key[1] != "symbol":
                raise RunError(f"{where} symbol projection requires the symbol route")
            _validate_native_span_projection(
                detail["native_projection"],
                row["candidates"],
                reference["candidates"],
                returned,
                where,
                rank_unit=rank_unit,
            )
        elif returned != len(row["candidates"]):
            raise RunError(f"{where} returned count differs from candidates")
        expected_status = (
            "abstained"
            if returned == 0 and exhausted
            else "error"
            if returned == 0
            else "success"
            if exhausted
            else "capped"
        )
        expected_error = "empty_non_exhausted_window" if expected_status == "error" else None
        if row["status"] != expected_status or row["error_code"] != expected_error:
            raise RunError(f"{where} status contradicts typed window")
        return lanes
    if kind == "rejected_response":
        detail = _exact_keys(
            response,
            {
                "window",
                "explanation",
                "observed_hit_count",
                "expected_generation",
                "observed_generation",
            },
            where,
        )
        returned, _exhausted, lanes = _typed_window(detail["window"], f"{where}.window")
        _validate_explanation(
            detail["explanation"],
            f"{where}.explanation",
            key[1],
            version,
            returned,
            observation_policy,
        )
        if (
            detail["observed_hit_count"] != returned
            or row["candidates"]
            or row["status"] != "error"
            or row["error_code"] != "stale_generation"
        ):
            raise RunError(f"{where} rejected response fields are invalid")
        for field in ("expected_generation", "observed_generation"):
            pin = _exact_keys(
                detail[field], {"repo_id", "revision_id", "manifest_generation"}, f"{where}.{field}"
            )
            if (
                not isinstance(pin["repo_id"], str)
                or not isinstance(pin["revision_id"], str)
                or type(pin["manifest_generation"]) is not int
                or pin["manifest_generation"] <= 0
            ):
                raise RunError(f"{where}.{field} is invalid")
        if detail["expected_generation"] == detail["observed_generation"]:
            raise RunError(f"{where} rejected generation pins are equal")
        return lanes
    raise RunError(f"{where} response_kind is invalid")


def server_observation_configuration(policy: object = "enabled") -> dict:
    """Declared default is enabled; arbitrary inherited daemon env is never authority."""
    if policy not in ("enabled", "disabled"):
        raise RunError("query stage observation must be exactly enabled or disabled")
    config = {"query_stages": policy, "scope": "server_query_stage_only_v1"}
    return {**config, "config_sha256": digest(canonical_bytes(config))}


def _validate_server_observation(payload: object) -> dict:
    config = _exact_keys(payload, {"query_stages", "scope", "config_sha256"}, "server observation")
    if config != server_observation_configuration(config["query_stages"]):
        raise RunError("server observation configuration digest/scope mismatch")
    return config


def hybrid_fetch_policy_configuration(floor: object = "100") -> dict:
    """Only bounded experimental selectors; the production default remains 100."""
    if type(floor) is not str or floor not in ("25", "50", "100"):
        raise RunError("experimental hybrid fetch floor must be exactly 25, 50 or 100")
    config = {"floor": int(floor), "scope": "experimental_hybrid_fetch_floor_v1"}
    return {**config, "config_sha256": digest(canonical_bytes(config))}


def _validate_hybrid_fetch_policy(payload: object) -> dict:
    config = _exact_keys(payload, {"floor", "scope", "config_sha256"}, "hybrid fetch policy")
    if type(config["floor"]) is not int or config != hybrid_fetch_policy_configuration(
        str(config["floor"])
    ):
        raise RunError("hybrid fetch policy configuration digest/scope mismatch")
    return config


def rank_study_configuration(
    payload: object, policy: str, routes: list[str], *, speed_claim: bool = False
) -> dict:
    """Explicit diagnostic limits; no inherited env or ranking policy override."""
    config = _exact_keys(
        payload, {"max_files", "max_pages", "timeout_ms"}, "code_search_rank_study"
    )
    if policy not in ("code_search_file", "code_search_exact_content_file") or routes != [
        "lexical"
    ]:
        raise RunError("rank study requires lexical-only ordinary CodeSearch file policy")
    if speed_claim:
        raise RunError(
            "rank-study paging/explanations contaminate whole-process resource measurements; use a separate performance capture"
        )
    for key, maximum in (("max_files", 100_000), ("max_pages", 10_000), ("timeout_ms", 300_000)):
        if type(config[key]) is not int or not 1 <= config[key] <= maximum:
            raise RunError(f"rank study {key} must be an integer in 1..{maximum}")
    return config


def _validate_hybrid_initial_fetch(trace: list[dict], top_k: int, policy: dict) -> None:
    # Independent public-cap/probe invariant, not the producer's self-reported count.
    effective = max(min(max(top_k, policy["floor"]), 10_000), top_k + 1)
    actual = [entry for entry in trace if entry["detail"].startswith("hybrid.internal_top_k=")]
    if actual != [{"stage": "plan", "detail": f"hybrid.internal_top_k={effective}"}]:
        raise RunError("hybrid actual initial fetch contradicts bounded policy/probe")


def ingest_request_identity(spec: dict) -> dict:
    return _validate_ingest_request_identity(
        {
            "repo_id": spec.get("repo_id", "bench-repo"),
            "revision_id": spec.get("revision_id", "bench-rev"),
            "generation": spec.get("generation", 7),
        }
    )


def _validate_ingest_request_identity(payload: object) -> dict:
    identity = _exact_keys(
        payload, {"repo_id", "revision_id", "generation"}, "ingest request identity"
    )
    if any(
        not isinstance(identity[key], str) or not identity[key]
        for key in ("repo_id", "revision_id")
    ):
        raise RunError("ingest request identity has an empty repo/revision")
    if type(identity["generation"]) is not int or not 0 < identity["generation"] < 2**64:
        raise RunError("ingest request identity generation is invalid")
    return identity


def _validate_ingest_diagnostic(
    payload: object,
    record: dict,
    *,
    lexical_stage_contract: bool = False,
    detailed_authority: bool = False,
) -> dict:
    """Bind transient stages to durable receipt/activation bytes, not self-reported totals.

    A fresh capture requires executed stages. Replay/partial/finalize-only are
    valid SDK outcomes but not fresh benchmark measurements. Nested timings
    overlap; activation is another request and must be explicitly unmeasured.
    """
    ingest = _exact_keys(payload, {"receipt", "activation_ack", "observation"}, "ingest diagnostic")
    receipt = _exact_keys(
        ingest["receipt"],
        {
            "generation",
            "manifest_digest",
            "batch_digest",
            "accepted_replace_scopes",
            "accepted_tombstone_scopes",
            "accepted_semantic_replace_scopes",
            "accepted_semantic_tombstone_scopes",
            "accepted_clear_surfaces",
            "sealed",
            "applied",
            "durable_sequence",
            "semantic_content",
        },
        "ingest receipt",
    )
    captures = record.get("captures")
    if not isinstance(captures, dict) or not captures:
        raise RunError("ingest diagnostic requires nonempty captures")
    capture = next(iter(captures.values()))
    if not isinstance(capture, dict) or capture.get("receipt_digest") != digest(
        canonical_bytes(receipt)
    ):
        raise RunError("ingest receipt digest differs from capture")
    ack = _exact_keys(
        ingest["activation_ack"], {"active", "previous_sealed_active"}, "ingest activation ack"
    )
    if capture.get("activation_digest") != digest(canonical_bytes(ack["active"])):
        raise RunError("ingest activation digest differs from capture")
    if any(
        not isinstance(item, dict)
        or any(
            item.get(key) != capture.get(key)
            for key in ("receipt_digest", "activation_digest", "generation")
        )
        for item in captures.values()
    ):
        raise RunError("ingest diagnostic capture bindings diverge")
    active = _exact_keys(ack["active"], {"generation", "activation_token"}, "ingest active head")
    generation = _exact_keys(
        active["generation"],
        {"lexical", "semantic", "semantic_content"},
        "ingest active generation",
    )
    pins = [
        _exact_keys(
            generation[lane],
            {"repo_id", "revision_id", "track", "manifest_generation", "manifest_digest"},
            f"ingest {lane} pin",
        )
        for lane in ("lexical", "semantic")
    ]
    observation = _exact_keys(
        ingest["observation"],
        {
            "request_id",
            "repo_id",
            "revision_id",
            "generation",
            "batch_digest",
            "status",
            "semantic",
            "lexical_build_ns",
            *({"lexical_stages"} if lexical_stage_contract else set()),
            "finalize_ns",
            "activation_ns",
        },
        "ingest observation",
    )
    identity = _validate_ingest_request_identity(
        {key: observation[key] for key in ("repo_id", "revision_id", "generation")}
    )
    expected_pin = {
        "repo_id": identity["repo_id"],
        "revision_id": identity["revision_id"],
        "manifest_generation": identity["generation"],
        "manifest_digest": receipt["manifest_digest"],
    }
    if (
        pins[0] != {**expected_pin, "track": "Lexical"}
        or pins[1] != {**expected_pin, "track": "Semantic"}
        or type(receipt["generation"]) is not int
        or receipt["generation"] != identity["generation"]
        or capture.get("generation") != identity["generation"]
        or observation["batch_digest"] != receipt["batch_digest"]
        or not _is_hex(receipt["batch_digest"], 64)
        or receipt["semantic_content"] != generation["semantic_content"]
        or receipt["semantic_content"] is None
        or receipt["sealed"] is not True
        or receipt["applied"] is not True
        or ack["previous_sealed_active"] is not None
    ):
        raise RunError("ingest observation identity/fresh receipt/activation mismatch")

    def u64(value: object, where: str, positive: bool = False) -> int:
        if type(value) is not int or not (int(positive) <= value < 2**64):
            raise RunError(f"{where} must be an unsigned integer")
        return value

    u64(observation["request_id"], "ingest request_id", True)
    for pin in pins:
        u64(pin["manifest_generation"], "ingest activation pin generation", True)
    u64(capture.get("generation"), "ingest capture generation", True)
    token = _exact_keys(
        active["activation_token"],
        {"root_incarnation", "activation_sequence"},
        "ingest activation token",
    )
    incarnation = token["root_incarnation"]
    if (
        not isinstance(incarnation, list)
        or len(incarnation) != 16
        or any(type(value) is not int or not 0 <= value < 256 for value in incarnation)
        or not any(incarnation)
    ):
        raise RunError("ingest activation root incarnation is invalid")
    u64(token["activation_sequence"], "ingest activation sequence", True)
    if token["activation_sequence"] != 1:
        raise RunError("fresh ingest activation sequence must be one")
    roots = _exact_keys(
        receipt["semantic_content"],
        {"row_root_digest", "membership_root_digest"},
        "ingest semantic content roots",
    )
    if any(
        not isinstance(value, str) or not value.startswith("sha256:") or not _is_hex(value[7:], 64)
        for value in roots.values()
    ):
        raise RunError("ingest semantic content roots are not canonical")
    u64(receipt["durable_sequence"], "ingest durable sequence", True)
    for key in (
        "accepted_replace_scopes",
        "accepted_tombstone_scopes",
        "accepted_semantic_replace_scopes",
        "accepted_semantic_tombstone_scopes",
        "accepted_clear_surfaces",
    ):
        if u64(receipt[key], f"ingest receipt {key}") >= 2**32:
            raise RunError("ingest receipt scope count exceeds u32")
    if not isinstance(receipt["manifest_digest"], str) or not receipt["manifest_digest"]:
        raise RunError("ingest receipt manifest digest is missing")
    if observation["status"] != "executed" or observation["activation_ns"] is not None:
        raise RunError("fresh ingest requires executed status and unmeasured separate activation")
    u64(observation["lexical_build_ns"], "ingest lexical build")
    u64(observation["finalize_ns"], "ingest finalize")
    if lexical_stage_contract:
        stages = _exact_keys(
            observation["lexical_stages"],
            {
                "preparation_ns",
                "writer_mutation_ns",
                "text_authority_ns",
                "file_authority_ns",
                "seal_ns",
                "seal_writer_commit_ns",
                "seal_merge_wait_ns",
                "seal_commitment_ns",
                "seal_file_admission_ns",
            }
            | (
                {
                    "text_authority_collect_ns",
                    "text_authority_shard_build_ns",
                    "text_authority_publish_ns",
                }
                if detailed_authority
                else set()
            ),
            "ingest lexical stages",
        )
        for key, value in stages.items():
            if (
                detailed_authority
                and key.startswith("text_authority_")
                and key != "text_authority_ns"
                and value is None
            ):
                continue
            u64(value, f"ingest lexical stages {key}")
        if detailed_authority:
            collect = stages["text_authority_collect_ns"]
            build = stages["text_authority_shard_build_ns"]
            publish = stages["text_authority_publish_ns"]
            if (
                (build is None) != (publish is None)
                or collect is not None
                and build is None
                or sum(value for value in (collect, build, publish) if value is not None)
                > stages["text_authority_ns"]
            ):
                raise RunError(
                    "ingest text authority child clocks are unavailable or exceed parent"
                )
        outer = sum(
            stages[key]
            for key in (
                "preparation_ns",
                "writer_mutation_ns",
                "text_authority_ns",
                "file_authority_ns",
                "seal_ns",
            )
        )
        nested = sum(
            stages[key]
            for key in ("seal_writer_commit_ns", "seal_merge_wait_ns", "seal_commitment_ns")
        )
        if (
            outer > observation["lexical_build_ns"]
            or nested > stages["seal_ns"]
            or stages["seal_file_admission_ns"] > stages["seal_commitment_ns"]
        ):
            raise RunError("ingest lexical stages exceed their containing interval")
    report = _exact_keys(
        observation["semantic"],
        {
            "owner_scopes",
            "windows",
            "semantic_delete_calls",
            "semantic_delete_commits",
            "membership_delete_calls",
            "membership_delete_commits",
            "semantic_append_calls",
            "membership_append_calls",
            "durations",
        },
        "ingest semantic report",
    )
    for key, value in report.items():
        if key != "durations":
            u64(value, f"ingest {key}")
    durations = _exact_keys(
        report["durations"],
        {
            "total",
            "prepare",
            "promotion",
            "clear_surfaces",
            "stream",
            "semantic_delete",
            "membership_delete",
            "semantic_append",
            "membership_append",
            "tombstones",
            "seal",
            "embedding",
        },
        "ingest durations",
    )
    for key, value in durations.items():
        if key != "embedding" or value is not None:
            u64(value, f"ingest duration {key}")
    if (
        sum(
            durations[key]
            for key in ("prepare", "promotion", "clear_surfaces", "stream", "tombstones", "seal")
        )
        > durations["total"]
        or (durations["embedding"] is not None and durations["embedding"] > durations["stream"])
        or sum(
            durations[key]
            for key in (
                "semantic_delete",
                "membership_delete",
                "semantic_append",
                "membership_append",
            )
        )
        > sum(durations[key] for key in ("clear_surfaces", "stream", "tombstones"))
    ):
        raise RunError("ingest nested durations exceed their containing stages")
    return identity


def validate_retrieval_diagnostic(
    payload: object, record: object, record_sha256: str, pack: object
) -> dict:
    """Reject a partial or unbound returned-window diagnostic.

    This is diagnostic evidence only: it cannot establish relevance or the
    identities of candidates the service did not return.
    """
    fields = {
        "schema_version",
        "kind",
        "record_sha256",
        "query_pack_sha256",
        "top_k",
        "scope",
        "results",
        "runner_timing_detail_ms",
    }
    if isinstance(payload, dict) and payload.get("schema_version") in (5, 6, 7, 8):
        fields.update({"server_observation", "ingest"})
    if isinstance(payload, dict) and payload.get("schema_version") in (6, 7, 8):
        fields.add("hybrid_fetch_policy")
    diagnostic = _exact_keys(
        payload,
        fields,
        "retrieval diagnostic",
    )
    if not isinstance(record, dict) or not isinstance(pack, dict):
        raise RunError("retrieval diagnostic requires record and pack objects")
    contract = record.get("comparison_contract")
    if not isinstance(contract, dict) or not _is_hex(record_sha256, 64):
        raise RunError("retrieval diagnostic requires a valid record contract and digest")
    if (
        type(diagnostic["schema_version"]) is not int
        or diagnostic["schema_version"] not in (2, 3, 4, 5, 6, 7, 8)
        or diagnostic["kind"] != "quanta_returned_window_diagnostic"
        or diagnostic["scope"] != "returned_window_only"
        or diagnostic["record_sha256"] != record_sha256
        or diagnostic["query_pack_sha256"] != record.get("query_pack_sha256")
        or diagnostic["query_pack_sha256"] != digest(canonical_bytes(pack))
        or type(diagnostic["top_k"]) is not int
        or diagnostic["top_k"] != contract.get("top_k")
    ):
        raise RunError("retrieval diagnostic identity or contract mismatch")
    observation_policy = (
        _validate_server_observation(diagnostic["server_observation"])["query_stages"]
        if diagnostic["schema_version"] in (5, 6, 7, 8)
        else "enabled"
    )
    if diagnostic["schema_version"] == 5:
        _validate_ingest_diagnostic(diagnostic["ingest"], record)
    if diagnostic["schema_version"] in (6, 7, 8):
        _validate_hybrid_fetch_policy(diagnostic["hybrid_fetch_policy"])
        _validate_ingest_diagnostic(
            diagnostic["ingest"],
            record,
            lexical_stage_contract=diagnostic["schema_version"] in (7, 8),
            detailed_authority=diagnostic["schema_version"] == 8,
        )
    detail = _exact_keys(
        diagnostic["runner_timing_detail_ms"],
        {
            "clock",
            "daemon_boot_and_readiness",
            "sdk_publish_and_activate_opaque",
            *({"sdk_publish", "sdk_activate"} if diagnostic["schema_version"] in (7, 8) else set()),
            "runner_record_assembly",
            "corpus_reverification",
            "daemon_shutdown",
        },
        "retrieval diagnostic timing",
    )
    if detail["clock"] != "runner_monotonic_wall_v1" or any(
        not is_finite_json_number(value) or value < 0
        for key, value in detail.items()
        if key != "clock"
    ):
        raise RunError("retrieval diagnostic timing is invalid")
    if diagnostic["schema_version"] in (7, 8) and (
        detail["sdk_publish"] + detail["sdk_activate"]
        > detail["sdk_publish_and_activate_opaque"] + 0.01
    ):
        raise RunError("retrieval diagnostic SDK children exceed publish/activate interval")
    record_results = record.get("results")
    pack_tasks = pack.get("tasks")
    provenance = record.get("route_provenance")
    if (
        not isinstance(record_results, list)
        or not isinstance(pack_tasks, list)
        or not isinstance(provenance, dict)
        or not isinstance(diagnostic["results"], list)
    ):
        raise RunError("retrieval diagnostic inputs have invalid result shape")
    tasks = {}
    for task in pack_tasks:
        if not isinstance(task, dict) or not isinstance(task.get("task_id"), str):
            raise RunError("retrieval diagnostic pack task is malformed")
        task_id = task["task_id"]
        if not task_id or task_id in tasks or not _is_hex(task.get("query_sha256"), 64):
            raise RunError("retrieval diagnostic pack tasks are duplicated or malformed")
        tasks[task_id] = task
    expected = {}
    for result in record_results:
        if not isinstance(result, dict):
            raise RunError("retrieval diagnostic record result is malformed")
        key = (result.get("task_id"), result.get("route"))
        if (
            not isinstance(key[0], str)
            or not isinstance(key[1], str)
            or key in expected
            or key[0] not in tasks
            or key[1] not in provenance
            or not isinstance(result.get("candidates"), list)
        ):
            raise RunError("retrieval diagnostic record result is duplicated or unknown")
        expected[key] = result
    if len(expected) != len(tasks) * len(provenance):
        raise RunError("retrieval diagnostic record results are incomplete")
    seen = set()
    seen_request_ids = set()
    for row in diagnostic["results"]:
        row_fields = {
            "task_id",
            "query_sha256",
            "route",
            "status",
            "error_code",
            "candidates",
            "response",
        }
        if diagnostic["schema_version"] in (3, 4, 5, 6, 7, 8):
            row_fields.add("response_kind")
        row = _exact_keys(
            row,
            row_fields,
            "retrieval diagnostic result",
        )
        if not isinstance(row["task_id"], str) or not isinstance(row["route"], str):
            raise RunError("retrieval diagnostic result has invalid identity")
        key = (row["task_id"], row["route"])
        if key in seen or key not in expected:
            raise RunError("retrieval diagnostic result is duplicated or unexpected")
        seen.add(key)
        reference = expected[key]
        if (
            row["query_sha256"] != tasks[key[0]].get("query_sha256")
            or row["status"] != reference.get("status")
            or not isinstance(row["candidates"], list)
            or len(row["candidates"]) != len(reference["candidates"])
            or len(row["candidates"]) > diagnostic["top_k"]
            or row["error_code"]
            != (
                reference.get("error", {}).get("code")
                if isinstance(reference.get("error"), dict)
                else None
            )
        ):
            raise RunError("retrieval diagnostic differs from runner record")
        if diagnostic["schema_version"] == 2:
            _validate_diagnostic_response_v2(row["response"], row["error_code"], key)
            lane_execution: dict[str, bool] = {}
        else:
            lane_execution = _validate_diagnostic_response_v3(
                row,
                key,
                diagnostic["schema_version"],
                observation_policy,
                reference,
                diagnostic["top_k"],
            )
            if (
                diagnostic["schema_version"] in (6, 7, 8)
                and key[1] == "hybrid"
                and row["response_kind"] == "returned_window"
            ):
                trace = row["response"]["explanation"]["planner_trace"]
                _validate_hybrid_initial_fetch(
                    trace, diagnostic["top_k"], diagnostic["hybrid_fetch_policy"]
                )
            if (
                diagnostic["schema_version"] in (4, 5, 6, 7, 8)
                and key[1] in ("lexical", "semantic", "hybrid")
                and row["response_kind"] != "sdk_failure"
            ):
                request_id = row["response"]["explanation"]["request_id"]
                if request_id in seen_request_ids:
                    raise RunError("retrieval diagnostic reuses a transport request id")
                seen_request_ids.add(request_id)
            if row["response_kind"] == "rejected_response":
                captures = record.get("captures")
                route_owner = provenance.get(key[1])
                capture_id = (
                    route_owner.get("capture_id") if isinstance(route_owner, dict) else None
                )
                capture = captures.get(capture_id) if isinstance(captures, dict) else None
                expected_pin = row["response"]["expected_generation"]
                if (
                    not isinstance(capture, dict)
                    or type(capture.get("generation")) is not int
                    or expected_pin["manifest_generation"] != capture["generation"]
                ):
                    raise RunError(
                        "retrieval diagnostic rejected response is not bound to capture generation"
                    )
        for position, (candidate, scored) in enumerate(
            zip(row["candidates"], reference["candidates"]), 1
        ):
            if not isinstance(scored, dict):
                raise RunError("retrieval diagnostic reference candidate is malformed")
            candidate = _exact_keys(
                candidate,
                {
                    "rank",
                    "candidate_id",
                    "path",
                    "start_line",
                    "end_line",
                    "score",
                    "contributions",
                },
                "retrieval diagnostic candidate",
            )
            if (
                type(candidate["rank"]) is not int
                or candidate["rank"] != position
                or candidate["rank"] != scored.get("rank")
                or type(candidate["path"]) is not str
                or candidate["path"] != scored.get("path")
                or type(candidate["start_line"]) is not int
                or candidate["start_line"] < 1
                or candidate["start_line"] != scored.get("start_line")
                or type(candidate["end_line"]) is not int
                or candidate["end_line"] < candidate["start_line"]
                or candidate["end_line"] != scored.get("end_line")
                or not isinstance(candidate["candidate_id"], str)
                or not candidate["candidate_id"]
                or not is_finite_json_number(candidate["score"])
                or not isinstance(candidate["contributions"], list)
            ):
                raise RunError("retrieval diagnostic candidate is invalid")
            lanes = candidate["contributions"]
            if (key[1] == "hybrid") != bool(lanes) or len(lanes) > 2:
                raise RunError("retrieval diagnostic lane provenance is missing or misplaced")
            seen_lanes = set()
            for lane in lanes:
                lane = _exact_keys(lane, {"lane", "rank", "raw_score"}, "retrieval lane")
                if (
                    lane["lane"] not in ("lexical", "dense")
                    or lane["lane"] in seen_lanes
                    or type(lane["rank"]) is not int
                    or lane["rank"] < 1
                    or not is_finite_json_number(lane["raw_score"])
                ):
                    raise RunError("retrieval diagnostic lane is invalid")
                if diagnostic["schema_version"] in (3, 4, 5, 6, 7, 8) and not (
                    lane_execution.get(lane["lane"], False)
                    or lane_execution.get(f"hybrid.{lane['lane']}", False)
                ):
                    raise RunError(
                        "retrieval diagnostic contribution names a lane that did not execute"
                    )
                seen_lanes.add(lane["lane"])
            if len(lanes) == 2 and [lane["lane"] for lane in lanes] != ["lexical", "dense"]:
                raise RunError("retrieval diagnostic lane order is invalid")
    if seen != set(expected):
        raise RunError("retrieval diagnostic results are incomplete")
    return diagnostic


def run_quanta(spec: dict, _spec_dir: Path) -> int:
    """Run the Rust SDK runner once per strategy. Returns process exit code."""
    if "_query_protocol" not in spec and _int(spec.get("repetitions", 1), "spec.repetitions") != 1:
        raise RunError("direct quanta capture supports one fresh root; use pair for repetitions")
    out_root = preflight_capture(spec)
    if out_root.exists():
        raise RunError(f"output root already exists (refusing reuse): {out_root}")
    runner_bin = spec.get("runner_binary")
    if not runner_bin or not Path(runner_bin).is_file():
        raise RunError("spec.runner_binary must name a built Rust runner binary")
    try:
        runner_binary_sha256 = sha_file(Path(runner_bin))
    except OSError as exc:
        raise RunError(f"cannot hash Rust runner binary: {exc}") from exc
    strategies = spec.get("strategies")
    if not isinstance(strategies, list) or not strategies:
        raise RunError("spec.strategies must be a nonempty list")
    preflight_daemon_socket_paths(out_root, strategies)
    out_root.mkdir(parents=True)
    if not spec.get("searchd_binary") or not _is_hex(spec.get("searchd_expected_sha256"), 64):
        raise RunError("spec must pin searchd_binary with searchd_expected_sha256")
    routes = spec.get("routes", ["lexical", "semantic", "hybrid"])
    if spec.get("blinding", "attested") == "isolated" and "_isolation" not in spec:
        raise RunError(
            "direct quanta capture cannot self-assert isolation; use pair with "
            "suite_secret_root so the driver can prove and enforce the boundary"
        )
    pack_path = write_projected_pack(
        Path(spec["query_pack"]),
        Path(spec["suite"]),
        routes,
        out_root / "quanta-pack.json",
    )
    capture_spec = dict(spec)
    task_ids = [task["task_id"] for task in read_json(pack_path)["tasks"]]
    if "_query_protocol" not in capture_spec and any(
        field in spec for field in ("query_warmup_passes", "query_repetitions_per_root")
    ):
        protocol = build_query_protocol(
            task_ids,
            _int(spec.get("seed", 0), "spec.seed"),
            _int(spec.get("query_warmup_passes", 0), "spec.query_warmup_passes"),
            _int(spec.get("query_repetitions_per_root", 1), "spec.query_repetitions_per_root"),
        )
        protocol_path = out_root / "query-protocol.json"
        protocol_path.write_bytes(canonical_bytes(protocol))
        capture_spec["_query_protocol"] = str(protocol_path)
    _requested_quanta_query_protocol(capture_spec, task_ids)
    runs = []
    for index, strategy in enumerate(strategies):
        runs.append(
            run_quanta_strategy(
                capture_spec, strategy, index, out_root, routes, pack_path, runner_binary_sha256
            )
        )
        if sha_file(Path(runner_bin)) != runner_binary_sha256:
            raise RunError("Rust runner binary changed during capture")
    (out_root / "quanta-manifest.json").write_text(
        json.dumps({"runs": runs}, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    print(json.dumps({"runs": len(runs), "output_root": str(out_root)}, indent=2))
    return 0


def _requested_quanta_query_protocol(spec: dict, task_ids: list[str]) -> dict | None:
    if "_query_protocol" not in spec:
        return None
    protocol = validate_query_protocol(
        read_json(Path(spec["_query_protocol"])), task_ids, "requested Quanta query protocol"
    )
    for field, schedule in (
        ("query_warmup_passes", "warmup_schedules"),
        ("query_repetitions_per_root", "measurement_schedules"),
    ):
        if field in spec and len(protocol[schedule]) != _int(spec[field], "spec." + field):
            raise RunError("requested Quanta query protocol differs from spec." + field)
    return protocol


def _validate_quanta_query_protocol_execution(expected: dict | None, phase: dict) -> None:
    if expected is not None and (
        phase.get("query_protocol") != expected
        or phase.get("warmup_passes") != len(expected["warmup_schedules"])
        or phase.get("measurement_repetitions") != len(expected["measurement_schedules"])
    ):
        raise RunError("Rust runner query protocol differs from requested schedule")


def run_quanta_strategy(
    spec: dict,
    strategy: dict,
    index: int,
    out_root: Path,
    routes: list[str],
    pack_path: Path,
    runner_binary_sha256: str,
) -> dict:
    name = strategy.get("name")
    if name not in RUNNABLE_STRATEGIES:
        raise RunError(f"unknown strategy: {name}")
    requested_protocol = (
        _requested_quanta_query_protocol(
            spec, [task["task_id"] for task in read_json(pack_path)["tasks"]]
        )
        if "_query_protocol" in spec
        else None
    )
    protocol_bytes = (
        Path(spec["_query_protocol"]).read_bytes() if requested_protocol is not None else None
    )
    out_abs = out_root.resolve()
    run_dir = out_root / _strategy_run_directory(index, name)
    run_dir.mkdir(parents=True)
    state_root = (run_dir / "state").resolve()
    record_path = (run_dir / "record.json").resolve()
    phase_path = (run_dir / "phase-metrics.json").resolve()
    diagnostic_path = (run_dir / "retrieval-diagnostic.json").resolve()
    rank_study_path = (run_dir / "code-search-rank-study.json").resolve()
    resource_path = (run_dir / "resource-metrics.json").resolve()
    refusal_path = (run_dir / "query-plan-refusal.json").resolve()
    preflight_path = (run_dir / "symbol-preflight.json").resolve()
    command = [
        spec["runner_binary"],
        "run",
        "--repo",
        spec["repo"],
        "--manifest",
        spec["manifest"],
        "--query-pack",
        str(pack_path),
        "--query-input-policy",
        spec["execution_profiles"]["quanta"]["policy"],
        "--query-stage-observation",
        server_observation_configuration(spec.get("query_stage_observation", "enabled"))[
            "query_stages"
        ],
        "--experimental-hybrid-fetch-floor",
        str(
            hybrid_fetch_policy_configuration(spec.get("experimental_hybrid_fetch_floor", "100"))[
                "floor"
            ]
        ),
        "--strategy",
        name,
        "--routes",
        ",".join(routes),
        "--top-k",
        str(spec["top_k"]),
        "--state-root",
        str(state_root),
        "--repo-id",
        spec.get("repo_id", "bench-repo"),
        "--revision-id",
        spec.get("revision_id", "bench-rev"),
        "--generation",
        str(spec.get("generation", 7)),
        "--embedder",
        spec.get("embedder", "potion-code"),
        "--runner-name",
        spec.get("runner_name", "quanta-sdk-runner"),
        "--runner-revision",
        f"sha256:{runner_binary_sha256}",
        "--run-id",
        f"{spec.get('run_id', 'run')}-{name}",
        "--blinding",
        spec.get("blinding", "attested"),
        "--isolation-method",
        spec.get("isolation_method", "attested-only: same-checkout pack consumer"),
        "--access-block-log",
        spec.get(
            "access_block_log",
            "attested-only: no suite path is passed to the runner; pack blindness verified by freeze",
        ),
        "--symbol-preflight-out",
        str(preflight_path),
        "--symbol-coverage",
        spec.get("symbol_coverage_policy", "require-complete"),
        "--metrics-out",
        str(phase_path),
        "--diagnostics-out",
        str(diagnostic_path),
        "--refusal-out",
        str(refusal_path),
        "--out",
        str(record_path),
    ]
    quanta_profile = spec["execution_profiles"]["quanta"]
    if (
        quanta_profile["policy"] in ("natural_language", "natural_language_file")
        and quanta_profile["config"]["max_tokens"] != qp.DEFAULT_NL_CONFIG["max_tokens"]
    ):
        command += ["--nl-max-tokens", str(quanta_profile["config"]["max_tokens"])]
    if "_query_protocol" in spec:
        command += ["--query-protocol", spec["_query_protocol"]]
    if "code_search_rank_study" in spec:
        study_limits = rank_study_configuration(
            spec["code_search_rank_study"],
            spec["execution_profiles"]["quanta"]["policy"],
            routes,
            speed_claim=spec.get("claims", {}).get("speed") is True,
        )
        command += ["--rank-study-out", str(rank_study_path)]
        for key, flag in (
            ("max_files", "max-files"),
            ("max_pages", "max-pages"),
            ("timeout_ms", "timeout-ms"),
        ):
            command += [f"--rank-study-{flag}", str(study_limits[key])]
    command += ["--searchd-bin", spec["searchd_binary"]]
    command += ["--searchd-expected-sha256", spec["searchd_expected_sha256"]]
    if "io_timeout_secs" in spec:
        command += ["--io-timeout-secs", str(spec["io_timeout_secs"])]
    if "symbol_total_timeout_ms" in spec:
        command += ["--symbol-total-timeout-ms", str(spec["symbol_total_timeout_ms"])]
    materialized = spec.get("_materialized_corpus")
    if materialized is not None:
        command += ["--materialized-corpus-sha256", materialized["proof_sha256"]]
    if "quanta_model_dir" in spec:
        command += ["--model-dir", spec["quanta_model_dir"]]
    for key, flag in (
        ("window_bytes", "--window-bytes"),
        ("overlap_bytes", "--overlap-bytes"),
        ("max_item_bytes", "--max-item-bytes"),
    ):
        if key in strategy:
            command += [flag, str(strategy[key])]
    command, isolation = sandbox_command(spec, command)
    process_env = capture_process_env(run_dir / "process-tmp")
    resource = run_monitored_process(
        command,
        stdout_path=run_dir / "runner.stdout.log",
        stderr_path=run_dir / "runner.stderr.log",
        resource_path=resource_path,
        timeout_secs=_int(spec.get("timeout_secs", 1800), "spec.timeout_secs"),
        subject_path=record_path,
        env=process_env,
        cwd=process_env["TMPDIR"],
        isolation=isolation,
        capture_scope=spec.get("scope", "exploratory"),
        linux_cgroup_parent=spec.get("linux_cgroup_parent"),
        linux_cgroup_parent_identity=spec.get("_linux_cgroup_parent_identity"),
    )
    if resource["timed_out"]:
        write_process_failure(
            run_dir,
            system="quanta",
            strategy=name,
            failure_type="timeout",
            resource_path=resource_path,
            stderr_path=run_dir / "runner.stderr.log",
            record_path=record_path,
            refusal_path=refusal_path,
        )
        raise RunError(f"Rust runner timed out for {name}")
    if resource["exit_code"] != 0:
        write_process_failure(
            run_dir,
            system="quanta",
            strategy=name,
            failure_type="nonzero_exit",
            resource_path=resource_path,
            stderr_path=run_dir / "runner.stderr.log",
            record_path=record_path,
            refusal_path=refusal_path,
        )
        stderr_tail = (run_dir / "runner.stderr.log").read_text(encoding="utf-8", errors="replace")[
            -2000:
        ]
        raise RunError(
            f"Rust runner failed for {name} (exit {resource['exit_code']}): {stderr_tail}"
        )
    if not phase_path.is_file():
        write_process_failure(
            run_dir,
            system="quanta",
            strategy=name,
            failure_type="missing_phase_metrics",
            resource_path=resource_path,
            stderr_path=run_dir / "runner.stderr.log",
            record_path=record_path,
            refusal_path=refusal_path,
        )
        raise RunError(f"Rust runner omitted phase metrics for {name}")
    if not diagnostic_path.is_file():
        raise RunError(f"Rust runner omitted retrieval diagnostics for {name}")
    diagnostic = validate_retrieval_diagnostic(
        read_json(diagnostic_path),
        read_json(record_path),
        sha_file(record_path),
        read_json(pack_path),
    )
    if diagnostic["schema_version"] != 8 or diagnostic[
        "server_observation"
    ] != server_observation_configuration(spec.get("query_stage_observation", "enabled")):
        raise RunError(
            "current capture omitted or contradicted the actual server observation policy"
        )
    if diagnostic["hybrid_fetch_policy"] != hybrid_fetch_policy_configuration(
        spec.get("experimental_hybrid_fetch_floor", "100")
    ):
        raise RunError("captured hybrid fetch floor differs from explicit spec policy")
    if _validate_ingest_diagnostic(
        diagnostic["ingest"],
        read_json(record_path),
        lexical_stage_contract=True,
        detailed_authority=True,
    ) != ingest_request_identity(spec):
        raise RunError("captured ingest identity differs from requested batch scope")
    index_bytes = tree_size(state_root)
    phase = _validate_phase_metrics(read_json(phase_path), f"Rust runner phase metrics for {name}")
    if phase["schema_version"] != 4:
        raise RunError("current Rust runner omitted SDK child clock phase schema v4")
    _validate_quanta_query_protocol_execution(requested_protocol, phase)
    if protocol_bytes is not None and Path(spec["_query_protocol"]).read_bytes() != protocol_bytes:
        raise RunError("Quanta query protocol changed during capture")
    if phase["symbol_coverage_policy"] != spec.get("symbol_coverage_policy", "require-complete"):
        raise RunError("Rust runner symbol coverage policy differs from the requested profile")
    try:
        symbol_coverage.verify_artifact(
            phase,
            phase_path,
            read_json(Path(spec["manifest"])),
            expected_timeout_total_ms=spec.get("symbol_total_timeout_ms", 120_000),
        )
    except (ValueError, OSError, KeyError, TypeError) as exc:
        raise RunError(f"Rust runner preflight evidence is invalid: {exc}") from exc
    model_dir = Path(spec["quanta_model_dir"]) if "quanta_model_dir" in spec else None
    bind_storage_metrics(
        resource_path,
        {
            "index_bytes": index_bytes,
            "model_cache_bytes": tree_size(model_dir) if model_dir is not None else 0,
            "parser_cache_bytes": 0,
            "embedding_cache_bytes": 0,
            "discovered_files": phase.get("file_count"),
            "indexed_chunks": phase.get("chunk_count"),
            "index_storage": "disk",
            "index_measurement": "filesystem_tree_v1",
        },
    )
    result = {
        "strategy": name,
        "strategy_config": strategy,
        # Rename-safe: paths stay relative to the quanta output root so a
        # staged tree can be atomically promoted without rebinding.
        "record": record_path.relative_to(out_abs).as_posix(),
        "record_digest": sha_file(record_path),
        "runner_binary_sha256": runner_binary_sha256,
        "driver_ms": resource["elapsed_ms"],
        "index_bytes": index_bytes,
        "symbol_preflight": preflight_path.relative_to(out_abs).as_posix(),
        "symbol_preflight_digest": sha_file(preflight_path),
        "phase_metrics": phase_path.relative_to(out_abs).as_posix(),
        "phase_metrics_digest": sha_file(phase_path),
        "retrieval_diagnostic": diagnostic_path.relative_to(out_abs).as_posix(),
        "retrieval_diagnostic_digest": sha_file(diagnostic_path),
        "resource_metrics": resource_path.relative_to(out_abs).as_posix(),
        "resource_metrics_digest": sha_file(resource_path),
        "state_root": state_root.relative_to(out_abs).as_posix(),
    }
    if "code_search_rank_study" in spec:
        # Optional diagnostics cannot discard the valid original quality record.
        # Failed validation stays explicit and the original artifact is retained.
        study_summary = {"status": "failed", "qualification": "diagnostic_unqualified"}
        if rank_study_path.is_file():
            study_summary.update(
                {
                    "artifact": rank_study_path.relative_to(out_abs).as_posix(),
                    "sha256": sha_file(rank_study_path),
                }
            )
            try:
                rows = code_search_rank_study.validate_artifact(
                    read_json(rank_study_path),
                    read_json(record_path),
                    sha_file(record_path),
                    read_json(pack_path),
                )
                study_summary.update(
                    {
                        "status": "verified",
                        "complete_pools": sum(
                            row["collection"]["status"] == "returned" for row in rows.values()
                        ),
                        "task_count": len(rows),
                    }
                )
            except (ValueError, KeyError, TypeError) as error:
                study_summary["reason"] = f"rank_study_validation: {error}"
        else:
            study_summary["reason"] = "rank_study_artifact_missing"
        result["code_search_rank_study"] = study_summary
    return result


def cmd_verdict(args: argparse.Namespace) -> int:
    try:
        verdict = build_verdict(Path(args.repo), Path(args.suite), Path(args.run_manifest))
    except (RunError, ValueError) as exc:
        print(f"ERROR: {exc}", file=sys.stderr)
        return 2
    Path(args.out).write_text(
        json.dumps(verdict, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    print(json.dumps(verdict["states"], indent=2, sort_keys=True))
    return 0


def _resolve_artifact(root: Path, ref: object, where: str) -> Path:
    if not isinstance(ref, str) or not ref:
        raise RunError(f"{where} must be a nonempty relative path")
    candidate = Path(ref)
    if candidate.is_absolute() or ".." in candidate.parts:
        raise RunError(f"{where} escapes the manifest directory: {ref}")
    resolved = (root / candidate).resolve()
    if resolved != root.resolve() and root.resolve() not in resolved.parents:
        raise RunError(f"{where} escapes the manifest directory: {ref}")
    if not resolved.is_file():
        raise RunError(f"{where} artifact is missing: {ref}")
    return resolved


def _validate_manifest_shape(payload: object) -> dict:
    manifest = _exact_keys(
        payload,
        {
            "manifest_version",
            "blinding",
            "isolation_method",
            "access_block_log",
            "scope",
            "claims",
            "repetitions",
            "evidence",
            "host",
            "artifacts",
            "provenance",
        },
        "run manifest",
    )
    if manifest["manifest_version"] != MANIFEST_VERSION:
        raise RunError("run manifest version mismatch")
    if manifest["blinding"] not in ("isolated", "attested"):
        raise RunError("run manifest blinding must be isolated or attested")
    for key in ("isolation_method", "access_block_log"):
        if not isinstance(manifest[key], str) or not manifest[key]:
            raise RunError(f"run manifest {key} must be a nonempty string")
    if manifest["blinding"] == "isolated" and (
        manifest["isolation_method"] not in (MACOS_ISOLATION_BACKEND, LINUX_ISOLATION_BACKEND)
        or not re.fullmatch(r"sha256:[0-9a-f]{64}", manifest["access_block_log"])
    ):
        raise RunError("isolated run manifest lacks the current tagged backend proof binding")
    if manifest["scope"] not in ("exploratory", "qualified"):
        raise RunError("run manifest scope must be exploratory or qualified")
    claims = _exact_keys(
        manifest["claims"],
        {"quality", "speed", "same_model", "incremental"},
        "run manifest claims",
    )
    for key, value in claims.items():
        if type(value) is not bool:
            raise RunError(f"run manifest claims.{key} must be a strict boolean")
    repetitions = manifest["repetitions"]
    if type(repetitions) is not int or repetitions < 1:
        raise RunError("run manifest repetitions must be an integer >= 1")
    evidence = manifest["evidence"]
    if not isinstance(evidence, dict):
        raise RunError("run manifest evidence must be an object")
    if (
        not {"pair", "perf"}
        <= set(evidence)
        <= {"pair", "perf", "contract_suites", "sdk_path", "model_parity", "incremental"}
    ):
        raise RunError("run manifest evidence holds missing/unknown keys")
    pair = _exact_keys(evidence["pair"], {"mapping_proof_digest"}, "manifest pair evidence")
    if not _is_hex(pair["mapping_proof_digest"], 64):
        raise RunError("manifest pair mapping_proof_digest must be a lowercase sha256")
    perf = _exact_keys(
        evidence["perf"],
        {"observations_floor", "fresh_roots", "phase_boundaries", "resource_accounting"},
        "manifest perf evidence",
    )
    for key in ("observations_floor", "fresh_roots"):
        if type(perf[key]) is not int or perf[key] < 0:
            raise RunError(f"manifest perf {key} must be an integer >= 0")
    for key in ("phase_boundaries", "resource_accounting"):
        if type(perf[key]) is not bool:
            raise RunError(f"manifest perf {key} must be a strict boolean")
    if "contract_suites" in evidence:
        suites = _exact_keys(
            evidence["contract_suites"], {"python", "rust"}, "manifest contract evidence"
        )
        for side in ("python", "rust"):
            claim = _exact_keys(
                suites[side],
                {"test_result_digest", "raw_evidence_digest", "inventory_digest"},
                f"manifest {side} claim",
            )
            for key in ("test_result_digest", "raw_evidence_digest", "inventory_digest"):
                if not _is_hex(claim[key], 64):
                    raise RunError(f"manifest {side} {key} must be a lowercase sha256")
    if "sdk_path" in evidence:
        sdk = _exact_keys(
            evidence["sdk_path"],
            {
                "test_result_digest",
                "separate_process",
                "sealed_receipt",
                "activation_ack",
                "empty_check",
                "nextest_digest",
                "runner_record_digest",
                "inventory_digest",
            },
            "manifest sdk evidence",
        )
        for key in (
            "test_result_digest",
            "nextest_digest",
            "runner_record_digest",
            "inventory_digest",
        ):
            if not _is_hex(sdk[key], 64):
                raise RunError(f"manifest sdk {key} must be a lowercase sha256")
        for key in ("separate_process", "sealed_receipt", "activation_ack", "empty_check"):
            if type(sdk[key]) is not bool:
                raise RunError(f"manifest sdk {key} must be a strict boolean")
    for key in ("model_parity", "incremental"):
        if key in evidence:
            claim = _exact_keys(evidence[key], {"test_result_digest"}, f"manifest {key} claim")
            if not _is_hex(claim["test_result_digest"], 64):
                raise RunError(f"manifest {key} test_result_digest must be a lowercase sha256")
    host_keys = {"start_digest", "end_digest", "cache_regime"}
    if isinstance(manifest["host"], dict) and "timeline_digest" in manifest["host"]:
        host_keys.add("timeline_digest")
    if isinstance(manifest["host"], dict) and "delegated_cgroup_parent" in manifest["host"]:
        host_keys.add("delegated_cgroup_parent")
    host = _exact_keys(manifest["host"], host_keys, "manifest host")
    for key in ("start_digest", "end_digest"):
        if not _is_hex(host[key], 64):
            raise RunError(f"manifest host {key} must be a lowercase sha256")
    if "timeline_digest" in host and not _is_hex(host["timeline_digest"], 64):
        raise RunError("manifest host timeline_digest must be a lowercase sha256")
    if host["cache_regime"] not in ("true_process_cold", "warm_cache", "undeclared"):
        raise RunError("manifest host cache_regime must be a frozen regime")
    if "delegated_cgroup_parent" in host:
        if manifest["scope"] != "qualified":
            raise RunError("exploratory manifest cannot claim a delegated cgroup parent")
        _validate_cgroup_parent_identity(
            host["delegated_cgroup_parent"], "manifest host.delegated_cgroup_parent"
        )
    artifacts = manifest["artifacts"]
    if not isinstance(artifacts, dict):
        raise RunError("run manifest artifacts must be an object")
    required_artifacts = {
        "suite",
        "query_pack",
        "corpus_manifest",
        "mapping_proof",
        "latency_matrix",
        "host_start",
        "host_end",
        "records",
        "host_profile",
        "reports",
        "quanta_manifests",
        "semble_adapter_manifest",
        "semble_lockfile",
        "semble_native",
        "semble_model_cache_manifests",
        "phase_metrics",
        "phase_metrics_digests",
        "symbol_preflights",
        "resource_metrics",
        "protocol_lock",
        "driver_source_closure",
    }
    admission_common = {
        "admission_manifest",
        "license_receipt",
        "annotation_receipts",
        "adjudication_receipt",
    }
    local_admission = admission_common | {"experiment_custody", "development_suite"}
    disjoint_admission = admission_common | {"split_manifest", "split_releases"}
    admission_artifacts = local_admission | disjoint_admission
    optional_artifacts = (
        set(RECEIPT_KEYS)
        | {"isolation_proof", "host_timeline", "host_timeline_raw"}
        | admission_artifacts
    )
    if ("host_timeline" in artifacts) != ("host_timeline_raw" in artifacts):
        raise RunError("manifest host timeline must bind the canonical monitor raw")
    if ("host_timeline" in artifacts) != ("timeline_digest" in host):
        raise RunError("manifest host timeline artifact and digest must be paired")
    if "driver_source_closure" not in artifacts:
        raise RunError("run manifest lacks the driver source closure")
    if not required_artifacts <= set(artifacts) <= required_artifacts | optional_artifacts:
        raise RunError("run manifest artifacts hold missing/unknown keys")
    present_admission = set(artifacts).intersection(admission_artifacts)
    if manifest["scope"] == "qualified" and present_admission not in (
        local_admission,
        disjoint_admission,
    ):
        raise RunError("qualified run manifest lacks the complete admission bundle")
    if manifest["scope"] != "qualified" and present_admission:
        raise RunError("exploratory run manifest carries qualification admission artifacts")
    for key in required_artifacts | optional_artifacts:
        if key not in artifacts:
            continue
        value = artifacts[key]
        if key == "phase_metrics_digests":
            phases = artifacts["phase_metrics"]
            if (
                not isinstance(value, dict)
                or not isinstance(phases, list)
                or any(not isinstance(ref, str) for ref in phases)
                or set(value) != set(phases)
                or len(value) != len(phases)
                or any(not _is_hex(sha, 64) for sha in value.values())
            ):
                raise RunError("manifest phase_metrics_digests must bind exactly every phase path")
        elif key in (
            "records",
            "reports",
            "quanta_manifests",
            "semble_native",
            "semble_model_cache_manifests",
            "phase_metrics",
            "symbol_preflights",
            "resource_metrics",
            "annotation_receipts",
        ):
            if not isinstance(value, list) or not all(
                isinstance(ref, str) and ref for ref in value
            ):
                raise RunError(f"manifest artifacts.{key} must be a path list")
        elif not isinstance(value, str) or not value:
            raise RunError(f"manifest artifacts.{key} must be a nonempty path")
    if not artifacts["records"]:
        raise RunError("run manifest artifacts.records must be nonempty")
    provenance = _exact_keys(
        manifest["provenance"],
        {"admission", "quanta", "semble", "corpus", "suite", "host"},
        "run manifest provenance",
    )
    admission_prov = _exact_keys(
        provenance["admission"], {"manifest_digest"}, "manifest admission provenance"
    )
    admission_digest = admission_prov["manifest_digest"]
    if manifest["scope"] == "qualified":
        if not _is_hex(admission_digest, 64):
            raise RunError("qualified manifest admission digest must be a sha256")
    elif admission_digest is not None:
        raise RunError("exploratory manifest admission digest must be null")
    quanta_fields = {"source_sha", "source_closure_digest", "binary_digest", "embedder"}
    if (
        isinstance(provenance["quanta"], dict)
        and "binary_build_source_revision" in provenance["quanta"]
    ):
        quanta_fields.add("binary_build_source_revision")
    quanta = _exact_keys(provenance["quanta"], quanta_fields, "manifest quanta")
    if quanta.get("binary_build_source_revision") is not None:
        raise RunError("binary build source revision is not attested by a build receipt")
    if not _is_hex(quanta["source_sha"], 40) or not _is_hex(quanta["binary_digest"], 64):
        raise RunError("manifest quanta provenance digests malformed")
    if quanta["embedder"] not in ("potion-code", "potion-code-full-v2", "hash-dev"):
        raise RunError("manifest quanta embedder must be a frozen embedder")
    if quanta["embedder"] == "potion-code-full-v2" and (
        manifest["scope"] != "exploratory" or any(claims.values())
    ):
        raise RunError("potion-code-full-v2 manifest is exploratory diagnostic only")
    if not _is_hex(quanta["source_closure_digest"], 64):
        raise RunError("manifest driver source closure digest is malformed")
    semble = _exact_keys(
        provenance["semble"],
        {"revision", "lockfile_digest", "interpreter_digest", "model_asset_digest"},
        "manifest semble",
    )
    if semble["revision"] != SEMBLE_PINNED_VERSION:
        raise RunError("manifest semble revision is not the pinned release")
    for key in ("lockfile_digest", "interpreter_digest", "model_asset_digest"):
        if not _is_hex(semble[key], 64):
            raise RunError(f"manifest semble {key} must be a lowercase sha256")
    corpus = _exact_keys(
        provenance["corpus"], {"digest", "path_sha_diff_digest"}, "manifest corpus"
    )
    for key in ("digest", "path_sha_diff_digest"):
        if not _is_hex(corpus[key], 64):
            raise RunError(f"manifest corpus {key} must be a lowercase sha256")
    suite_prov = _exact_keys(
        provenance["suite"],
        {"suite_digest", "query_pack_digest", "tokenizer_budget_version"},
        "manifest suite",
    )
    for key in ("suite_digest", "query_pack_digest"):
        if not _is_hex(suite_prov[key], 64):
            raise RunError(f"manifest suite {key} must be a lowercase sha256")
    if suite_prov["tokenizer_budget_version"] != TOKENIZER_BUDGET_VERSION:
        raise RunError("manifest suite tokenizer budget version mismatch")
    host_prov = _exact_keys(
        provenance["host"], {"profile_digest", "check_record_digest"}, "manifest host provenance"
    )
    for key in ("profile_digest", "check_record_digest"):
        if not _is_hex(host_prov[key], 64):
            raise RunError(f"manifest host {key} must be a lowercase sha256")
    return manifest


def _validate_single_record(
    repo: Path, suite: dict, pack: dict, source: SourceSnapshot, path: Path
) -> dict:
    """Validate one historical v3/v4 or current v5 record against its pack."""
    raw = read_json(path)
    if not isinstance(raw, dict):
        raise RunError(f"record is not an object: {path}")
    if raw.get("schema_version") not in (3, 4, 5):
        raise RunError(f"v3/v4/v5 record required: {path}")
    routes = sorted(raw.get("route_provenance", {}).keys())
    if not routes:
        raise RunError(f"record names no routes: {path}")
    projected_pack, projected_suite = project_pack_and_suite(pack, suite, routes)
    expected_sha = digest(canonical_bytes(projected_pack))
    if raw.get("query_pack_sha256") != expected_sha:
        raise RunError(f"record {path} pack digest does not match its projected pack")
    return validate_evidence_against_suite(repo, projected_suite, projected_pack, source, raw)


def _rep_segment(path: Path, root: Path) -> str:
    try:
        rel = path.resolve().relative_to(root.resolve())
    except ValueError as exc:
        raise RunError(f"artifact escapes the manifest directory: {path}") from exc
    segs = [p for p in rel.parts if p.startswith("rep-") and len(p) > 4 and p[4:].isdigit()]
    if len(segs) != 1:
        raise RunError(f"artifact outside a single rep directory: {rel.as_posix()}")
    return segs[0]


def _rep_sort_key(rep: str) -> int:
    return int(rep[4:])


def _record_identity(payload: dict, where: str) -> tuple[str, str]:
    captures = payload.get("captures")
    if not isinstance(captures, dict) or not captures:
        raise RunError(f"{where} must have captures")
    identities = set()
    for capture in captures.values():
        if not isinstance(capture, dict):
            raise RunError(f"{where} capture is not an object")
        identities.add((capture.get("system"), capture.get("chunk_strategy")))
    if len(identities) != 1:
        raise RunError(f"{where} mixes capture systems or strategies")
    system, strategy = identities.pop()
    if system == "semble" and len(captures) != 1:
        raise RunError(f"{where} Semble record must have one capture")
    return system, strategy


def _probe_clean(probe: object, profile: dict) -> bool:
    if not isinstance(probe, dict) or _host_fingerprint(probe) != profile["fingerprint"]:
        return False
    if probe.get("system") == "Linux":
        return _linux_probe_clean(probe, profile)
    return (
        probe.get("concurrent_processes", {}) in ({}, {"none": []})
        and probe.get("contention_override") is not True
        and isinstance(probe.get("thermal"), dict)
        and probe["thermal"].get("status") == "clean"
        and isinstance(probe.get("frequency"), dict)
        and probe["frequency"].get("status") in ("stable", "bounded")
        and isinstance(probe.get("power"), dict)
        and probe["power"].get("status") == "bounded"
    )


def _linux_probe_clean(probe: dict, profile: dict) -> bool:
    limits = profile.get("linux_limits")
    if not isinstance(limits, dict):
        return False
    thermal = probe.get("thermal")
    frequency = probe.get("frequency")
    power = probe.get("power")
    if not (
        probe.get("concurrent_processes") in ({}, {"none": []})
        and probe.get("contention_override") is not True
        and isinstance(thermal, dict)
        and thermal.get("status") == "observed"
        and isinstance(frequency, dict)
        and frequency.get("status") == "observed"
        and isinstance(power, dict)
        and power.get("status") == "bounded"
    ):
        return False
    observed_zones = thermal.get("evidence")
    observed_cpus = frequency.get("evidence")
    governors = power.get("governors")
    settings = power.get("settings")
    maximums = limits.get("cpu_max_khz")
    if not all(
        isinstance(value, dict)
        for value in (observed_zones, observed_cpus, governors, settings, maximums)
    ):
        return False
    if set(observed_cpus) != set(maximums) or set(governors) != set(maximums):
        return False
    if any(value != "performance" for value in governors.values()):
        return False
    if set(settings) != {"governors", "minimum_khz", "maximum_khz", "drivers", "boost"}:
        return False
    if settings["governors"] != governors or not all(
        isinstance(settings[key], dict)
        for key in ("minimum_khz", "maximum_khz", "drivers", "boost")
    ):
        return False
    if any(
        set(settings[key]) != set(maximums) for key in ("minimum_khz", "maximum_khz", "drivers")
    ):
        return False
    allowed_boost = {
        "/sys/devices/system/cpu/intel_pstate/no_turbo": "1",
        "/sys/devices/system/cpu/cpufreq/boost": "0",
    }
    if not settings["boost"] or any(
        allowed_boost.get(path) != value for path, value in settings["boost"].items()
    ):
        return False
    if power.get("digest") != digest(canonical(settings)):
        return False
    for name, sensor_type in limits["thermal_zones"].items():
        entry = observed_zones.get(name)
        if not isinstance(entry, dict) or entry.get("type") != sensor_type:
            return False
        temperature = entry.get("temp_millidegrees")
        if (
            type(temperature) is not int
            or not 0 <= temperature <= limits["max_thermal_millidegrees"]
        ):
            return False
    for name, maximum in maximums.items():
        entry = observed_cpus[name]
        if not isinstance(entry, dict) or entry.get("maximum_khz") != maximum:
            return False
        current = entry.get("current_khz")
        if type(current) is not int or not 0 < current <= maximum:
            return False
        minimum_setting = settings["minimum_khz"][name]
        maximum_setting = settings["maximum_khz"][name]
        if (
            type(minimum_setting) is not int
            or type(maximum_setting) is not int
            or not 0 < minimum_setting <= current <= maximum_setting <= maximum
            or not isinstance(settings["drivers"][name], str)
            or not settings["drivers"][name]
        ):
            return False
        if current * 100 < maximum * limits["min_frequency_percent"]:
            return False
    return True


def _valid_bootstrap_ci(ci: object, sample_count: int, *, estimable: bool) -> bool:
    if not isinstance(ci, dict) or ci.get("sample_count") != sample_count:
        return False
    if ci.get("method") != "paired_stratified_bootstrap_percentile_v1":
        return False
    counts = ci.get("strata")
    if (
        not isinstance(counts, dict)
        or any(type(value) is not int or value < 1 for value in counts.values())
        or sum(counts.values()) != sample_count
    ):
        return False
    if ci.get("status") == "not_applicable":
        return not estimable and ci.get("reason") == "insufficient_sample"
    return (
        ci.get("resamples") == 10_000
        and _is_hex(ci.get("seed_sha256"), 64)
        and all(is_finite_json_number(ci.get(key)) for key in ("mean", "lower_95", "upper_95"))
        and ci["lower_95"] <= ci["mean"] <= ci["upper_95"]
    )


def _valid_stratified_delta(strata: object, sample_count: int, expected_mean: float | None) -> bool:
    if type(sample_count) is not int or not is_finite_json_number(sample_count) or sample_count < 0:
        return False
    if not isinstance(strata, dict) or set(strata) != {"category", "language", "repository"}:
        return False
    for dimension in strata.values():
        if not isinstance(dimension, dict) or (sample_count > 0 and not dimension):
            return False
        observed = 0
        weighted_sum = 0.0
        for entry in dimension.values():
            if not isinstance(entry, dict) or set(entry) != {"sample_count", "mean_delta", "ci_95"}:
                return False
            if (
                type(entry["sample_count"]) is not int
                or not 1 <= entry["sample_count"] <= sample_count - observed
            ):
                return False
            observed += entry["sample_count"]
            if not is_finite_json_number(entry["mean_delta"]):
                return False
            weighted_sum += float(entry["mean_delta"]) * entry["sample_count"]
            ci = entry["ci_95"]
            if not _valid_bootstrap_ci(ci, entry["sample_count"], estimable=False):
                return False
            if ci.get("status") != "not_applicable" and not math.isclose(
                float(ci["mean"]), float(entry["mean_delta"]), rel_tol=1e-12, abs_tol=1e-12
            ):
                return False
        if observed != sample_count:
            return False
        if sample_count > 0 and (
            expected_mean is None
            or not math.isclose(
                weighted_sum / sample_count, expected_mean, rel_tol=1e-12, abs_tol=1e-12
            )
        ):
            return False
    return True


def _qualified_uncertainty(comparison: object) -> bool:
    if not isinstance(comparison, dict):
        return False
    ci = comparison.get("primary_delta_ci_95")
    if not isinstance(ci, dict) or type(ci.get("sample_count")) is not int:
        return False
    primary_count = ci["sample_count"]
    if primary_count < 1 or not _valid_bootstrap_ci(ci, primary_count, estimable=True):
        return False
    primary_mean = comparison.get("primary_delta")
    if (
        comparison.get("sample_count") != primary_count
        or not is_finite_json_number(primary_mean)
        or not math.isclose(float(ci["mean"]), float(primary_mean), rel_tol=1e-12, abs_tol=1e-12)
    ):
        return False
    outcome_counts = [
        comparison.get("paired_wins"),
        comparison.get("paired_losses"),
        comparison.get("paired_ties"),
    ]
    if (
        any(type(value) is not int or value < 0 for value in outcome_counts)
        or sum(outcome_counts) != primary_count
    ):
        return False
    if not _valid_stratified_delta(
        comparison.get("stratified_primary_delta"), primary_count, float(primary_mean)
    ):
        return False
    no_answer = comparison.get("no_answer_abstention_delta")
    if not isinstance(no_answer, dict) or set(no_answer) != {
        "metric",
        "sample_count",
        "mean_delta",
        "ci_95",
        "strata",
    }:
        return False
    if (
        no_answer["metric"] != "no_answer_abstention"
        or type(no_answer["sample_count"]) is not int
        or no_answer["sample_count"] < 0
        or not _valid_bootstrap_ci(no_answer["ci_95"], no_answer["sample_count"], estimable=False)
    ):
        return False
    if no_answer["sample_count"] == 0:
        if (
            no_answer["mean_delta"] != "not_applicable"
            or not _valid_stratified_delta(no_answer["strata"], 0, None)
            or any(no_answer["strata"].values())
        ):
            return False
    elif not is_finite_json_number(no_answer["mean_delta"]):
        return False
    elif (
        no_answer["ci_95"].get("status") != "not_applicable"
        and not math.isclose(
            float(no_answer["ci_95"]["mean"]),
            float(no_answer["mean_delta"]),
            rel_tol=1e-12,
            abs_tol=1e-12,
        )
    ) or not _valid_stratified_delta(
        no_answer["strata"], no_answer["sample_count"], float(no_answer["mean_delta"])
    ):
        return False
    return True


def _qualified_cluster_uncertainty(comparison: object) -> bool:
    if not isinstance(comparison, dict):
        return False
    ci = comparison.get("primary_delta_cluster_ci_95")
    if not isinstance(ci, dict) or ci.get("status") is not None:
        return False
    sample_count = comparison.get("sample_count")
    cluster_count = ci.get("cluster_count")
    strata = ci.get("strata")
    if (
        ci.get("method") != "paired_query_family_cluster_bootstrap_percentile_v1"
        or type(sample_count) is not int
        or type(cluster_count) is not int
        or type(ci.get("sample_count")) is not int
        or ci["sample_count"] != sample_count
        or type(ci.get("min_cluster_count")) is not int
        or not 2 <= ci["min_cluster_count"] <= 20
        or not ci["min_cluster_count"] <= cluster_count <= sample_count
        or not isinstance(strata, dict)
        or any(type(count) is not int or count < 1 for count in strata.values())
        or sum(strata.values()) != cluster_count
        or ci.get("resamples") != 10_000
        or not _is_hex(ci.get("seed_sha256"), 64)
        or any(not is_finite_json_number(ci.get(key)) for key in ("mean", "lower_95", "upper_95"))
        or not is_finite_json_number(comparison.get("primary_delta"))
    ):
        return False
    return math.isclose(
        float(ci["mean"]), float(comparison["primary_delta"]), rel_tol=1e-12, abs_tol=1e-12
    )


def validate_completed_query_timing(metrics: dict, record: dict | None = None) -> None:
    """Bind one continuous client clock to every scheduled completed response."""
    timing = metrics.get("query_timing")
    if not isinstance(timing, dict) or set(timing) != {"boundary", "clock", "observations"}:
        raise RunError("completed-response timing contract is missing or malformed")
    if timing["boundary"] != semble_adapter.QUERY_TIMING_BOUNDARY:
        raise RunError("completed-response timing boundary differs from the canonical contract")
    if timing["clock"] != semble_adapter.QUERY_TIMING_CLOCK:
        raise RunError("completed-response timing clock differs from the canonical contract")
    observations = timing["observations"]
    if not isinstance(observations, list) or not observations:
        raise RunError("completed-response timing observations are missing")
    protocol = metrics.get("query_protocol")
    # The runner follows the suite's route order. Fix that order from the
    # first complete query and require every later query to repeat it.
    routes = [
        entry.get("route") if isinstance(entry, dict) else None
        for entry in observations[: metrics["route_count"]]
    ]
    if (
        len(routes) != metrics["route_count"]
        or any(not isinstance(route, str) or not route for route in routes)
        or len(set(routes)) != len(routes)
        or (protocol and set(routes) != set(metrics["warm_latencies_ms"]))
    ):
        raise RunError("completed-response timing route inventory differs")
    expected = []
    if protocol:
        expected.extend(("cold", 0, protocol["cold_probe_task_id"], route) for route in routes)
        warmups = protocol["warmup_schedules"]
        measured = protocol["measurement_schedules"]
    else:
        warmups = [metrics["query_schedule"]] * metrics["warmup_passes"]
        measured = [metrics["query_schedule"]] * metrics["measurement_repetitions"]
    for phase, schedules in (("warmup", warmups), ("measured", measured)):
        expected.extend(
            (phase, iteration, task, route)
            for iteration, schedule in enumerate(schedules)
            for task in schedule
            for route in routes
        )
    observed_keys = []
    previous_end = 0
    sdk_child_keys = (
        {"sdk_execute_ns", "sdk_post_execute_ns", "runner_result_materialize_ns"}
        if metrics.get("system") == "quanta" and metrics.get("schema_version") == 4
        else set()
    )
    record_rows = (
        {(row["task_id"], row["route"]): row for row in record["results"]} if record else {}
    )
    for index, entry in enumerate(observations):
        if (
            not isinstance(entry, dict)
            or set(entry)
            != {
                "task_id",
                "route",
                "phase",
                "iteration",
                "start_ns",
                "end_ns",
                "status",
                "output_bytes",
            }
            | sdk_child_keys
        ):
            raise RunError("completed-response timing observation is malformed")
        start, end = entry["start_ns"], entry["end_ns"]
        if type(start) is not int or type(end) is not int or start < previous_end or end < start:
            raise RunError("completed-response timing clock is not monotonic and serial")
        previous_end = end
        if sdk_child_keys and (
            any(
                type(entry[key]) is not int or not 0 <= entry[key] <= (1 << 64) - 1
                for key in sdk_child_keys
            )
            or sum(entry[key] for key in sdk_child_keys) > end - start
        ):
            raise RunError(
                "completed-response SDK child clocks are invalid or exceed outer interval"
            )
        if type(entry["iteration"]) is not int or entry["iteration"] < 0:
            raise RunError("completed-response timing iteration is invalid")
        if type(entry["output_bytes"]) is not int or entry["output_bytes"] <= 0:
            raise RunError("completed-response timing required output is absent")
        if entry["status"] not in {
            "success",
            "abstained",
            "capped",
            "error",
            "timeout",
            "unavailable",
        }:
            raise RunError("completed-response timing status is invalid")
        key = (entry["phase"], entry["iteration"], entry["task_id"], entry["route"])
        if index >= len(expected) or key != expected[index]:
            raise RunError(
                "completed-response timing observations differ from the complete schedule"
            )
        observed_keys.append(key)
        elapsed_ms = (end - start) / 1e6
        if protocol and entry["phase"] in {"cold", "measured"}:
            if entry["phase"] == "cold":
                sample = metrics["cold_latencies_ms"][entry["route"]]
            else:
                sample = metrics["warm_latencies_ms"][entry["route"]][entry["task_id"]][
                    entry["iteration"]
                ]
            if not math.isclose(sample, elapsed_ms, rel_tol=1e-9, abs_tol=1e-9):
                raise RunError(
                    "completed-response timing sample differs from its own clock interval"
                )
        if record:
            if entry["status"] not in {"success", "abstained", "capped"}:
                raise RunError("completed-response timing includes incomplete or failed requests")
        if record and entry["phase"] == "measured":
            row = record_rows.get((entry["task_id"], entry["route"]))
            if row is None or row["status"] != entry["status"]:
                raise RunError(
                    "completed-response timing status differs from the normalized response"
                )
            if entry["iteration"] == 0 and not math.isclose(
                row["timings"]["query_latency_ms"], elapsed_ms, rel_tol=1e-9, abs_tol=1e-9
            ):
                raise RunError("completed-response timing differs from normalized row latency")
    if observed_keys != expected:
        raise RunError("completed-response timing observations differ from the complete schedule")


def _validate_phase_metrics(payload: object, where: str) -> dict:
    if not isinstance(payload, dict):
        raise RunError(f"{where} must be an object")
    system = payload.get("system")
    schema_version = payload.get("schema_version")
    protocol_mode = "query_protocol" in payload
    system_key = "runner_binary_sha256" if system == "quanta" else "worker_sha256"
    if system == "quanta":
        expected_phases = {
            "discovery",
            "chunk",
            "daemon_boot_and_readiness" if schema_version in (3, 4) else "model_provider_prepare",
            "embed_publish_seal_activate",
            "cold_query" if protocol_mode else "first_query",
            "warm_query",
            "unattributed",
        }
        if schema_version in (2, 3, 4):
            expected_phases.add("symbol_preflight")
        if schema_version in (3, 4):
            expected_phases.update({"sdk_publish", "sdk_activate"})
        if protocol_mode:
            expected_phases.add("warmup")
    else:
        expected_phases = {
            "discovery",
            "model_provider_prepare",
            "index",
            "warmup",
            "cold_query" if protocol_mode else "first_query",
            "warm_query",
            "unattributed",
        }
    metric_keys = {
        "schema_version",
        "system",
        "timing_layer",
        "strategy",
        "record_sha256",
        system_key,
        "task_count",
        "route_count",
        "file_count",
        "chunk_count",
        "query_schedule",
        "warmup_passes",
        "measurement_repetitions",
        "phases_ms",
        "total_ms",
    }
    if "query_timing" in payload:
        metric_keys.add("query_timing")
    if system == "quanta" and schema_version == 4:
        metric_keys.add("query_timing")
    if system == "semble":
        metric_keys.add("phase_boundaries_ns")
        if schema_version == 2:
            metric_keys.update(
                {
                    "profile",
                    "requested_alpha",
                    "rerank_applied",
                    "lane_call_counts",
                    "execution_events_sha256",
                    "function_identity",
                    "observed_wrapped_call_ns",
                }
            )
    elif schema_version in (2, 3, 4):
        metric_keys.update(
            {
                "symbol_count",
                "symbol_producer_identity",
                "symbol_grammars",
                "symbol_coverage",
                "symbol_unsupported_files",
                "symbol_unsupported_details",
                "symbol_only_scopes",
                "empty_scopes",
                "symbol_coverage_policy",
                "symbol_preflight_out",
                "symbol_preflight_sha256",
                "symbol_producer_policy_sha256",
                "symbol_incomplete_files",
            }
        )
    if protocol_mode:
        metric_keys.update({"query_protocol", "warm_latencies_ms", "cold_latencies_ms"})
    metrics = _exact_keys(
        payload,
        metric_keys,
        where,
    )
    if (
        type(schema_version) is not int
        or system not in ("quanta", "semble")
        or (system == "semble" and schema_version not in (1, 2))
        or (system == "quanta" and schema_version not in (1, 2, 3, 4))
    ):
        raise RunError(f"{where} has unknown schema/system")
    if system == "semble" and schema_version == 2:
        profile = metrics["profile"]
        alpha = metrics["requested_alpha"]
        rerank = metrics["rerank_applied"]
        if profile not in {
            "native-default",
            "hybrid-no-rerank",
            "lexical-only",
            "lexical-file",
            "semantic-only",
        }:
            raise RunError(f"{where} has unknown Semble profile")
        if profile == "hybrid-no-rerank":
            if not is_finite_json_number(alpha) or not 0 <= alpha <= 1 or rerank is not False:
                raise RunError(f"{where} has invalid controlled Semble profile evidence")
        elif alpha is not None or rerank is not (profile == "native-default"):
            raise RunError(f"{where} has invalid Semble profile evidence")
        lane_counts = metrics["lane_call_counts"]
        if (
            not isinstance(lane_counts, dict)
            or set(lane_counts) != {"bm25", "semantic", "encode"}
            or any(type(value) is not int or value < 0 for value in lane_counts.values())
        ):
            raise RunError(f"{where} has invalid Semble lane counts")
        if (
            profile in {"lexical-only", "lexical-file"}
            and not (
                lane_counts["bm25"] > 0
                and lane_counts["semantic"] == 0
                and lane_counts["encode"] == 0
            )
            or profile == "semantic-only"
            and not (
                lane_counts["bm25"] == 0
                and lane_counts["semantic"] > 0
                and lane_counts["encode"] > 0
            )
            or profile in {"native-default", "hybrid-no-rerank"}
            and not (
                lane_counts["bm25"] > 0
                and lane_counts["semantic"] > 0
                and lane_counts["encode"] > 0
            )
        ):
            raise RunError(f"{where} Semble lane counts contradict the profile")
        if lane_counts["encode"] != lane_counts["semantic"]:
            raise RunError(f"{where} Semble semantic and encode call counts differ")
        if not _is_hex(metrics["execution_events_sha256"], 64):
            raise RunError(f"{where} has invalid Semble execution event digest")
        identities = metrics["function_identity"]
        if not isinstance(identities, dict) or set(identities) != {
            "bm25",
            "index_search",
            "module_search",
            "resolve_alpha",
            "semantic",
        }:
            raise RunError(f"{where} has invalid Semble function identity set")
        for name, identity in identities.items():
            if (
                not isinstance(identity, dict)
                or set(identity) != {"module", "qualname", "source_sha256"}
                or not isinstance(identity["module"], str)
                or not identity["module"]
                or not isinstance(identity["qualname"], str)
                or not identity["qualname"]
                or not _is_hex(identity["source_sha256"], 64)
            ):
                raise RunError(f"{where} has invalid Semble function identity: {name}")
        if (
            type(metrics["observed_wrapped_call_ns"]) is not int
            or metrics["observed_wrapped_call_ns"] < 0
        ):
            raise RunError(f"{where} has invalid observed wrapped-call duration")
    if system == "quanta" and schema_version in (2, 3, 4):
        try:
            symbol_coverage.validate_metrics(metrics, QUANTA_SYMBOL_GRAMMARS)
        except (ValueError, KeyError, TypeError) as exc:
            raise RunError(f"{where}: {exc}") from exc
    expected_layer = (
        "runner_monotonic_wall_v1" if system == "quanta" else "worker_monotonic_wall_v1"
    )
    if metrics["timing_layer"] != expected_layer:
        raise RunError(f"{where} timing layer mismatch")
    if not isinstance(metrics["strategy"], str) or not metrics["strategy"]:
        raise RunError(f"{where} strategy must be nonempty")
    for key in ("record_sha256", system_key):
        if not _is_hex(metrics[key], 64):
            raise RunError(f"{where}.{key} must be a lowercase sha256")
    for key in ("task_count", "route_count", "file_count", "chunk_count"):
        if type(metrics[key]) is not int or metrics[key] < 1:
            raise RunError(f"{where}.{key} must be an integer >= 1")
    schedule = metrics["query_schedule"]
    if (
        not isinstance(schedule, list)
        or len(schedule) != metrics["task_count"]
        or len(set(schedule)) != len(schedule)
        or any(not isinstance(task_id, str) or not task_id for task_id in schedule)
    ):
        raise RunError(f"{where}.query_schedule must list each task exactly once")
    if type(metrics["warmup_passes"]) is not int or metrics["warmup_passes"] < 0:
        raise RunError(f"{where}.warmup_passes must be nonnegative")
    if (
        type(metrics["measurement_repetitions"]) is not int
        or metrics["measurement_repetitions"] < 1
    ):
        raise RunError(f"{where}.measurement_repetitions must be positive")
    if protocol_mode:
        protocol = validate_query_protocol(
            metrics["query_protocol"], schedule, f"{where}.query_protocol"
        )
        if len(protocol["warmup_schedules"]) != metrics["warmup_passes"]:
            raise RunError(f"{where} warmup count differs from query protocol")
        if len(protocol["measurement_schedules"]) != metrics["measurement_repetitions"]:
            raise RunError(f"{where} repetition count differs from query protocol")
        warm = metrics["warm_latencies_ms"]
        cold = metrics["cold_latencies_ms"]
        if (
            not isinstance(warm, dict)
            or set(warm) != set(cold)
            or len(warm) != metrics["route_count"]
        ):
            raise RunError(f"{where} warm/cold route maps differ")
        for route, by_task in warm.items():
            if (
                not isinstance(route, str)
                or not isinstance(by_task, dict)
                or set(by_task) != set(schedule)
            ):
                raise RunError(f"{where} warm latency task map differs from the protocol")
            if not is_finite_json_number(cold[route]) or cold[route] < 0:
                raise RunError(f"{where} cold latency is invalid")
            for task_id, values in by_task.items():
                if (
                    not isinstance(values, list)
                    or len(values) != metrics["measurement_repetitions"]
                ):
                    raise RunError(f"{where} warm latency count differs for {route}/{task_id}")
                for value in values:
                    if not is_finite_json_number(value) or value < 0:
                        raise RunError(f"{where} warm latency is invalid for {route}/{task_id}")
    phases = _exact_keys(metrics["phases_ms"], expected_phases, f"{where}.phases_ms")
    for key, value in phases.items():
        if not is_finite_json_number(value) or value < 0:
            raise RunError(f"{where}.phases_ms.{key} must be finite and nonnegative")
    total = metrics["total_ms"]
    if not is_finite_json_number(total) or total <= 0:
        raise RunError(f"{where}.total_ms must be finite and positive")
    nested_keys = (
        {"sdk_publish", "sdk_activate"}
        if system == "quanta" and schema_version in (3, 4)
        else set()
    )
    if (
        nested_keys
        and sum(phases[key] for key in nested_keys) > phases["embed_publish_seal_activate"] + 0.01
    ):
        raise RunError(f"{where} SDK children exceed publish/activate interval")
    partition = sum(value for key, value in phases.items() if key not in nested_keys)
    if not math.isclose(partition, total, rel_tol=1e-9, abs_tol=0.01):
        raise RunError(f"{where} phase sum differs from total")
    if protocol_mode:
        cold_duration = sum(cold.values())
        warm_duration = sum(sum(values) for by_task in warm.values() for values in by_task.values())
        # Calls are serial within these monotonic windows; overhead can only
        # make the enclosing phase longer, allowing clock rounding at 0.01 ms.
        if cold_duration > phases["cold_query"] + 0.01:
            raise RunError(f"{where} cold samples exceed the cold query phase")
        if warm_duration > phases["warm_query"] + 0.01:
            raise RunError(f"{where} warm samples exceed the warm query phase")
    if system == "semble":
        boundary_keys = {
            "worker_start",
            "discovery_end",
            "model_provider_prepare_end",
            "index_end",
            "warmup_end",
            "query_start",
            "first_query_start",
            "first_query_end",
            "query_end",
            "worker_end",
        }
        if protocol_mode:
            boundary_keys.update({"cold_query_start", "cold_query_end"})
        boundaries = _exact_keys(
            metrics["phase_boundaries_ns"],
            boundary_keys,
            f"{where}.phase_boundaries_ns",
        )
        boundary_order = (
            (
                "worker_start",
                "discovery_end",
                "model_provider_prepare_end",
                "index_end",
                "cold_query_start",
                "cold_query_end",
                "warmup_end",
                "query_start",
                "first_query_start",
                "first_query_end",
                "query_end",
                "worker_end",
            )
            if protocol_mode
            else (
                "worker_start",
                "discovery_end",
                "model_provider_prepare_end",
                "index_end",
                "warmup_end",
                "query_start",
                "first_query_start",
                "first_query_end",
                "query_end",
                "worker_end",
            )
        )
        ordered = [boundaries[key] for key in boundary_order]
        if any(type(value) is not int or value < 0 for value in ordered):
            raise RunError(f"{where} phase boundaries must be nonnegative integers")
        if ordered != sorted(ordered):
            raise RunError(f"{where} phase boundaries are not monotonic")
        derived = {
            "discovery": (boundaries["discovery_end"] - boundaries["worker_start"]) / 1e6,
            "model_provider_prepare": (
                boundaries["model_provider_prepare_end"] - boundaries["discovery_end"]
            )
            / 1e6,
            "index": (boundaries["index_end"] - boundaries["model_provider_prepare_end"]) / 1e6,
        }
        if protocol_mode:
            derived["cold_query"] = (
                boundaries["cold_query_end"] - boundaries["cold_query_start"]
            ) / 1e6
            derived["warmup"] = (boundaries["warmup_end"] - boundaries["cold_query_end"]) / 1e6
            derived["warm_query"] = (boundaries["query_end"] - boundaries["query_start"]) / 1e6
        else:
            derived["warmup"] = (boundaries["warmup_end"] - boundaries["index_end"]) / 1e6
            derived["first_query"] = (
                boundaries["first_query_end"] - boundaries["first_query_start"]
            ) / 1e6
            derived["warm_query"] = (
                boundaries["query_end"]
                - boundaries["query_start"]
                - boundaries["first_query_end"]
                + boundaries["first_query_start"]
            ) / 1e6
        derived["unattributed"] = (
            boundaries["worker_end"] - boundaries["worker_start"]
        ) / 1e6 - sum(derived.values())
        for key, value in derived.items():
            if value < 0 or not math.isclose(value, phases[key], rel_tol=1e-9, abs_tol=0.01):
                raise RunError(f"{where} phase {key} is not derived from boundaries")
        if protocol_mode and "query_timing" not in metrics:
            first_task = protocol["measurement_schedules"][0][0]
            first_duration = (boundaries["first_query_end"] - boundaries["first_query_start"]) / 1e6
            for route, by_task in warm.items():
                if not math.isclose(
                    by_task[first_task][0], first_duration, rel_tol=1e-9, abs_tol=0.01
                ):
                    raise RunError(
                        f"{where} first warm sample differs from first query boundaries: {route}"
                    )
    if "query_timing" in metrics:
        validate_completed_query_timing(metrics)
    return metrics


def _verify_symbol_coverage_corpus(metrics: dict, corpus: object) -> None:
    """Bind a marked Quanta record's successful coverage to admitted bytes."""
    if "symbol_coverage" not in metrics:
        raise RunError("current Quanta phase metrics lack file-level symbol coverage")
    corpus_files = corpus.get("files") if isinstance(corpus, dict) else None
    if not isinstance(corpus_files, list):
        raise RunError("corpus manifest lacks file list for symbol coverage")
    try:
        expected_rows = sorted((entry["path"], entry["file_sha256"]) for entry in corpus_files)
        actual_rows = [
            (entry["path"], entry["source_sha256"]) for entry in metrics["symbol_coverage"]
        ]
    except (KeyError, TypeError) as exc:
        raise RunError("malformed symbol coverage or corpus file row") from exc
    if actual_rows != expected_rows:
        raise RunError("symbol coverage differs from frozen corpus manifest")


def _validate_linux_resource_metrics(payload: dict, where: str) -> dict:
    keys = {
        "schema_version",
        "sampler",
        "capture_scope",
        "owner_backend",
        "sample_interval_ms",
        "command_sha256",
        "subject_sha256",
        "root_pid",
        "root_start_ticks",
        "exit_code",
        "timed_out",
        "elapsed_ms",
        "peak_rss_bytes",
        "peak_cgroup_memory_bytes",
        "total_user_cpu_ns",
        "total_kernel_cpu_ns",
        "cgroup_cpu_usage_ns",
        "cgroup_path",
        "delegated_cgroup_parent",
        "processes",
        "escaped",
        "samples",
        "sampling_complete",
        "cleanup_complete",
        "ownership_complete",
        "isolation",
        "storage",
    }
    metrics = _exact_keys(
        payload,
        keys | ({"exec_command_sha256"} if "exec_command_sha256" in payload else set()),
        where,
    )
    if metrics["schema_version"] != 2 or metrics["sampler"] != "linux-process-owner-v1":
        raise RunError(f"{where} has unknown Linux resource schema/sampler")
    if metrics["capture_scope"] not in ("exploratory", "qualified"):
        raise RunError(f"{where}.capture_scope is invalid")
    backend = metrics["owner_backend"]
    if backend not in ("cgroup-v2", "process-group"):
        raise RunError(f"{where}.owner_backend is invalid")
    if type(metrics["sample_interval_ms"]) is not int or metrics["sample_interval_ms"] < 1:
        raise RunError(f"{where}.sample_interval_ms must be positive")
    for key in ("command_sha256", "subject_sha256"):
        if not _is_hex(metrics[key], 64):
            raise RunError(f"{where}.{key} must be a lowercase sha256")
    for key in ("root_pid", "root_start_ticks", "peak_rss_bytes", "samples"):
        if type(metrics[key]) is not int or metrics[key] < 1:
            raise RunError(f"{where}.{key} must be positive")
    for key in ("total_user_cpu_ns", "total_kernel_cpu_ns"):
        if type(metrics[key]) is not int or metrics[key] < 0:
            raise RunError(f"{where}.{key} must be nonnegative")
    if metrics["exit_code"] != 0 or metrics["timed_out"] is not False:
        raise RunError(f"{where} does not describe a successful bounded process")
    if not is_finite_json_number(metrics["elapsed_ms"]) or metrics["elapsed_ms"] <= 0:
        raise RunError(f"{where}.elapsed_ms must be finite and positive")
    if metrics["sampling_complete"] is not True or metrics["cleanup_complete"] is not True:
        raise RunError(f"{where} Linux owner sampling or cleanup is incomplete")
    processes = metrics["processes"]
    if not isinstance(processes, list) or not processes:
        raise RunError(f"{where}.processes must be nonempty")
    identities = set()
    for index, item in enumerate(processes):
        row = _exact_keys(
            item,
            {"pid", "start_ticks", "peak_rss_bytes", "user_cpu_ns", "kernel_cpu_ns"},
            f"{where}.processes[{index}]",
        )
        identity = (row["pid"], row["start_ticks"])
        if any(type(part) is not int or part < 1 for part in identity) or identity in identities:
            raise RunError(f"{where}.processes[{index}] identity is invalid or reused")
        identities.add(identity)
        for key in ("peak_rss_bytes", "user_cpu_ns", "kernel_cpu_ns"):
            if type(row[key]) is not int or row[key] < 0:
                raise RunError(f"{where}.processes[{index}].{key} must be nonnegative")
    if (metrics["root_pid"], metrics["root_start_ticks"]) not in identities:
        raise RunError(f"{where} root identity is not in owned processes")
    escaped = metrics["escaped"]
    if not isinstance(escaped, list):
        raise RunError(f"{where}.escaped must be a list")
    escaped_ids = set()
    for index, item in enumerate(escaped):
        row = _exact_keys(item, {"pid", "start_ticks"}, f"{where}.escaped[{index}]")
        identity = (row["pid"], row["start_ticks"])
        if identity not in identities or identity in escaped_ids:
            raise RunError(f"{where}.escaped[{index}] lacks unique owned identity")
        escaped_ids.add(identity)
    if backend == "cgroup-v2":
        path = metrics["cgroup_path"]
        if (
            not isinstance(path, str)
            or not Path(path).is_absolute()
            or ".." in Path(path).parts
            or not Path(path).name.startswith("quanta-retrieval-")
        ):
            raise RunError(f"{where}.cgroup_path is not a dedicated child")
        parent = _validate_cgroup_parent_identity(
            metrics["delegated_cgroup_parent"], f"{where}.delegated_cgroup_parent"
        )
        if Path(path).parent != Path(parent["path"]):
            raise RunError(f"{where}.cgroup_path is outside the delegated parent")
        for key in ("peak_cgroup_memory_bytes", "cgroup_cpu_usage_ns"):
            if type(metrics[key]) is not int or metrics[key] < 0:
                raise RunError(f"{where}.{key} must be nonnegative cgroup accounting")
    elif any(
        metrics[key] is not None
        for key in (
            "cgroup_path",
            "peak_cgroup_memory_bytes",
            "cgroup_cpu_usage_ns",
            "delegated_cgroup_parent",
        )
    ):
        raise RunError(f"{where} diagnostic group must not claim cgroup accounting")
    owned = backend == "cgroup-v2" and not escaped_ids
    if type(metrics["ownership_complete"]) is not bool or metrics["ownership_complete"] != owned:
        raise RunError(f"{where}.ownership_complete contradicts owner evidence")
    if (metrics["capture_scope"] == "qualified") != owned:
        raise RunError(f"{where} qualified Linux requires complete cgroup-v2 ownership")
    if metrics["capture_scope"] == "exploratory" and backend != "process-group":
        raise RunError(f"{where} exploratory Linux must use diagnostic process group")
    storage = _exact_keys(
        metrics["storage"],
        {
            "index_bytes",
            "model_cache_bytes",
            "parser_cache_bytes",
            "embedding_cache_bytes",
            "discovered_files",
            "indexed_chunks",
            "index_storage",
            "index_measurement",
        },
        f"{where}.storage",
    )
    for key in ("index_bytes", "model_cache_bytes", "parser_cache_bytes", "embedding_cache_bytes"):
        if type(storage[key]) is not int or storage[key] < 0:
            raise RunError(f"{where}.storage.{key} must be nonnegative")
    for key in ("discovered_files", "indexed_chunks"):
        if type(storage[key]) is not int or storage[key] < 1:
            raise RunError(f"{where}.storage.{key} must be positive")
    expected_measurement = {"disk": "filesystem_tree_v1", "memory": "process_peak_rss_delta_v1"}
    if (
        storage["index_storage"] not in expected_measurement
        or storage["index_measurement"] != expected_measurement[storage["index_storage"]]
        or storage["index_bytes"] <= 0
    ):
        raise RunError(f"{where}.storage measurement is invalid")
    isolation = metrics["isolation"]
    if isolation is not None:
        proof = _exact_keys(
            isolation,
            {"backend", "policy_sha256", "proof_sha256", "child_attestation"},
            f"{where}.isolation",
        )
        if proof["backend"] != LINUX_ISOLATION_BACKEND:
            raise RunError(f"{where}.isolation must be Linux Landlock")
        if not all(_is_hex(proof[key], 64) for key in ("policy_sha256", "proof_sha256")):
            raise RunError(f"{where}.isolation proof digest is invalid")
        if not _is_hex(metrics.get("exec_command_sha256"), 64):
            raise RunError(f"{where}.exec_command_sha256 is invalid")
        child = _exact_keys(
            proof["child_attestation"],
            {
                "nonce",
                "abi",
                "exec_sha256",
                "suite_read_denied",
                "query_pack_read_allowed",
                "proc_read_denied",
            },
            f"{where}.isolation.child_attestation",
        )
        if (
            not _is_hex(child["nonce"], 64)
            or child["exec_sha256"] != metrics["exec_command_sha256"]
            or type(child["abi"]) is not int
            or child["abi"] < linux_isolation.MIN_ABI
            or any(
                child[key] is not True
                for key in ("suite_read_denied", "query_pack_read_allowed", "proc_read_denied")
            )
        ):
            raise RunError(f"{where}.isolation child deny/allow proof is invalid")
    elif "exec_command_sha256" in metrics or metrics["capture_scope"] == "qualified":
        raise RunError(f"{where} qualified Linux requires child Landlock attestation")
    return metrics


def _validate_resource_metrics(payload: object, where: str) -> dict:
    if isinstance(payload, dict) and payload.get("schema_version") == 2:
        return _validate_linux_resource_metrics(payload, where)
    keys = {
        "schema_version",
        "sampler",
        "sample_interval_ms",
        "command_sha256",
        "root_pid",
        "subject_sha256",
        "exit_code",
        "timed_out",
        "elapsed_ms",
        "peak_rss_bytes",
        "peak_cpu_percent",
        "processes",
        "storage",
        "samples",
        "complete",
        "error",
        "cleanup_complete",
        "cleanup_escalated",
        "cleanup_error",
    }
    if not isinstance(payload, dict) or set(payload) not in (
        keys,
        keys | {"isolation"},
        keys | {"isolation", "exec_command_sha256"},
    ):
        raise RunError(f"{where} must hold the exact resource metric fields")
    metrics = payload
    if metrics["schema_version"] != 1 or metrics["sampler"] != "ps-process-tree-rss-cpu-v2":
        raise RunError(f"{where} has unknown schema/sampler")
    if type(metrics["sample_interval_ms"]) is not int or metrics["sample_interval_ms"] < 1:
        raise RunError(f"{where}.sample_interval_ms must be positive")
    for key in ("command_sha256", "subject_sha256"):
        if not _is_hex(metrics[key], 64):
            raise RunError(f"{where}.{key} must be a lowercase sha256")
    if type(metrics["root_pid"]) is not int or metrics["root_pid"] < 1:
        raise RunError(f"{where}.root_pid must be positive")
    if metrics["exit_code"] != 0 or metrics["timed_out"] is not False:
        raise RunError(f"{where} does not describe a successful bounded process")
    if not is_finite_json_number(metrics["elapsed_ms"]) or metrics["elapsed_ms"] <= 0:
        raise RunError(f"{where}.elapsed_ms must be finite and positive")
    if type(metrics["peak_rss_bytes"]) is not int or metrics["peak_rss_bytes"] <= 0:
        raise RunError(f"{where}.peak_rss_bytes must be positive")
    if not is_finite_json_number(metrics["peak_cpu_percent"]) or metrics["peak_cpu_percent"] < 0:
        raise RunError(f"{where}.peak_cpu_percent must be finite and nonnegative")
    processes = metrics["processes"]
    if not isinstance(processes, list) or not processes:
        raise RunError(f"{where}.processes must be nonempty")
    if type(metrics["samples"]) is not int or metrics["samples"] < 1:
        raise RunError(f"{where}.samples must be positive")
    process_ids: set[int] = set()
    for index, process in enumerate(processes):
        row = _exact_keys(
            process,
            {"pid", "command", "peak_rss_bytes", "peak_cpu_percent", "samples"},
            f"{where}.processes[{index}]",
        )
        if type(row["pid"]) is not int or row["pid"] < 1:
            raise RunError(f"{where}.processes[{index}].pid must be positive")
        if row["pid"] in process_ids:
            raise RunError(f"{where}.processes[{index}].pid is duplicated")
        process_ids.add(row["pid"])
        if not isinstance(row["command"], str) or not row["command"]:
            raise RunError(f"{where}.processes[{index}].command must be nonempty")
        if type(row["peak_rss_bytes"]) is not int or row["peak_rss_bytes"] <= 0:
            raise RunError(f"{where}.processes[{index}].peak_rss_bytes must be positive")
        if type(row["samples"]) is not int or row["samples"] < 1:
            raise RunError(f"{where}.processes[{index}].samples must be positive")
        if row["samples"] > metrics["samples"]:
            raise RunError(f"{where}.processes[{index}].samples exceeds total samples")
        if not is_finite_json_number(row["peak_cpu_percent"]) or row["peak_cpu_percent"] < 0:
            raise RunError(f"{where}.processes[{index}].peak_cpu_percent is invalid")
        if row["peak_rss_bytes"] > metrics["peak_rss_bytes"]:
            raise RunError(f"{where}.processes[{index}].peak_rss_bytes exceeds tree peak")
        if row["peak_cpu_percent"] > metrics["peak_cpu_percent"]:
            raise RunError(f"{where}.processes[{index}].peak_cpu_percent exceeds tree peak")
    storage = _exact_keys(
        metrics["storage"],
        {
            "index_bytes",
            "model_cache_bytes",
            "parser_cache_bytes",
            "embedding_cache_bytes",
            "discovered_files",
            "indexed_chunks",
            "index_storage",
            "index_measurement",
        },
        f"{where}.storage",
    )
    for key in ("index_bytes", "model_cache_bytes", "parser_cache_bytes", "embedding_cache_bytes"):
        if type(storage[key]) is not int or storage[key] < 0:
            raise RunError(f"{where}.storage.{key} must be nonnegative")
    for key in ("discovered_files", "indexed_chunks"):
        if type(storage[key]) is not int or storage[key] < 1:
            raise RunError(f"{where}.storage.{key} must be positive")
    if storage["index_storage"] not in ("disk", "memory"):
        raise RunError(f"{where}.storage.index_storage is invalid")
    expected_measurement = (
        "filesystem_tree_v1" if storage["index_storage"] == "disk" else "process_peak_rss_delta_v1"
    )
    if storage["index_measurement"] != expected_measurement:
        raise RunError(f"{where}.storage.index_measurement is invalid")
    if storage["index_bytes"] <= 0:
        raise RunError(f"{where}.storage.index_bytes must be positive")
    if metrics["complete"] is not True or metrics["error"] is not None:
        raise RunError(f"{where} resource sampling is incomplete")
    if (
        metrics["cleanup_complete"] is not True
        or type(metrics["cleanup_escalated"]) is not bool
        or metrics["cleanup_error"] is not None
    ):
        raise RunError(f"{where} owned process cleanup is incomplete")
    isolation = metrics.get("isolation")
    if "exec_command_sha256" in metrics and (
        not isinstance(isolation, dict) or isolation.get("backend") != LINUX_ISOLATION_BACKEND
    ):
        raise RunError(f"{where}.exec_command_sha256 requires Linux isolation")
    if isolation is not None:
        backend_name = isolation.get("backend") if isinstance(isolation, dict) else None
        proof = _exact_keys(
            isolation,
            {"backend", "policy_sha256", "proof_sha256"}
            | ({"child_attestation"} if backend_name == LINUX_ISOLATION_BACKEND else set()),
            f"{where}.isolation",
        )
        if proof["backend"] not in (MACOS_ISOLATION_BACKEND, LINUX_ISOLATION_BACKEND):
            raise RunError(f"{where}.isolation backend mismatch")
        for key in ("policy_sha256", "proof_sha256"):
            if not _is_hex(proof[key], 64):
                raise RunError(f"{where}.isolation.{key} must be a lowercase sha256")
        if backend_name == LINUX_ISOLATION_BACKEND:
            if not _is_hex(metrics.get("exec_command_sha256"), 64):
                raise RunError(f"{where}.exec_command_sha256 must be a lowercase sha256")
            child = _exact_keys(
                proof["child_attestation"],
                {
                    "nonce",
                    "abi",
                    "exec_sha256",
                    "suite_read_denied",
                    "query_pack_read_allowed",
                    "proc_read_denied",
                },
                f"{where}.isolation.child_attestation",
            )
            if (
                not _is_hex(child["nonce"], 64)
                or not _is_hex(child["exec_sha256"], 64)
                or child["exec_sha256"] != metrics["exec_command_sha256"]
                or type(child["abi"]) is not int
                or child["abi"] < linux_isolation.MIN_ABI
                or any(
                    child[key] is not True
                    for key in ("suite_read_denied", "query_pack_read_allowed", "proc_read_denied")
                )
            ):
                raise RunError(f"{where}.isolation child deny/allow proof is invalid")
    return metrics


def _validate_resource_capture_binding(
    metrics: dict,
    *,
    host_system: str,
    scope: str,
    manifest_parent: dict | None,
    protocol_parent: dict | None,
) -> None:
    linux_host = host_system == "Linux"
    if linux_host != (metrics["schema_version"] == 2):
        raise RunError("resource schema does not match capture host")
    if not linux_host:
        if manifest_parent is not None or protocol_parent is not None:
            raise RunError("non-Linux capture cannot claim a delegated cgroup parent")
        return
    if metrics["capture_scope"] != scope:
        raise RunError("Linux owner resource scope differs from run manifest")
    if scope == "qualified":
        if (
            metrics["owner_backend"] != "cgroup-v2"
            or not metrics["ownership_complete"]
            or manifest_parent is None
            or protocol_parent != manifest_parent
            or metrics["delegated_cgroup_parent"] != manifest_parent
        ):
            raise RunError("qualified Linux resource lacks frozen delegated owner binding")
    elif (
        any(
            value is not None
            for value in (manifest_parent, protocol_parent, metrics["delegated_cgroup_parent"])
        )
        or metrics["owner_backend"] != "process-group"
    ):
        raise RunError("exploratory Linux resource cannot upgrade diagnostic owner")


def _validate_unique_linux_attestations(entries: list[dict]) -> None:
    nonces = [entry["child_attestation"]["nonce"] for entry in entries]
    exec_digests = [entry["child_attestation"]["exec_sha256"] for entry in entries]
    if len(set(nonces)) != len(nonces) or len(set(exec_digests)) != len(exec_digests):
        raise RunError("Linux child attestations are reused across capture resources")


def _validate_isolation_proof(
    payload: object,
    *,
    root: Path,
    suite_path: Path,
    pack_path: Path,
    proof_path: Path,
    source_repo: Path,
    manifest_path: Path,
) -> dict:
    if not isinstance(payload, dict):
        raise RunError("isolation proof must be an object")
    backend_name = payload.get("backend")
    if backend_name not in (MACOS_ISOLATION_BACKEND, LINUX_ISOLATION_BACKEND):
        raise RunError("isolation proof backend is unknown")
    proof = _exact_keys(
        payload,
        {
            "schema_version",
            "backend",
            "sandbox_exec" if backend_name == MACOS_ISOLATION_BACKEND else "landlock",
            "policy_sha256",
            "denied_roots",
            "allowed_read_roots",
            "allowed_write_roots",
            "suite",
            "query_pack",
            "corpus_view",
            "runner_bundle",
            "platform_helpers",
            "probes",
        },
        "isolation proof",
    )
    if proof["schema_version"] != ISOLATION_PROOF_VERSION:
        raise RunError("isolation proof schema/backend mismatch")
    if backend_name == MACOS_ISOLATION_BACKEND:
        backend = _exact_keys(
            proof["sandbox_exec"], {"path", "sha256"}, "isolation proof sandbox_exec"
        )
        if backend["path"] != str(SANDBOX_EXEC) or not _is_hex(backend["sha256"], 64):
            raise RunError("isolation proof sandbox executable identity is malformed")
        if not SANDBOX_EXEC.is_file() or sha_file(SANDBOX_EXEC) != backend["sha256"]:
            raise RunError("isolation proof sandbox executable digest drifted")
    else:
        landlock = _exact_keys(
            proof["landlock"],
            {"abi", "threat_model", "module", "python", "policy"},
            "isolation proof landlock",
        )
        if type(landlock["abi"]) is not int or landlock["abi"] < linux_isolation.MIN_ABI:
            raise RunError("isolation proof Landlock ABI is unsupported")
        if landlock["threat_model"] != "filesystem-path-read-v1":
            raise RunError("isolation proof Landlock threat model is unsupported")
        module_ref = _exact_keys(landlock["module"], {"path", "sha256"}, "isolation proof module")
        module = _resolve_artifact(root, module_ref["path"], "isolation proof module")
        if (
            sha_file(module) != module_ref["sha256"]
            or sha_file(Path(linux_isolation.__file__)) != module_ref["sha256"]
        ):
            raise RunError("isolation proof Landlock module digest drifted")
        python_ref = _exact_keys(landlock["python"], {"path", "sha256"}, "isolation proof Python")
        python = Path(python_ref["path"])
        if (
            not python.is_absolute()
            or not python.is_file()
            or sha_file(python) != python_ref["sha256"]
        ):
            raise RunError("isolation proof Python digest drifted")
        policy_ref = _exact_keys(landlock["policy"], {"path", "sha256"}, "isolation proof policy")
        policy_path = _resolve_artifact(root, policy_ref["path"], "isolation proof policy")
        if (
            sha_file(policy_path) != policy_ref["sha256"]
            or policy_ref["sha256"] != proof["policy_sha256"]
        ):
            raise RunError("isolation proof policy digest drifted")
    roots = proof["denied_roots"]
    if (
        not isinstance(roots, list)
        or len(roots) < 2
        or roots != sorted(set(roots))
        or any(not isinstance(path, str) or not Path(path).is_absolute() for path in roots)
    ):
        raise RunError("isolation proof denied_roots must be sorted unique absolute paths")

    def absolute_roots(key: str) -> list[str]:
        values = proof[key]
        if (
            not isinstance(values, list)
            or values != sorted(set(values))
            or any(not isinstance(path, str) or not Path(path).is_absolute() for path in values)
        ):
            raise RunError(f"isolation proof {key} must be sorted unique absolute paths")
        return values

    allowed_read_roots = absolute_roots("allowed_read_roots")
    allowed_write_roots = absolute_roots("allowed_write_roots")
    if backend_name == MACOS_ISOLATION_BACKEND:
        profile = _seatbelt_profile(roots, allowed_read_roots, allowed_write_roots)
        policy_sha = hashlib.sha256(profile.encode("utf-8")).hexdigest()
    else:
        policy = {"readonly": allowed_read_roots, "writable": allowed_write_roots, "denied": roots}
        if read_json(policy_path) != policy:
            raise RunError("isolation proof Linux policy bytes differ from its roots")
        policy_sha = hashlib.sha256(
            (json.dumps(policy, sort_keys=True) + "\n").encode()
        ).hexdigest()
    if proof["policy_sha256"] != policy_sha:
        raise RunError("isolation proof policy digest mismatch")
    if not any(_path_within(source_repo.resolve(), Path(boundary)) for boundary in roots):
        raise RunError("isolation proof does not deny the source checkout")
    capture_paths: dict[str, Path] = {}
    for key, actual_path in (("suite", suite_path), ("query_pack", pack_path)):
        entry = _exact_keys(
            proof[key],
            {"path", "capture_path", "sha256"},
            f"isolation proof {key}",
        )
        if _resolve_artifact(root, entry["path"], f"isolation proof {key}.path") != actual_path:
            raise RunError(f"isolation proof {key} path mismatch")
        if not _is_hex(entry["sha256"], 64) or sha_file(actual_path) != entry["sha256"]:
            raise RunError(f"isolation proof {key} digest mismatch")
        capture_path = Path(entry["capture_path"])
        if not capture_path.is_absolute():
            raise RunError(f"isolation proof {key} capture_path must be absolute")
        capture_paths[key] = capture_path
        denied = any(_path_within(capture_path, Path(boundary)) for boundary in roots)
        if key == "suite" and not denied:
            raise RunError("isolation proof did not place suite under a denied root")
        if key == "query_pack" and denied:
            raise RunError("isolation proof denied the blind query pack")
    corpus_view = _exact_keys(
        proof["corpus_view"],
        {"path", "manifest_sha256", "proof_sha256", "file_count"},
        "isolation proof corpus_view",
    )
    view_ref = Path(corpus_view["path"])
    if view_ref.is_absolute() or ".." in view_ref.parts:
        raise RunError("isolation proof corpus_view.path escapes the run root")
    view_path = (root / view_ref).resolve()
    if root.resolve() not in view_path.parents or not view_path.is_dir():
        raise RunError("isolation proof corpus_view.path is not a run-local directory")
    if any(_path_within(view_path, Path(boundary)) for boundary in roots):
        raise RunError("isolation proof denies the admitted corpus view")
    if corpus_view["manifest_sha256"] != sha_file(manifest_path):
        raise RunError("isolation proof corpus manifest digest mismatch")
    materialized = _verify_materialized_corpus(
        view_path, manifest_path, corpus_view["proof_sha256"]
    )
    if corpus_view["file_count"] != len(materialized["files"]):
        raise RunError("isolation proof corpus file count mismatch")
    bundle = _exact_keys(
        proof["runner_bundle"],
        {"path", "sha256", "manifest_sha256", "manifest"},
        "isolation proof runner_bundle",
    )
    bundle_path = _resolve_artifact(root, bundle["path"], "isolation proof runner bundle")
    validate_runner_bundle(bundle_path, bundle)
    helpers = proof["platform_helpers"]
    expected_helpers = 1 if backend_name == LINUX_ISOLATION_BACKEND else 0
    if not isinstance(helpers, list) or len(helpers) != expected_helpers:
        raise RunError("isolation proof platform helper set is invalid")
    for index, entry in enumerate(helpers):
        row = _exact_keys(
            entry,
            {"path", "sha256", "source"},
            f"isolation proof platform_helpers[{index}]",
        )
        if row["source"] != "linux_isolation.py":
            raise RunError("isolation proof platform helper source is unknown")
        helper_path = _resolve_artifact(root, row["path"], "isolation proof platform helper")
        if (
            not _is_hex(row["sha256"], 64)
            or sha_file(helper_path) != row["sha256"]
            or helper_path.read_bytes() != Path(linux_isolation.__file__).read_bytes()
        ):
            raise RunError("isolation proof platform helper digest mismatch")
    probes = _exact_keys(
        proof["probes"],
        {"suite_read_denied", "query_pack_read_allowed"}
        | ({"proc_read_denied"} if backend_name == LINUX_ISOLATION_BACKEND else set()),
        "isolation proof probes",
    )
    if any(value is not True for value in probes.values()):
        raise RunError("isolation proof probes did not pass")
    if backend_name == LINUX_ISOLATION_BACKEND:
        captured_stage = capture_paths["suite"].parent.parent

        def relocate(values: list[str]) -> list[str]:
            relocated = []
            for value in values:
                path = Path(value)
                if path == captured_stage or captured_stage in path.parents:
                    path = root.resolve() / path.relative_to(captured_stage)
                relocated.append(str(path))
            return sorted(set(relocated))

        relocated_policy = {
            "readonly": relocate(allowed_read_roots),
            "writable": relocate(allowed_write_roots),
            "denied": relocate(roots),
        }
        try:
            linux_isolation.validate_policy(relocated_policy)
        except linux_isolation.IsolationError as exc:
            raise RunError(f"isolation proof Linux policy is invalid: {exc}") from exc
        observed_linux = _probe_linux(relocated_policy, module, python, suite_path, pack_path)
        if observed_linux["abi"] < linux_isolation.MIN_ABI:
            raise RunError("isolation proof Landlock ABI became unsupported")
        observed = observed_linux["probes"]
    elif capture_paths["suite"].is_file() and capture_paths["query_pack"].is_file():
        observed = _probe_seatbelt(profile, capture_paths["suite"], capture_paths["query_pack"])
    else:
        # Atomic promotion renames the staging tree. Re-probe the frozen final
        # bytes under an equivalent denial root while retaining external secret
        # roots from the capture profile.
        relocated_roots = [
            str(suite_path.parent.resolve())
            if _path_within(capture_paths["suite"], Path(boundary))
            else boundary
            for boundary in roots
        ]
        captured_stage = capture_paths["suite"].parent.parent

        def relocate(values: list[str]) -> list[str]:
            relocated = []
            for value in values:
                path = Path(value)
                if path == captured_stage or captured_stage in path.parents:
                    path = root.resolve() / path.relative_to(captured_stage)
                relocated.append(str(path))
            return sorted(set(relocated))

        observed = _probe_seatbelt(
            _seatbelt_profile(
                sorted(set(relocated_roots)),
                relocate(allowed_read_roots),
                relocate(allowed_write_roots),
            ),
            suite_path,
            pack_path,
        )
    if observed != probes:
        raise RunError("isolation proof could not be independently reproduced")
    proof_sha = sha_file(proof_path)
    result = {
        "backend": backend_name,
        "policy_sha256": policy_sha,
        "proof_sha256": proof_sha,
    }
    if backend_name == LINUX_ISOLATION_BACKEND:
        result["abi"] = landlock["abi"]
    return result


def _quanta_admission_model_revision(records: Iterable[object]) -> str:
    """Derive model authority from bound routes, never an unused encoder option."""
    identities: set[tuple[str, str]] = set()
    observed = False
    for record in records:
        if not isinstance(record, dict):
            raise RunError("Quanta admission model record is malformed")
        captures = record.get("captures")
        routes = record.get("route_provenance")
        if not isinstance(captures, dict) or not isinstance(routes, dict) or not routes:
            raise RunError("Quanta admission model lacks bound routes and captures")
        bound_ids = set()
        for route, binding in routes.items():
            if route not in ("lexical", "symbol", "semantic", "hybrid") or not isinstance(
                binding, dict
            ):
                raise RunError("Quanta admission model route is malformed")
            capture_id = binding.get("capture_id")
            if not isinstance(capture_id, str) or not isinstance(captures.get(capture_id), dict):
                raise RunError("Quanta admission model route has no capture")
            bound_ids.add(capture_id)
            capture = captures[capture_id]
            model, revision = capture.get("model"), capture.get("model_revision")
            if route in ("lexical", "symbol"):
                if (model, revision) != (f"none:{route}", "not-applicable"):
                    raise RunError("Quanta no-model route has an invalid model identity")
            else:
                if (
                    not isinstance(model, str)
                    or not model.strip()
                    or model != model.strip()
                    or model.startswith("none:")
                    or not isinstance(revision, str)
                    or not revision.strip()
                    or revision != revision.strip()
                    or revision == "not-applicable"
                ):
                    raise RunError("Quanta modeled route lacks a real model identity")
                identities.add((model, revision))
            observed = True
        if bound_ids != set(captures):
            raise RunError("Quanta admission model has unbound captures")
    if not observed or len(identities) > 1:
        raise RunError("Quanta admission requires one consistent routed model identity")
    return next(iter(identities))[1] if identities else "not-applicable"


def _quanta_semantic_capture_identity_matches(
    validated: dict, selector: str, declared_quanta_routes: set[str]
) -> bool:
    expected_revision = {
        "potion-code": "e9d2a44ca6a05ac6685f3b23709ea57eb7352d5b:model2vec-rs-0.3.0:fancy-regex:full-length-v1",
        "potion-code-full-v2": "e9d2a44ca6a05ac6685f3b23709ea57eb7352d5b:model2vec-rs-0.3.0:fancy-regex:full-length-v2",
    }.get(selector)
    if expected_revision is None:
        return True
    observed = {
        (capture.get("model"), capture.get("model_revision"))
        for entry in validated.values()
        if entry["system"] == "quanta"
        for route, binding in entry["run"]["route_provenance"].items()
        if route in ("semantic", "hybrid")
        for capture in [entry["run"]["captures"].get(binding.get("capture_id"), {})]
    }
    if declared_quanta_routes & {"semantic", "hybrid"} and not observed:
        return False
    return not observed or observed == {
        ("model2vec:minishlab/potion-code-16M-v2", expected_revision)
    }


def replay_paired_file_diagnostic_report(
    suite: dict,
    pack: dict,
    merged: dict,
    report: dict,
    strategy: str,
    report_digest: str,
) -> dict:
    """Re-derive one file-only paired report before it enters a verdict."""
    if report.get("report_scope") != "paired_independent_file_judgment_diagnostic_v1":
        raise RunError("wrong paired file diagnostic report scope")
    baseline = report["baseline_route"]
    candidate = report["candidate_route"]
    rescored = evaluate_paired_file_diagnostic(suite, pack, merged, baseline, candidate)
    if digest(canonical(rescored)) != digest(canonical(report)):
        raise RunError("paired file diagnostic differs from independent replay")
    comparison = rescored["judgment_metrics"]["file_judgments"]["comparison"]
    diagnostic_delta = comparison["delta"]["ndcg_at_10"]
    return {
        "strategy": strategy,
        "baseline_route": baseline,
        "candidate_route": candidate,
        "primary_metric": "diagnostic_file_ndcg_at_10",
        "primary_delta": diagnostic_delta if isinstance(diagnostic_delta, float) else None,
        "record_digest": digest(canonical(merged)),
        "report_digest": report_digest,
        "report_sha": digest(canonical(report)),
        "graded": False,
    }


def replay_complete_scored_file_report(
    suite: dict,
    pack: dict,
    merged: dict,
    report: dict,
    strategy: str,
    report_digest: str,
) -> dict:
    """Re-score complete file judgments and paired uncertainty from frozen rows."""
    if report.get("report_scope") != "paired_complete_scored_file_evidence_v1":
        raise RunError("wrong complete scored file report scope")
    rank = report.get("rank_metrics")
    comparison = rank.get("comparison") if isinstance(rank, dict) else None
    if not isinstance(comparison, dict):
        raise RunError("complete scored file comparison is missing")
    baseline = comparison.get("baseline")
    candidate = comparison.get("candidate")
    rescored = evaluate_complete_scored_file_evidence(suite, pack, merged, baseline, candidate)
    if digest(canonical(rescored)) != digest(canonical(report)):
        raise RunError("complete scored file report differs from independent replay")
    cluster_ci = qualified_query_family_ci(suite, rescored, baseline, candidate)
    observed = rescored["rank_metrics"]["comparison"]
    return {
        "strategy": strategy,
        "baseline_route": baseline,
        "candidate_route": candidate,
        "primary_metric": "file_ndcg_at_10",
        "primary_delta": observed["primary_delta"],
        "sample_count": observed["sample_count"],
        "paired_wins": observed["paired_wins"],
        "paired_losses": observed["paired_losses"],
        "paired_ties": observed["paired_ties"],
        "record_digest": digest(canonical(merged)),
        "report_digest": report_digest,
        "graded": True,
        "primary_delta_ci_95": observed["primary_delta_ci_95"],
        "primary_delta_cluster_ci_95": cluster_ci,
        "stratified_primary_delta": observed["stratified_primary_delta"],
        "no_answer_abstention_delta": observed["no_answer_abstention_delta"],
        "report_sha": digest(canonical(report)),
    }


def build_verdict(repo: Path, suite_path: Path, manifest_path: Path) -> dict:
    """Re-derive every digest from frozen bytes and emit the TEST-PLAN §8 verdict.

    Manifest numbers are never authority: receipts, mappings, matrices and
    reports are re-verified from the sibling artifact bytes. A missing
    required artifact or an unverifiable suite refuses the verdict
    outright; content lies fail the owning state.
    """
    root = manifest_path.resolve().parent
    manifest = _validate_manifest_shape(read_json(manifest_path))
    artifacts = manifest["artifacts"]
    resolved: dict[str, object] = {}
    for key in (
        "suite",
        "query_pack",
        "corpus_manifest",
        "mapping_proof",
        "latency_matrix",
        "host_start",
        "host_end",
        "host_profile",
        "semble_adapter_manifest",
        "semble_lockfile",
        "protocol_lock",
    ):
        resolved[key] = _resolve_artifact(root, artifacts[key], f"artifacts.{key}")
    for key in (
        "records",
        "reports",
        "quanta_manifests",
        "semble_native",
        "semble_model_cache_manifests",
        "phase_metrics",
        "symbol_preflights",
        "resource_metrics",
    ):
        resolved[key] = [_resolve_artifact(root, ref, f"artifacts.{key}") for ref in artifacts[key]]
    for key in RECEIPT_KEYS:
        if key in artifacts:
            resolved[key] = _resolve_artifact(root, artifacts[key], f"artifacts.{key}")
    if "host_timeline" in artifacts:
        resolved["host_timeline_raw"] = _resolve_artifact(
            root, artifacts["host_timeline_raw"], "artifacts.host_timeline_raw"
        )
        resolved["host_timeline"] = _resolve_artifact(
            root, artifacts["host_timeline"], "artifacts.host_timeline"
        )
    resolved["driver_source_closure"] = _resolve_artifact(
        root, artifacts["driver_source_closure"], "artifacts.driver_source_closure"
    )
    if manifest["scope"] == "qualified":
        for key in (
            "admission_manifest",
            "license_receipt",
            "adjudication_receipt",
        ):
            resolved[key] = _resolve_artifact(root, artifacts[key], f"artifacts.{key}")
        for key in (
            ("split_manifest", "split_releases")
            if "split_manifest" in artifacts
            else ("experiment_custody", "development_suite")
        ):
            resolved[key] = _resolve_artifact(root, artifacts[key], f"artifacts.{key}")
        resolved["annotation_receipts"] = [
            _resolve_artifact(root, ref, "artifacts.annotation_receipts")
            for ref in artifacts["annotation_receipts"]
        ]
    if "isolation_proof" in artifacts:
        resolved["isolation_proof"] = _resolve_artifact(
            root, artifacts["isolation_proof"], "artifacts.isolation_proof"
        )
    try:
        cli_suite_bytes = suite_path.read_bytes()
    except OSError as exc:
        raise RunError(f"cannot read CLI suite: {exc}") from exc
    if digest(cli_suite_bytes) != sha_file(resolved["suite"]):
        raise RunError("CLI suite differs from the frozen manifest suite")

    evidence = manifest["evidence"]
    claims = manifest["claims"]
    provenance_claims = manifest["provenance"]
    driver_closure = _validate_source_closure_shape(
        read_json(resolved["driver_source_closure"]), "driver source closure"
    )
    host_profile = validate_host_profile(read_json(resolved["host_profile"]))
    if sha_file(resolved["host_profile"]) != provenance_claims["host"]["profile_digest"]:
        raise RunError("host profile artifact digest mismatch")
    linux_host = host_profile["fingerprint"]["system"] == "Linux"
    parent_binding = manifest["host"].get("delegated_cgroup_parent")
    if (manifest["scope"] == "qualified" and linux_host) != (parent_binding is not None):
        raise RunError("qualified Linux manifest lacks exclusive delegated cgroup parent binding")
    if linux_host and manifest["scope"] == "qualified":
        if platform.system() != "Linux":
            raise RunError("qualified Linux verdict requires same-host Linux cgroup revalidation")
        if _linux_parent_identity(parent_binding["path"]) != parent_binding:
            raise RunError("delegated cgroup parent identity drifted before verdict")
        source_paths = {entry["path"] for entry in driver_closure["files"]}
        required_owner_sources = {
            "tools/benchmark/retrieval/run.py",
            "tools/benchmark/retrieval/linux_process.py",
            "tools/benchmark/retrieval/linux_isolation.py",
        }
        if not required_owner_sources <= source_paths:
            raise RunError("qualified Linux source closure omits owner or Landlock source")
    pair_notes: list[tuple[str, list[str], str]] = []

    def pair_note(reason: str, t_ids: tuple[str, ...] = (), fail_class: str = "provenance") -> None:
        pair_notes.append((reason, list(t_ids), fail_class))

    def read_note(path: Path, where: str, t_ids: tuple[str, ...]) -> object:
        try:
            return read_json(path)
        except ValueError:
            pair_note(f"{where}_unreadable", t_ids)
            return None

    def sha_note(path: Path, where: str, t_ids: tuple[str, ...]) -> str | None:
        try:
            return sha_file(path)
        except OSError:
            pair_note(f"{where}_unreadable", t_ids)
            return None

    # Root of trust: an unverifiable suite refuses the verdict, full stop.
    try:
        suite_payload = read_json(resolved["suite"])
        if not isinstance(suite_payload, dict):
            raise RunError("frozen suite is not an object")
        suite, pack, source = validate_suite(repo, suite_payload)
    except ValueError as exc:
        raise RunError(f"suite unverifiable; refusing verdict: {exc}") from exc
    try:
        verify_repo(repo, suite["repository_commit"])
    except ValueError as exc:
        raise RunError(f"repository does not match the frozen suite commit: {exc}") from exc

    pack_payload = read_note(resolved["query_pack"], "query_pack", ("T01", "T12"))
    corpus_payload = read_note(resolved["corpus_manifest"], "corpus_manifest", ("T00", "T12"))
    mapping_payload = read_note(resolved["mapping_proof"], "mapping_proof", ("T00", "T11"))
    suite_digest = sha_note(resolved["suite"], "suite_bytes", ("T01",))
    pack_digest = sha_note(resolved["query_pack"], "pack_bytes", ("T01",))
    corpus_digest = sha_note(resolved["corpus_manifest"], "corpus_bytes", ("T00",))
    mapping_digest = sha_note(resolved["mapping_proof"], "mapping_bytes", ("T00", "T11"))
    host_profile_digest = sha_note(resolved["host_profile"], "host_profile_bytes", ("T12",))
    if suite_digest != provenance_claims["suite"]["suite_digest"]:
        pair_note("suite_digest_mismatch", ("T01", "T12"))
    if pack_digest != provenance_claims["suite"]["query_pack_digest"]:
        pair_note("pack_digest_mismatch", ("T01", "T12"))
    if corpus_digest != provenance_claims["corpus"]["digest"]:
        pair_note("corpus_digest_mismatch", ("T00", "T12"), "corpus_mismatch")
    if mapping_digest != evidence["pair"]["mapping_proof_digest"]:
        pair_note("mapping_proof_digest_mismatch", ("T00", "T11"))
    protocol_payload = read_note(resolved["protocol_lock"], "protocol_lock", ("T12",))
    profiles = (
        protocol_payload.get("execution_profiles") if isinstance(protocol_payload, dict) else None
    )
    quanta_profile = profiles.get("quanta") if isinstance(profiles, dict) else None
    if (
        manifest["scope"] == "qualified"
        and isinstance(quanta_profile, dict)
        and quanta_profile.get("policy") in qp.QUALIFIED_FILE_PAIR_POLICIES
    ):
        require_reviewed_file_labels(suite, quanta_profile["policy"])
    protocol_keys = {
        "lock_version",
        "suite_digest",
        "query_pack_digest",
        "corpus_manifest_digest",
        "top_k",
        "strategies",
        "quanta_routes",
        "semble_route",
        "searchd_expected_sha256",
        "semble_lockfile_sha256",
        "host_profile_digest",
        "admission_digest",
        "driver_source_closure_digest",
        "repetitions",
        "system_orders",
        "base_seed",
        "query_warmup_passes",
        "query_repetitions_per_root",
        "query_protocol_sha256s",
        "execution_profiles",
        "execution_profiles_sha256",
        "retrieval_diagnostic_version",
        "symbol_coverage_policy",
        "rank_metric_k_policy",
    }
    if parent_binding is not None:
        protocol_keys.add("delegated_cgroup_parent")
    if isinstance(protocol_payload, dict) and protocol_payload.get("lock_version") in (3, 4, 5, 6):
        protocol_keys.update({"server_observation", "ingest_request_identity"})
    if isinstance(protocol_payload, dict) and protocol_payload.get("lock_version") in (4, 5, 6):
        protocol_keys.add("hybrid_fetch_policy")
    if isinstance(protocol_payload, dict) and "symbol_total_timeout_ms" in protocol_payload:
        protocol_keys.add("symbol_total_timeout_ms")
    protocol_shape_valid = (
        isinstance(protocol_payload, dict) and set(protocol_payload) == protocol_keys
    )
    if protocol_shape_valid:
        symbol_timeout = protocol_payload.get("symbol_total_timeout_ms", 120_000)
        if parent_binding is not None:
            try:
                protocol_parent = _validate_cgroup_parent_identity(
                    protocol_payload["delegated_cgroup_parent"],
                    "protocol lock.delegated_cgroup_parent",
                )
            except RunError:
                protocol_shape_valid = False
            else:
                protocol_shape_valid = protocol_parent == parent_binding
        strategies = protocol_payload["strategies"]
        system_orders = protocol_payload["system_orders"]
        root_digests = protocol_payload["query_protocol_sha256s"]
        protocol_shape_valid = protocol_shape_valid and (
            type(protocol_payload["lock_version"]) is int
            and protocol_payload["lock_version"] in (2, 3, 4, 5, 6)
            and type(protocol_payload["retrieval_diagnostic_version"]) is int
            and protocol_payload["symbol_coverage_policy"]
            in ("require-complete", "allow-incomplete")
            and type(symbol_timeout) is int
            and 0 < symbol_timeout < 2**64
            and all(
                _is_hex(protocol_payload[key], 64)
                for key in (
                    "suite_digest",
                    "query_pack_digest",
                    "corpus_manifest_digest",
                    "searchd_expected_sha256",
                    "semble_lockfile_sha256",
                    "host_profile_digest",
                )
            )
            and type(protocol_payload["top_k"]) is int
            and protocol_payload["top_k"] > 0
            and type(protocol_payload["repetitions"]) is int
            and protocol_payload["repetitions"] > 0
            and isinstance(system_orders, list)
            and len(system_orders) == protocol_payload["repetitions"]
            and all(
                order in (["quanta", "semble"], ["semble", "quanta"]) for order in system_orders
            )
            and type(protocol_payload["base_seed"]) is int
            and protocol_payload["base_seed"] >= 0
            and type(protocol_payload["query_warmup_passes"]) is int
            and protocol_payload["query_warmup_passes"] >= 0
            and type(protocol_payload["query_repetitions_per_root"]) is int
            and protocol_payload["query_repetitions_per_root"] > 0
            and isinstance(strategies, list)
            and bool(strategies)
            and all(isinstance(strategy, str) and strategy for strategy in strategies)
            and len(strategies) == len(set(strategies))
            and isinstance(protocol_payload["quanta_routes"], list)
            and bool(protocol_payload["quanta_routes"])
            and all(isinstance(route, str) and route for route in protocol_payload["quanta_routes"])
            and len(protocol_payload["quanta_routes"])
            == len(set(protocol_payload["quanta_routes"]))
            and isinstance(protocol_payload["semble_route"], str)
            and bool(protocol_payload["semble_route"])
            and isinstance(root_digests, list)
            and len(root_digests) == protocol_payload["repetitions"]
            and all(_is_hex(value, 64) for value in root_digests)
            and isinstance(protocol_payload["execution_profiles"], dict)
            and protocol_payload["execution_profiles_sha256"]
            == digest(canonical_bytes(protocol_payload["execution_profiles"]))
            and protocol_payload["retrieval_diagnostic_version"]
            == {2: 4, 3: 5, 4: 6, 5: 7, 6: 8}.get(protocol_payload["lock_version"])
            and protocol_payload["rank_metric_k_policy"] == "declared_top_k_v1"
        )
        if protocol_shape_valid:
            try:
                if protocol_payload["lock_version"] in (3, 4, 5, 6):
                    _validate_server_observation(protocol_payload["server_observation"])
                    _validate_ingest_request_identity(protocol_payload["ingest_request_identity"])
                if protocol_payload["lock_version"] in (4, 5, 6):
                    _validate_hybrid_fetch_policy(protocol_payload["hybrid_fetch_policy"])
                profiles = protocol_payload["execution_profiles"]
                if set(profiles) != {"quanta", "semble"}:
                    raise RunError("protocol execution profile systems are incomplete")
                quanta = profiles["quanta"]
                policy = quanta.get("policy")
                expected_quanta = qp.execution_profile(
                    policy,
                    quanta.get("config")
                    if policy in ("natural_language", "natural_language_file")
                    else None,
                )
                if quanta != expected_quanta:
                    raise RunError("protocol Quanta execution profile is invalid")
                if (
                    policy in ("natural_language", "natural_language_file")
                    and quanta["config"]["max_tokens"] != qp.DEFAULT_NL_CONFIG["max_tokens"]
                    and manifest["scope"] != "exploratory"
                ):
                    raise RunError(
                        "custom natural-language token budget cannot carry qualified scope"
                    )
                _validate_semble_profile(profiles["semble"], "protocol execution profile")
            except (AttributeError, KeyError, RunError, ValueError):
                protocol_shape_valid = False
        authority_digests = (
            protocol_payload["admission_digest"],
            protocol_payload["driver_source_closure_digest"],
        )
        protocol_shape_valid = protocol_shape_valid and (
            (
                _is_hex(authority_digests[0], 64)
                if manifest["scope"] == "qualified"
                else authority_digests[0] is None
            )
            and _is_hex(authority_digests[1], 64)
        )
    if not protocol_shape_valid:
        pair_note("protocol_lock_malformed", ("T12",))
        protocol_payload = {}
    else:
        if protocol_payload.get("suite_digest") != suite_digest:
            pair_note("protocol_lock_suite_drift", ("T12",))
        if protocol_payload.get("query_pack_digest") != pack_digest:
            pair_note("protocol_lock_pack_drift", ("T12",))
        if protocol_payload.get("corpus_manifest_digest") != corpus_digest:
            pair_note("protocol_lock_corpus_drift", ("T12",))
        if (
            protocol_payload.get("admission_digest")
            != provenance_claims["admission"]["manifest_digest"]
        ):
            pair_note("protocol_lock_admission_drift", ("T17",))
        if driver_closure["revision"] != provenance_claims["quanta"]["source_sha"]:
            pair_note("driver_source_closure_revision_drift", ("T12", "T17"))
        if driver_closure["digest"] != provenance_claims["quanta"]["source_closure_digest"]:
            pair_note("driver_source_closure_digest_drift", ("T12", "T17"))
        if protocol_payload.get("driver_source_closure_digest") != driver_closure["digest"]:
            pair_note("protocol_lock_source_closure_drift", ("T12", "T17"))
        if protocol_payload["host_profile_digest"] != host_profile_digest:
            pair_note("protocol_lock_host_profile_drift", ("T12",))
        if protocol_payload["top_k"] != pack["comparison_contract"]["top_k"]:
            pair_note("protocol_lock_top_k_drift", ("T12",))
        if protocol_payload["repetitions"] != manifest["repetitions"]:
            pair_note("protocol_lock_repetitions_drift", ("T12",))
    if isinstance(corpus_payload, dict) and isinstance(mapping_payload, dict):
        if not mapping_matches_manifest(mapping_payload, corpus_payload):
            pair_note("mapping_proof_not_clean", ("T00", "T11", "T12"), "corpus_mismatch")
        if (
            mapping_payload.get("diff_digest")
            != provenance_claims["corpus"]["path_sha_diff_digest"]
        ):
            pair_note("path_sha_diff_digest_mismatch", ("T00", "T11"))
        pack_commit = (
            pack_payload.get("repository_commit") if isinstance(pack_payload, dict) else None
        )
        commits = {corpus_payload.get("repository_commit"), pack_commit, suite["repository_commit"]}
        if len(commits) != 1 or None in commits:
            pair_note("commit_mismatch", ("T00", "T12"), "corpus_mismatch")
    if root.resolve() == repo.resolve() or repo.resolve() in root.resolve().parents:
        pair_note("output_inside_repository", ("T14",), "infra")

    host_start_payload = read_note(resolved["host_start"], "host_start", ("T12",))
    host_end_payload = read_note(resolved["host_end"], "host_end", ("T12",))
    host_start_digest = sha_note(resolved["host_start"], "host_start_bytes", ("T12",))
    host_end_digest = sha_note(resolved["host_end"], "host_end_bytes", ("T12",))
    if host_start_digest != manifest["host"]["start_digest"]:
        pair_note("host_start_digest_mismatch", ("T12",))
    if host_end_digest != manifest["host"]["end_digest"]:
        pair_note("host_end_digest_mismatch", ("T12",))
    check_record_digest: str | None = None
    if isinstance(host_start_payload, dict) and isinstance(host_end_payload, dict):
        check_record_digest = digest(
            canonical({"start": host_start_payload, "end": host_end_payload})
        )
        if check_record_digest != provenance_claims["host"]["check_record_digest"]:
            pair_note("host_check_record_mismatch", ("T12",))

    host_timeline_payload = None
    host_timeline_error = "host_timeline_missing"
    if "host_timeline" in resolved:
        host_timeline_payload = read_note(resolved["host_timeline"], "host_timeline", ("T12",))
        timeline_digest = sha_note(resolved["host_timeline"], "host_timeline_bytes", ("T12",))
        if timeline_digest != manifest["host"]["timeline_digest"]:
            pair_note("host_timeline_digest_mismatch", ("T12",))
            host_timeline_error = "host_timeline_digest_mismatch"
        else:
            try:
                validate_host_timeline(host_timeline_payload, host_profile)
                validate_host_timeline_monitor(host_timeline_payload, resolved["host_timeline_raw"])
            except (
                RunError,
                KeyError,
                TypeError,
                ValueError,
                OSError,
                host_monitor.EvidenceError,
            ) as exc:
                host_timeline_error = f"host_timeline_unverified: {exc}"
            else:
                host_timeline_error = None

    # Records: validate every rep record, then merge rep-00 combos.
    validated: dict[str, dict] = {}
    rep_records: dict[str, list[str]] = {}
    for path in resolved["records"]:
        try:
            rep = _rep_segment(path, root)
        except RunError:
            pair_note("record_outside_rep_layout", ("T12", "T14"))
            continue
        try:
            run = _validate_single_record(repo, suite, pack, source, path)
            system, strategy = _record_identity(run, f"record {path.name}")
        except (RunError, ValueError) as exc:
            pair_note(f"record_invalid: {exc}", ("T03", "T12"))
            continue
        validated[str(path)] = {"run": run, "rep": rep, "system": system, "strategy": strategy}
        rep_records.setdefault(rep, []).append(str(path))
    expected_reps = {f"rep-{index:02d}" for index in range(protocol_payload.get("repetitions", 0))}
    if set(rep_records) != expected_reps:
        pair_note("declared_rep_set_mismatch", ("T12", "T13"))
    native_reps: dict[str, list[str]] = {}
    for path in resolved["semble_native"]:
        try:
            rep = _rep_segment(path, root)
        except RunError:
            pair_note("native_outside_rep_layout", ("T12", "T14"))
            continue
        native_reps.setdefault(rep, []).append(str(path))
    if set(native_reps) != set(rep_records):
        pair_note("rep_set_mismatch", ("T12",))
    for rep, paths in rep_records.items():
        systems = [validated[p]["system"] for p in paths]
        if systems.count("semble") != 1:
            pair_note(f"rep_semble_count:{rep}", ("T12",))
        if systems.count("quanta") < 1:
            pair_note(f"rep_quanta_missing:{rep}", ("T12",))
        strategies = [validated[p]["strategy"] for p in paths if validated[p]["system"] == "quanta"]
        if sorted(strategies) != sorted(protocol_payload.get("strategies", [])):
            pair_note(f"protocol_lock_strategies_drift:{rep}", ("T12",))
        for record_path in paths:
            entry = validated[record_path]
            observed_routes = set(entry["run"].get("route_provenance", {}))
            expected_routes = (
                set(protocol_payload.get("quanta_routes", []))
                if entry["system"] == "quanta"
                else {protocol_payload.get("semble_route")}
            )
            if observed_routes != expected_routes:
                pair_note(f"declared_route_coverage_drift:{rep}:{entry['system']}", ("T12",))
            expected_profile = protocol_payload.get("execution_profiles", {}).get(entry["system"])
            captures = entry["run"].get("captures", {})
            if any(
                capture.get("execution_profile") != expected_profile
                or capture.get("execution_profile_sha256")
                != digest(canonical_bytes(expected_profile))
                for capture in captures.values()
            ):
                pair_note(f"execution_profile_record_drift:{rep}:{entry['system']}", ("T11", "T12"))
    for rep, paths in native_reps.items():
        if len(paths) != 1:
            pair_note(f"rep_native_count:{rep}", ("T12",))

    quanta_by_strategy: dict[str, str] = {}
    semble_rep0: str | None = None
    for path in rep_records.get("rep-00", []):
        entry = validated[path]
        if entry["system"] == "semble":
            semble_rep0 = path
        elif entry["strategy"] in quanta_by_strategy:
            pair_note(f"duplicate_strategy_record:{entry['strategy']}", ("T12",))
        else:
            quanta_by_strategy[entry["strategy"]] = path
    combos: dict[str, tuple[dict, str]] = {}
    if semble_rep0 is not None:
        for strategy in sorted(quanta_by_strategy):
            try:
                _suite, _pack, merged = _merge_validated_records(
                    repo,
                    suite,
                    pack,
                    source,
                    [Path(quanta_by_strategy[strategy]), Path(semble_rep0)],
                )
            except (RunError, ValueError):
                pair_note(f"combo_merge_failed:{strategy}", ("T03", "T12", "T13"))
                continue
            combos[strategy] = (merged, digest(canonical(merged)))

    matched: list[dict] = []
    for path in resolved["reports"]:
        content = read_note(path, "report", ("T13",))
        if not isinstance(content, dict):
            continue
        key = content.get("runner_record_sha256")
        hits = [s for s, (_merged, md) in combos.items() if md == key]
        if len(hits) != 1:
            pair_note("report_without_unique_combo", ("T13",))
            continue
        strategy = hits[0]
        merged, merged_digest = combos[strategy]
        if (
            protocol_payload.get("execution_profiles", {}).get("quanta", {}).get("policy")
            in qp.FILE_PAIR_POLICIES
        ):
            try:
                report_digest = sha_note(path, "report_bytes", ("T13",))
                if manifest["scope"] == "qualified":
                    matched.append(
                        replay_complete_scored_file_report(
                            suite, pack, merged, content, strategy, report_digest
                        )
                    )
                else:
                    matched.append(
                        replay_paired_file_diagnostic_report(
                            suite, pack, merged, content, strategy, report_digest
                        )
                    )
            except (KeyError, TypeError, ValueError, RunError):
                pair_note(
                    "complete_file_report_rescore_failed"
                    if manifest["scope"] == "qualified"
                    else "diagnostic_file_report_rescore_failed",
                    ("T04", "T13"),
                )
            continue
        comparison = (
            content.get("rank_metrics", {}).get("comparison", {})
            if isinstance(content.get("rank_metrics"), dict)
            else {}
        )
        try:
            rescored = evaluate(
                suite,
                pack,
                merged,
                comparison.get("baseline"),
                comparison.get("candidate"),
                strict_k=protocol_payload.get("rank_metric_k_policy") == "declared_top_k_v1",
            )
            cluster_ci = qualified_query_family_ci(
                suite, rescored, comparison.get("baseline"), comparison.get("candidate")
            )
        except (RunError, ValueError, TypeError):
            pair_note("report_rescore_failed", ("T04", "T13"))
            continue
        if digest(canonical(rescored)) != digest(canonical(content)):
            pair_note("report_not_reproducible", ("T13",))
            continue
        rank_comparison = rescored["rank_metrics"]["comparison"]
        primary_delta = rank_comparison["primary_delta"]
        if primary_delta == "not_applicable":
            primary_delta = None
        report_digest = sha_note(path, "report_bytes", ("T13",))
        matched.append(
            {
                "strategy": strategy,
                "baseline_route": rank_comparison["baseline"],
                "candidate_route": rank_comparison["candidate"],
                "primary_metric": rank_comparison["primary_metric"],
                "primary_delta": primary_delta,
                "sample_count": rank_comparison["sample_count"],
                "paired_wins": rank_comparison["paired_wins"],
                "paired_losses": rank_comparison["paired_losses"],
                "paired_ties": rank_comparison["paired_ties"],
                "record_digest": merged_digest,
                "report_digest": report_digest,
                "graded": bool(rescored.get("graded")),
                "primary_delta_ci_95": rank_comparison["primary_delta_ci_95"],
                "primary_delta_cluster_ci_95": cluster_ci,
                "stratified_primary_delta": rank_comparison["stratified_primary_delta"],
                "no_answer_abstention_delta": rank_comparison["no_answer_abstention_delta"],
                "report_sha": digest(canonical(content)),
            }
        )
    for strategy in quanta_by_strategy:
        if strategy not in {entry["strategy"] for entry in matched}:
            pair_note(f"strategy_without_report:{strategy}", ("T12", "T13"))
    expected_report_keys = {
        (strategy, protocol_payload.get("semble_route"), route)
        for strategy in protocol_payload.get("strategies", [])
        for route in protocol_payload.get("quanta_routes", [])
    }
    observed_report_keys = [
        (entry["strategy"], entry["baseline_route"], entry["candidate_route"]) for entry in matched
    ]
    if len(observed_report_keys) != len(set(observed_report_keys)):
        pair_note("duplicate_logical_report", ("T12", "T13"))
    if set(observed_report_keys) != expected_report_keys:
        pair_note("declared_report_set_mismatch", ("T12", "T13"))

    # T10: one model identity across strategies, rebuilt sources per strategy.
    rep0_captures = []
    for path in rep_records.get("rep-00", []):
        entry = validated.get(path)
        if entry is not None and entry["system"] == "quanta":
            _cid, capture = next(iter(entry["run"]["captures"].items()))
            rep0_captures.append(capture)
    if rep0_captures:
        models = {(c.get("model"), c.get("model_revision")) for c in rep0_captures}
        if len(models) != 1:
            pair_note("strategy_model_divergence", ("T10",))
        receipts = [c.get("receipt_digest") for c in rep0_captures]
        if len(set(receipts)) != len(receipts):
            pair_note("strategy_receipt_reuse", ("T10",))
    if not _quanta_semantic_capture_identity_matches(
        validated,
        provenance_claims["quanta"]["embedder"],
        set(protocol_payload.get("quanta_routes", [])),
    ):
        pair_note("embedder_revision_mismatch", ("T10",))

    # Record <-> capture-manifest binding.
    bound_records: set[str] = set()
    record_digests = {sha_file(Path(path)) for path in resolved["records"]}
    bound_preflights: list[Path] = []
    phase_record_digests: list[str] = []
    phase_by_record: dict[str, dict] = {}
    phase_ok = len(resolved["phase_metrics"]) == len(resolved["records"])
    expected_query_schedule = [task["task_id"] for task in pack["tasks"]]
    for ref, path in zip(artifacts["phase_metrics"], resolved["phase_metrics"], strict=True):
        try:
            if sha_file(Path(path)) != artifacts["phase_metrics_digests"][ref]:
                raise RunError("phase metrics digest differs from the capture manifest")
            metrics = _validate_phase_metrics(read_json(Path(path)), f"phase metrics {path}")
            if metrics["schema_version"] not in (
                (2, 3, 4) if metrics["system"] == "quanta" else (2,)
            ):
                raise RunError("current pair replay requires Quanta phase v2/v3/v4 or Semble v2")
            if (
                metrics["system"] == "quanta"
                and protocol_payload.get("lock_version") in (5, 6)
                and metrics["schema_version"] not in (3, 4)
            ):
                raise RunError("protocol v5 requires measured Quanta phase schema v3/v4")
            if (
                metrics["system"] == "quanta"
                and protocol_payload.get("lock_version") == 6
                and metrics["schema_version"] != 4
            ):
                raise RunError("protocol v6 requires measured Quanta phase schema v4")
            if metrics["system"] == "quanta":
                _verify_symbol_coverage_corpus(metrics, corpus_payload)
                bound_preflights.append(
                    symbol_coverage.verify_artifact(
                        metrics,
                        Path(path),
                        corpus_payload,
                        expected_timeout_total_ms=protocol_payload.get(
                            "symbol_total_timeout_ms", 120_000
                        ),
                    ).resolve()
                )
                if metrics["symbol_coverage_policy"] != protocol_payload.get(
                    "symbol_coverage_policy"
                ):
                    raise RunError("symbol coverage admission policy differs from frozen protocol")
            phase_record_digests.append(metrics["record_sha256"])
            if metrics["record_sha256"] in phase_by_record:
                phase_ok = False
            phase_by_record[metrics["record_sha256"]] = metrics
            if metrics["query_schedule"] != expected_query_schedule:
                phase_ok = False
            if (
                manifest["scope"] == "qualified"
                and claims["speed"]
                and "query_protocol" in metrics
                and metrics["warmup_passes"] < 1
            ):
                phase_ok = False
        except (RunError, ValueError, OSError) as exc:
            phase_ok = False
            pair_note(f"phase_metrics_invalid:{exc}", ("T12",))
    if sorted(bound_preflights) != sorted(resolved["symbol_preflights"]) or len(
        bound_preflights
    ) != len(set(bound_preflights)):
        phase_ok = False
        pair_note("symbol_preflight_archive_inventory_mismatch", ("T12",))
    if set(phase_record_digests) != record_digests or len(phase_record_digests) != len(
        record_digests
    ):
        phase_ok = False
    protocol_root_drifts: list[str] = []
    for index, rep in enumerate(sorted(rep_records, key=_rep_sort_key)):
        protocols = []
        for path in rep_records[rep]:
            phase = phase_by_record.get(sha_file(Path(path)))
            if not isinstance(phase, dict) or "query_protocol" not in phase:
                protocols = []
                break
            protocols.append(phase["query_protocol"])
        expected_digests = protocol_payload.get("query_protocol_sha256s", [])
        if (
            not protocols
            or any(protocol != protocols[0] for protocol in protocols[1:])
            or index >= len(expected_digests)
            or protocols[0]["sha256"] != expected_digests[index]
            or protocols[0]["seed"] != protocol_payload.get("base_seed", -1) + index
            or len(protocols[0]["warmup_schedules"]) != protocol_payload.get("query_warmup_passes")
            or len(protocols[0]["measurement_schedules"])
            != protocol_payload.get("query_repetitions_per_root")
        ):
            protocol_root_drifts.append(rep)

    resource_ok = len(resolved["resource_metrics"]) == len(resolved["records"])
    resource_subject_digests: list[str] = []
    resource_by_subject: dict[str, dict] = {}
    resource_isolation: list[dict | None] = []
    for path in resolved["resource_metrics"]:
        try:
            metrics = _validate_resource_metrics(read_json(Path(path)), f"resource metrics {path}")
            _validate_resource_capture_binding(
                metrics,
                host_system=host_profile["fingerprint"]["system"],
                scope=manifest["scope"],
                manifest_parent=parent_binding,
                protocol_parent=protocol_payload.get("delegated_cgroup_parent"),
            )
            resource_subject_digests.append(metrics["subject_sha256"])
            if metrics["subject_sha256"] in resource_by_subject:
                resource_ok = False
            resource_by_subject[metrics["subject_sha256"]] = metrics
            resource_isolation.append(metrics.get("isolation"))
        except (RunError, ValueError, OSError):
            resource_ok = False
    if set(resource_subject_digests) != record_digests or len(resource_subject_digests) != len(
        record_digests
    ):
        resource_ok = False
    for path, entry in validated.items():
        subject_digest = sha_file(Path(path))
        metrics = resource_by_subject.get(subject_digest)
        if metrics is None:
            resource_ok = False
            continue
        expected_storage = "memory" if entry["system"] == "semble" else "disk"
        if metrics["storage"]["index_storage"] != expected_storage:
            resource_ok = False
    expected_semble_profile = protocol_payload.get("execution_profiles", {}).get("semble")
    expected_query_sha256 = {
        task["task_id"]: task["query_sha256"] for task in pack.get("tasks", [])
    }
    native_binding_fields = (
        "actual_alpha_by_task",
        "lane_call_counts",
        "execution_events_sha256",
        "function_identity",
        "observed_wrapped_call_ns",
        "rerank_applied",
        "requested_alpha",
    )
    validated_semble_native: dict[str, dict] = {}
    for path in resolved["semble_native"]:
        native = None
        try:
            rep = _rep_segment(Path(path), root)
            semble_records = [
                record_path
                for record_path in rep_records.get(rep, [])
                if validated[record_path]["system"] == "semble"
            ]
            if len(semble_records) != 1:
                raise RunError("Semble native artifact lacks one record owner")
            native = read_json(Path(path))
            metrics = resource_by_subject[sha_file(Path(semble_records[0]))]
            stats = native.get("stats") if isinstance(native, dict) else None
            if not isinstance(stats, dict):
                raise RunError("Semble native artifact lacks index statistics")
            if (
                stats.get("index_resident_bytes") != metrics["storage"]["index_bytes"]
                or stats.get("index_measurement") != metrics["storage"]["index_measurement"]
            ):
                raise RunError("Semble index memory attribution differs from native evidence")
        except (KeyError, RunError, ValueError, OSError):
            resource_ok = False
        try:
            if not isinstance(expected_semble_profile, dict):
                raise RunError("protocol lock lacks the Semble execution profile")
            rep = _rep_segment(Path(path), root)
            semble_records = [
                record_path
                for record_path in rep_records.get(rep, [])
                if validated[record_path]["system"] == "semble"
            ]
            if len(semble_records) != 1:
                raise RunError("Semble native artifact lacks one record owner")
            record_digest = sha_file(Path(semble_records[0]))
            phase = phase_by_record.get(record_digest)
            if not isinstance(phase, dict) or phase.get("schema_version") != 2:
                raise RunError("Semble native artifact lacks current phase metrics")
            if not isinstance(native, dict):
                raise RunError("Semble native artifact must be an object")
            semble_adapter.validate_native_profile_report(
                native,
                expected_semble_profile["mode"],
                expected_semble_profile["alpha"],
                expected_query_sha256=expected_query_sha256,
            )
            native_phase_bindings = {
                "profile": native["semble_profile"],
                "requested_alpha": native["requested_alpha"],
                "rerank_applied": native["rerank_applied"],
                "lane_call_counts": native["lane_call_counts"],
                "execution_events_sha256": native["execution_events_sha256"],
                "function_identity": native["function_identity"],
                "observed_wrapped_call_ns": native["observed_wrapped_call_ns"],
            }
            if any(phase.get(key) != value for key, value in native_phase_bindings.items()):
                raise RunError("Semble native actual-call evidence differs from phase metrics")
            if native.get("query_protocol") != phase.get("query_protocol"):
                raise RunError("Semble native query protocol differs from phase metrics")
            if native.get("query_timing") != phase.get("query_timing"):
                raise RunError("Semble completed-response timing differs from phase metrics")
            if "query_timing" in phase and "warm_latencies_ms" in phase:
                route = next(iter(phase["warm_latencies_ms"]))
                if (
                    native.get("latencies_ms") != phase["warm_latencies_ms"][route]
                    or native.get("cold_latency_ms") != phase["cold_latencies_ms"][route]
                ):
                    raise RunError("Semble completed-response samples differ from phase metrics")
            validated_semble_native[rep] = {
                field: native.get(field) for field in native_binding_fields
            }
        except (KeyError, RunError, ValueError, OSError) as exc:
            phase_ok = False
            pair_note(f"semble_native_actual_call_invalid:{exc}", ("T11", "T12"))
        finally:
            native = None

    for path in resolved["quanta_manifests"]:
        content = read_note(path, "quanta_manifest", ("T12",))
        if not isinstance(content, dict) or not isinstance(content.get("runs"), list):
            pair_note("quanta_manifest_malformed", ("T12",))
            resource_ok = False
            continue
        for run_entry in content["runs"]:
            if not isinstance(run_entry, dict):
                pair_note("quanta_manifest_malformed", ("T12",))
                resource_ok = False
                continue
            index_bytes = run_entry.get("index_bytes")
            if type(index_bytes) is not int or index_bytes < 0:
                resource_ok = False
            for metric_key in ("phase_metrics", "symbol_preflight", "resource_metrics"):
                metric_ref = run_entry.get(metric_key)
                metric_digest = run_entry.get(f"{metric_key}_digest")
                if not isinstance(metric_ref, str) or not _is_hex(metric_digest, 64):
                    if metric_key in ("phase_metrics", "symbol_preflight"):
                        phase_ok = False
                    else:
                        resource_ok = False
                    continue
                metric_path = (path.parent / metric_ref).resolve()
                try:
                    actual_metric_digest = sha_file(metric_path)
                except OSError:
                    actual_metric_digest = None
                if actual_metric_digest != metric_digest:
                    if metric_key in ("phase_metrics", "symbol_preflight"):
                        phase_ok = False
                    else:
                        resource_ok = False
            ref = run_entry.get("record")
            want = run_entry.get("record_digest")
            try:
                phase_path = _resolve_artifact(
                    path.parent, run_entry.get("phase_metrics"), "capture phase"
                )
                preflight_path = _resolve_artifact(
                    path.parent, run_entry.get("symbol_preflight"), "capture preflight"
                )
                phase_payload = read_json(phase_path)
                if (
                    phase_payload["record_sha256"] != want
                    or preflight_path
                    != (phase_path.parent / phase_payload["symbol_preflight_out"]).resolve()
                    or sha_file(preflight_path) != phase_payload["symbol_preflight_sha256"]
                ):
                    raise RunError("capture preflight owner binding differs")
            except (KeyError, TypeError, ValueError, OSError) as exc:
                phase_ok = False
                pair_note(f"symbol_preflight_capture_binding:{exc}", ("T12",))
            target = None
            if isinstance(ref, str) and ref:
                candidate = Path(ref)
                target = candidate if candidate.is_absolute() else (path.parent / candidate)
                try:
                    target = target.resolve()
                except OSError:
                    target = None
            if target is None or root.resolve() not in target.parents:
                pair_note("quanta_manifest_record_escape", ("T12", "T14"))
                continue
            try:
                observed = sha_file(target)
            except OSError:
                pair_note("quanta_manifest_record_missing", ("T12",))
                continue
            if observed != want:
                pair_note("record_digest_mismatch", ("T12", "T13"))
            else:
                bound_records.add(observed)
            diagnostic_ref = run_entry.get("retrieval_diagnostic")
            diagnostic_digest = run_entry.get("retrieval_diagnostic_digest")
            if protocol_payload.get("retrieval_diagnostic_version") in (2, 3, 4, 5, 6, 7, 8) and (
                diagnostic_ref is None or diagnostic_digest is None
            ):
                pair_note("retrieval_diagnostic_missing", ("T12",))
            if diagnostic_ref is not None or diagnostic_digest is not None:
                try:
                    if not isinstance(diagnostic_ref, str) or not _is_hex(diagnostic_digest, 64):
                        raise RunError("diagnostic reference/digest is incomplete")
                    diagnostic_path = (path.parent / diagnostic_ref).resolve()
                    if root.resolve() not in diagnostic_path.parents:
                        raise RunError("diagnostic path escapes pair output")
                    if sha_file(diagnostic_path) != diagnostic_digest:
                        raise RunError("diagnostic digest mismatch")
                    record_payload = read_json(target)
                    projected_pack, _ = project_pack_and_suite(
                        pack_payload,
                        suite_payload,
                        sorted(record_payload["route_provenance"]),
                    )
                    diagnostic = validate_retrieval_diagnostic(
                        read_json(diagnostic_path), record_payload, observed, projected_pack
                    )
                    if diagnostic["schema_version"] != protocol_payload.get(
                        "retrieval_diagnostic_version"
                    ):
                        raise RunError(
                            "retrieval diagnostic version differs from the current protocol lock"
                        )
                    if diagnostic["schema_version"] in (5, 6, 7, 8) and diagnostic[
                        "server_observation"
                    ] != protocol_payload.get("server_observation"):
                        raise RunError(
                            "retrieval diagnostic server configuration differs from protocol"
                        )
                    if diagnostic["schema_version"] in (6, 7, 8) and diagnostic[
                        "hybrid_fetch_policy"
                    ] != protocol_payload.get("hybrid_fetch_policy"):
                        raise RunError(
                            "retrieval diagnostic hybrid fetch policy differs from protocol"
                        )
                    if diagnostic["schema_version"] in (5, 6, 7, 8) and _validate_ingest_diagnostic(
                        diagnostic["ingest"],
                        record_payload,
                        lexical_stage_contract=diagnostic["schema_version"] in (7, 8),
                        detailed_authority=diagnostic["schema_version"] == 8,
                    ) != protocol_payload.get("ingest_request_identity"):
                        raise RunError("retrieval diagnostic ingest identity differs from protocol")
                except (KeyError, TypeError, ValueError, OSError) as exc:
                    pair_note(f"retrieval_diagnostic_invalid:{exc}", ("T12",))
    for path, entry in validated.items():
        if entry["system"] != "quanta":
            continue
        try:
            file_digest = sha_file(Path(path))
        except OSError:
            pair_note("record_unreadable", ("T12",))
            continue
        if file_digest not in bound_records:
            pair_note("record_without_manifest_binding", ("T12",))

    adapter = read_note(resolved["semble_adapter_manifest"], "adapter_manifest", ("T11",))
    lockfile_digest = sha_note(resolved["semble_lockfile"], "lockfile_bytes", ("T11",))
    if protocol_payload.get("semble_lockfile_sha256") != lockfile_digest:
        pair_note("protocol_lock_semble_pin_drift", ("T11", "T12"))
    if isinstance(adapter, dict):
        if adapter.get("semble_version") != SEMBLE_PINNED_VERSION:
            pair_note("semble_version_drift", ("T11",))
        if semble_rep0 is not None:
            try:
                rep0_digest = sha_file(Path(semble_rep0))
            except OSError:
                rep0_digest = None
            if adapter.get("record_digest") != rep0_digest:
                pair_note("semble_record_binding_broken", ("T11", "T12"))
        if adapter.get("lockfile_digest") != lockfile_digest:
            pair_note("adapter_lockfile_mismatch", ("T11",))
        if lockfile_digest != provenance_claims["semble"]["lockfile_digest"]:
            pair_note("lockfile_digest_mismatch", ("T11",))
        if (
            adapter.get("interpreter", {}).get("digest")
            != provenance_claims["semble"]["interpreter_digest"]
        ):
            pair_note("interpreter_digest_mismatch", ("T11",))
        if adapter.get("model_asset_digest") != provenance_claims["semble"]["model_asset_digest"]:
            pair_note("model_asset_digest_mismatch", ("T11",))
        model_cache_paths = resolved.get("semble_model_cache_manifests", [])
        if not isinstance(model_cache_paths, list) or len(model_cache_paths) != len(expected_reps):
            pair_note("model_cache_manifest_set_mismatch", ("T11", "T12"))
        else:
            seen_model_reps = set()
            model_cache_identities = set()
            for model_cache_path in model_cache_paths:
                try:
                    rep = _rep_segment(Path(model_cache_path), root)
                    cache_manifest = _validate_model_cache_manifest(
                        read_json(Path(model_cache_path)), f"model cache manifest {rep}"
                    )
                    if rep in seen_model_reps:
                        raise RunError("duplicate model cache manifest root")
                    seen_model_reps.add(rep)
                    if cache_manifest["revision"] != adapter.get("model_revision"):
                        raise RunError("model cache revision differs from adapter")
                    if cache_manifest["model_id"] != adapter.get("model_id"):
                        raise RunError("model cache id differs from adapter")
                    if cache_manifest["model_asset_digest"] != adapter.get("model_asset_digest"):
                        raise RunError("model cache asset digest differs from adapter")
                    model_cache_identities.add(
                        (
                            cache_manifest["model_id"],
                            cache_manifest["revision"],
                            cache_manifest["model_asset_digest"],
                            cache_manifest["snapshot_digest"],
                        )
                    )
                except (RunError, ValueError, OSError) as exc:
                    pair_note(f"model_cache_manifest_invalid:{exc}", ("T11", "T12"))
            if seen_model_reps != expected_reps:
                pair_note("model_cache_manifest_root_mismatch", ("T11", "T12"))
            if len(model_cache_identities) != 1:
                pair_note("model_cache_identity_drift", ("T11", "T12"))
            rep0_model_cache = next(
                (Path(path) for path in model_cache_paths if "rep-00" in Path(path).parts),
                None,
            )
            if rep0_model_cache is None or adapter.get("model_cache_manifest_digest") != sha_file(
                rep0_model_cache
            ):
                pair_note("adapter_model_cache_binding_broken", ("T11", "T12"))
        if protocol_payload.get("retrieval_diagnostic_version") in (3, 4, 5, 6, 7, 8):
            expected_profile = protocol_payload.get("execution_profiles", {}).get("semble", {})
            expected_mode = expected_profile.get("mode")
            expected_alpha = expected_profile.get("alpha")
            if adapter.get("profile") != expected_profile:
                pair_note("semble_profile_drift", ("T11", "T12"))
            if adapter.get("requested_alpha") != expected_alpha:
                pair_note("semble_alpha_drift", ("T11", "T12"))
            expected_rerank = expected_mode == "native-default"
            if adapter.get("rerank_applied") is not expected_rerank:
                pair_note("semble_rerank_profile_drift", ("T11", "T12"))
            lane_counts = adapter.get("lane_call_counts")
            if (
                not isinstance(lane_counts, dict)
                or set(lane_counts) != {"bm25", "semantic", "encode"}
                or any(type(value) is not int or value < 0 for value in lane_counts.values())
            ):
                pair_note("semble_lane_counts_malformed", ("T11",))
            elif (
                expected_mode in ("lexical-only", "lexical-file")
                and (lane_counts["bm25"] <= 0 or lane_counts["semantic"] or lane_counts["encode"])
                or expected_mode == "semantic-only"
                and (lane_counts["bm25"] or lane_counts["semantic"] <= 0)
                or expected_mode in ("native-default", "hybrid-no-rerank")
                and (lane_counts["bm25"] <= 0 or lane_counts["semantic"] <= 0)
            ):
                pair_note("semble_lane_profile_drift", ("T11", "T12"))
            rep0_native = validated_semble_native.get("rep-00")
            if not isinstance(rep0_native, dict) or any(
                adapter.get(field) != rep0_native.get(field) for field in native_binding_fields
            ):
                pair_note("adapter_native_actual_call_binding_broken", ("T11", "T12"))
    mapping_diff = mapping_payload.get("diff_digest") if isinstance(mapping_payload, dict) else None
    for _path, entry in validated.items():
        if entry["system"] != "semble":
            continue
        _cid, capture = next(iter(entry["run"]["captures"].items()))
        if capture.get("receipt_digest") != mapping_diff:
            pair_note("semble_receipt_anchor_drift", ("T11", "T12"))

    quanta_binaries = set()
    quanta_searchd_binaries = set()
    for _path, entry in validated.items():
        if entry["system"] == "quanta":
            _cid, capture = next(iter(entry["run"]["captures"].items()))
            quanta_binaries.add(capture.get("runner_binary", {}).get("digest"))
            quanta_searchd_binaries.add(capture.get("searchd_binary", {}).get("binary_digest"))
    binary_digest = sorted(quanta_binaries)[0] if quanta_binaries else "0" * 64
    if len(quanta_binaries) != 1:
        pair_note("runner_binary_divergence", ("T12",))
    elif binary_digest != provenance_claims["quanta"]["binary_digest"]:
        pair_note("binary_digest_mismatch", ("T12",))
    if len(quanta_searchd_binaries) != 1:
        pair_note("searchd_binary_divergence", ("T12",))
    elif next(iter(quanta_searchd_binaries)) != protocol_payload.get("searchd_expected_sha256"):
        pair_note("protocol_lock_searchd_pin_drift", ("T12",))

    admission_evidence = None
    admission_error = None
    if manifest["scope"] == "qualified":
        try:
            annotation_paths = resolved.get("annotation_receipts")
            if not isinstance(annotation_paths, list):
                raise RunError("qualified verdict lacks annotation receipts")
            quanta_model_revision = _quanta_admission_model_revision(
                entry["run"] for entry in validated.values() if entry["system"] == "quanta"
            )
            if not isinstance(adapter, dict):
                raise RunError("qualified verdict lacks the Semble adapter manifest")
            receipt_paths = {
                key: resolved[key]
                for key in ("contract_python_receipt", "contract_rust_receipt", "sdk_receipt")
                if key in resolved
            }
            driver_closure = _validate_source_closure_shape(
                read_json(resolved["driver_source_closure"]), "driver source closure"
            )
            for key, path in receipt_paths.items():
                receipt = _validate_receipt_shape(read_json(path), key)
                if receipt["source_closure"]["digest"] != driver_closure["digest"]:
                    raise RunError(f"{key} source closure differs from capture closure")
            admission_evidence = verify_admission_bundle(
                resolved["admission_manifest"],
                resolved["license_receipt"],
                annotation_paths,
                resolved["adjudication_receipt"],
                source_revision=provenance_claims["quanta"]["source_sha"],
                corpus_manifest_path=resolved["corpus_manifest"],
                suite_path=resolved["suite"],
                development_suite_path=resolved.get("development_suite"),
                experiment_custody_path=resolved.get("experiment_custody"),
                split_manifest_path=resolved.get("split_manifest"),
                split_releases_path=resolved.get("split_releases"),
                repo=repo,
                query_pack_path=resolved["query_pack"],
                lockfile_path=resolved["semble_lockfile"],
                host_profile_path=resolved["host_profile"],
                cache_regime=manifest["host"]["cache_regime"],
                receipt_paths=receipt_paths,
                quanta_model_revision=quanta_model_revision,
                semble_model_revision=adapter.get("model_revision"),
                semble_model_asset_sha256=adapter.get("model_asset_digest"),
            )
            observed_admission_digest = sha_file(resolved["admission_manifest"])
            if observed_admission_digest != provenance_claims["admission"]["manifest_digest"]:
                raise RunError("qualification admission manifest digest mismatch")
        except (RunError, ValueError, OSError) as exc:
            admission_error = str(exc)

    # T12: error/timeout/unavailable rows are incomplete observations — the
    # pair never silently compares a system that failed to observe a query.
    for _path, entry in validated.items():
        rows = entry["run"].get("results", [])
        bad = sorted(
            {
                row["task_id"]
                for row in rows
                if row.get("status") in ("error", "timeout", "unavailable")
            }
        )
        if bad:
            pair_note(
                f"incomplete_observation:{entry['rep']}:{entry['system']}"
                f":{entry['strategy']}:{','.join(bad)}",
                ("T12",),
            )
    for rep in protocol_root_drifts:
        pair_note(f"protocol_lock_root_drift:{rep}", ("T12",))

    pair_t_ids: list[str] = []
    for _reason, t_ids, _class in pair_notes:
        pair_t_ids.extend(t_ids)
    pair_proof = (
        digest(
            canonical(
                {
                    "mapping": mapping_payload,
                    "reports": sorted(entry["report_sha"] for entry in matched),
                }
            )
        )
        if not pair_notes
        else None
    )
    pair_state = "fail" if pair_notes else "pass"
    pair_reason = pair_notes[0][0] if pair_notes else "mapping_reports_rederived"
    pair_class = pair_notes[0][2] if pair_notes else ""

    # CONTRACT_GREEN + SDK_PATH_GREEN from frozen receipt bytes.
    states: dict[str, str] = {}
    state_evidence: dict[str, dict] = {}
    missing: list[str] = []
    not_applicable: list[str] = []
    classes: list[str] = []

    def set_state(name: str, value: str, reason: str, proof: str | None) -> None:
        states[name] = value
        state_evidence[name] = {"reason": reason, "proof_digest": proof}

    verified_source_digests: set[str] = set()
    contract_ids = ["T01", "T02", "T03", "T04", "T08", "T09"]
    if "contract_suites" not in evidence:
        set_state("CONTRACT_GREEN", "not_run", "no_evidence", None)
        missing.extend(contract_ids)
    else:
        try:
            proofs: dict[str, dict] = {}
            contract_source_digests: set[str] = set()
            if any(key not in resolved for key in CONTRACT_EVIDENCE_KEYS):
                raise RunError("contract execution context or receipt artifacts missing")
            contract_closure = _verify_execution_context(
                resolved["contract_execution_context"],
                resolved["contract_source_closure"],
                resolved["contract_execution_logs"],
                rail="contract",
                raw={
                    "python-inventory.json": resolved["contract_python_inventory"],
                    "rust-inventory.json": resolved["contract_rust_inventory"],
                    "python-junit.xml": resolved["contract_python_raw"],
                    "rust-nextest.jsonl": resolved["contract_rust_raw"],
                },
            )
            contract_authority = {
                "python": (
                    "retrieval-contract-python",
                    portable_proof.PYTHON_COMMAND,
                    "pytest-junit",
                    pytest_summary,
                ),
                "rust": (
                    "retrieval-contract-rust",
                    "./scripts/cargow nextest run -p quanta-index-retrieval-bench "
                    "--lib --test chunking_contract --test l5_parser_regressions --all-features --locked",
                    "nextest-jsonl",
                    nextest_summary,
                ),
            }
            for side, (rail, command, role, producer) in contract_authority.items():
                receipt_ref = f"contract_{side}_receipt"
                results_ref = f"contract_{side}_results"
                raw_ref = f"contract_{side}_raw"
                inventory_ref = f"contract_{side}_inventory"
                if any(
                    ref not in resolved
                    for ref in (receipt_ref, results_ref, raw_ref, inventory_ref)
                ):
                    raise RunError(f"contract {side} artifacts missing")
                receipt = _validate_receipt_shape(
                    read_json(resolved[receipt_ref]), f"contract {side} receipt"
                )
                if receipt["source_closure"] != contract_closure:
                    raise RunError(f"contract {side} execution context source closure mismatch")
                results = _validate_counts_shape(
                    read_json(resolved[results_ref]), f"contract {side} results"
                )
                actual = sha_file(resolved[results_ref])
                if actual != receipt["evidence_sha256"]:
                    raise RunError(f"contract {side} receipt digest mismatch")
                if actual != evidence["contract_suites"][side]["test_result_digest"]:
                    raise RunError(f"contract {side} manifest digest mismatch")
                raw_actual = sha_file(resolved[raw_ref])
                if raw_actual != evidence["contract_suites"][side]["raw_evidence_digest"]:
                    raise RunError(f"contract {side} raw manifest digest mismatch")
                if (
                    sha_file(resolved[inventory_ref])
                    != evidence["contract_suites"][side]["inventory_digest"]
                ):
                    raise RunError(f"contract {side} inventory manifest digest mismatch")
                if receipt["rail"] != rail or receipt["command"] != command:
                    raise RunError(f"contract {side} receipt authority mismatch")
                if results["command"] != command:
                    raise RunError(f"contract {side} command mismatch")
                _verify_receipt_inputs(
                    receipt,
                    {
                        "execution-context": resolved["contract_execution_context"],
                        role: resolved[raw_ref],
                        ("pytest-inventory" if side == "python" else "nextest-inventory"): resolved[
                            inventory_ref
                        ],
                    },
                    f"contract {side} receipt",
                )
                _verify_required_inventory(resolved[inventory_ref], side, receipt)
                try:
                    rebuilt = producer(resolved[raw_ref], resolved[inventory_ref])
                except SystemExit as exc:
                    raise RunError(f"contract {side} raw evidence refused: {exc}") from exc
                if rebuilt != results:
                    raise RunError(f"contract {side} summary is not reproducible")
                _verify_receipt_test_count(receipt, rebuilt, f"contract {side} receipt")
                if receipt["revision"] != provenance_claims["quanta"]["source_sha"]:
                    raise RunError(f"contract {side} revision mismatch")
                if (
                    driver_closure is not None
                    and receipt["source_closure"]["digest"] != driver_closure["digest"]
                ):
                    raise RunError(f"contract {side} source closure differs from capture closure")
                if not (
                    results["failed"] == 0
                    and results["passed"] > 0
                    and results["passed"] + results["failed"] == results["executed"]
                    and results["executed"] <= results["selected"]
                ):
                    raise RunError(f"contract {side} counts inconsistent")
                contract_source_digests.add(receipt["source_closure"]["digest"])
                proofs[side] = results
            if len(contract_source_digests) != 1:
                raise RunError("contract receipts use different source closures")
            verified_source_digests.update(contract_source_digests)
        except (RunError, ValueError, OSError) as exc:
            set_state("CONTRACT_GREEN", "fail", f"contract_refused: {exc}", None)
            missing.extend(contract_ids)
            classes.append("scoring")
        else:
            set_state("CONTRACT_GREEN", "pass", "receipts_verified", digest(canonical(proofs)))

    # This is derived only after the complete SDK receipt chain verifies.
    # The manifest's caller-provided field remains null and is never authority.
    binary_build_source_revision = None
    sdk_ids = ["T05", "T06", "T07"]
    if "sdk_path" not in evidence:
        set_state("SDK_PATH_GREEN", "not_run", "no_evidence", None)
        missing.extend(sdk_ids)
    else:
        try:
            if any(key not in resolved for key in SDK_EVIDENCE_KEYS):
                raise RunError("SDK execution context or receipt artifacts missing")
            sdk_build_revisions: list[str] = []
            sdk_closure = _verify_execution_context(
                resolved["sdk_execution_context"],
                resolved["sdk_source_closure"],
                resolved["sdk_execution_logs"],
                rail="sdk",
                raw={
                    "nextest-inventory.json": resolved["sdk_inventory"],
                    "nextest.jsonl": resolved["sdk_nextest_raw"],
                    "actual-runner-record.json": resolved["sdk_record_raw"],
                },
                runner_sha=binary_digest,
                searchd_sha=protocol_payload.get("searchd_expected_sha256"),
                build_source_revisions=sdk_build_revisions,
            )
            if "sdk_receipt" not in resolved or "sdk_results" not in resolved:
                raise RunError("sdk artifacts missing")
            sdk_receipt = _validate_receipt_shape(read_json(resolved["sdk_receipt"]), "sdk receipt")
            if sdk_receipt["source_closure"] != sdk_closure:
                raise RunError("SDK execution context source closure mismatch")
            sdk_results = _validate_sdk_results_shape(
                read_json(resolved["sdk_results"]), "sdk results"
            )
            sdk_actual = sha_file(resolved["sdk_results"])
            if sdk_actual != sdk_receipt["evidence_sha256"]:
                raise RunError("sdk receipt digest mismatch")
            if sdk_actual != evidence["sdk_path"]["test_result_digest"]:
                raise RunError("sdk manifest digest mismatch")
            sdk_command = (
                "just retrieval-sdk-proof-fresh"
                if sdk_build_revisions
                else "just retrieval-sdk-proof"
            )
            if (
                sdk_receipt["rail"] != "retrieval-sdk-proof"
                or sdk_receipt["command"] != sdk_command
            ):
                raise RunError("sdk receipt authority mismatch")
            if sdk_results["command"] != sdk_command:
                raise RunError("sdk results command mismatch")
            if any(
                ref not in resolved
                for ref in ("sdk_nextest_raw", "sdk_record_raw", "sdk_inventory")
            ):
                raise RunError("sdk raw artifacts missing")
            if sha_file(resolved["sdk_nextest_raw"]) != evidence["sdk_path"]["nextest_digest"]:
                raise RunError("sdk nextest manifest digest mismatch")
            if sha_file(resolved["sdk_record_raw"]) != evidence["sdk_path"]["runner_record_digest"]:
                raise RunError("sdk record manifest digest mismatch")
            if sha_file(resolved["sdk_inventory"]) != evidence["sdk_path"]["inventory_digest"]:
                raise RunError("sdk inventory manifest digest mismatch")
            _verify_receipt_inputs(
                sdk_receipt,
                {
                    "execution-context": resolved["sdk_execution_context"],
                    "nextest-jsonl": resolved["sdk_nextest_raw"],
                    "runner-record": resolved["sdk_record_raw"],
                    "nextest-inventory": resolved["sdk_inventory"],
                },
                "sdk receipt",
            )
            _verify_required_inventory(resolved["sdk_inventory"], "sdk", sdk_receipt)
            try:
                rebuilt_sdk = build_summary_from_evidence(
                    resolved["sdk_record_raw"],
                    resolved["sdk_nextest_raw"],
                    binary_digest,
                    resolved["sdk_inventory"],
                    command=sdk_command,
                )
            except SystemExit as exc:
                raise RunError(f"sdk raw evidence refused: {exc}") from exc
            if rebuilt_sdk != sdk_results:
                raise RunError("sdk summary is not reproducible")
            _verify_receipt_test_count(sdk_receipt, rebuilt_sdk, "sdk receipt")
            if sdk_receipt["revision"] != provenance_claims["quanta"]["source_sha"]:
                raise RunError("sdk revision mismatch")
            if (
                driver_closure is not None
                and sdk_receipt["source_closure"]["digest"] != driver_closure["digest"]
            ):
                raise RunError("sdk source closure differs from capture closure")
            if verified_source_digests and sdk_receipt["source_closure"]["digest"] not in (
                verified_source_digests
            ):
                raise RunError("sdk/contract source closure mismatch")
            claimed = evidence["sdk_path"]
            for key in ("separate_process", "empty_check"):
                if claimed[key] is not True or sdk_results[key] is not True:
                    raise RunError(f"sdk {key} not proven")
            if claimed["sealed_receipt"] is not True or claimed["activation_ack"] is not True:
                raise RunError("sdk receipt/ack not claimed")
            if sdk_results["binary_digest"] != binary_digest:
                raise RunError("sdk binary differs from pair captures")
            if not (
                sdk_results["failed"] == 0
                and sdk_results["passed"] > 0
                and sdk_results["passed"] + sdk_results["failed"] == sdk_results["executed"]
                and sdk_results["executed"] <= sdk_results["selected"]
            ):
                raise RunError("sdk counts inconsistent")
        except (RunError, ValueError, OSError) as exc:
            set_state("SDK_PATH_GREEN", "fail", f"sdk_refused: {exc}", None)
            missing.extend(sdk_ids)
            classes.append("provenance")
        else:
            if sdk_build_revisions:
                binary_build_source_revision = sdk_build_revisions[0]
            set_state(
                "SDK_PATH_GREEN", "pass", "sdk_proof_verified", digest(canonical(sdk_results))
            )

    set_state("PAIR_VALID", pair_state, pair_reason, pair_proof)
    if pair_state == "fail":
        missing.extend(pair_t_ids)
        classes.append(pair_class)
    qualification_dependency = next(
        (
            name
            for name in ("PAIR_VALID", "CONTRACT_GREEN", "SDK_PATH_GREEN")
            if states[name] != "pass"
        ),
        None,
    )

    # PERF_QUALIFIED: matrix re-derivation + floors + host, only on speed claims.
    if not claims["speed"]:
        set_state("PERF_QUALIFIED", "not_applicable", "no_speed_claim", None)
    elif manifest["scope"] != "qualified":
        set_state("PERF_QUALIFIED", "not_applicable", "exploratory_only", None)
        not_applicable.append("scope:exploratory_only")
    elif admission_error is not None or admission_evidence is None:
        set_state(
            "PERF_QUALIFIED",
            "fail",
            f"admission_unverified: {admission_error or 'missing admission'}",
            None,
        )
        missing.append("T17")
        classes.append("admission")
    else:
        if evidence["perf"]["phase_boundaries"] is not True or not phase_ok:
            perf_fail: tuple[str, str] | None = (
                "phase_boundaries_incomplete",
                "provenance",
            )
        elif manifest.get("host", {}).get("cache_regime") != "true_process_cold":
            perf_fail = ("unsupported_cache_protocol", "host")
        else:
            perf_fail = None
        try:
            cells = []
            shared_protocol_ok = True
            protocol_failure_reason = "shared_warm_query_protocol_unimplemented"
            rep_protocol_evidence: list[dict] = []
            for rep in sorted(rep_records, key=_rep_sort_key):
                rep_protocols = []
                quanta_paths = sorted(
                    (p for p in rep_records[rep] if validated[p]["system"] == "quanta"),
                    key=lambda p: (validated[p]["strategy"], p),
                )
                for path in quanta_paths:
                    entry = validated[path]
                    cell = _cell_from_record("quanta", entry["strategy"], entry["run"])
                    phase = phase_by_record.get(sha_file(Path(path)))
                    rep_protocols.append(
                        phase.get("query_protocol") if isinstance(phase, dict) else None
                    )
                    if isinstance(phase, dict) and "warm_latencies_ms" in phase:
                        cell["warm_latencies"] = phase["warm_latencies_ms"]
                    cells.append(cell)
                semble_paths = [p for p in rep_records[rep] if validated[p]["system"] == "semble"]
                if len(semble_paths) != 1:
                    raise RunError(f"rep {rep} lacks exactly one semble record")
                scell = _cell_from_record("semble", "native", validated[semble_paths[0]]["run"])
                phase = phase_by_record.get(sha_file(Path(semble_paths[0])))
                rep_protocols.append(
                    phase.get("query_protocol") if isinstance(phase, dict) else None
                )
                if isinstance(phase, dict) and "warm_latencies_ms" in phase:
                    scell["warm_latencies"] = phase["warm_latencies_ms"]
                else:
                    native_paths = native_reps.get(rep, [])
                    if len(native_paths) != 1:
                        raise RunError(f"rep {rep} lacks exactly one native file")
                    native_content = read_json(Path(native_paths[0]))
                    routes = {
                        row["route"] for row in validated[semble_paths[0]]["run"].get("results", [])
                    }
                    if not isinstance(native_content, dict) or len(routes) != 1:
                        raise RunError(f"rep {rep} native latency evidence is malformed")
                    scell["native_latencies"] = native_content.get("latencies_ms", {})
                    scell["native_route"] = next(iter(routes))
                cells.append(scell)
                if any(protocol is None for protocol in rep_protocols) or any(
                    protocol != rep_protocols[0] for protocol in rep_protocols[1:]
                ):
                    shared_protocol_ok = False
                else:
                    rep_protocol_evidence.append(rep_protocols[0])
            if shared_protocol_ok:
                if not isinstance(protocol_payload, dict):
                    raise RunError("protocol lock is not an object")
                base_seed = protocol_payload.get("base_seed")
                warmup_passes = protocol_payload.get("query_warmup_passes")
                measurement_repetitions = protocol_payload.get("query_repetitions_per_root")
                locked_digests = protocol_payload.get("query_protocol_sha256s")
                if (
                    type(base_seed) is not int
                    or type(warmup_passes) is not int
                    or type(measurement_repetitions) is not int
                    or not isinstance(locked_digests, list)
                    or len(rep_protocol_evidence) != len(rep_records)
                    or len(locked_digests) != len(rep_records)
                ):
                    shared_protocol_ok = False
                    protocol_failure_reason = "query_protocol_root_sequence_unverified"
                else:
                    for index, protocol in enumerate(rep_protocol_evidence):
                        if (
                            protocol["seed"] != base_seed + index
                            or len(protocol["warmup_schedules"]) != warmup_passes
                            or len(protocol["measurement_schedules"]) != measurement_repetitions
                            or protocol["sha256"] != locked_digests[index]
                        ):
                            shared_protocol_ok = False
                            protocol_failure_reason = "query_protocol_root_sequence_unverified"
                            break
            rebuilt = aggregate_matrix(cells, len(rep_records))
            matrix_content = read_json(resolved["latency_matrix"])
        except (RunError, ValueError, OSError) as exc:
            perf_fail = (f"matrix_rebuild_failed: {exc}", "provenance")
            rebuilt = None
            matrix_content = None
        if perf_fail is None:
            if digest(canonical(rebuilt)) != digest(canonical(matrix_content)):
                perf_fail = ("matrix_not_reproducible", "provenance")
            elif rebuilt["observations_floor"] != evidence["perf"]["observations_floor"]:
                perf_fail = ("perf_floor_mismatch", "provenance")
            elif rebuilt["fresh_roots"] != evidence["perf"]["fresh_roots"]:
                perf_fail = ("perf_roots_mismatch", "provenance")
            elif rebuilt["observations_floor"] < PILOT_OBSERVATIONS_FLOOR:
                perf_fail = ("observations_floor_unmet", "provenance")
            elif rebuilt["fresh_roots"] < FRESH_ROOTS_FLOOR:
                perf_fail = ("fresh_roots_unmet", "provenance")
            elif sum(rebuilt["nulls"].values()) > 0:
                perf_fail = ("null_timings_on_speed_claim", "provenance")
            elif evidence["perf"]["resource_accounting"] is not True or not resource_ok:
                perf_fail = ("resource_accounting_incomplete", "provenance")
            elif not (
                isinstance(host_start_payload, dict)
                and isinstance(host_end_payload, dict)
                and _probe_clean(host_start_payload, host_profile)
                and _probe_clean(host_end_payload, host_profile)
            ):
                perf_fail = ("host_contended", "host")
            elif host_timeline_error is not None:
                perf_fail = (host_timeline_error, "host")
            elif not shared_protocol_ok:
                perf_fail = (protocol_failure_reason, "provenance")
            elif any(
                order
                != (
                    protocol_payload["system_orders"][0]
                    if index % 2 == 0
                    else list(reversed(protocol_payload["system_orders"][0]))
                )
                for index, order in enumerate(protocol_payload["system_orders"])
            ):
                perf_fail = ("measurement_order_unverified", "provenance")
        if perf_fail is None:
            try:
                quanta_routes_by_rep = []
                for rep in sorted(rep_records, key=_rep_sort_key):
                    routes = {
                        row["route"]
                        for path in rep_records[rep]
                        if validated[path]["system"] == "quanta"
                        for row in validated[path]["run"]["results"]
                    }
                    quanta_routes_by_rep.append(routes)
                if not quanta_routes_by_rep or any(
                    routes != quanta_routes_by_rep[0] for routes in quanta_routes_by_rep
                ):
                    raise RunError("qualified speed requires identical Quanta routes in every root")
                validate_qualified_speed_spec(
                    {
                        "repetitions": len(rep_records),
                        "query_warmup_passes": protocol_payload.get("query_warmup_passes"),
                        "query_repetitions_per_root": protocol_payload.get(
                            "query_repetitions_per_root"
                        ),
                        "routes": sorted(quanta_routes_by_rep[0]),
                    },
                    len(pack["tasks"]),
                )
            except (RunError, TypeError, ValueError) as exc:
                perf_fail = (f"measurement_protocol_ineligible: {exc}", "provenance")
        if perf_fail is None and qualification_dependency is not None:
            perf_fail = (
                f"qualification_dependency_unverified:{qualification_dependency}",
                "provenance",
            )
        if perf_fail is None:
            try:
                output_units = set()
                for path, entry in validated.items():
                    phase = phase_by_record[sha_file(Path(path))]
                    validate_completed_query_timing(phase, entry["run"])
                    output_units.update(
                        row.get("rank_unit", "source_span") for row in entry["run"]["results"]
                    )
                if len(output_units) != 1:
                    raise RunError("completed-response output units differ across paired products")
            except (RunError, KeyError, TypeError, ValueError) as exc:
                perf_fail = (f"completed_response_timing_unverified: {exc}", "provenance")
        if perf_fail is None and binary_build_source_revision is None:
            perf_fail = ("binary_build_source_unattested", "provenance")
        if perf_fail is None:
            set_state(
                "PERF_QUALIFIED",
                "pass",
                "completed_response_and_resources_verified",
                digest(
                    canonical(
                        {
                            "admission": admission_evidence,
                            "latency_matrix": rebuilt,
                            "host_timeline_digest": manifest["host"]["timeline_digest"],
                            "phase_metrics": [
                                sha_file(Path(path)) for path in resolved["phase_metrics"]
                            ],
                            "resource_metrics": [
                                sha_file(Path(path)) for path in resolved["resource_metrics"]
                            ],
                        }
                    )
                ),
            )
        else:
            set_state("PERF_QUALIFIED", "fail", perf_fail[0], None)
            classes.append(perf_fail[1])

    # QUALITY_DELTA: blinded, graded, in-scope quality only.
    file_quality_policy = (
        protocol_payload.get("execution_profiles", {}).get("quanta", {}).get("policy")
        in qp.QUALIFIED_FILE_PAIR_POLICIES
    )
    isolation_claimed = manifest["blinding"] == "isolated"
    all_isolated = isolation_claimed
    if all_isolated:
        for path in resolved["records"]:
            try:
                payload = read_json(path)
                runner = payload.get("runner", {}) if isinstance(payload, dict) else {}
                blinding = runner.get("blinding")
                if (
                    runner.get("isolation_method") != manifest["isolation_method"]
                    or runner.get("access_block_log") != manifest["access_block_log"]
                ):
                    blinding = None
            except ValueError:
                blinding = None
            if blinding != "isolated":
                all_isolated = False
                break
    isolation_evidence = None
    isolation_error = None
    if all_isolated:
        try:
            if "isolation_proof" not in resolved:
                raise RunError("isolation proof artifact is missing")
            isolation_evidence = _validate_isolation_proof(
                read_json(resolved["isolation_proof"]),
                root=root,
                suite_path=resolved["suite"],
                pack_path=resolved["query_pack"],
                proof_path=resolved["isolation_proof"],
                source_repo=repo,
                manifest_path=resolved["corpus_manifest"],
            )
            if manifest["isolation_method"] != isolation_evidence["backend"]:
                raise RunError("manifest isolation method differs from the proof backend")
            if manifest["access_block_log"] != ("sha256:" + isolation_evidence["proof_sha256"]):
                raise RunError("manifest access_block_log does not bind the isolation proof")
            if len(resource_isolation) != len(resolved["records"]) or any(
                not isinstance(entry, dict)
                or any(
                    entry.get(key) != isolation_evidence[key]
                    for key in ("backend", "policy_sha256", "proof_sha256")
                )
                or (
                    isolation_evidence["backend"] == LINUX_ISOLATION_BACKEND
                    and (
                        not isinstance(entry.get("child_attestation"), dict)
                        or entry["child_attestation"].get("abi") != isolation_evidence["abi"]
                    )
                )
                for entry in resource_isolation
            ):
                raise RunError("capture resources do not all bind the isolation profile")
            if isolation_evidence["backend"] == LINUX_ISOLATION_BACKEND:
                _validate_unique_linux_attestations(resource_isolation)
        except (RunError, ValueError, OSError) as exc:
            isolation_error = str(exc)
    if not claims["quality"]:
        set_state("QUALITY_DELTA", "not_applicable", "no_quality_claim", None)
    elif (
        protocol_payload.get("execution_profiles", {}).get("quanta", {}).get("policy")
        not in PAIR_CONTEXT_QUALITY_POLICIES
        and not file_quality_policy
    ):
        set_state("QUALITY_DELTA", "fail", "diagnostic_rank_profile", None)
        classes.append("scoring")
    elif not matched:
        set_state("QUALITY_DELTA", "not_run", "reports_unmatched", None)
    elif manifest["scope"] != "qualified":
        set_state("QUALITY_DELTA", "not_applicable", "exploratory_only", None)
        not_applicable.append("scope:exploratory_only")
    elif admission_error is not None or admission_evidence is None:
        set_state(
            "QUALITY_DELTA",
            "fail",
            f"admission_unverified: {admission_error or 'missing admission'}",
            None,
        )
        missing.append("T17")
        classes.append("admission")
    elif file_quality_policy and admission_evidence.get("schema_version") != 3:
        set_state("QUALITY_DELTA", "fail", "file_quality_requires_disjoint_admission", None)
        classes.append("admission")
    elif file_quality_policy and any(
        entry.get("primary_metric") != "file_ndcg_at_10" for entry in matched
    ):
        set_state("QUALITY_DELTA", "fail", "file_quality_metric_mismatch", None)
        classes.append("scoring")
    elif (
        set(protocol_payload.get("quanta_routes", [])) & {"semantic", "hybrid"}
        and provenance_claims["quanta"].get("embedder") != "potion-code"
    ):
        # T10: non-default encoder controls lack a qualified quality gate.
        set_state("QUALITY_DELTA", "fail", "model_quality_embedder", None)
        classes.append("model")
    elif isolation_claimed and not all_isolated:
        set_state("QUALITY_DELTA", "fail", "isolation_record_mismatch", None)
        classes.append("blinding")
    elif not all_isolated:
        set_state("QUALITY_DELTA", "not_applicable", "attested_only", None)
        not_applicable.append("blinding:attested_only")
    elif isolation_error is not None or isolation_evidence is None:
        set_state(
            "QUALITY_DELTA",
            "fail",
            f"isolation_proof_unverified: {isolation_error or 'missing proof'}",
            None,
        )
        classes.append("blinding")
    elif not all(entry["graded"] for entry in matched):
        set_state("QUALITY_DELTA", "fail", "reports_ungraded", None)
        classes.append("scoring")
    elif any(
        not _qualified_uncertainty(entry) or not _qualified_cluster_uncertainty(entry)
        for entry in matched
    ):
        set_state("QUALITY_DELTA", "fail", "uncertainty_unqualified", None)
        classes.append("scoring")
    elif qualification_dependency is not None:
        set_state(
            "QUALITY_DELTA",
            "fail",
            f"qualification_dependency_unverified:{qualification_dependency}",
            None,
        )
        classes.append("provenance")
    elif binary_build_source_revision is None:
        set_state("QUALITY_DELTA", "fail", "binary_build_source_unattested", None)
        classes.append("provenance")
    else:
        set_state(
            "QUALITY_DELTA",
            "pass",
            "blinded_graded_file_ndcg_delta"
            if file_quality_policy
            else "blinded_graded_context_density_delta",
            digest(
                canonical(
                    {
                        "admission": admission_evidence,
                        "reports": sorted(entry["report_sha"] for entry in matched),
                        "cluster_uncertainty": sorted(
                            (entry["primary_delta_cluster_ci_95"] for entry in matched),
                            key=lambda ci: ci["seed_sha256"],
                        ),
                        "isolation": isolation_evidence,
                    }
                )
            ),
        )

    for key, claim_key, tid, fail_class in (
        ("model_parity", "same_model", "T15", "model"),
        ("incremental", "incremental", "T16", "infra"),
    ):
        if not claims[claim_key]:
            not_applicable.append(tid)
            continue
        try:
            if key not in evidence:
                raise RunError("no evidence")
            ref = f"{key}_results"
            if ref not in resolved:
                raise RunError("no artifact")
            conditional = _validate_parity_results_shape(
                read_json(resolved[ref]),
                f"{key} results",
                "model_vectors" if key == "model_parity" else "incremental_rows",
            )
            if sha_file(resolved[ref]) != evidence[key]["test_result_digest"]:
                raise RunError("manifest digest mismatch")
            model_rows = [
                (entry["system"], capture.get("model"), capture.get("model_revision"))
                for entry in validated.values()
                for capture in entry["run"].get("captures", {}).values()
            ]
            if not model_rows or any(
                not isinstance(value, str) or not value for row in model_rows for value in row
            ):
                raise RunError(f"{tid} lacks frozen model identity")
            suite_identity = read_json(resolved["suite"])
            _require_conditional_identity(
                conditional,
                source_revision=provenance_claims["quanta"]["source_sha"],
                repository_commit=suite_identity["repository_commit"],
                model_sha256=digest(canonical(sorted(set(model_rows)))),
                dependency_sha256=sha_file(resolved["semble_lockfile"]),
            )
            from tools.benchmark.retrieval import conditional_proof

            kind = "model_vectors" if key == "model_parity" else "incremental_rows"
            conditional_proof.validate_results(conditional, kind, verify_source=True)
            context = conditional["execution_context"]
            if (
                context["suite"]["sha256"] != sha_file(resolved["suite"])
                or context["corpus"]["sha256"] != sha_file(resolved["corpus_manifest"])
                or conditional_proof.load(conditional_proof.decode(context["records"]))
                != sorted(
                    [read_json(Path(path)) for path in validated],
                    key=lambda record: conditional_proof.sha(conditional_proof.canonical(record)),
                )
            ):
                raise RunError("conditional execution inputs differ from frozen pair")
            if conditional["status"] != "pass" or conditional["failed"] != 0:
                raise RunError("conditional raw replay reports failure")
        except (RunError, ValueError, OSError, KeyError, TypeError, IndexError):
            missing.append(tid)
            classes.append(fail_class)

    failure_class = classes[0] if classes else "none"
    # none means nothing failed: not_run/not_applicable states do not taint
    # it, but their missing T-IDs stay listed. Any failed state must set
    # a class, and a claimed-but-unproven T15/T16 taints the run too.
    if failure_class == "none" and any(value == "fail" for value in states.values()):
        raise RunError("verdict invariant broken: failing state without failure class")

    blinding = "isolated" if all_isolated else "attested"
    total_rows = [row for entry in validated.values() for row in entry["run"].get("results", [])]
    rep0_rows = [
        row
        for path, entry in validated.items()
        if entry["rep"] == "rep-00"
        for row in entry["run"].get("results", [])
    ]
    counts = {
        "selected": len(rep0_rows),
        "executed": len(total_rows),
        "passed": sum(1 for row in total_rows if row.get("status") in ("success", "capped")),
        "failed": sum(
            1 for row in total_rows if row.get("status") not in ("success", "capped", "abstained")
        ),
    }
    comparisons = [
        {
            "strategy": entry["strategy"],
            "baseline_route": entry["baseline_route"],
            "candidate_route": entry["candidate_route"],
            "primary_metric": entry["primary_metric"],
            "primary_delta": entry["primary_delta"],
            "record_digest": entry["record_digest"],
            "report_digest": entry["report_digest"],
        }
        for entry in sorted(
            matched, key=lambda e: (e["strategy"], e["baseline_route"], e["candidate_route"])
        )
    ]
    diff_digest = mapping_diff if _is_hex(mapping_diff, 64) else "0" * 64
    provenance = {
        "admission": {"manifest_digest": provenance_claims["admission"]["manifest_digest"]},
        "quanta": {
            "source_sha": provenance_claims["quanta"]["source_sha"],
            "binary_digest": binary_digest,
            "embedder": provenance_claims["quanta"].get("embedder", "undeclared"),
            **(
                {"binary_build_source_revision": binary_build_source_revision}
                if "binary_build_source_revision" in provenance_claims["quanta"]
                or binary_build_source_revision is not None
                else {}
            ),
        },
        "semble": {
            "revision": SEMBLE_PINNED_VERSION,
            "lockfile_digest": lockfile_digest or "0" * 64,
        },
        "corpus": {"digest": corpus_digest or "0" * 64, "path_sha_diff_digest": diff_digest},
        "suite": {
            "suite_digest": suite_digest or "0" * 64,
            "query_pack_digest": pack_digest or "0" * 64,
            "tokenizer_budget_version": TOKENIZER_BUDGET_VERSION,
        },
        "host": {
            "profile_digest": provenance_claims["host"]["profile_digest"],
            "check_record_digest": check_record_digest or "0" * 64,
        },
    }
    return {
        "verdict_version": VERDICT_VERSION,
        "os_portability": {"qualified": False, "reason": "execution_os_tool_identity_unverified"},
        "states": states,
        "state_evidence": state_evidence,
        "blinding": blinding,
        "isolation_method": manifest["isolation_method"],
        "access_block_log": manifest["access_block_log"],
        "missing_t_ids": sorted(set(missing)),
        "not_applicable_t_ids": sorted(set(not_applicable)),
        "failure_class": failure_class,
        "provenance": provenance,
        "counts": counts,
        "comparisons": comparisons,
    }


def cmd_pair(args: argparse.Namespace) -> int:
    try:
        return run_pair(load_spec(Path(args.spec)))
    except (RunError, ValueError) as exc:
        print(f"ERROR: {exc}", file=sys.stderr)
        return 2


def _source_closure(
    repo_root: Path, command: str, path: Path | None = None, *, reuse_from: Path | None = None
) -> None:
    args = [sys.executable, str(repo_root / "tools/ci/source_closure.py"), command]
    if command == "capture":
        args.extend(("--profile", "retrieval", "--out", str(path)))
    elif command == "verify":
        args.extend(("--manifest", str(path)))
    elif command == "reuse" and reuse_from is not None:
        args.extend(("--manifest", str(reuse_from), "--out", str(path)))
    else:
        raise RunError(f"unsupported source-closure command: {command}")
    try:
        completed = subprocess.run(args, cwd=repo_root, capture_output=True, text=True, timeout=300)
    except (OSError, subprocess.SubprocessError) as exc:
        raise RunError(f"source-closure {command} failed: {exc}") from exc
    if completed.returncode != 0:
        detail = (completed.stderr or completed.stdout).strip()
        raise RunError(f"source-closure {command} refused: {detail}")


def run_pair(spec: dict) -> int:
    """Sequential Quanta + Semble capture with merged scoring and verdict.

    External repetitions re-run both systems on fresh state (fresh index
    samples); system order alternates per repetition unless disabled.
    Quality merges rep-0 records; every rep feeds the latency matrix.
    Everything builds in a sibling staging directory; only a complete
    tree (manifest + verdict) is atomically renamed onto the output
    root. Partial output is never resumed: rerun from a fresh root.
    """
    if sys.version_info < (3, 10):
        raise RunError("retrieval pair requires Python 3.10 or newer")
    driver_started_ns = time.monotonic_ns()
    scope = spec.get("scope", "exploratory")
    _validate_file_pair_contract(spec, paired=True)
    closure_source = spec.get("source_closure_reuse")
    if closure_source is not None and (
        not isinstance(closure_source, str)
        or not Path(closure_source).is_absolute()
        or scope != "exploratory"
        or any(spec.get("claims", {}).values())
    ):
        raise RunError("source closure reuse is only valid for exploratory captures without claims")
    if spec.get("embedder") == "potion-code-full-v2" and (
        scope != "exploratory" or any(spec.get("claims", {}).values())
    ):
        raise RunError("potion-code-full-v2 is exploratory diagnostic only; claims must be false")
    if scope != "qualified" and "linux_cgroup_parent" in spec:
        raise RunError("exploratory pair must not claim linux_cgroup_parent")
    if (
        platform.system() == "Linux"
        and scope == "qualified"
        and (spec.get("blinding") != "isolated" or not spec.get("linux_cgroup_parent"))
    ):
        raise RunError("qualified Linux pair requires Landlock and linux_cgroup_parent")
    if scope == "qualified" and not isinstance(spec.get("admission"), dict):
        raise RunError("qualified pair capture requires spec.admission")
    if scope != "qualified" and "admission" in spec:
        raise RunError("spec.admission is valid only for a qualified capture")
    if scope == "qualified" and spec.get("claims", {}).get("speed") is True:
        if spec.get("alternate_order", True) is not True:
            raise RunError("qualified speed requires alternating system order")
        pack_payload = read_json(Path(spec["query_pack"]))
        tasks = pack_payload.get("tasks") if isinstance(pack_payload, dict) else None
        if not isinstance(tasks, list):
            raise RunError("qualified speed query pack lacks tasks")
        validate_qualified_speed_spec(spec, len(tasks))
    lockfile_sha = spec.get("semble_lockfile_sha256")
    if not _is_hex(lockfile_sha, 64):
        raise RunError("pair requires a pinned semble_lockfile_sha256")
    if not spec.get("semble_python"):
        raise RunError("pair requires spec.semble_python")
    if not spec.get("semble_lockfile"):
        raise RunError(
            "pair requires spec.semble_lockfile naming the digest-pinned environment freeze"
        )
    if not spec.get("host_profile"):
        raise RunError("pair requires spec.host_profile naming the canonical host profile")
    cache_root = spec.get("semble_cache_root")
    if not isinstance(cache_root, str) or not cache_root:
        raise RunError("pair requires spec.semble_cache_root naming an existing model cache")
    cache_path = Path(cache_root)
    if not cache_path.is_absolute() or not cache_path.is_dir():
        raise RunError("spec.semble_cache_root must be an existing absolute directory")
    model_revision = spec.get("semble_model_revision")
    if not _is_hex(model_revision, 40):
        raise RunError("pair requires spec.semble_model_revision as a pinned 40-hex revision")
    try:
        semble_adapter.resolve_model_revision(
            cache_path / "hf", semble_adapter.DEFAULT_MODEL_ID, model_revision
        )
    except semble_adapter.AdapterError as exc:
        raise RunError(f"Semble model cache preflight refused: {exc}") from exc
    if (
        spec.get("claims", {}).get("speed") is True
        and spec.get("embedder", "potion-code") in ("potion-code", "potion-code-full-v2")
        and not spec.get("quanta_model_dir")
    ):
        raise RunError("qualified speed capture with potion-code requires quanta_model_dir")
    if "quanta_model_dir" in spec and not Path(spec["quanta_model_dir"]).is_dir():
        raise RunError("quanta_model_dir must name an existing directory")
    if platform.system() == "Linux" and scope == "qualified":
        spec = dict(
            spec,
            _linux_cgroup_parent_identity=_linux_parent_identity(spec["linux_cgroup_parent"]),
        )
    out_root = preflight_capture(spec)
    preflight_finished_ns = time.monotonic_ns()
    stage = out_root.parent / (out_root.name + ".staging")
    if out_root.exists() or stage.exists():
        raise RunError("output root or staging dir already exists (refusing reuse)")
    if spec.get("strategies"):
        preflight_daemon_socket_paths(
            stage,
            spec["strategies"],
            repetitions=_int(spec.get("repetitions", 1), "spec.repetitions"),
            paired=True,
        )
    stage.mkdir(parents=True)
    closure_path = stage / "driver-source-closure.json"
    if closure_source is None:
        _source_closure(Path(__file__).resolve().parents[3], "capture", closure_path)
    else:
        _source_closure(
            Path(__file__).resolve().parents[3],
            "reuse",
            closure_path,
            reuse_from=Path(closure_source),
        )
        _validate_source_closure_shape(read_json(closure_path), "reused driver source closure")
    closure_capture_finished_ns = time.monotonic_ns()
    spec = dict(spec, _driver_source_closure=str(closure_path))
    try:
        summary = _run_pair_staged(spec, stage)
    except Exception:
        # The stage is left for forensics, but the authoritative output
        # root is never promoted from a failed run.
        raise
    staged_finished_ns = time.monotonic_ns()
    _source_closure(Path(__file__).resolve().parents[3], "verify", closure_path)
    closure_verify_finished_ns = time.monotonic_ns()
    if out_root.exists():
        raise RunError("output root appeared during capture (refusing promotion)")
    os.rename(stage, out_root)
    promoted_ns = time.monotonic_ns()
    summary["output_root"] = str(out_root)
    summary["driver_outer_ms"] = {
        "preflight": (preflight_finished_ns - driver_started_ns) / 1_000_000,
        "source_closure_capture": (closure_capture_finished_ns - preflight_finished_ns) / 1_000_000,
        "staged": (staged_finished_ns - closure_capture_finished_ns) / 1_000_000,
        "source_closure_verify": (closure_verify_finished_ns - staged_finished_ns) / 1_000_000,
        "promotion": (promoted_ns - closure_verify_finished_ns) / 1_000_000,
        "total": (promoted_ns - driver_started_ns) / 1_000_000,
    }
    print(json.dumps(summary, indent=2))
    return 0


def _run_pair_staged(spec: dict, stage: Path) -> dict:
    stage_started_ns = time.monotonic_ns()
    if "semble" not in spec["execution_profiles"]:
        raise RunError("pair requires spec.execution_profiles.semble")
    scope = spec.get("scope", "exploratory")
    file_pair = _validate_file_pair_contract(spec, paired=True)
    order = spec.get("order", ["quanta", "semble"])
    if sorted(order) != ["quanta", "semble"]:
        raise RunError("spec.order must list quanta and semble exactly once")
    repetitions = _int(spec.get("repetitions", 1), "spec.repetitions")
    if repetitions <= 0:
        raise RunError("spec.repetitions must be positive")
    alternate = spec.get("alternate_order", True)
    # Captures consume frozen copies, so the verdict re-verifies the
    # exact bytes used rather than whatever external paths hold later.
    original_suite = Path(spec["suite"])
    source_repo = Path(spec["repo"]).resolve()
    frozen_inputs = freeze_inputs(spec, stage)
    frozen_receipts = freeze_receipts(spec, stage)
    spec = dict(spec, **frozen_inputs)
    frozen_admission = freeze_admission(spec, stage, frozen_receipts)
    if spec.get("blinding", "attested") == "isolated":
        spec = materialize_corpus_view(spec, stage, source_repo)
    spec = prepare_isolation(spec, stage, original_suite)
    override = spec.get("contention_override", False)
    host_start = host_probe()
    host_start["contention_override"] = override
    (stage / "host-start.json").write_text(
        json.dumps(host_start, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    semble_routes = [
        spec.get("semble_route", SEMBLE_ROUTE_BY_MODE[spec["execution_profiles"]["semble"]["mode"]])
    ]
    semble_pack = write_projected_pack(
        Path(spec["query_pack"]),
        Path(spec["suite"]),
        semble_routes,
        stage
        / (
            "runner-input/semble-pack.json"
            if spec.get("isolation_method") == LINUX_ISOLATION_BACKEND
            else "semble-pack.json"
        ),
    )
    rep_layouts: list[dict] = []
    semble_spec = dict(spec)
    setup_finished_ns = time.monotonic_ns()
    product_ns = {"quanta": 0, "semble": 0}
    # One pinned model cache across reps; each rep still rebuilds its index.
    monitor = (
        HostTimeline(
            stage / "host-timeline.json",
            host_start,
            override,
            owned_semble_adapter=Path(
                spec.get("_semble_adapter", Path(__file__).resolve().parent / "semble.py")
            ),
        )
        if spec.get("claims", {}).get("speed")
        else nullcontext()
    )
    with monitor:
        for rep in range(repetitions):
            rep_order = order if (rep % 2 == 0 or not alternate) else list(reversed(order))
            rep_dir = stage / f"rep-{rep:02d}"
            rep_dir.mkdir(
                parents=True, exist_ok=spec.get("isolation_method") == LINUX_ISOLATION_BACKEND
            )
            pack_payload = read_json(Path(spec["query_pack"]))
            tasks = pack_payload.get("tasks") if isinstance(pack_payload, dict) else None
            if not isinstance(tasks, list):
                raise RunError("frozen query pack lacks tasks")
            task_ids = [task.get("task_id") for task in tasks if isinstance(task, dict)]
            if len(task_ids) != len(tasks):
                raise RunError("frozen query pack has malformed tasks")
            protocol = build_query_protocol(
                task_ids,
                _int(spec.get("seed", 0), "spec.seed") + rep,
                _int(
                    spec.get("query_warmup_passes", 1),
                    "spec.query_warmup_passes",
                ),
                _int(
                    spec.get("query_repetitions_per_root", 1),
                    "spec.query_repetitions_per_root",
                ),
            )
            protocol_path = rep_dir / "query-protocol.json"
            protocol_path.write_text(
                json.dumps(protocol, indent=2, sort_keys=True) + "\n", encoding="utf-8"
            )
            layout: dict = {
                "rep": rep,
                "order": rep_order,
                "query_protocol": str(protocol_path),
                "quanta": {},
                "quanta_phase_metrics": {},
                "semble": "",
            }
            for system in rep_order:
                product_started_ns = time.monotonic_ns()
                if system == "quanta":
                    quanta_out = rep_dir / "quanta"
                    quanta_spec = dict(spec)
                    quanta_spec["output_root"] = str(quanta_out)
                    quanta_spec["run_id"] = f"{spec.get('run_id', 'run')}-r{rep}"
                    quanta_spec["_query_protocol"] = str(protocol_path)
                    if run_quanta(quanta_spec, Path(".")) != 0:
                        raise RunError(f"quanta capture failed at rep {rep}")
                    quanta_manifest_path = quanta_out / "quanta-manifest.json"
                    quanta_manifest = read_json(quanta_manifest_path)
                    if not isinstance(quanta_manifest, dict):
                        raise RunError("quanta manifest is not an object")
                    for run in quanta_manifest["runs"]:
                        layout["quanta"][run["strategy"]] = str(quanta_out / run["record"])
                        layout["quanta_phase_metrics"][run["strategy"]] = str(
                            quanta_out / run["phase_metrics"]
                        )
                    layout["quanta_manifest"] = str(quanta_manifest_path)
                else:
                    semble_out = rep_dir / "semble"
                    rep_semble_spec = dict(semble_spec, _query_protocol=str(protocol_path))
                    semble_metrics = run_semble_capture(
                        rep_semble_spec, semble_out, semble_pack, semble_routes[0], rep=rep
                    )
                    layout["semble"] = str(semble_out / "record.json")
                    layout["semble_phase_metrics"] = semble_metrics["phase_metrics"]
                    layout["semble_resource_metrics"] = semble_metrics["resource_metrics"]
                product_ns[system] += time.monotonic_ns() - product_started_ns
            rep_layouts.append(layout)
    host_end = host_probe()
    host_end["contention_override"] = override
    (stage / "host-end.json").write_text(
        json.dumps(host_end, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    product_envelope_finished_ns = time.monotonic_ns()
    # Every rep's records validate through the evaluator before any use:
    # each (strategy, semble) pair merges exactly like the scored join,
    # and each capture echoes the strategy it was invoked with.
    # Quality reports merge rep-0 records only.
    repo = source_repo
    suite_path = Path(spec["suite"])
    suite, pack, source = validate_suite(repo, read_json(suite_path))
    suite_validation_finished_ns = time.monotonic_ns()
    for rep_index, layout in enumerate(rep_layouts):
        for strategy, record in sorted(layout["quanta"].items()):
            payload = read_json(Path(record))
            if not isinstance(payload, dict):
                raise RunError(f"record is not an object: {record}")
            system, captured_strategy = _record_identity(payload, f"quanta record {record}")
            if system != "quanta" or captured_strategy != strategy:
                raise RunError(f"strategy echo mismatch for {record}: {strategy}")
            # The rep-0 merge below also validates the records before scoring.
            if rep_index:
                _merge_validated_records(
                    repo, suite, pack, source, [Path(record), Path(layout["semble"])]
                )
    baseline = spec.get("baseline_route", semble_routes[0])
    rep0 = rep_layouts[0]
    reports = []
    for strategy, record in sorted(rep0["quanta"].items()):
        payload = read_json(Path(record))
        if not isinstance(payload, dict):
            raise RunError(f"record is not an object: {record}")
        _suite, _pack, combined = _merge_validated_records(
            repo, suite, pack, source, [Path(record), Path(rep0["semble"])]
        )
        candidate_routes = sorted({row["route"] for row in payload["results"]})
        for candidate in candidate_routes:
            if file_pair and scope == "qualified":
                report = evaluate_complete_scored_file_evidence(
                    suite, pack, combined, baseline, candidate
                )
            elif file_pair:
                report = evaluate_paired_file_diagnostic(suite, pack, combined, baseline, candidate)
            else:
                report = evaluate(suite, pack, combined, baseline, candidate, strict_k=True)
            name = f"report-{baseline}-vs-{candidate}-{strategy}.json"
            (stage / name).write_text(
                json.dumps(report, indent=2, sort_keys=True) + "\n", encoding="utf-8"
            )
            reports.append(name)
    report_scoring_finished_ns = time.monotonic_ns()
    latency_path = stage / "latency-matrix.json"
    latency_path.write_text(
        json.dumps(build_latency_matrix(rep_layouts), indent=2, sort_keys=True) + "\n",
        encoding="utf-8",
    )
    closure_path = Path(spec["_driver_source_closure"])
    # The outer driver verifies the captured closure after the verdict and
    # immediately before promotion. A second full scan here adds no custody
    # boundary: the manifest binds the already captured closure digest.
    driver_closure = _validate_source_closure_shape(
        read_json(closure_path), "driver source closure"
    )
    driver_closure_digest = driver_closure["digest"]
    protocol_lock = {
        "lock_version": 6,
        "retrieval_diagnostic_version": 8,
        "symbol_coverage_policy": spec.get("symbol_coverage_policy", "require-complete"),
        "server_observation": server_observation_configuration(
            spec.get("query_stage_observation", "enabled")
        ),
        "hybrid_fetch_policy": hybrid_fetch_policy_configuration(
            spec.get("experimental_hybrid_fetch_floor", "100")
        ),
        "ingest_request_identity": ingest_request_identity(spec),
        "rank_metric_k_policy": "declared_top_k_v1",
        "suite_digest": sha_file(Path(spec["suite"])),
        "query_pack_digest": sha_file(stage / "query-pack.json"),
        "corpus_manifest_digest": sha_file(stage / "corpus-manifest.json"),
        "top_k": spec["top_k"],
        "strategies": [entry["name"] for entry in spec["strategies"]],
        "quanta_routes": list(spec.get("routes", ["lexical", "semantic", "hybrid"])),
        "semble_route": spec.get(
            "semble_route", SEMBLE_ROUTE_BY_MODE[spec["execution_profiles"]["semble"]["mode"]]
        ),
        "searchd_expected_sha256": spec["searchd_expected_sha256"],
        "semble_lockfile_sha256": spec["semble_lockfile_sha256"],
        "execution_profiles": spec["execution_profiles"],
        "execution_profiles_sha256": digest(canonical_bytes(spec["execution_profiles"])),
        "host_profile_digest": sha_file(Path(spec["host_profile"])),
        **(
            {"delegated_cgroup_parent": spec["_linux_cgroup_parent_identity"]}
            if "_linux_cgroup_parent_identity" in spec
            else {}
        ),
        "admission_digest": (
            sha_file(Path(str(frozen_admission["manifest"]))) if frozen_admission else None
        ),
        "driver_source_closure_digest": driver_closure_digest,
        "repetitions": repetitions,
        "system_orders": [layout["order"] for layout in rep_layouts],
        "base_seed": _int(spec.get("seed", 0), "spec.seed"),
        "query_warmup_passes": _int(
            spec.get("query_warmup_passes", 1),
            "spec.query_warmup_passes",
        ),
        "query_repetitions_per_root": _int(
            spec.get("query_repetitions_per_root", 1),
            "spec.query_repetitions_per_root",
        ),
        "query_protocol_sha256s": [
            validate_query_protocol(
                read_json(Path(layout["query_protocol"])),
                [task["task_id"] for task in read_json(Path(spec["query_pack"]))["tasks"]],
                f"rep {layout['rep']} query protocol",
            )["sha256"]
            for layout in rep_layouts
        ],
    }
    if "symbol_total_timeout_ms" in spec:
        protocol_lock["symbol_total_timeout_ms"] = spec["symbol_total_timeout_ms"]
    (stage / "protocol-lock.json").write_text(
        json.dumps(protocol_lock, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    protocol_lock_finished_ns = time.monotonic_ns()
    manifest = build_run_manifest(
        spec,
        stage,
        rep_layouts,
        host_start,
        host_end,
        reports,
        frozen_receipts,
        frozen_admission,
    )
    manifest_path = stage / "run-manifest.json"
    manifest_path.write_text(
        json.dumps(manifest, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    manifest_finished_ns = time.monotonic_ns()
    verdict = build_verdict(source_repo, suite_path, manifest_path)
    verdict_path = stage / "verdict.json"
    verdict_path.write_text(json.dumps(verdict, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    verdict_finished_ns = time.monotonic_ns()
    boundaries = (
        ("source_setup", stage_started_ns, setup_finished_ns),
        ("product_envelope", setup_finished_ns, product_envelope_finished_ns),
        ("suite_validation", product_envelope_finished_ns, suite_validation_finished_ns),
        ("report_scoring", suite_validation_finished_ns, report_scoring_finished_ns),
        ("protocol_lock", report_scoring_finished_ns, protocol_lock_finished_ns),
        ("manifest", protocol_lock_finished_ns, manifest_finished_ns),
        ("verdict", manifest_finished_ns, verdict_finished_ns),
    )
    phases_ms = {name: (finished - started) / 1_000_000 for name, started, finished in boundaries}
    stage_wall_ms = (verdict_finished_ns - stage_started_ns) / 1_000_000
    driver_timings = {
        "schema_version": 1,
        "authority": "diagnostic_only",
        "stage_wall_ms": stage_wall_ms,
        "phases_ms": phases_ms,
        "product_subphases_ms": {
            system: elapsed / 1_000_000 for system, elapsed in product_ns.items()
        },
        "manifest_sha256": sha_file(manifest_path),
        "verdict_sha256": sha_file(verdict_path),
    }
    (stage / "driver-stage-timings.json").write_text(
        json.dumps(driver_timings, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    return {
        "reports": reports,
        "repetitions": repetitions,
        "states": verdict["states"],
        "output_root": str(stage),
        "driver_stage_timings": driver_timings,
    }


ERROR_STATUSES = ("error", "timeout", "unavailable")


def _sample_value(value: object, where: str) -> float | None:
    """A matrix sample: finite number >= 0, or None when unknown. Never 0-filled."""
    if value is None:
        return None
    if type(value) not in (int, float):
        raise RunError(f"{where} latency is not a number or null")
    if not is_finite_json_number(value) or value < 0:
        raise RunError(f"{where} latency is not finite and >= 0")
    return float(value)


def aggregate_matrix(cells: list[dict], fresh_roots: int) -> dict:
    """Aggregate latency cells into the v2 matrix.

    Each cell is {"system", "strategy", "rows", "native_latencies"|None,
    "native_route"|None} with rows as (route, task_id, status, timing)
    tuples. Qualified samples, attempts, errors, nulls and abstentions
    are kept as separate per-(system, strategy, route) counts: merging
    strategies can never inflate another key's floor. Native extra
    samples join only when the native first sample exactly equals the
    normalized row timing (one dedupe); any disagreement is a refusal.
    """
    if type(fresh_roots) is not int or fresh_roots <= 0:
        raise RunError("matrix needs a positive fresh-root count")
    samples: dict[str, list[float]] = {}
    sample_owner: dict[str, str] = {}
    floors: dict[str, int] = {}
    attempts: dict[str, int] = {}
    errors: dict[str, int] = {}
    nulls: dict[str, int] = {}
    abstained: dict[str, int] = {}

    def bump(table: dict[str, int], key: str) -> None:
        table[key] = table.get(key, 0) + 1

    for cell in cells:
        system = cell["system"]
        strategy = cell["strategy"]
        timings_by_task: dict[tuple[str, str], float | None] = {}
        statuses_by_task: dict[tuple[str, str], str] = {}
        for route, task_id, status, timing in cell["rows"]:
            sys_key = f"{system}:{strategy}:{route}"
            bump(attempts, sys_key)
            sample_key = f"{sys_key}:{task_id}"
            value = _sample_value(timing, f"{sample_key}")
            timings_by_task[(route, task_id)] = value
            statuses_by_task[(route, task_id)] = status
            if status in ERROR_STATUSES:
                bump(errors, sys_key)
                continue
            if status == "abstained":
                bump(abstained, sys_key)
            if value is None:
                bump(nulls, sys_key)
                continue
            samples.setdefault(sample_key, []).append(value)
            sample_owner[sample_key] = sys_key
        warm_by_route = cell.get("warm_latencies")
        if warm_by_route is None and cell.get("native_latencies") is not None:
            route = cell.get("native_route")
            if not isinstance(route, str) or not route:
                raise RunError("native latencies need their record route")
            warm_by_route = {route: cell["native_latencies"]}
        if warm_by_route is not None:
            if not isinstance(warm_by_route, dict):
                raise RunError("warm latencies must be an object")
            for route, by_task in warm_by_route.items():
                if not isinstance(route, str) or not isinstance(by_task, dict):
                    raise RunError("warm latency route entries must be objects")
                for task_id, values in by_task.items():
                    if not isinstance(values, list):
                        raise RunError(f"warm latencies for {route}/{task_id} are not a list")
                    sample_key = f"{system}:{cell['strategy']}:{route}:{task_id}"
                    sys_key = f"{system}:{cell['strategy']}:{route}"
                    if (route, task_id) not in timings_by_task:
                        raise RunError(f"warm task without a record row: {route}/{task_id}")
                    if not values:
                        continue
                    recorded = timings_by_task[(route, task_id)]
                    extras: list[object] = list(values)
                    if recorded is not None:
                        first = _sample_value(values[0], f"{sample_key}[warm]")
                        if first != recorded:
                            raise RunError(
                                f"warm timing and normalized timing disagree for {route}/{task_id}"
                            )
                        extras = values[1:]
                    for value in extras:
                        parsed = _sample_value(value, f"{sample_key}[warm]")
                        # Apply the normalized row's failure policy to every
                        # repetition; cheap failure timings are not retrieval
                        # observations and must not inflate floors or p95.
                        if statuses_by_task[(route, task_id)] in ERROR_STATUSES:
                            continue
                        if parsed is None:
                            bump(nulls, sys_key)
                        else:
                            samples.setdefault(sample_key, []).append(parsed)
                            sample_owner[sample_key] = sys_key
    for key in attempts:
        floors[key] = 0
    for key, values in samples.items():
        owner = sample_owner[key]
        floors[owner] = floors.get(owner, 0) + len(values)
    ordered_keys = sorted(floors)
    return {
        "samples": {key: samples[key] for key in sorted(samples)},
        "summary": {key: latency_summary(samples[key]) for key in sorted(samples)},
        "floors": {key: floors[key] for key in ordered_keys},
        "attempts": {key: attempts.get(key, 0) for key in ordered_keys},
        "errors": {key: errors.get(key, 0) for key in ordered_keys},
        "nulls": {key: nulls.get(key, 0) for key in ordered_keys},
        "abstained": {key: abstained.get(key, 0) for key in ordered_keys},
        "observations_floor": min(floors.values()) if floors else 0,
        "floor_keys": ordered_keys,
        "fresh_roots": fresh_roots,
    }


def _cell_from_record(system: str, strategy: str, payload: dict) -> dict:
    rows = []
    for row in payload.get("results", []):
        rows.append(
            (
                row["route"],
                row["task_id"],
                row["status"],
                row.get("timings", {}).get("query_latency_ms"),
            )
        )
    return {
        "system": system,
        "strategy": strategy,
        "rows": rows,
        "native_latencies": None,
        "native_route": None,
        "warm_latencies": None,
    }


def build_latency_matrix(rep_layouts: list[dict]) -> dict:
    """Aggregate per (system, strategy, route, task) latencies across reps."""
    if not rep_layouts:
        raise RunError("latency matrix needs at least one rep")
    cells = []
    for layout in rep_layouts:
        protocol = None
        if "query_protocol" in layout:
            raw_protocol = read_json(Path(layout["query_protocol"]))
            task_ids = raw_protocol.get("task_ids") if isinstance(raw_protocol, dict) else None
            if not isinstance(task_ids, list):
                raise RunError(f"rep {layout['rep']} query protocol lacks task ids")
            protocol = validate_query_protocol(
                raw_protocol, task_ids, f"rep {layout['rep']} query protocol"
            )
        for strategy, record in sorted(layout["quanta"].items()):
            payload = read_json(Path(record))
            if not isinstance(payload, dict):
                raise RunError(f"record is not an object: {record}")
            cell = _cell_from_record("quanta", strategy, payload)
            if protocol is not None:
                phase = _validate_phase_metrics(
                    read_json(Path(layout["quanta_phase_metrics"][strategy])),
                    f"rep {layout['rep']} quanta {strategy} phase metrics",
                )
                if phase["schema_version"] not in (2, 3, 4):
                    raise RunError("current Quanta latency matrix requires phase metrics v2/v3/v4")
                if phase.get("query_protocol") != protocol:
                    raise RunError("Quanta phase metrics do not echo the shared query protocol")
                cell["warm_latencies"] = phase["warm_latencies_ms"]
            cells.append(cell)
        semble_record = read_json(Path(layout["semble"]))
        if not isinstance(semble_record, dict):
            raise RunError(f"semble record is not an object: {layout['semble']}")
        cell = _cell_from_record("semble", "native", semble_record)
        if protocol is not None:
            phase = _validate_phase_metrics(
                read_json(Path(layout["semble_phase_metrics"])),
                f"rep {layout['rep']} semble phase metrics",
            )
            if phase["schema_version"] != 2:
                raise RunError("current Semble latency matrix requires phase metrics v2")
            if phase.get("query_protocol") != protocol:
                raise RunError("Semble phase metrics do not echo the shared query protocol")
            cell["warm_latencies"] = phase["warm_latencies_ms"]
        else:
            native = read_json(Path(layout["semble"]).parent / "native.json")
            if not isinstance(native, dict):
                raise RunError(f"semble native output is not an object: {layout['semble']}")
            routes = {row["route"] for row in semble_record.get("results", [])}
            if len(routes) != 1:
                raise RunError("semble record must carry exactly one route")
            cell["native_latencies"] = native.get("latencies_ms", {})
            cell["native_route"] = next(iter(routes))
        cells.append(cell)
    return aggregate_matrix(cells, len(rep_layouts))


def git_head_sha(path: Path) -> str:
    try:
        completed = subprocess.run(
            ["git", "-C", str(path), "rev-parse", "HEAD"],
            capture_output=True,
            text=True,
            timeout=30,
        )
    except (OSError, subprocess.SubprocessError):
        return "unresolved"
    sha = completed.stdout.strip() if completed.returncode == 0 else ""
    return sha if len(sha) == 40 else "unresolved"


def mapping_matches_manifest(mapping: object, manifest: object) -> bool:
    """Require exact admitted path+bytes on both sides, not just no skipped names."""
    if not isinstance(mapping, dict) or not isinstance(manifest, dict):
        return False
    files = manifest.get("files")
    quanta = mapping.get("quanta_side")
    semble = mapping.get("semble_side")
    per_file = mapping.get("per_file")
    if not all(isinstance(rows, list) for rows in (files, quanta, semble, per_file)):
        return False
    if not all(isinstance(row, dict) for rows in (files, quanta, semble, per_file) for row in rows):
        return False
    if (
        mapping.get("skipped") != []
        or mapping.get("extra") != []
        or mapping.get("mismatched") != []
    ):
        return False
    if any(row.get("status") != "indexed" for row in per_file):
        return False
    if any(row.get("readable") is not True for row in semble):
        return False
    try:
        expected = sorted(files, key=lambda row: row["path"])
    except (KeyError, TypeError):
        return False
    observed = [{"path": row.get("path"), "file_sha256": row.get("file_sha256")} for row in semble]
    return bool(expected) and quanta == expected == observed and len(per_file) == len(expected)


def _exact_keys(payload: object, keys: set[str], where: str) -> dict:
    if not isinstance(payload, dict) or set(payload) != keys:
        raise RunError(f"{where} must hold exactly {sorted(keys)}")
    return payload


def _validate_receipt_shape(payload: object, where: str) -> dict:
    """Mirror of tools/ci/verification-receipt.schema.json (locked by test)."""
    receipt = _exact_keys(
        payload,
        {
            "schema_version",
            "revision",
            "rail",
            "tier",
            "command",
            "evidence_path",
            "evidence_sha256",
            "test_event_count",
            "source_closure",
            "input_evidence",
        },
        where,
    )
    if receipt["schema_version"] != 2:
        raise RunError(f"{where}.schema_version must be 2")
    if not _is_hex(receipt["revision"], 40):
        raise RunError(f"{where}.revision must be a full lowercase Git SHA")
    if not isinstance(receipt["rail"], str) or not receipt["rail"]:
        raise RunError(f"{where}.rail must be a nonempty string")
    if receipt["tier"] not in ("pr", "merge", "correctness", "nightly", "weekly"):
        raise RunError(f"{where}.tier is not a known tier")
    if not isinstance(receipt["command"], str) or not receipt["command"]:
        raise RunError(f"{where}.command must be a nonempty string")
    if not isinstance(receipt["evidence_path"], str) or not receipt["evidence_path"]:
        raise RunError(f"{where}.evidence_path must be a nonempty string")
    if not _is_hex(receipt["evidence_sha256"], 64):
        raise RunError(f"{where}.evidence_sha256 must be a lowercase sha256")
    count = receipt["test_event_count"]
    if type(count) is not int or isinstance(count, bool) or count < 1:
        raise RunError(f"{where}.test_event_count must be an integer >= 1")
    closure = _exact_keys(
        receipt["source_closure"],
        {"schema_version", "profile", "revision", "roots", "files", "digest"},
        f"{where}.source_closure",
    )
    if closure["schema_version"] != 1:
        raise RunError(f"{where}.source_closure.schema_version must be 1")
    if closure["profile"] != "retrieval":
        raise RunError(f"{where}.source_closure.profile must be retrieval")
    if closure["revision"] != receipt["revision"]:
        raise RunError(f"{where}.source_closure revision mismatch")
    roots = closure["roots"]
    if (
        not isinstance(roots, list)
        or not roots
        or any(not isinstance(root, str) or not root for root in roots)
        or roots != sorted(set(roots))
    ):
        raise RunError(f"{where}.source_closure.roots must be sorted and unique")
    files = closure["files"]
    if not isinstance(files, list) or not files:
        raise RunError(f"{where}.source_closure.files must be nonempty")
    paths = []
    for index, entry in enumerate(files):
        row = _exact_keys(entry, {"path", "sha256"}, f"{where}.source_closure.files[{index}]")
        if not isinstance(row["path"], str) or not row["path"]:
            raise RunError(f"{where}.source_closure.files[{index}].path is invalid")
        if not _is_hex(row["sha256"], 64):
            raise RunError(f"{where}.source_closure.files[{index}].sha256 is invalid")
        paths.append(row["path"])
    if paths != sorted(set(paths)):
        raise RunError(f"{where}.source_closure.files must be sorted and unique")
    core = {
        key: closure[key] for key in ("schema_version", "profile", "revision", "roots", "files")
    }
    if closure["digest"] != digest(canonical(core)):
        raise RunError(f"{where}.source_closure digest mismatch")
    inputs = receipt["input_evidence"]
    if not isinstance(inputs, list) or not inputs:
        raise RunError(f"{where}.input_evidence must be nonempty")
    roles = []
    for index, entry in enumerate(inputs):
        row = _exact_keys(entry, {"role", "sha256"}, f"{where}.input_evidence[{index}]")
        role = row["role"]
        if (
            not isinstance(role, str)
            or not role
            or any(character not in "abcdefghijklmnopqrstuvwxyz0123456789-_" for character in role)
        ):
            raise RunError(f"{where}.input_evidence[{index}].role is invalid")
        if not _is_hex(row["sha256"], 64):
            raise RunError(f"{where}.input_evidence[{index}].sha256 is invalid")
        roles.append(role)
    if roles != sorted(set(roles)):
        raise RunError(f"{where}.input_evidence must be sorted by unique role")
    return receipt


def _validate_source_closure_shape(payload: object, where: str) -> dict:
    closure = _exact_keys(
        payload,
        {"schema_version", "profile", "revision", "roots", "files", "digest"},
        where,
    )
    if closure["schema_version"] != 1 or closure["profile"] != "retrieval":
        raise RunError(f"{where} schema/profile mismatch")
    if not _is_hex(closure["revision"], 40):
        raise RunError(f"{where}.revision must be a full lowercase Git SHA")
    roots = closure["roots"]
    if (
        not isinstance(roots, list)
        or not roots
        or any(not isinstance(root, str) or not root for root in roots)
        or roots != sorted(set(roots))
    ):
        raise RunError(f"{where}.roots must be nonempty, sorted and unique")
    files = closure["files"]
    if not isinstance(files, list) or not files:
        raise RunError(f"{where}.files must be nonempty")
    paths = []
    for index, entry in enumerate(files):
        row = _exact_keys(entry, {"path", "sha256"}, f"{where}.files[{index}]")
        if not isinstance(row["path"], str) or not row["path"] or not _is_hex(row["sha256"], 64):
            raise RunError(f"{where}.files[{index}] is malformed")
        paths.append(row["path"])
    if paths != sorted(set(paths)):
        raise RunError(f"{where}.files must be sorted and unique")
    core = {
        key: closure[key] for key in ("schema_version", "profile", "revision", "roots", "files")
    }
    if closure["digest"] != digest(canonical(core)):
        raise RunError(f"{where}.digest mismatch")
    return closure


def _verify_receipt_inputs(receipt: dict, expected: dict[str, Path], where: str) -> None:
    observed = {entry["role"]: entry["sha256"] for entry in receipt["input_evidence"]}
    wanted = {role: sha_file(path) for role, path in expected.items()}
    if observed != wanted:
        raise RunError(f"{where} raw input evidence mismatch")


def _context_log_names(rail: str) -> set[str]:
    return {
        f"{name}.{stream}"
        for name in CONTEXT_COMMAND_NAMES[rail]
        for stream in ("stdout", "stderr")
    }


def _context_archive_limits(rail: str) -> raw_archive.ArchiveLimits:
    return raw_archive.ArchiveLimits(
        max_bytes=MAX_CONTEXT_LOG_BYTES + CONTEXT_ZIP_OVERHEAD_BYTES,
        max_entries=len(_context_log_names(rail)),
        max_directory_bytes=CONTEXT_ZIP_DIRECTORY_BYTES,
    )


@contextmanager
def _frozen_context_logs(path: Path | RawFile, rail: str):
    """Own extracted file lifetimes; only bounded controls may become bytes."""
    expected = _context_log_names(rail)

    def admit(names):
        if set(names) != expected or len(names) != len(expected):
            raise RunError("frozen command logs missing or duplicated")

    with tempfile.TemporaryDirectory(prefix="retrieval-command-logs-") as directory:
        root = Path(directory).resolve(strict=True)
        try:
            raw_archive.unpack(
                path if isinstance(path, RawFile) else RawFile.capture(path),
                root,
                limits=_context_archive_limits(rail),
                admit_names=admit,
            )
            logs = {name: RawFile.capture(root / name) for name in expected}
            if sum(log.size for log in logs.values()) > MAX_CONTEXT_LOG_BYTES:
                raise RunError("oversized command logs")
        except (OSError, ValueError) as exc:
            raise RunError(
                f"{rail} execution context cannot inspect frozen command logs: {exc}"
            ) from exc
        yield logs


def _proof_control(path: Path | RawFile):
    try:
        return parse_json(read_control(path).decode("utf-8"))
    except (OSError, ValueError) as exc:
        raise RunError(f"cannot read bounded proof control: {exc}") from exc


def _proof_file(path: Path) -> RawFile:
    try:
        return RawFile.capture(path)
    except (OSError, ValueError) as exc:
        raise RunError(f"cannot capture proof input: {exc}") from exc


def _verify_execution_context(
    path: Path,
    closure_path: Path,
    logs_path: Path,
    *,
    rail: str,
    raw: dict[str, Path],
    runner_sha: str | None = None,
    searchd_sha: str | None = None,
    build_source_revisions: list[str] | None = None,
) -> dict:
    """Check frozen bytes and prescribed syntax; OS/tool execution remains unattested."""
    where = f"{rail} execution context"
    context_file, closure_file = _proof_file(path), _proof_file(closure_path)
    logs_file = _proof_file(logs_path)
    raw_files = {name: _proof_file(artifact) for name, artifact in raw.items()}
    inputs = [context_file, closure_file, logs_file, *raw_files.values()]
    raw_context = _proof_control(context_file)
    fresh_context = isinstance(raw_context, dict) and raw_context.get("schema_version") == (
        portable_proof.FRESH_EXECUTION_CONTEXT_VERSION
    )
    context = _exact_keys(
        raw_context,
        {
            "schema_version",
            "rail",
            "revision",
            "os",
            "tools",
            "binaries",
            "commands",
            "raw_evidence",
        }
        | ({"build_profile"} if fresh_context else set()),
        where,
    )
    if (
        type(context["schema_version"]) is not int
        or context["schema_version"]
        not in (
            portable_proof.EXECUTION_CONTEXT_VERSION,
            portable_proof.FRESH_EXECUTION_CONTEXT_VERSION,
        )
        or context["rail"] != rail
        or fresh_context
        and (rail != "sdk" or context["build_profile"] != portable_proof.FRESH_BUILD_PROFILE)
        or not _is_hex(context["revision"], 40)
    ):
        raise RunError(f"{where} schema/rail/revision mismatch")
    closure = _validate_source_closure_shape(
        _proof_control(closure_file), f"{where} source closure"
    )
    if context["revision"] != closure["revision"]:
        raise RunError(f"{where} source revision mismatch")
    expected_raw = {"source-closure.json": closure_file, **raw_files}
    recorded_raw = _exact_keys(context["raw_evidence"], set(expected_raw), f"{where}.raw_evidence")
    for name, artifact in expected_raw.items():
        if recorded_raw[name] != artifact.sha256.removeprefix("sha256:"):
            raise RunError(f"{where} raw evidence digest mismatch: {name}")
    os_row = _exact_keys(
        context["os"], {"system", "release", "machine", "python_version"}, f"{where}.os"
    )
    if any(not isinstance(value, str) or not value for value in os_row.values()):
        raise RunError(f"{where} malformed OS identity")
    tool_names = {"python", "cargo", "cargo-nextest", "rustc", "git", "bash", "just", "cargow"}
    tools = _exact_keys(context["tools"], tool_names, f"{where}.tools")
    for name, row in tools.items():
        tool = _exact_keys(row, {"path", "realpath", "sha256", "version"}, f"{where}.tools.{name}")
        if (
            not all(
                isinstance(tool[key], str) and tool[key] for key in ("path", "realpath", "version")
            )
            or not Path(tool["path"]).is_absolute()
            or not Path(tool["realpath"]).is_absolute()
            or not _is_hex(tool["sha256"], 64)
        ):
            raise RunError(f"{where} malformed tool identity: {name}")
    collection_name = "nextest-inventory.json" if rail == "sdk" else "rust-inventory.json"
    try:
        selected_binaries = portable_proof.selected_test_binaries(raw_files[collection_name])
    except (OSError, ValueError) as exc:
        raise RunError(f"{where} selected executable collection refused: {exc}") from exc
    binary_names = set(selected_binaries) | ({"runner", "searchd"} if rail == "sdk" else set())
    binaries = _exact_keys(context["binaries"], binary_names, f"{where}.binaries")
    binary_root = path.parent / f"{rail}-binaries"
    try:
        if set(entry.name for entry in binary_root.iterdir()) != binary_names:
            raise RunError(f"{where} frozen binary inventory mismatch")
    except OSError as exc:
        raise RunError(f"{where} frozen binary inventory refused: {exc}") from exc
    for name, row in binaries.items():
        binary = _exact_keys(row, {"path", "sha256"}, f"{where}.binaries.{name}")
        if (
            not isinstance(binary["path"], str)
            or not Path(binary["path"]).is_absolute()
            or not _is_hex(binary["sha256"], 64)
        ):
            raise RunError(f"{where} malformed binary identity: {name}")
        if name in selected_binaries and binary["path"] != str(selected_binaries[name]):
            raise RunError(f"{where} binary path differs from raw collection: {name}")
        try:
            frozen_binary = _proof_file(binary_root / name)
            inputs.append(frozen_binary)
            frozen_sha = frozen_binary.sha256.removeprefix("sha256:")
        except (OSError, ValueError) as exc:
            raise RunError(f"{where} frozen binary refused: {name}: {exc}") from exc
        if frozen_sha != binary["sha256"]:
            raise RunError(f"{where} frozen binary digest mismatch: {name}")
    if rail == "sdk" and (
        binaries["runner"]["sha256"] != runner_sha or binaries["searchd"]["sha256"] != searchd_sha
    ):
        raise RunError(f"{where} binary digest differs from independent capture/protocol pin")
    commands = context["commands"]
    if not isinstance(commands, list) or not commands or not isinstance(commands[0], dict):
        raise RunError(f"{where} missing commands")
    first_argv = commands[0].get("argv")
    if (
        not isinstance(first_argv, list)
        or len(first_argv) != 7
        or not isinstance(first_argv[-1], str)
    ):
        raise RunError(f"{where} malformed source command")
    original_out = Path(first_argv[-1]).parent
    if not original_out.is_absolute():
        raise RunError(f"{where} command output root is not absolute")
    try:
        build_profile = portable_proof.validated_build_profile(context, original_out)
    except ValueError as exc:
        raise RunError(f"{where} build profile refused: {exc}") from exc
    first_inherited = commands[0].get("inherited_environment")
    if not isinstance(first_inherited, dict) or any(
        key not in portable_proof.RELEVANT_ENV or not isinstance(value, str)
        for key, value in first_inherited.items()
    ):
        raise RunError(f"{where} malformed inherited environment")
    expected = portable_proof._expected_commands(
        rail,
        original_out,
        tools,
        binaries,
        inherited_environment=first_inherited,
        build_profile=build_profile,
    )
    if len(commands) != len(expected):
        raise RunError(f"{where} command count mismatch")
    with _frozen_context_logs(logs_file, rail) as logs:
        try:
            portable_proof.validate_fresh_binary_paths(
                context, original_out, _proof_control(logs["metadata.stdout"])
            )
        except ValueError as exc:
            raise RunError(f"{where} fresh binary paths refused: {exc}") from exc
        _verify_context_commands(
            commands,
            expected,
            logs,
            raw_files,
            collection_name,
            rail,
            build_profile=build_profile,
        )
    for captured in inputs:
        if _proof_file(captured.path) != captured:
            raise RunError(f"{where} input changed during verification: {captured.path}")
    try:
        if set(entry.name for entry in binary_root.iterdir()) != binary_names:
            raise RunError(f"{where} frozen binary inventory changed during verification")
    except OSError as exc:
        raise RunError(f"{where} frozen binary inventory refused: {exc}") from exc
    if build_profile is not None and build_source_revisions is not None:
        build_source_revisions.append(closure["revision"])
    return closure


def _verify_context_commands(
    commands, expected, logs, raw, collection_name, rail, *, build_profile=None
):
    where = f"{rail} execution context"
    reuse_build_raw = {}
    for index, (row, (name, argv, overrides)) in enumerate(zip(commands, expected)):
        command = _exact_keys(
            row,
            {
                "name",
                "argv",
                "cwd",
                "environment",
                "inherited_environment",
                "environment_sha256",
                "exit_code",
                "stdout",
                "stdout_sha256",
                "stderr",
                "stderr_sha256",
            },
            f"{where}.commands[{index}]",
        )
        inherited = command["inherited_environment"]
        if (
            command["name"] != name
            or command["argv"] != argv
            or command["cwd"] != str(portable_proof.ROOT)
            or command["environment"] != overrides
            or type(command["exit_code"]) is not int
            or command["exit_code"] != 0
            or not isinstance(inherited, dict)
            or any(
                key not in portable_proof.RELEVANT_ENV or not isinstance(value, str)
                for key, value in inherited.items()
            )
            or command["environment_sha256"]
            != portable_proof._environment_digest({**inherited, **overrides})
        ):
            raise RunError(f"{where} prescribed command/environment mismatch: {name}")
        for stream in ("stdout", "stderr"):
            if command[stream] != f"{name}.{stream}" or not _is_hex(
                command[f"{stream}_sha256"], 64
            ):
                raise RunError(f"{where} malformed command output digest: {name}")
            output = logs[command[stream]]
            observed_digest = output.sha256.removeprefix("sha256:")
            if observed_digest != command[f"{stream}_sha256"]:
                raise RunError(f"{where} frozen command output digest mismatch: {command[stream]}")
            if stream == "stdout" and name in {"rust-build", "metadata", "rust-collection"}:
                reuse_build_raw[name] = output
            raw_name = {
                "rust-collection": collection_name,
                "rust-test": "nextest.jsonl" if rail == "sdk" else "rust-nextest.jsonl",
            }.get(name)
            if stream == "stdout" and raw_name is not None:
                if observed_digest != raw[raw_name].sha256.removeprefix("sha256:"):
                    raise RunError(f"{where} raw evidence differs from command output: {name}")
    try:
        portable_proof.verify_reused_build(
            reuse_build_raw["rust-build"],
            reuse_build_raw["metadata"],
            reuse_build_raw["rust-collection"],
            workspace_root=Path(commands[0]["cwd"]),
            build_profile=build_profile,
        )
    except ValueError as exc:
        raise RunError(f"{where} native reused build refused: {exc}") from exc


def _verify_receipt_test_count(receipt: dict, results: dict, where: str) -> None:
    if receipt["test_event_count"] != results["executed"]:
        raise RunError(f"{where} test_event_count differs from executed tests")


def _verify_required_inventory(inventory: Path, role: str, receipt: dict) -> None:
    """Compare collection with the authority committed at the receipt revision."""
    authority_ref = "benchmarks/retrieval/proof-required-tests.json"
    closure = receipt["source_closure"]
    matching = [row["sha256"] for row in closure["files"] if row["path"] == authority_ref]
    if len(matching) != 1:
        raise RunError(f"{role} receipt source closure lacks required test authority")
    source_root = Path(__file__).resolve().parents[3]
    try:
        committed = subprocess.check_output(
            ["git", "show", f"{closure['revision']}:{authority_ref}"],
            cwd=source_root,
            stderr=subprocess.PIPE,
        )
    except (OSError, subprocess.CalledProcessError) as exc:
        raise RunError(
            f"{role} required test authority is unavailable at receipt revision"
        ) from exc
    if hashlib.sha256(committed).hexdigest() != matching[0]:
        raise RunError(f"{role} required test authority differs from receipt source closure")
    with tempfile.TemporaryDirectory(prefix="qi-required-tests-") as directory:
        # This is our newly allocated scratch directory, not an evidence path.
        # Resolve host aliases such as macOS /tmp before the strict reader.
        committed_path = Path(directory).resolve(strict=True) / "proof-required-tests.json"
        committed_path.write_bytes(committed)
        try:
            verify_inventory_authority(inventory, role, committed_path)
        except ValueError as exc:
            raise RunError(f"{role} inventory differs from source authority: {exc}") from exc


def _validate_counts_shape(payload: object, where: str) -> dict:
    results = _exact_keys(payload, {"command", "selected", "executed", "passed", "failed"}, where)
    if not isinstance(results["command"], str) or not results["command"]:
        raise RunError(f"{where}.command must be a nonempty string")
    for key in ("selected", "executed", "passed", "failed"):
        value = results[key]
        if type(value) is not int or isinstance(value, bool) or value < 0:
            raise RunError(f"{where}.{key} must be an integer >= 0")
    return results


def _validate_sdk_results_shape(payload: object, where: str) -> dict:
    results = _exact_keys(
        payload,
        {
            "command",
            "separate_process",
            "sealed_receipt_digest",
            "activation_ack_digest",
            "empty_check",
            "binary_digest",
            "sdk_route",
            "selected",
            "executed",
            "passed",
            "failed",
        },
        where,
    )
    if not isinstance(results["command"], str) or not results["command"]:
        raise RunError(f"{where}.command must be a nonempty string")
    for key in ("separate_process", "empty_check"):
        if type(results[key]) is not bool:
            raise RunError(f"{where}.{key} must be a strict boolean")
    for key in ("sealed_receipt_digest", "activation_ack_digest", "binary_digest"):
        if not _is_hex(results[key], 64):
            raise RunError(f"{where}.{key} must be a lowercase sha256")
    if not isinstance(results["sdk_route"], str) or not results["sdk_route"]:
        raise RunError(f"{where}.sdk_route must be a nonempty string")
    for key in ("selected", "executed", "passed", "failed"):
        value = results[key]
        if type(value) is not int or isinstance(value, bool) or value < 0:
            raise RunError(f"{where}.{key} must be an integer >= 0")
    return results


def _require_conditional_identity(
    results: dict,
    *,
    source_revision: str,
    repository_commit: str,
    model_sha256: str,
    dependency_sha256: str,
) -> None:
    identity = results["identity"]
    if identity != {
        "source_revision": source_revision,
        "repository_commit": repository_commit,
        "model_sha256": model_sha256,
        "dependency_sha256": dependency_sha256,
    }:
        raise RunError("conditional raw proof identity differs from frozen source/model/dependency")


def _validate_parity_results_shape(payload: object, where: str, raw_kind: str) -> dict:
    if isinstance(payload, dict) and payload.get("schema_version") == 2:
        from tools.benchmark.retrieval import conditional_proof

        try:
            return conditional_proof.validate_results(payload, raw_kind)
        except (ValueError, KeyError, TypeError, IndexError) as error:
            raise RunError(f"{where}: {error}") from error
    results = _exact_keys(
        payload,
        {
            "schema_version",
            "command",
            "status",
            "selected",
            "executed",
            "passed",
            "failed",
            "identity",
            "raw_proof",
            "execution_receipt",
        },
        where,
    )
    if type(results["schema_version"]) is not int or results["schema_version"] != 1:
        raise RunError(f"{where}.schema_version must be 1")
    if not isinstance(results["command"], str) or not results["command"]:
        raise RunError(f"{where}.command must be a nonempty string")
    if results["status"] not in ("pass", "fail"):
        raise RunError(f"{where}.status must be pass or fail")
    for key in ("selected", "executed", "passed", "failed"):
        value = results[key]
        if type(value) is not int or isinstance(value, bool) or value < 0:
            raise RunError(f"{where}.{key} must be an integer >= 0")
    identity = _exact_keys(
        results["identity"],
        {"source_revision", "repository_commit", "model_sha256", "dependency_sha256"},
        f"{where}.identity",
    )
    for key, length in (
        ("source_revision", 40),
        ("repository_commit", 40),
        ("model_sha256", 64),
        ("dependency_sha256", 64),
    ):
        if not _is_hex(identity[key], length):
            raise RunError(f"{where}.identity.{key} has invalid digest")
    raw = _exact_keys(results["raw_proof"], {"kind", "rows"}, f"{where}.raw_proof")
    if raw["kind"] != raw_kind or not isinstance(raw["rows"], list) or not raw["rows"]:
        raise RunError(f"{where} lacks raw {raw_kind} rows")
    cases = []
    passed = 0
    for index, value in enumerate(raw["rows"]):
        row_where = f"{where}.raw_proof.rows[{index}]"
        if raw_kind == "model_vectors":
            row = _exact_keys(value, {"case_id", "reference_vector", "observed_vector"}, row_where)
            reference = row["reference_vector"]
            observed = row["observed_vector"]
            if (
                not isinstance(reference, list)
                or not reference
                or len(reference) > 4096
                or not isinstance(observed, list)
                or len(reference) != len(observed)
                or any(
                    type(component) not in (int, float)
                    or abs(component) > 1_000_000
                    or not math.isfinite(component)
                    for vector in (reference, observed)
                    for component in vector
                )
            ):
                raise RunError(f"{row_where} has invalid raw vectors")
            passed += reference == observed
        elif raw_kind == "incremental_rows":
            row = _exact_keys(value, {"case_id", "fresh_row_ids", "incremental_row_ids"}, row_where)
            for key in ("fresh_row_ids", "incremental_row_ids"):
                ids = row[key]
                if (
                    not isinstance(ids, list)
                    or not ids
                    or any(not isinstance(item, str) or not item for item in ids)
                    or ids != sorted(set(ids))
                ):
                    raise RunError(f"{row_where}.{key} must be sorted unique row IDs")
            passed += row["fresh_row_ids"] == row["incremental_row_ids"]
        else:
            raise RunError(f"{where} has unknown raw proof kind")
        case_id = row["case_id"]
        if not isinstance(case_id, str) or not case_id:
            raise RunError(f"{row_where}.case_id must be nonempty")
        cases.append(case_id)
    if cases != sorted(set(cases)):
        raise RunError(f"{where} raw case IDs must be sorted and unique")
    if (
        results["selected"] != len(cases)
        or results["executed"] != len(cases)
        or results["passed"] != passed
        or results["failed"] != len(cases) - passed
        or (results["status"] == "pass") != (results["failed"] == 0)
    ):
        raise RunError(f"{where} summary differs from raw {raw_kind} rows")
    receipt = _exact_keys(
        results["execution_receipt"],
        {
            "schema_version",
            "command",
            "exit_code",
            "source_revision",
            "repository_commit",
            "model_sha256",
            "dependency_sha256",
            "runner_binary_sha256",
            "raw_sha256",
        },
        f"{where}.execution_receipt",
    )
    if (
        type(receipt["schema_version"]) is not int
        or receipt["schema_version"] != 1
        or receipt["command"] != results["command"]
        or type(receipt["exit_code"]) is not int
        or receipt["exit_code"] != 0
        or any(receipt[key] != identity[key] for key in identity)
        or not _is_hex(receipt["runner_binary_sha256"], 64)
        or receipt["raw_sha256"] != digest(canonical(raw))
    ):
        raise RunError(f"{where} execution receipt does not bind raw proof and identity")
    return results


def freeze_receipts(spec: dict, stage: Path) -> dict[str, str]:
    """Copy spec receipt artifacts into the stage. Returns key -> stage path."""
    receipts = spec.get("receipts", {})
    if not receipts:
        return {}
    target_dir = stage / "receipts"
    target_dir.mkdir(parents=True, exist_ok=True)
    frozen = {}
    for key in RECEIPT_KEYS:
        if key in ("contract_execution_logs", "sdk_execution_logs"):
            continue  # Generated from the context's sibling command transcripts below.
        if key not in receipts:
            continue
        source = Path(receipts[key])
        try:
            target = target_dir / f"{key}{source.suffix or '.json'}"
            RawFile.capture(source).copy_to(target)
        except (OSError, ValueError) as exc:
            raise RunError(f"cannot freeze receipt artifact {key}: {exc}") from exc
        frozen[key] = str(target)
    for rail in ("contract", "sdk"):
        key = f"{rail}_execution_context"
        if key not in receipts:
            continue
        source_dir = Path(receipts[key]).parent
        context = _proof_control(Path(frozen[key]))
        if (
            not isinstance(context, dict)
            or type(context.get("schema_version")) is not int
            or context["schema_version"]
            not in (
                portable_proof.EXECUTION_CONTEXT_VERSION,
                portable_proof.FRESH_EXECUTION_CONTEXT_VERSION,
            )
        ):
            raise RunError("execution context schema mismatch during freeze")
        try:
            portable_proof.validated_build_profile(context, source_dir)
        except ValueError as exc:
            raise RunError(f"execution context build profile refused during freeze: {exc}") from exc
        try:
            selected = portable_proof.selected_test_binaries(
                RawFile.capture(source_dir / "rust-collection.stdout")
            )
        except (OSError, ValueError) as exc:
            raise RunError(f"cannot freeze selected executable collection: {exc}") from exc
        roles = set(selected) | ({"runner", "searchd"} if rail == "sdk" else set())
        binaries = _exact_keys(context.get("binaries"), roles, "execution context binaries")
        binary_root = target_dir / f"{rail}-binaries"
        binary_root.mkdir(exist_ok=True)
        for role, row in binaries.items():
            binary = _exact_keys(row, {"path", "sha256"}, "execution context binary")
            if (
                not isinstance(binary["path"], str)
                or not Path(binary["path"]).is_absolute()
                or not _is_hex(binary["sha256"], 64)
                or role in selected
                and binary["path"] != str(selected[role])
            ):
                raise RunError("execution context binary differs from selected collection")
            source_binary = Path(binary["path"])
            try:
                captured = RawFile.capture(source_binary)
                if captured.sha256.removeprefix("sha256:") != binary["sha256"]:
                    raise RunError(f"execution context binary changed during freeze: {role}")
                target_binary = binary_root / role
                captured.copy_to(target_binary)
            except (OSError, ValueError) as exc:
                raise RunError(f"cannot freeze execution context binary: {role}: {exc}") from exc
        try:
            if set(entry.name for entry in binary_root.iterdir()) != roles:
                raise RunError("frozen execution context binary inventory mismatch")
        except OSError as exc:
            raise RunError(f"cannot inspect frozen execution context binaries: {exc}") from exc
        target = target_dir / f"{rail}_execution_logs.zip"
        try:
            logs = {name: RawFile.capture(source_dir / name) for name in _context_log_names(rail)}
            if sum(log.size for log in logs.values()) > MAX_CONTEXT_LOG_BYTES:
                raise RunError("execution command logs are oversized")
            raw_archive.pack(logs, target, limits=_context_archive_limits(rail))
        except (OSError, ValueError) as exc:
            raise RunError(f"cannot freeze {rail} execution command logs: {exc}") from exc
        frozen[f"{rail}_execution_logs"] = str(target)
    return frozen


def freeze_admission(spec: dict, stage: Path, frozen_receipts: dict[str, str]) -> dict[str, object]:
    """Freeze and preflight the W0-B authority packet for a qualified run."""
    scope = spec.get("scope", "exploratory")
    raw = spec.get("admission")
    if scope != "qualified":
        if raw is not None:
            raise RunError("admission authority is valid only for qualified capture")
        return {}
    if not isinstance(raw, dict):
        raise RunError("qualified capture requires the W0-B admission bundle")
    keys = _admission_keys(raw)
    admission = _exact_keys(raw, set(keys), "spec.admission")
    manifest = validate_admission_manifest(read_json(Path(admission["manifest"])))
    expected_keys = (
        ADMISSION_LOCAL_KEYS if manifest["schema_version"] == 2 else ADMISSION_DISJOINT_KEYS
    )
    if set(keys) != set(expected_keys):
        raise RunError("qualification admission schema and custody paths differ")
    target_dir = stage / "admission"
    target_dir.mkdir(parents=True, exist_ok=True)

    frozen: dict[str, object] = {}
    scalar_names = {
        "manifest": "admission.json",
        "license_receipt": "license-receipt.json",
        "adjudication_receipt": "adjudication-receipt.json",
    }
    if manifest["schema_version"] == 2:
        scalar_names.update(
            {
                "experiment_custody": "experiment-custody.json",
                "development_suite": "development-suite.json",
            }
        )
    else:
        scalar_names.update(
            {"split_manifest": "split-manifest.json", "split_releases": "split-releases.json"}
        )
    for key, name in scalar_names.items():
        source = Path(admission[key])
        target = target_dir / name
        try:
            before = sha_file(source)
            shutil.copyfile(source, target)
            after = sha_file(target)
        except OSError as exc:
            raise RunError(f"cannot freeze qualification admission {key}: {exc}") from exc
        if before != after:
            raise RunError(f"qualification admission changed during freeze: {key}")
        frozen[key] = str(target)

    annotation_paths: list[str] = []
    annotation_sources = admission["annotation_receipts"]
    if not isinstance(annotation_sources, list) or len(annotation_sources) != 2:
        raise RunError("qualification admission requires two annotation receipt paths")
    for index, source_name in enumerate(annotation_sources):
        source = Path(source_name)
        target = target_dir / f"annotation-{index + 1}-receipt.json"
        try:
            before = sha_file(source)
            shutil.copyfile(source, target)
            after = sha_file(target)
        except OSError as exc:
            raise RunError(
                f"cannot freeze qualification annotation receipt {index}: {exc}"
            ) from exc
        if before != after:
            raise RunError("qualification annotation receipt changed during freeze")
        annotation_paths.append(str(target))
    frozen["annotation_receipts"] = annotation_paths

    required_receipts = {
        key: Path(frozen_receipts[key])
        for key in ("contract_python_receipt", "contract_rust_receipt", "sdk_receipt")
        if key in frozen_receipts
    }
    verify_admission_bundle(
        Path(str(frozen["manifest"])),
        Path(str(frozen["license_receipt"])),
        [Path(path) for path in annotation_paths],
        Path(str(frozen["adjudication_receipt"])),
        source_revision=git_head_sha(Path(__file__).resolve().parents[3]),
        corpus_manifest_path=Path(spec["manifest"]),
        suite_path=Path(spec["suite"]),
        development_suite_path=(
            Path(str(frozen["development_suite"])) if manifest["schema_version"] == 2 else None
        ),
        experiment_custody_path=(
            Path(str(frozen["experiment_custody"])) if manifest["schema_version"] == 2 else None
        ),
        split_manifest_path=(
            Path(str(frozen["split_manifest"])) if manifest["schema_version"] == 3 else None
        ),
        split_releases_path=(
            Path(str(frozen["split_releases"])) if manifest["schema_version"] == 3 else None
        ),
        repo=Path(spec["repo"]),
        query_pack_path=Path(spec["query_pack"]),
        lockfile_path=Path(spec["semble_lockfile"]),
        host_profile_path=Path(spec["host_profile"]),
        cache_regime=spec.get("cache_regime", "undeclared"),
        receipt_paths=required_receipts,
    )
    return frozen


def freeze_inputs(spec: dict, stage: Path) -> dict:
    """Copy capture inputs into the stage root and return byte-verified paths.

    Captures consume these frozen copies, so the verdict re-verifies the
    exact bytes used rather than whatever the external paths hold later.
    """
    frozen = {}
    for key, name in (
        ("suite", "evaluator-only/suite.json"),
        ("query_pack", "query-pack.json"),
        ("manifest", "corpus-manifest.json"),
        ("semble_lockfile", "semble-lockfile.txt"),
        ("host_profile", "host-profile.json"),
    ):
        source = Path(spec[key])
        try:
            before = sha_file(source)
            target = stage / name
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(source, target)
            after = sha_file(target)
        except OSError as exc:
            raise RunError(f"cannot freeze capture input {key}: {exc}") from exc
        if before != after:
            raise RunError(f"capture input changed during freeze: {key}")
        if key == "host_profile":
            validate_host_profile(read_json(target))
        frozen[key] = str(target)
    return frozen


def build_run_manifest(
    spec: dict,
    out_root: Path,
    rep_layouts: list[dict],
    host_start: dict,
    host_end: dict,
    reports: list[str],
    frozen_receipts: dict[str, str] | None = None,
    frozen_admission: dict[str, object] | None = None,
) -> dict:
    """Emit the driver-observed run manifest consumed by `verdict`.

    The driver binds digests of the frozen artifacts; the verdict
    re-derives every digest from the sibling artifact bytes. Manifest
    booleans/counts are never verdict authority. All artifact paths are
    relative to the manifest directory so the staged tree can be
    atomically renamed without rebinding. Receipt evidence comes only
    from frozen receipt bytes, never from spec content.
    """
    if not rep_layouts:
        raise RunError("run manifest needs at least one rep")
    rep0 = rep_layouts[0]
    checkout = Path(__file__).resolve().parent.parent.parent.parent
    source_sha = git_head_sha(checkout)
    if not _is_hex(source_sha, 40):
        raise RunError("driver checkout HEAD is unresolved; refusing manifest")
    semble_dir = Path(rep0["semble"]).parent
    mapping_path = semble_dir / "mapping-proof.json"
    mapping = read_json(mapping_path)
    if not isinstance(mapping, dict) or not _is_hex(mapping.get("diff_digest"), 64):
        raise RunError("rep-0 mapping proof lacks a diff digest")
    adapter_path = semble_dir / "adapter-manifest.json"
    adapter_manifest = read_json(adapter_path)
    if not isinstance(adapter_manifest, dict):
        raise RunError("rep-0 adapter manifest is not an object")
    if adapter_manifest.get("semble_version") != SEMBLE_PINNED_VERSION:
        raise RunError(
            f"run requires Semble {SEMBLE_PINNED_VERSION}; adapter holds "
            f"{adapter_manifest.get('semble_version')!r}"
        )
    interpreter = adapter_manifest.get("interpreter", {})
    if not isinstance(interpreter, dict) or not _is_hex(interpreter.get("digest"), 64):
        raise RunError("rep-0 adapter manifest lacks the interpreter digest")
    if not _is_hex(adapter_manifest.get("model_asset_digest"), 64):
        raise RunError("rep-0 adapter manifest lacks the model asset digest")
    semble_profile = spec["execution_profiles"]["semble"]
    expected_profile = semble_profile["mode"]
    expected_alpha = semble_profile["alpha"]
    if adapter_manifest.get("profile") != semble_profile:
        raise RunError("rep-0 adapter profile differs from the pair spec")
    if adapter_manifest.get("requested_alpha") != expected_alpha:
        raise RunError("rep-0 adapter alpha differs from the pair spec")
    if adapter_manifest.get("rerank_applied") is not (expected_profile == "native-default"):
        raise RunError("rep-0 adapter rerank state differs from the pair profile")
    lane_counts = adapter_manifest.get("lane_call_counts")
    if not isinstance(lane_counts, dict) or set(lane_counts) != {"bm25", "semantic", "encode"}:
        raise RunError("rep-0 adapter manifest lacks lane call counts")
    if any(type(count) is not int or count < 0 for count in lane_counts.values()):
        raise RunError("rep-0 adapter lane call counts are malformed")
    lockfile_path = semble_dir / "lockfile.txt"
    if not lockfile_path.is_file():
        raise RunError("rep-0 Semble lockfile is missing")
    latency_path = out_root / "latency-matrix.json"
    latency = read_json(latency_path)
    if not isinstance(latency, dict):
        raise RunError("latency matrix is not an object")
    for key in ("observations_floor", "fresh_roots"):
        value = latency.get(key)
        if type(value) is not int or isinstance(value, bool) or value < 0:
            raise RunError(f"latency matrix {key} must be an integer >= 0")
    try:
        runner_binary_digest = sha_file(Path(spec["runner_binary"]))
    except OSError as exc:
        raise RunError(f"cannot hash Rust runner binary: {exc}") from exc

    def relative(path: Path) -> str:
        try:
            return path.resolve().relative_to(out_root.resolve()).as_posix()
        except ValueError as exc:
            raise RunError(f"artifact escapes the output root: {path}") from exc

    records: list[str] = []
    natives: list[str] = []
    model_cache_manifests: list[str] = []
    quanta_manifests: list[str] = []
    phase_metrics: list[str] = []
    symbol_preflights: list[str] = []
    resource_metrics: list[str] = []
    for layout in rep_layouts:
        for record in sorted(layout["quanta"].values()):
            records.append(relative(Path(record)))
        records.append(relative(Path(layout["semble"])))
        natives.append(relative(Path(layout["semble"]).parent / "native.json"))
        model_cache_manifests.append(
            relative(Path(layout["semble"]).parent / "model-cache-manifest.json")
        )
        if "quanta_manifest" not in layout:
            raise RunError("rep layout lacks the quanta manifest path")
        quanta_manifest_path = Path(layout["quanta_manifest"])
        quanta_manifests.append(relative(quanta_manifest_path))
        quanta_manifest = read_json(quanta_manifest_path)
        if not isinstance(quanta_manifest, dict) or not isinstance(
            quanta_manifest.get("runs"), list
        ):
            raise RunError("quanta manifest is malformed while collecting metrics")
        for run_entry in quanta_manifest["runs"]:
            if not isinstance(run_entry, dict):
                raise RunError("quanta manifest run is malformed while collecting metrics")
            for key, target in (
                ("phase_metrics", phase_metrics),
                ("symbol_preflight", symbol_preflights),
                ("resource_metrics", resource_metrics),
            ):
                ref = run_entry.get(key)
                if not isinstance(ref, str) or not ref:
                    raise RunError(f"quanta manifest run omitted {key}")
                target.append(relative(quanta_manifest_path.parent / ref))
        for key, target in (
            ("semble_phase_metrics", phase_metrics),
            ("semble_resource_metrics", resource_metrics),
        ):
            ref = layout.get(key)
            if not isinstance(ref, str) or not ref:
                raise RunError(f"rep layout omitted {key}")
            target.append(relative(Path(ref)))
    expected_inputs = {
        "suite": out_root / "evaluator-only" / "suite.json",
        "query_pack": out_root / "query-pack.json",
        "manifest": out_root / "corpus-manifest.json",
    }
    for name, expected in expected_inputs.items():
        if Path(spec[name]).resolve() != expected.resolve():
            raise RunError(f"spec.{name} must name the frozen stage copy")
    host_start_path = out_root / "host-start.json"
    host_end_path = out_root / "host-end.json"
    host_timeline_path = out_root / "host-timeline.json"
    evidence: dict = {
        "pair": {"mapping_proof_digest": sha_file(mapping_path)},
        "perf": {
            "observations_floor": latency["observations_floor"],
            "fresh_roots": latency["fresh_roots"],
            "phase_boundaries": bool(phase_metrics),
            "resource_accounting": bool(resource_metrics),
        },
    }
    frozen = frozen_receipts or {}
    unknown_frozen = sorted(set(frozen) - set(RECEIPT_KEYS))
    if unknown_frozen:
        raise RunError(f"frozen receipts hold unknown keys: {unknown_frozen}")
    receipt_artifacts: dict[str, str] = {}
    for key, path in sorted(frozen.items()):
        receipt_artifacts[key] = relative(Path(path))
    if any(key in frozen for key in CONTRACT_EVIDENCE_KEYS):
        if not all(key in frozen for key in CONTRACT_EVIDENCE_KEYS):
            raise RunError("incomplete frozen contract receipt set")
        contract_closure = _verify_execution_context(
            Path(frozen["contract_execution_context"]),
            Path(frozen["contract_source_closure"]),
            Path(frozen["contract_execution_logs"]),
            rail="contract",
            raw={
                "python-inventory.json": Path(frozen["contract_python_inventory"]),
                "rust-inventory.json": Path(frozen["contract_rust_inventory"]),
                "python-junit.xml": Path(frozen["contract_python_raw"]),
                "rust-nextest.jsonl": Path(frozen["contract_rust_raw"]),
            },
        )
        for side in ("python", "rust"):
            _validate_counts_shape(
                read_json(Path(frozen[f"contract_{side}_results"])),
                f"contract {side} results",
            )
            receipt = _validate_receipt_shape(
                read_json(Path(frozen[f"contract_{side}_receipt"])),
                f"contract {side} receipt",
            )
            if receipt["source_closure"] != contract_closure:
                raise RunError(f"contract {side} execution context source closure mismatch")
            role = "pytest-junit" if side == "python" else "nextest-jsonl"
            _verify_receipt_inputs(
                receipt,
                {
                    "execution-context": Path(frozen["contract_execution_context"]),
                    role: Path(frozen[f"contract_{side}_raw"]),
                    ("pytest-inventory" if side == "python" else "nextest-inventory"): Path(
                        frozen[f"contract_{side}_inventory"]
                    ),
                },
                f"contract {side} receipt",
            )
            evidence.setdefault("contract_suites", {})[side] = {
                "test_result_digest": sha_file(Path(frozen[f"contract_{side}_results"])),
                "raw_evidence_digest": sha_file(Path(frozen[f"contract_{side}_raw"])),
                "inventory_digest": sha_file(Path(frozen[f"contract_{side}_inventory"])),
            }
    if any(key in frozen for key in SDK_EVIDENCE_KEYS):
        if not all(key in frozen for key in SDK_EVIDENCE_KEYS):
            raise RunError("incomplete frozen SDK receipt set")
        sdk_closure = _verify_execution_context(
            Path(frozen["sdk_execution_context"]),
            Path(frozen["sdk_source_closure"]),
            Path(frozen["sdk_execution_logs"]),
            rail="sdk",
            raw={
                "nextest-inventory.json": Path(frozen["sdk_inventory"]),
                "nextest.jsonl": Path(frozen["sdk_nextest_raw"]),
                "actual-runner-record.json": Path(frozen["sdk_record_raw"]),
            },
            runner_sha=sha_file(Path(spec["runner_binary"])),
            searchd_sha=read_json(out_root / "protocol-lock.json")["searchd_expected_sha256"],
        )
        sdk_results = _validate_sdk_results_shape(
            read_json(Path(frozen["sdk_results"])), "sdk results"
        )
        sdk_receipt = _validate_receipt_shape(read_json(Path(frozen["sdk_receipt"])), "sdk receipt")
        if sdk_receipt["source_closure"] != sdk_closure:
            raise RunError("SDK execution context source closure mismatch")
        _verify_receipt_inputs(
            sdk_receipt,
            {
                "execution-context": Path(frozen["sdk_execution_context"]),
                "nextest-jsonl": Path(frozen["sdk_nextest_raw"]),
                "runner-record": Path(frozen["sdk_record_raw"]),
                "nextest-inventory": Path(frozen["sdk_inventory"]),
            },
            "sdk receipt",
        )
        evidence["sdk_path"] = {
            "test_result_digest": sha_file(Path(frozen["sdk_results"])),
            "nextest_digest": sha_file(Path(frozen["sdk_nextest_raw"])),
            "runner_record_digest": sha_file(Path(frozen["sdk_record_raw"])),
            "inventory_digest": sha_file(Path(frozen["sdk_inventory"])),
            "separate_process": sdk_results["separate_process"],
            "sealed_receipt": True,
            "activation_ack": True,
            "empty_check": sdk_results["empty_check"],
        }
    if "model_parity_results" in frozen:
        _validate_parity_results_shape(
            read_json(Path(frozen["model_parity_results"])),
            "model parity results",
            "model_vectors",
        )
        evidence["model_parity"] = {
            "test_result_digest": sha_file(Path(frozen["model_parity_results"]))
        }
    if "incremental_results" in frozen:
        _validate_parity_results_shape(
            read_json(Path(frozen["incremental_results"])),
            "incremental results",
            "incremental_rows",
        )
        evidence["incremental"] = {
            "test_result_digest": sha_file(Path(frozen["incremental_results"]))
        }
    claims = spec.get("claims", {})
    if not isinstance(claims, dict):
        raise RunError("spec.claims must be an object")
    strict_claims = {}
    for key in ("quality", "speed", "same_model", "incremental"):
        value = claims.get(key, False)
        if type(value) is not bool:
            raise RunError(f"spec.claims.{key} must be a strict boolean")
        strict_claims[key] = value
    scope = spec.get("scope", "exploratory")
    if scope not in ("exploratory", "qualified"):
        raise RunError("spec.scope must be exploratory or qualified")
    admission_files = frozen_admission or {}
    if scope == "qualified" and set(admission_files) not in (
        set(ADMISSION_LOCAL_KEYS),
        set(ADMISSION_DISJOINT_KEYS),
    ):
        raise RunError("qualified run lacks the complete frozen admission bundle")
    if scope != "qualified" and admission_files:
        raise RunError("exploratory run cannot carry qualification admission authority")
    profile_path = Path(spec.get("host_profile", ""))
    validate_host_profile(read_json(profile_path))
    admission_digest = None
    if admission_files:
        disjoint = "split_manifest" in admission_files
        annotation_refs = admission_files["annotation_receipts"]
        if not isinstance(annotation_refs, list):
            raise RunError("frozen admission annotation receipts are malformed")
        quanta_model_revision = _quanta_admission_model_revision(
            read_json(Path(record_path))
            for layout in rep_layouts
            for record_path in layout["quanta"].values()
        )
        admission = verify_admission_bundle(
            Path(str(admission_files["manifest"])),
            Path(str(admission_files["license_receipt"])),
            [Path(str(path)) for path in annotation_refs],
            Path(str(admission_files["adjudication_receipt"])),
            source_revision=source_sha,
            corpus_manifest_path=Path(spec["manifest"]),
            suite_path=Path(spec["suite"]),
            development_suite_path=(
                None if disjoint else Path(str(admission_files["development_suite"]))
            ),
            experiment_custody_path=(
                None if disjoint else Path(str(admission_files["experiment_custody"]))
            ),
            split_manifest_path=(
                Path(str(admission_files["split_manifest"])) if disjoint else None
            ),
            split_releases_path=(
                Path(str(admission_files["split_releases"])) if disjoint else None
            ),
            repo=Path(spec["repo"]),
            query_pack_path=Path(spec["query_pack"]),
            lockfile_path=Path(spec["semble_lockfile"]),
            host_profile_path=profile_path,
            cache_regime=spec.get("cache_regime", "undeclared"),
            receipt_paths={
                key: Path(frozen[key])
                for key in ("contract_python_receipt", "contract_rust_receipt", "sdk_receipt")
                if key in frozen
            },
            quanta_model_revision=quanta_model_revision,
            semble_model_revision=adapter_manifest.get("model_revision"),
            semble_model_asset_sha256=adapter_manifest.get("model_asset_digest"),
        )
        admission_digest = sha_file(Path(str(admission_files["manifest"])))
        if admission["source_revision"] != source_sha:
            raise RunError("qualified admission is not bound to the driver source")
    artifacts = {
        "suite": relative(Path(spec["suite"])),
        "query_pack": relative(Path(spec["query_pack"])),
        "corpus_manifest": relative(Path(spec["manifest"])),
        "mapping_proof": relative(mapping_path),
        "latency_matrix": relative(latency_path),
        "host_start": relative(host_start_path),
        "host_end": relative(host_end_path),
        "host_profile": relative(profile_path),
        "records": sorted(records),
        "reports": sorted(reports),
        "quanta_manifests": sorted(quanta_manifests),
        "semble_adapter_manifest": relative(adapter_path),
        "semble_lockfile": relative(lockfile_path),
        "semble_native": sorted(natives),
        "semble_model_cache_manifests": sorted(model_cache_manifests),
        "phase_metrics": sorted(phase_metrics),
        "phase_metrics_digests": {ref: sha_file(out_root / ref) for ref in sorted(phase_metrics)},
        "symbol_preflights": sorted(symbol_preflights),
        "resource_metrics": sorted(resource_metrics),
        "protocol_lock": "protocol-lock.json",
    }
    if host_timeline_path.is_file():
        artifacts["host_timeline"] = relative(host_timeline_path)
        artifacts["host_timeline_raw"] = relative(host_timeline_path.with_suffix(".jsonl"))
    closure_path = Path(spec["_driver_source_closure"])
    closure = _validate_source_closure_shape(read_json(closure_path), "driver source closure")
    if closure["revision"] != source_sha:
        raise RunError("driver source closure revision differs from current HEAD")
    source_closure_digest = closure["digest"]
    artifacts["driver_source_closure"] = relative(closure_path)
    if scope == "qualified":
        for key in ("contract_python_receipt", "contract_rust_receipt", "sdk_receipt"):
            receipt = _validate_receipt_shape(read_json(Path(frozen[key])), key)
            if receipt["source_closure"]["digest"] != source_closure_digest:
                raise RunError(f"{key} source closure differs from the capture closure")
    if spec.get("blinding", "attested") == "isolated":
        proof_path = out_root / "isolation-proof.json"
        if not proof_path.is_file():
            raise RunError("isolated run lacks isolation-proof.json")
        artifacts["isolation_proof"] = relative(proof_path)
    artifacts.update(receipt_artifacts)
    if admission_files:
        artifacts.update(
            {
                "admission_manifest": relative(Path(str(admission_files["manifest"]))),
                "license_receipt": relative(Path(str(admission_files["license_receipt"]))),
                "annotation_receipts": [
                    relative(Path(str(path))) for path in admission_files["annotation_receipts"]
                ],
                "adjudication_receipt": relative(
                    Path(str(admission_files["adjudication_receipt"]))
                ),
            }
        )
        for key in (
            ("split_manifest", "split_releases")
            if "split_manifest" in admission_files
            else ("experiment_custody", "development_suite")
        ):
            artifacts[key] = relative(Path(str(admission_files[key])))
    if not (out_root / "protocol-lock.json").is_file():
        raise RunError("protocol-lock.json must exist before the run manifest")
    return {
        "manifest_version": MANIFEST_VERSION,
        "blinding": spec.get("blinding", "attested"),
        "isolation_method": spec.get("isolation_method", "attested-only"),
        "access_block_log": spec.get("access_block_log", "attested-only"),
        "scope": scope,
        "claims": strict_claims,
        "repetitions": len(rep_layouts),
        "evidence": evidence,
        "host": {
            "start_digest": sha_file(host_start_path),
            "end_digest": sha_file(host_end_path),
            **(
                {"timeline_digest": sha_file(host_timeline_path)}
                if host_timeline_path.is_file()
                else {}
            ),
            "cache_regime": spec.get("cache_regime", "undeclared"),
            **(
                {"delegated_cgroup_parent": spec["_linux_cgroup_parent_identity"]}
                if "_linux_cgroup_parent_identity" in spec
                else {}
            ),
        },
        "artifacts": artifacts,
        "provenance": {
            "admission": {"manifest_digest": admission_digest},
            "quanta": {
                "source_sha": source_sha,
                "source_closure_digest": source_closure_digest,
                "binary_digest": runner_binary_digest,
                "embedder": spec.get("embedder", "potion-code"),
                # Binary digests are pinned, but the build source is not
                # independently attested by this capture contract.
                "binary_build_source_revision": None,
            },
            "semble": {
                "revision": SEMBLE_PINNED_VERSION,
                "lockfile_digest": sha_file(lockfile_path),
                "interpreter_digest": interpreter["digest"],
                "model_asset_digest": adapter_manifest["model_asset_digest"],
            },
            "corpus": {
                "digest": sha_file(Path(spec["manifest"])),
                "path_sha_diff_digest": mapping["diff_digest"],
            },
            "suite": {
                "suite_digest": sha_file(Path(spec["suite"])),
                "query_pack_digest": sha_file(Path(spec["query_pack"])),
                "tokenizer_budget_version": TOKENIZER_BUDGET_VERSION,
            },
            "host": {
                "profile_digest": sha_file(profile_path),
                "check_record_digest": digest(canonical({"start": host_start, "end": host_end})),
            },
        },
    }


def run_semble_capture(
    spec: dict, out_dir: Path, pack_path: Path, route: str, rep: int = 0
) -> dict[str, str]:
    adapter = Path(spec.get("_semble_adapter", Path(__file__).resolve().parent / "semble.py"))
    command = [
        sys.executable,
        *(["-I", "-S"] if adapter.suffix == ".pyz" else []),
        str(adapter),
        "run",
        "--repo",
        spec["repo"],
        "--manifest",
        spec["manifest"],
        "--query-pack",
        str(pack_path),
        "--top-k",
        str(spec["top_k"]),
        "--python",
        spec["semble_python"],
        "--lockfile",
        spec["semble_lockfile"],
        "--lockfile-sha256",
        spec["semble_lockfile_sha256"],
        "--cache-root",
        spec["semble_cache_root"],
        "--output-root",
        str(out_dir),
        "--route",
        route,
        "--run-id",
        f"{spec.get('run_id', 'run')}-semble-r{rep}",
        "--seed",
        str(_int(spec.get("seed", 0), "spec.seed") + rep),
        "--blinding",
        spec.get("blinding", "attested"),
        "--isolation-method",
        spec.get("isolation_method", "attested-only: worker sees pack+corpus only"),
        "--access-block-log",
        spec.get("access_block_log", "attested-only: no suite path is passed to the worker"),
        "--repetitions",
        str(spec.get("query_repetitions_per_root", 1)),
        "--warmup-passes",
        str(spec.get("query_warmup_passes", 1)),
        "--semble-profile",
        spec["execution_profiles"]["semble"]["mode"],
    ]
    if spec["execution_profiles"]["semble"]["mode"] == "hybrid-no-rerank":
        command += ["--alpha", str(spec["execution_profiles"]["semble"]["alpha"])]
    if "_query_protocol" in spec:
        command += ["--query-protocol", spec["_query_protocol"]]
    if "_materialized_corpus" in spec:
        command += ["--materialized-corpus"]
    command += ["--model-revision", spec["semble_model_revision"]]
    command, isolation = sandbox_command(spec, command)
    evidence_root = out_dir.parent
    resource_path = evidence_root / "semble-resource-metrics.json"
    stdout_path = evidence_root / "semble-adapter.stdout.log"
    stderr_path = evidence_root / "semble-adapter.stderr.log"
    process_env = capture_process_env(evidence_root / "semble-process-tmp")
    resource = run_monitored_process(
        command,
        stdout_path=stdout_path,
        stderr_path=stderr_path,
        resource_path=resource_path,
        timeout_secs=_int(spec.get("timeout_secs", 1800), "spec.timeout_secs"),
        subject_path=out_dir / "record.json",
        env=process_env,
        cwd=process_env["TMPDIR"],
        isolation=isolation,
        capture_scope=spec.get("scope", "exploratory"),
        linux_cgroup_parent=spec.get("linux_cgroup_parent"),
        linux_cgroup_parent_identity=spec.get("_linux_cgroup_parent_identity"),
    )
    if resource["timed_out"]:
        write_process_failure(
            evidence_root,
            system="semble",
            strategy="native",
            failure_type="timeout",
            resource_path=resource_path,
            stderr_path=stderr_path,
            record_path=out_dir / "record.json",
        )
        raise RunError("Semble capture timed out")
    if resource["exit_code"] != 0:
        write_process_failure(
            evidence_root,
            system="semble",
            strategy="native",
            failure_type="nonzero_exit",
            resource_path=resource_path,
            stderr_path=stderr_path,
            record_path=out_dir / "record.json",
        )
        tail = stderr_path.read_text(encoding="utf-8", errors="replace")[-2000:]
        raise RunError(f"Semble capture failed: {tail}")
    phase_path = out_dir / "phase-metrics.json"
    if not phase_path.is_file():
        write_process_failure(
            evidence_root,
            system="semble",
            strategy="native",
            failure_type="missing_phase_metrics",
            resource_path=resource_path,
            stderr_path=stderr_path,
            record_path=out_dir / "record.json",
        )
        raise RunError("Semble capture omitted phase metrics")
    native = read_json(out_dir / "native.json")
    stats = native.get("stats", {}) if isinstance(native, dict) else {}
    if not isinstance(stats, dict):
        raise RunError("Semble native output lacks index statistics")
    cache_root = Path(spec["semble_cache_root"])
    bind_storage_metrics(
        resource_path,
        {
            "index_bytes": stats.get("index_resident_bytes"),
            "model_cache_bytes": tree_size(cache_root),
            "parser_cache_bytes": 0,
            "embedding_cache_bytes": 0,
            "discovered_files": stats.get("indexed_files"),
            "indexed_chunks": stats.get("total_chunks"),
            "index_storage": "memory",
            "index_measurement": stats.get("index_measurement"),
        },
    )
    return {"phase_metrics": str(phase_path), "resource_metrics": str(resource_path)}


def load_quality_batch_spec(path: Path) -> dict:
    batch = _exact_keys(
        read_json(path), {"schema_version", "member_specs", "output_root"}, "quality batch spec"
    )
    members = batch["member_specs"]
    if batch["schema_version"] != 1 or not isinstance(members, list) or len(members) < 2:
        raise RunError("quality batch requires version 1 and at least two member specs")
    if any(not isinstance(value, str) or not Path(value).is_absolute() for value in members):
        raise RunError("quality batch member specs must be absolute paths")
    if len(set(members)) != len(members):
        raise RunError("quality batch member specs must be distinct")
    output = batch["output_root"]
    if not isinstance(output, str) or not Path(output).is_absolute():
        raise RunError("quality batch output root must be an absolute path")
    return batch


def _quality_batch_members(
    batch: dict,
) -> tuple[list[tuple[Path, dict, dict, dict, SourceSnapshot]], dict]:
    excluded = {"suite", "query_pack", "output_root", "run_id", "semble_cache_root"}
    members = []
    shared = None
    model_asset = None
    parser_identity = source_oracle.census_parser_identity()
    census_cache = source_oracle.DeclarationCensusCache(parser_identity)
    shared_snapshot: SourceSnapshot | None = None
    for name in batch["member_specs"]:
        path = Path(name)
        spec = load_spec(path)
        if spec.get("scope", "exploratory") != "exploratory" or any(
            spec.get("claims", {}).values()
        ):
            raise RunError("quality batch is exploratory only and cannot carry claims")
        if "source_closure_reuse" in spec:
            raise RunError(
                "quality batch captures one new source closure for its complete execution"
            )
        if spec.get("repetitions", 1) != 1 or len(spec["strategies"]) != 1:
            raise RunError("quality batch requires one fresh index and one strategy per repository")
        current = {key: value for key, value in spec.items() if key not in excluded}
        if shared is None:
            shared = current
        elif current != shared:
            differing = sorted(
                set(current) ^ set(shared) | {k for k in current if current.get(k) != shared.get(k)}
            )
            raise RunError(f"quality batch product/source contract differs: {differing}")
        cache = Path(spec["semble_cache_root"])
        if not cache.is_absolute() or not cache.is_dir():
            raise RunError("quality batch Semble cache root is missing")
        try:
            _, observed_asset = semble_adapter.resolve_model_revision(
                cache / "hf", semble_adapter.DEFAULT_MODEL_ID, spec["semble_model_revision"]
            )
        except semble_adapter.AdapterError as exc:
            raise RunError(f"quality batch Semble model cache refused: {exc}") from exc
        if model_asset is None:
            model_asset = observed_asset
        elif observed_asset != model_asset:
            raise RunError("quality batch Semble model assets differ")
        suite, pack, source = validate_suite(
            Path(spec["repo"]),
            read_json(Path(spec["suite"])),
            declaration_census_cache=census_cache,
            source_snapshot=shared_snapshot,
        )
        if shared_snapshot is None:
            shared_snapshot = source
        if pack != read_json(Path(spec["query_pack"])):
            raise RunError(f"quality batch blind pack differs from its suite: {path}")
        members.append((path, spec, suite, pack, source))
    return members, {
        "model_asset_sha256": model_asset,
        "oracle_parser_identity": parser_identity,
    }


def _quality_batch_input_snapshot(members: list[tuple]) -> list[dict]:
    """Bind external member input bytes before product execution and promotion."""
    snapshot = []
    for spec_path, spec, suite, pack, _source in members:
        suite_path = Path(spec["suite"])
        pack_path = Path(spec["query_pack"])
        if (
            read_json(spec_path) != spec
            or read_json(suite_path) != suite
            or read_json(pack_path) != pack
        ):
            raise RunError(f"quality batch member inputs changed while loading: {spec_path}")
        snapshot.append(
            {
                "member_spec_sha256": sha_file(spec_path),
                "suite_sha256": sha_file(suite_path),
                "query_pack_file_sha256": sha_file(pack_path),
            }
        )
    return snapshot


def _quality_batch_model_assets_unchanged(members: list[tuple], expected: str) -> None:
    for spec_path, spec, _suite, _pack, _source in members:
        try:
            _, observed = semble_adapter.resolve_model_revision(
                Path(spec["semble_cache_root"]) / "hf",
                semble_adapter.DEFAULT_MODEL_ID,
                spec["semble_model_revision"],
            )
        except semble_adapter.AdapterError as exc:
            raise RunError(f"quality batch Semble model cache changed: {spec_path}: {exc}") from exc
        if observed != expected:
            raise RunError(f"quality batch Semble model asset changed: {spec_path}")


def run_quality_batch(
    batch: dict,
    *,
    prevalidated: tuple[list[tuple[Path, dict, dict, dict, SourceSnapshot]], dict] | None = None,
) -> int:
    """Run compatible blind packs through one index per product, then score separately.

    Native records remain union records. Each per-intent scoring view is
    revalidated against its original suite/pack; no projected view is saved
    or represented as a native capture.
    """
    members, model = prevalidated if prevalidated is not None else _quality_batch_members(batch)
    input_snapshot = _quality_batch_input_snapshot(members)
    _quality_batch_model_assets_unchanged(members, model["model_asset_sha256"])
    if source_oracle.census_parser_identity() != model["oracle_parser_identity"]:
        raise RunError("quality batch oracle parser identity changed before capture")
    first_spec = members[0][1]
    out_root = Path(batch["output_root"]).resolve()
    source_repo = Path(first_spec["repo"]).resolve()
    driver_repo = Path(__file__).resolve().parents[3]
    if (
        out_root in (source_repo, driver_repo)
        or source_repo in out_root.parents
        or driver_repo in out_root.parents
    ):
        raise RunError("quality batch output root must be outside source and driver repositories")
    if out_root.exists():
        raise RunError("quality batch output root already exists")
    preflight_capture(first_spec)
    execution_pack, membership = eb.build_execution_pack([row[3] for row in members])
    execution_view = eb.execution_validation_view(execution_pack)
    stage = out_root.parent / (out_root.name + ".staging")
    if stage.exists():
        raise RunError("quality batch staging root already exists")
    preflight_daemon_socket_paths(stage / "quanta", first_spec["strategies"])
    stage.mkdir(parents=True)
    closure_path = stage / "driver-source-closure.json"
    _source_closure(driver_repo, "capture", closure_path)
    (stage / "execution-pack.json").write_bytes(canonical_bytes(execution_pack))
    (stage / "membership.json").write_bytes(canonical_bytes(membership))
    task_ids = [task["task_id"] for task in execution_pack["tasks"]]
    protocol = build_query_protocol(
        task_ids,
        _int(first_spec.get("seed", 0), "spec.seed"),
        _int(first_spec.get("query_warmup_passes", 1), "spec.query_warmup_passes"),
        _int(first_spec.get("query_repetitions_per_root", 1), "spec.query_repetitions_per_root"),
    )
    protocol_path = stage / "query-protocol.json"
    protocol_path.write_bytes(canonical_bytes(protocol))
    run_spec = dict(first_spec)
    run_spec["run_id"] = "quality-batch-" + membership["execution_pack_sha256"][:16]
    run_spec["output_root"] = str(out_root)
    run_spec["_query_protocol"] = str(protocol_path)
    runner_digest = sha_file(Path(run_spec["runner_binary"]))
    product_records = []
    product_packs = []
    for system, routes in (("quanta", ["lexical"]), ("semble", ["semble-lexical-file"])):
        product_pack, _ = project_pack_and_suite(execution_pack, execution_view, routes)
        pack_path = stage / f"{system}-execution-pack.json"
        pack_path.write_bytes(canonical_bytes(product_pack))
        product_packs.append({"path": pack_path.name, "sha256": sha_file(pack_path)})
        if system == "quanta":
            result = run_quanta_strategy(
                run_spec,
                run_spec["strategies"][0],
                0,
                stage / "quanta",
                routes,
                pack_path,
                runner_digest,
            )
            record_path = stage / "quanta" / result["record"]
        else:
            run_semble_capture(run_spec, stage / "semble", pack_path, routes[0])
            record_path = stage / "semble" / "record.json"
        product_records.append(record_path)
    repo = Path(first_spec["repo"])
    _, _, combined = _merge_validated_records(
        repo, execution_view, execution_pack, members[0][4], product_records
    )
    report_rows = []
    for index, (spec_path, _spec, suite, pack, source) in enumerate(members):
        view = eb.project_scoring_view(execution_pack, membership, pack, combined)
        validate_evidence_against_suite(repo, suite, pack, source, view)
        report = evaluate_paired_file_diagnostic(
            suite, pack, view, "semble-lexical-file", "lexical"
        )
        report_path = stage / f"member-{index:02d}-report.json"
        report_path.write_text(
            json.dumps(report, indent=2, sort_keys=True) + "\n", encoding="utf-8"
        )
        report_rows.append(
            {
                "suite_id": suite["suite_id"],
                "member_spec_path": str(spec_path),
                **input_snapshot[index],
                "blind_pack_sha256": digest(canonical(pack)),
                "scoring_view_sha256": digest(canonical(view)),
                "report": report_path.name,
                "report_sha256": sha_file(report_path),
            }
        )
    manifest = {
        "schema_version": 1,
        "kind": "retrieval_quality_execution_batch_v1",
        "qualification": "diagnostic_unqualified",
        "batch_spec_sha256": digest(canonical(batch)),
        "source_revision": git_head_sha(driver_repo),
        "driver_source_closure_digest": _validate_source_closure_shape(
            read_json(closure_path), "batch driver source closure"
        )["digest"],
        "corpus_repository_commit": execution_pack["repository_commit"],
        "file_universe_digest": execution_pack["file_universe_digest"],
        "model_asset_sha256": model["model_asset_sha256"],
        "oracle_parser_identity": model["oracle_parser_identity"],
        "runner_binary_sha256": runner_digest,
        "searchd_binary_sha256": sha_file(Path(run_spec["searchd_binary"])),
        "execution_pack_sha256": membership["execution_pack_sha256"],
        "membership_sha256": sha_file(stage / "membership.json"),
        "product_packs": product_packs,
        "native_records": [
            {"path": path.relative_to(stage).as_posix(), "sha256": sha_file(path)}
            for path in product_records
        ],
        "members": report_rows,
    }
    if _quality_batch_input_snapshot(members) != input_snapshot:
        raise RunError("quality batch member inputs changed during product capture")
    _quality_batch_model_assets_unchanged(members, model["model_asset_sha256"])
    verify_repo(repo, execution_pack["repository_commit"])
    if source_oracle.census_parser_identity() != model["oracle_parser_identity"]:
        raise RunError("quality batch oracle parser identity changed during capture")
    (stage / "batch-manifest.json").write_text(
        json.dumps(manifest, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    _source_closure(driver_repo, "verify", closure_path)
    if out_root.exists():
        raise RunError("quality batch output root appeared before promotion")
    os.rename(stage, out_root)
    print(json.dumps({"output_root": str(out_root), "members": len(members), "native_records": 2}))
    return 0


def verify_quality_batch(batch: dict) -> int:
    """Replay score views from saved native records and original member inputs."""
    root = Path(batch["output_root"]).resolve()
    if not root.is_dir():
        raise RunError("quality batch output root is missing")
    manifest = _exact_keys(
        read_json(root / "batch-manifest.json"),
        {
            "schema_version",
            "kind",
            "qualification",
            "batch_spec_sha256",
            "source_revision",
            "driver_source_closure_digest",
            "corpus_repository_commit",
            "file_universe_digest",
            "model_asset_sha256",
            "oracle_parser_identity",
            "runner_binary_sha256",
            "searchd_binary_sha256",
            "execution_pack_sha256",
            "membership_sha256",
            "product_packs",
            "native_records",
            "members",
        },
        "quality batch manifest",
    )
    if (
        manifest["schema_version"] != 1
        or manifest["kind"] != "retrieval_quality_execution_batch_v1"
        or manifest["qualification"] != "diagnostic_unqualified"
        or manifest["batch_spec_sha256"] != digest(canonical(batch))
    ):
        raise RunError("quality batch manifest contract differs from its batch spec")
    driver_repo = Path(__file__).resolve().parents[3]
    if manifest["source_revision"] != git_head_sha(driver_repo):
        raise RunError("quality batch driver revision changed")
    closure_path = root / "driver-source-closure.json"
    _source_closure(driver_repo, "verify", closure_path)
    closure_digest = _validate_source_closure_shape(
        read_json(closure_path), "batch driver source closure"
    )["digest"]
    if closure_digest != manifest["driver_source_closure_digest"]:
        raise RunError("quality batch driver source closure digest changed")
    members, model = _quality_batch_members(batch)
    input_snapshot = _quality_batch_input_snapshot(members)
    if model["model_asset_sha256"] != manifest["model_asset_sha256"]:
        raise RunError("quality batch model asset changed")
    if model["oracle_parser_identity"] != manifest["oracle_parser_identity"]:
        raise RunError("quality batch oracle parser identity changed")
    packs = [row[3] for row in members]
    execution_pack = read_json(root / "execution-pack.json")
    membership = read_json(root / "membership.json")
    eb.verify_execution_membership(packs, execution_pack, membership)
    if (
        manifest["execution_pack_sha256"] != membership["execution_pack_sha256"]
        or manifest["membership_sha256"] != sha_file(root / "membership.json")
        or manifest["corpus_repository_commit"] != execution_pack["repository_commit"]
        or manifest["file_universe_digest"] != execution_pack["file_universe_digest"]
    ):
        raise RunError("quality batch execution inputs changed")
    first_spec = members[0][1]
    preflight_capture(first_spec)
    if manifest["runner_binary_sha256"] != sha_file(Path(first_spec["runner_binary"])) or manifest[
        "searchd_binary_sha256"
    ] != sha_file(Path(first_spec["searchd_binary"])):
        raise RunError("quality batch product binary changed")
    expected_paths = [
        (
            root
            / "quanta"
            / _strategy_run_directory(0, first_spec["strategies"][0]["name"])
            / "record.json"
        ),
        root / "semble" / "record.json",
    ]
    validation_view = eb.execution_validation_view(execution_pack)
    expected_product_packs = []
    for system, routes in (("quanta", ["lexical"]), ("semble", ["semble-lexical-file"])):
        projected_pack, _ = project_pack_and_suite(execution_pack, validation_view, routes)
        path = root / f"{system}-execution-pack.json"
        if read_json(path) != projected_pack:
            raise RunError(f"quality batch {system} product pack changed")
        expected_product_packs.append({"path": path.name, "sha256": sha_file(path)})
    if manifest["product_packs"] != expected_product_packs:
        raise RunError("quality batch product pack custody changed")
    actual_records = manifest["native_records"]
    if not isinstance(actual_records, list) or len(actual_records) != 2:
        raise RunError("quality batch requires two native product records")
    for row, expected in zip(actual_records, expected_paths, strict=True):
        if row != {"path": expected.relative_to(root).as_posix(), "sha256": sha_file(expected)}:
            raise RunError("quality batch native record path or digest changed")
    repo = Path(first_spec["repo"])
    _, _, combined = _merge_validated_records(
        repo,
        validation_view,
        execution_pack,
        members[0][4],
        expected_paths,
    )
    if not isinstance(manifest["members"], list) or len(manifest["members"]) != len(members):
        raise RunError("quality batch member report count changed")
    for index, ((spec_path, _spec, suite, pack, source), row) in enumerate(
        zip(members, manifest["members"], strict=True)
    ):
        view = eb.project_scoring_view(execution_pack, membership, pack, combined)
        validate_evidence_against_suite(repo, suite, pack, source, view)
        report = evaluate_paired_file_diagnostic(
            suite, pack, view, "semble-lexical-file", "lexical"
        )
        report_path = root / f"member-{index:02d}-report.json"
        expected_row = {
            "suite_id": suite["suite_id"],
            "member_spec_path": str(spec_path),
            **input_snapshot[index],
            "blind_pack_sha256": digest(canonical(pack)),
            "scoring_view_sha256": digest(canonical(view)),
            "report": report_path.name,
            "report_sha256": sha_file(report_path),
        }
        if row != expected_row or read_json(report_path) != report:
            raise RunError(f"quality batch member {index} report or provenance changed")
    print(json.dumps({"verified_members": len(members), "native_records": 2}))
    return 0


def _quality_matrix_groups(matrix: dict) -> list[tuple[str, str, list[str]]]:
    by_repo: dict[str, list[str]] = {}
    for name in matrix["member_specs"]:
        spec = load_spec(Path(name))
        repo = str(Path(spec["repo"]).resolve())
        by_repo.setdefault(repo, []).append(name)
    if len(by_repo) < 2 or any(len(paths) < 2 for paths in by_repo.values()):
        raise RunError("quality matrix requires at least two repositories and two specs each")
    groups = [
        ("r-" + hashlib.sha256(repo.encode()).hexdigest()[:12], repo, sorted(paths))
        for repo, paths in sorted(by_repo.items())
    ]
    if len({name for name, _repo, _paths in groups}) != len(groups):
        raise RunError("quality matrix repository artifact name collision")
    return groups


def _quality_matrix_batch(matrix: dict, group: tuple[str, str, list[str]]) -> dict:
    name, _repo, paths = group
    return {
        "schema_version": 1,
        "member_specs": paths,
        "output_root": str(Path(matrix["output_root"]).resolve() / name),
    }


def run_quality_matrix(matrix: dict) -> int:
    """Prevalidate all groups, then publish one diagnostic batch per repository."""
    root = Path(matrix["output_root"]).resolve()
    if root.exists():
        raise RunError("quality matrix output root already exists")
    driver_repo = Path(__file__).resolve().parents[3]
    groups = _quality_matrix_groups(matrix)
    prepared = []
    for group in groups:
        batch = _quality_matrix_batch(matrix, group)
        repo = Path(group[1])
        if root in (repo, driver_repo) or repo in root.parents or driver_repo in root.parents:
            raise RunError(
                "quality matrix output root must be outside source and driver repositories"
            )
        members, model = _quality_batch_members(batch)
        _quality_batch_input_snapshot(members)
        preflight_daemon_socket_paths(
            Path(batch["output_root"] + ".staging") / "quanta",
            members[0][1]["strategies"],
        )
        prepared.append((group, batch, (members, model)))
    root.mkdir(parents=True)
    rows = []
    for group, batch, validated in prepared:
        name, repo, paths = group
        batch_path = root / f"{name}-spec.json"
        batch_path.write_bytes(canonical_bytes(batch))
        run_quality_batch(batch, prevalidated=validated)
        manifest_path = Path(batch["output_root"]) / "batch-manifest.json"
        rows.append(
            {
                "name": name,
                "repo": repo,
                "member_specs": paths,
                "batch_spec_sha256": sha_file(batch_path),
                "batch_manifest_sha256": sha_file(manifest_path),
            }
        )
    manifest = {
        "schema_version": 1,
        "kind": "retrieval_quality_matrix_v1",
        "qualification": "diagnostic_unqualified",
        "matrix_spec_sha256": digest(canonical(matrix)),
        "groups": rows,
    }
    (root / "matrix-manifest.json").write_text(
        json.dumps(manifest, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    print(json.dumps({"output_root": str(root), "repositories": len(rows)}))
    return 0


def verify_quality_matrix(matrix: dict) -> int:
    root = Path(matrix["output_root"]).resolve()
    if not root.is_dir():
        raise RunError("quality matrix output root is missing")
    manifest = _exact_keys(
        read_json(root / "matrix-manifest.json"),
        {"schema_version", "kind", "qualification", "matrix_spec_sha256", "groups"},
        "quality matrix manifest",
    )
    groups = _quality_matrix_groups(matrix)
    if (
        manifest["schema_version"] != 1
        or manifest["kind"] != "retrieval_quality_matrix_v1"
        or manifest["qualification"] != "diagnostic_unqualified"
        or manifest["matrix_spec_sha256"] != digest(canonical(matrix))
        or not isinstance(manifest["groups"], list)
        or len(manifest["groups"]) != len(groups)
    ):
        raise RunError("quality matrix manifest contract differs from its spec")
    for group, row in zip(groups, manifest["groups"], strict=True):
        name, repo, paths = group
        batch = _quality_matrix_batch(matrix, group)
        batch_path = root / f"{name}-spec.json"
        child_manifest = root / name / "batch-manifest.json"
        if read_json(batch_path) != batch or row != {
            "name": name,
            "repo": repo,
            "member_specs": paths,
            "batch_spec_sha256": sha_file(batch_path),
            "batch_manifest_sha256": sha_file(child_manifest),
        }:
            raise RunError(f"quality matrix batch custody changed: {name}")
        verify_quality_batch(batch)
    print(json.dumps({"verified_repositories": len(groups)}))
    return 0


def cmd_quality_batch(args: argparse.Namespace) -> int:
    try:
        return run_quality_batch(load_quality_batch_spec(Path(args.spec)))
    except (RunError, eb.BatchError, ValueError) as exc:
        print(f"ERROR: {exc}", file=sys.stderr)
        return 2


def cmd_quality_batch_verify(args: argparse.Namespace) -> int:
    try:
        return verify_quality_batch(load_quality_batch_spec(Path(args.spec)))
    except (RunError, eb.BatchError, ValueError) as exc:
        print(f"ERROR: {exc}", file=sys.stderr)
        return 2


def cmd_quality_matrix(args: argparse.Namespace, *, verify: bool) -> int:
    try:
        matrix = load_quality_batch_spec(Path(args.spec))
        return verify_quality_matrix(matrix) if verify else run_quality_matrix(matrix)
    except (RunError, eb.BatchError, ValueError) as exc:
        print(f"ERROR: {exc}", file=sys.stderr)
        return 2


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    sub = parser.add_subparsers(dest="command", required=True)
    quanta = sub.add_parser("quanta", help="run the Rust runner per strategy")
    quanta.add_argument("--spec", required=True)
    pair = sub.add_parser("pair", help="sequential paired capture + scoring")
    pair.add_argument("--spec", required=True)
    quality_batch = sub.add_parser(
        "quality-batch", help="one native index per product, separate diagnostic suite reports"
    )
    quality_batch.add_argument("--spec", required=True)
    quality_batch_verify = sub.add_parser(
        "quality-batch-verify", help="replay native batch records against original member suites"
    )
    quality_batch_verify.add_argument("--spec", required=True)
    for name, help_text in (
        ("quality-matrix", "prevalidate and run one quality batch per repository"),
        ("quality-matrix-verify", "replay every repository quality batch"),
    ):
        sub.add_parser(name, help=help_text).add_argument("--spec", required=True)
    merge = sub.add_parser("merge", help="merge per-system records")
    merge.add_argument("--repo", required=True)
    merge.add_argument("--suite", required=True)
    merge.add_argument("--records", nargs="+", required=True)
    merge.add_argument("--out", required=True)
    verdict = sub.add_parser("verdict", help="re-derive evidence and emit verdict")
    verdict.add_argument("--repo", required=True)
    verdict.add_argument("--suite", required=True)
    verdict.add_argument("--run-manifest", required=True)
    verdict.add_argument("--out", required=True)
    probe = sub.add_parser("host-probe", help="emit host check-record")
    probe.add_argument("--out", default=None)
    profile = sub.add_parser("host-profile", help="freeze a canonical host profile")
    profile.add_argument("--profile-id", required=True)
    profile.add_argument("--out", required=True)
    profile.add_argument("--linux-thermal-zone", action="append")
    profile.add_argument("--linux-max-thermal-millidegrees", type=int)
    profile.add_argument("--linux-min-frequency-percent", type=int)
    return parser


def main(argv: list[str] | None = None) -> int:
    parser = build_parser()
    args = parser.parse_args(argv)
    if args.command in (
        "pair",
        "quanta",
        "merge",
        "verdict",
        "quality-batch",
        "quality-batch-verify",
        "quality-matrix",
        "quality-matrix-verify",
    ) and sys.version_info < (3, 10):
        print("ERROR: retrieval benchmark requires Python 3.10 or newer", file=sys.stderr)
        return 2
    if args.command == "merge":
        return cmd_merge(args)
    if args.command == "host-probe":
        return cmd_host_probe(args)
    if args.command == "host-profile":
        return cmd_host_profile(args)
    if args.command == "quanta":
        return cmd_quanta(args)
    if args.command == "quality-batch":
        return cmd_quality_batch(args)
    if args.command == "quality-batch-verify":
        return cmd_quality_batch_verify(args)
    if args.command in ("quality-matrix", "quality-matrix-verify"):
        return cmd_quality_matrix(args, verify=args.command.endswith("-verify"))
    if args.command == "verdict":
        return cmd_verdict(args)
    return cmd_pair(args)


if __name__ == "__main__":
    raise SystemExit(main())
