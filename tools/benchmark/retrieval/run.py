#!/usr/bin/env python3
"""Paired quality/speed/resource measurement orchestration (RB-05).

Subcommands:
  quanta    run the Rust SDK runner per strategy from a pinned spec
  pair      quanta + Semble sequential capture, merge, score, verdict
  merge     deterministically merge per-system v2 records into one record
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
import shutil
import signal
import subprocess
import sys
import time
from pathlib import Path

try:
    from tools.benchmark.retrieval.contract_proof import nextest_summary, pytest_summary
    from tools.benchmark.retrieval.evaluator import (
        CHUNK_STRATEGIES,
        TOKENIZER_BUDGET_VERSION,
        canonical,
        digest,
        evaluate,
        load_evidence,
        validate_comparison_contract,
        validate_suite,
        verify_repo,
    )
    from tools.benchmark.retrieval.evaluator import (
        read_json as read_evidence_json,
    )
    from tools.benchmark.retrieval.sdk_proof import build_summary_from_evidence
except ImportError:  # direct script invocation: import the sibling module
    sys.path.insert(0, str(Path(__file__).resolve().parent))
    from contract_proof import nextest_summary, pytest_summary  # noqa: E402
    from evaluator import (  # noqa: E402
        CHUNK_STRATEGIES,
        TOKENIZER_BUDGET_VERSION,
        canonical,
        digest,
        evaluate,
        load_evidence,
        validate_comparison_contract,
        validate_suite,
        verify_repo,
    )
    from evaluator import (
        read_json as read_evidence_json,
    )
    from sdk_proof import build_summary_from_evidence  # noqa: E402

VERDICT_VERSION = 2
MANIFEST_VERSION = 1
PILOT_OBSERVATIONS_FLOOR = 1000
FRESH_ROOTS_FLOOR = 5
RUNNABLE_STRATEGIES = tuple(s for s in CHUNK_STRATEGIES if s != "semble_native")
SEMBLE_PINNED_VERSION = "0.6.0"
SANDBOX_EXEC = Path("/usr/bin/sandbox-exec")
ISOLATION_BACKEND = "macos-seatbelt-v1"


class RunError(ValueError):
    """Paired-run evidence is absent, inconsistent or ineligible."""


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
    if task_count < 20:
        raise RunError("qualified speed requires at least 20 frozen tasks")
    if warmups < 1:
        raise RunError("qualified speed requires at least one shared warmup pass")
    if task_count * measurements * roots < PILOT_OBSERVATIONS_FLOOR:
        raise RunError(
            f"qualified speed requires at least {PILOT_OBSERVATIONS_FLOOR} warm observations per route"
        )
    if not isinstance(routes, list) or len(routes) != 1:
        raise RunError("qualified speed requires exactly one Quanta route")


def _process_tree_sample(root_pid: int) -> list[dict]:
    """Return one owned-process-tree RSS/CPU sample."""
    output = subprocess.check_output(
        ["ps", "-axo", "pid=,ppid=,rss=,pcpu=,comm="],
        text=True,
        stderr=subprocess.STDOUT,
    )
    rows: dict[int, tuple[int, int, float, str]] = {}
    for line in output.splitlines():
        fields = line.split(maxsplit=4)
        if len(fields) != 5:
            continue
        try:
            pid, ppid, rss_kib = (int(field) for field in fields[:3])
            cpu_percent = float(fields[3])
        except ValueError:
            continue
        if pid > 0 and ppid >= 0 and rss_kib >= 0 and math.isfinite(cpu_percent):
            rows[pid] = (ppid, rss_kib, max(cpu_percent, 0.0), fields[4])
    owned = {root_pid}
    changed = True
    while changed:
        changed = False
        for pid, (ppid, _rss, _cpu, _command) in rows.items():
            if pid not in owned and ppid in owned:
                owned.add(pid)
                changed = True
    return [
        {
            "pid": pid,
            "ppid": rows[pid][0],
            "rss_bytes": rows[pid][1] * 1024,
            "cpu_percent": rows[pid][2],
            "command": rows[pid][3],
        }
        for pid in sorted(owned)
        if pid in rows
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


def run_monitored_process(
    command: list[str],
    *,
    stdout_path: Path,
    stderr_path: Path,
    resource_path: Path,
    timeout_secs: int,
    subject_path: Path | None = None,
    env: dict[str, str] | None = None,
    sample_interval_ms: int = 50,
    isolation: dict | None = None,
) -> dict:
    """Run one owned process group and persist process-tree peak RSS evidence."""
    if timeout_secs <= 0:
        raise RunError("monitored process timeout must be positive")
    if sample_interval_ms <= 0:
        raise RunError("resource sample interval must be positive")
    for path in (stdout_path, stderr_path, resource_path):
        if path.exists():
            raise RunError(f"refusing existing process evidence: {path}")
        path.parent.mkdir(parents=True, exist_ok=True)
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
                command,
                stdout=stdout,
                stderr=stderr,
                text=True,
                env=env,
                start_new_session=True,
            )
        except OSError as error:
            raise RunError(f"cannot start monitored process: {error}") from error
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
            except (OSError, subprocess.CalledProcessError) as error:
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
    subject_sha256 = None
    if subject_path is not None and subject_path.is_file():
        subject_sha256 = sha_file(subject_path)
    payload = {
        "schema_version": 1,
        "sampler": "ps-process-tree-rss-cpu-v2",
        "sample_interval_ms": sample_interval_ms,
        "command_sha256": digest(canonical(command)),
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
        "isolation": isolation,
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
) -> Path:
    """Freeze a typed non-scoreable process failure for forensic review."""
    failure_path = run_dir / "failure.json"
    payload = {
        "schema_version": 1,
        "system": system,
        "strategy": strategy,
        "failure_type": failure_type,
        "resource_sha256": sha_file(resource_path),
        "stderr_sha256": sha_file(stderr_path),
        "record_emitted": record_path.is_file(),
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


def prepare_isolation(spec: dict, stage: Path, original_suite: Path) -> dict:
    """Create and verify the evaluator-only denial boundary for paired capture."""
    if spec.get("blinding", "attested") != "isolated":
        return spec
    if platform.system() != "Darwin" or not SANDBOX_EXEC.is_file():
        raise RunError("isolated blinding requires the macos-seatbelt-v1 backend")
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
    runner_tools = (stage / "runner-tools").resolve()
    runner_tools.mkdir()
    for name in ("semble.py", "evaluator.py"):
        shutil.copyfile(Path(__file__).resolve().parent / name, runner_tools / name)
    suite_path = Path(spec["suite"]).resolve()
    pack_path = Path(spec["query_pack"]).resolve()
    if not _path_within(suite_path, evaluator_root):
        raise RunError("frozen suite must be under the evaluator-only stage root")
    denied_roots = sorted({str(secret_root), str(evaluator_root), str(source_repo)})
    semble_env_root = Path(spec["semble_python"]).resolve().parent.parent
    extra_read_roots = [
        str(repo),
        str(Path(spec["manifest"]).resolve()),
        str(pack_path),
        str(Path(spec["runner_binary"]).resolve()),
        str(Path(spec["searchd_binary"]).resolve()),
        str(Path(spec["semble_lockfile"]).resolve()),
        str(semble_env_root),
        str(Path(sys.executable).resolve().parent.parent),
        str(Path(spec.get("semble_cache_root", stage / "semble-cache")).resolve()),
        str(stage.resolve()),
        str(Path(spec["output_root"]).resolve()),
    ]
    for optional in ("quanta_model_dir",):
        if optional in spec:
            extra_read_roots.append(str(Path(spec[optional]).resolve()))
    allowed_read_roots = sorted(set(extra_read_roots))
    allowed_write_roots = sorted({str(stage.resolve()), str(Path(spec["output_root"]).resolve())})
    profile = _seatbelt_profile(denied_roots, allowed_read_roots, allowed_write_roots)
    probes = _probe_seatbelt(profile, suite_path, pack_path)
    proof = {
        "schema_version": 1,
        "backend": ISOLATION_BACKEND,
        "sandbox_exec": {
            "path": str(SANDBOX_EXEC),
            "sha256": sha_file(SANDBOX_EXEC),
        },
        "profile_sha256": hashlib.sha256(profile.encode("utf-8")).hexdigest(),
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
        "runner_tools": [
            {
                "path": (runner_tools / name).relative_to(stage.resolve()).as_posix(),
                "sha256": sha_file(runner_tools / name),
            }
            for name in ("evaluator.py", "semble.py")
        ],
        "probes": probes,
    }
    proof_path = stage / "isolation-proof.json"
    with proof_path.open("x", encoding="utf-8") as handle:
        handle.write(json.dumps(proof, indent=2, sort_keys=True) + "\n")
    proof_sha256 = sha_file(proof_path)
    updated = dict(spec)
    updated["isolation_method"] = ISOLATION_BACKEND
    updated["access_block_log"] = f"sha256:{proof_sha256}"
    updated["_isolation"] = {
        "profile": profile,
        "profile_sha256": proof["profile_sha256"],
        "proof_sha256": proof_sha256,
    }
    updated["_semble_adapter"] = str(runner_tools / "semble.py")
    return updated


def sandbox_command(spec: dict, command: list[str]) -> tuple[list[str], dict | None]:
    if spec.get("blinding", "attested") != "isolated":
        return command, None
    isolation = spec.get("_isolation")
    if not isinstance(isolation, dict) or set(isolation) != {
        "profile",
        "profile_sha256",
        "proof_sha256",
    }:
        raise RunError("isolated capture lacks the verified driver isolation context")
    profile = isolation["profile"]
    if (
        not isinstance(profile, str)
        or hashlib.sha256(profile.encode()).hexdigest() != isolation["profile_sha256"]
    ):
        raise RunError("isolated capture profile digest mismatch")
    evidence = {
        "backend": ISOLATION_BACKEND,
        "profile_sha256": isolation["profile_sha256"],
        "proof_sha256": isolation["proof_sha256"],
    }
    return [str(SANDBOX_EXEC), "-p", profile, *command], evidence


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
        for node in sorted(
            Path("/sys/devices/system/cpu").glob("cpu[0-9]*/cpufreq/scaling_governor")
        ):
            try:
                governors[node.parent.parent.name] = node.read_text().strip()
            except OSError:
                continue
        rendered = canonical(governors)
        return {
            "status": "bounded" if governors else "unavailable",
            "digest": digest(rendered) if governors else None,
        }
    return {"status": "unavailable", "digest": None}


def find_competing_processes() -> dict:
    """Look for concurrent builds/benchmarks. Absence is recorded, not assumed."""
    patterns = ["cargo", "rustc", "semble", "pytest", "run_benchmark", "speed_benchmark"]
    found: dict[str, list[int]] = {}
    if shutil.which("pgrep") is None:
        return {"pgrep": "unavailable"}
    own = os.getpid()
    for pattern in patterns:
        try:
            completed = subprocess.run(
                ["pgrep", "-f", pattern], capture_output=True, text=True, timeout=15
            )
        except (OSError, subprocess.SubprocessError):
            found[pattern] = []
            continue
        pids = []
        for line in completed.stdout.splitlines():
            try:
                pid = int(line.strip())
            except ValueError:
                continue
            if pid != own:
                pids.append(pid)
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
                out[zone.parent.name] = zone.read_text().strip()
            except OSError:
                continue
        return {"status": "clean" if out else "unavailable", "evidence": out}
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
                out[node.parent.parent.name] = node.read_text().strip()
            except OSError:
                continue
        return {"status": "stable" if out else "unavailable", "evidence": out}
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
    suite, pack, _source = validate_suite(repo, suite_payload)
    provenance: dict = {}
    captures: dict = {}
    capture_sources: dict = {}
    results: dict[tuple[str, str], dict] = {}
    runners: list[dict] = []
    contracts: list[dict] = []
    for path in record_paths:
        run = _validate_single_record(repo, suite, pack, path)
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
        runners.append({"path": str(path), "runner": run["runner"]})
    first, *rest = contracts
    for other in rest:
        if other != first:
            differing = sorted(k for k in first if first[k] != other.get(k))
            raise RunError(
                f"merged records disagree on the comparison contract: {differing}; refusing merge"
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
    runners.sort(key=lambda entry: entry["path"])
    content_digests = sorted(digest(canonical_bytes(read_json(path))) for path in record_paths)
    merge_id = digest(canonical_bytes(content_digests))[:16]
    combined = {
        "schema_version": 3,
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
    # The merge itself must validate: re-run the evaluator over it.
    with tempfile_record(combined) as merged_path:
        _, _, checked = load_evidence(repo, suite_path, merged_path)
    if len(checked["results"]) != len(ordered):
        raise RunError("merged record failed evaluator re-validation")
    return suite, pack, combined


class tempfile_record:
    """Write a payload to a temp file for evaluator re-validation."""

    def __init__(self, record: dict, suffix: str = ".json") -> None:
        self.record = record
        self.suffix = suffix
        self.path: Path | None = None

    def __enter__(self) -> Path:
        import tempfile

        handle, name = tempfile.mkstemp(prefix="retrieval-merge-", suffix=self.suffix)
        with os.fdopen(handle, "w", encoding="utf-8") as stream:
            json.dump(self.record, stream, sort_keys=True)
        self.path = Path(name)
        return self.path

    def __exit__(self, *exc: object) -> None:
        if self.path is not None:
            try:
                self.path.unlink()
            except OSError:
                pass


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
    profile = _exact_keys(payload, {"schema_version", "profile_id", "fingerprint"}, "host profile")
    if profile["schema_version"] != 1:
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
    return profile


def cmd_host_profile(args: argparse.Namespace) -> int:
    probe = host_probe()
    profile = validate_host_profile(
        {
            "schema_version": 1,
            "profile_id": args.profile_id,
            "fingerprint": _host_fingerprint(probe),
        }
    )
    Path(args.out).write_text(
        json.dumps(profile, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    return 0


SPEC_REQUIRED = (
    "repo",
    "manifest",
    "suite",
    "query_pack",
    "top_k",
    "output_root",
    "runner_binary",
    "strategies",
    "searchd_binary",
    "searchd_expected_sha256",
)
SPEC_OPTIONAL = (
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
    "semble_repetitions",
    "semble_warmup_passes",
    "query_repetitions_per_root",
    "query_warmup_passes",
    "baseline_route",
    "candidate_route",
    "host_profile",
    "admission",
    "claims",
    "receipts",
    "contention_override",
)
RECEIPT_KEYS = (
    "contract_python_receipt",
    "contract_python_results",
    "contract_python_raw",
    "contract_rust_receipt",
    "contract_rust_results",
    "contract_rust_raw",
    "sdk_receipt",
    "sdk_results",
    "sdk_nextest_raw",
    "sdk_record_raw",
    "model_parity_results",
    "incremental_results",
)
ADMISSION_KEYS = (
    "manifest",
    "license_receipt",
    "annotation_receipts",
    "adjudication_receipt",
)
CONTRACT_EVIDENCE_KEYS = (
    "contract_python_receipt",
    "contract_python_results",
    "contract_python_raw",
    "contract_rust_receipt",
    "contract_rust_results",
    "contract_rust_raw",
)
SDK_EVIDENCE_KEYS = (
    "sdk_receipt",
    "sdk_results",
    "sdk_nextest_raw",
    "sdk_record_raw",
)


def _is_hex(value: object, length: int) -> bool:
    return (
        isinstance(value, str)
        and len(value) == length
        and all(c in "0123456789abcdef" for c in value)
    )


def validate_admission_manifest(payload: object) -> dict:
    """Validate the closed W0-B qualification authority packet."""
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
        },
        "qualification admission",
    )
    if admission["schema_version"] != 1:
        raise RunError("qualification admission schema version mismatch")
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


def verify_admission_bundle(
    manifest_path: Path,
    license_path: Path,
    annotation_paths: list[Path],
    adjudication_path: Path,
    *,
    source_revision: str,
    corpus_manifest_path: Path,
    suite_path: Path,
    query_pack_path: Path,
    lockfile_path: Path,
    host_profile_path: Path,
    cache_regime: str,
    receipt_paths: dict[str, Path],
    quanta_model_revision: str | None = None,
    semble_model_revision: str | None = None,
    semble_model_asset_sha256: str | None = None,
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
    if admission["cache_regime"] != cache_regime:
        raise RunError("qualification admission cache regime mismatch")
    if admission["license"]["receipt_sha256"] != sha_file(license_path):
        raise RunError("qualification admission license receipt mismatch")
    if len(annotation_paths) != 2:
        raise RunError("qualification admission requires two frozen annotation receipts")
    expected_annotations = [row["receipt_sha256"] for row in admission["gold"]["annotators"]]
    observed_annotations = [sha_file(path) for path in annotation_paths]
    if expected_annotations != observed_annotations:
        raise RunError("qualification admission annotation receipt mismatch")
    if admission["gold"]["adjudication_receipt_sha256"] != sha_file(adjudication_path):
        raise RunError("qualification admission adjudication receipt mismatch")
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


def load_spec(path: Path) -> dict:
    """Load a capture spec under the pair-spec contract (closed keys, typed)."""
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
    _spec_int(spec, "top_k", 1)
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
    if "blinding" in spec and spec["blinding"] not in ("isolated", "attested"):
        raise RunError("spec.blinding must be isolated or attested")
    if "scope" in spec and spec["scope"] not in ("exploratory", "qualified"):
        raise RunError("spec.scope must be exploratory or qualified")
    if "embedder" in spec and spec["embedder"] not in ("potion-code", "hash-dev"):
        raise RunError("spec.embedder must be potion-code or hash-dev")
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
        ("repetitions", 1),
        ("semble_repetitions", 1),
        ("semble_warmup_passes", 0),
        ("query_repetitions_per_root", 1),
        ("query_warmup_passes", 1),
    ):
        if key in spec:
            _spec_int(spec, key, minimum)
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
    ):
        if key in spec and (not isinstance(spec[key], str) or not spec[key]):
            raise RunError(f"spec.{key} must be a nonempty string")
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
    if "receipts" in spec:
        receipts = spec["receipts"]
        if not isinstance(receipts, dict):
            raise RunError("spec.receipts must be an object")
        unknown_receipts = sorted(set(receipts) - set(RECEIPT_KEYS))
        if unknown_receipts:
            raise RunError(f"spec.receipts has unknown keys: {unknown_receipts}")
        for key, value in receipts.items():
            if not isinstance(value, str) or not value:
                raise RunError(f"spec.receipts.{key} must be a nonempty path")
    if "admission" in spec:
        admission = _exact_keys(spec["admission"], set(ADMISSION_KEYS), "spec.admission")
        for key in ("manifest", "license_receipt", "adjudication_receipt"):
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


def cmd_quanta(args: argparse.Namespace) -> int:
    try:
        return run_quanta(load_spec(Path(args.spec)), Path(args.spec).parent)
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


def run_quanta(spec: dict, _spec_dir: Path) -> int:
    """Run the Rust SDK runner once per strategy. Returns process exit code."""
    out_root = preflight_capture(spec)
    if out_root.exists():
        raise RunError(f"output root already exists (refusing reuse): {out_root}")
    out_root.mkdir(parents=True)
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
    runs = []
    for index, strategy in enumerate(strategies):
        runs.append(
            run_quanta_strategy(
                spec, strategy, index, out_root, routes, pack_path, runner_binary_sha256
            )
        )
        if sha_file(Path(runner_bin)) != runner_binary_sha256:
            raise RunError("Rust runner binary changed during capture")
    (out_root / "quanta-manifest.json").write_text(
        json.dumps({"runs": runs}, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    print(json.dumps({"runs": len(runs), "output_root": str(out_root)}, indent=2))
    return 0


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
    out_abs = out_root.resolve()
    run_dir = out_root / f"strategy-{index:02d}-{name}"
    run_dir.mkdir(parents=True)
    state_root = (run_dir / "state").resolve()
    record_path = (run_dir / "record.json").resolve()
    phase_path = (run_dir / "phase-metrics.json").resolve()
    resource_path = (run_dir / "resource-metrics.json").resolve()
    command = [
        spec["runner_binary"],
        "run",
        "--repo",
        spec["repo"],
        "--manifest",
        spec["manifest"],
        "--query-pack",
        str(pack_path),
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
        "--metrics-out",
        str(phase_path),
        "--out",
        str(record_path),
    ]
    if "_query_protocol" in spec:
        command += ["--query-protocol", spec["_query_protocol"]]
    command += ["--searchd-bin", spec["searchd_binary"]]
    command += ["--searchd-expected-sha256", spec["searchd_expected_sha256"]]
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
    resource = run_monitored_process(
        command,
        stdout_path=run_dir / "runner.stdout.log",
        stderr_path=run_dir / "runner.stderr.log",
        resource_path=resource_path,
        timeout_secs=_int(spec.get("timeout_secs", 1800), "spec.timeout_secs"),
        subject_path=record_path,
        isolation=isolation,
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
        )
        raise RunError(f"Rust runner omitted phase metrics for {name}")
    index_bytes = tree_size(state_root)
    phase = read_json(phase_path)
    if not isinstance(phase, dict):
        raise RunError(f"Rust runner phase metrics are not an object for {name}")
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
    return {
        "strategy": name,
        "strategy_config": strategy,
        # Rename-safe: paths stay relative to the quanta output root so a
        # staged tree can be atomically promoted without rebinding.
        "record": record_path.relative_to(out_abs).as_posix(),
        "record_digest": sha_file(record_path),
        "runner_binary_sha256": runner_binary_sha256,
        "driver_ms": resource["elapsed_ms"],
        "index_bytes": index_bytes,
        "phase_metrics": phase_path.relative_to(out_abs).as_posix(),
        "phase_metrics_digest": sha_file(phase_path),
        "resource_metrics": resource_path.relative_to(out_abs).as_posix(),
        "resource_metrics_digest": sha_file(resource_path),
        "state_root": state_root.relative_to(out_abs).as_posix(),
    }


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
                {"test_result_digest", "raw_evidence_digest"},
                f"manifest {side} claim",
            )
            for key in ("test_result_digest", "raw_evidence_digest"):
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
            },
            "manifest sdk evidence",
        )
        for key in ("test_result_digest", "nextest_digest", "runner_record_digest"):
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
    host = _exact_keys(
        manifest["host"], {"start_digest", "end_digest", "cache_regime"}, "manifest host"
    )
    for key in ("start_digest", "end_digest"):
        if not _is_hex(host[key], 64):
            raise RunError(f"manifest host {key} must be a lowercase sha256")
    if host["cache_regime"] not in ("true_process_cold", "warm_cache", "undeclared"):
        raise RunError("manifest host cache_regime must be a frozen regime")
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
        "phase_metrics",
        "resource_metrics",
        "protocol_lock",
    }
    admission_artifacts = {
        "admission_manifest",
        "license_receipt",
        "annotation_receipts",
        "adjudication_receipt",
    }
    optional_artifacts = (
        set(RECEIPT_KEYS) | {"isolation_proof", "driver_source_closure"} | admission_artifacts
    )
    if not required_artifacts <= set(artifacts) <= required_artifacts | optional_artifacts:
        raise RunError("run manifest artifacts hold missing/unknown keys")
    present_admission = set(artifacts).intersection(admission_artifacts)
    if manifest["scope"] == "qualified" and present_admission != admission_artifacts:
        raise RunError("qualified run manifest lacks the complete admission bundle")
    if manifest["scope"] == "qualified" and "driver_source_closure" not in artifacts:
        raise RunError("qualified run manifest lacks the driver source closure")
    if manifest["scope"] != "qualified" and present_admission:
        raise RunError("exploratory run manifest carries qualification admission artifacts")
    for key in required_artifacts | optional_artifacts:
        if key not in artifacts:
            continue
        value = artifacts[key]
        if key in (
            "records",
            "reports",
            "quanta_manifests",
            "semble_native",
            "phase_metrics",
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
    quanta = _exact_keys(
        provenance["quanta"],
        {"source_sha", "source_closure_digest", "binary_digest", "embedder"},
        "manifest quanta",
    )
    if not _is_hex(quanta["source_sha"], 40) or not _is_hex(quanta["binary_digest"], 64):
        raise RunError("manifest quanta provenance digests malformed")
    if quanta["embedder"] not in ("potion-code", "hash-dev"):
        raise RunError("manifest quanta embedder must be a frozen embedder")
    if manifest["scope"] == "qualified":
        if not _is_hex(quanta["source_closure_digest"], 64):
            raise RunError("qualified manifest source closure digest is malformed")
    elif quanta["source_closure_digest"] is not None:
        raise RunError("exploratory manifest source closure digest must be null")
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


def _validate_single_record(repo: Path, suite: dict, pack: dict, path: Path) -> dict:
    """Validate one raw v3 record against its re-derived projected pack."""
    raw = read_json(path)
    if not isinstance(raw, dict):
        raise RunError(f"record is not an object: {path}")
    if raw.get("schema_version") != 3:
        raise RunError(f"v3 record required: {path}")
    routes = sorted(raw.get("route_provenance", {}).keys())
    if not routes:
        raise RunError(f"record names no routes: {path}")
    projected_pack, projected_suite = project_pack_and_suite(pack, suite, routes)
    expected_sha = digest(canonical_bytes(projected_pack))
    if raw.get("query_pack_sha256") != expected_sha:
        raise RunError(f"record {path} pack digest does not match its projected pack")
    with tempfile_record(projected_suite, suffix=".suite.json") as suite_file:
        _, _, run = load_evidence(repo, suite_file, path)
    return run


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
    if not isinstance(captures, dict) or len(captures) != 1:
        raise RunError(f"{where} must be a raw single-capture record")
    _capture_id, capture = next(iter(captures.items()))
    if not isinstance(capture, dict):
        raise RunError(f"{where} capture is not an object")
    return capture.get("system"), capture.get("chunk_strategy")


def _probe_clean(probe: object, profile: dict) -> bool:
    return (
        isinstance(probe, dict)
        and _host_fingerprint(probe) == profile["fingerprint"]
        and probe.get("concurrent_processes", {}) in ({}, {"none": []})
        and probe.get("contention_override") is not True
        and isinstance(probe.get("thermal"), dict)
        and probe["thermal"].get("status") == "clean"
        and isinstance(probe.get("frequency"), dict)
        and probe["frequency"].get("status") in ("stable", "bounded")
        and isinstance(probe.get("power"), dict)
        and probe["power"].get("status") == "bounded"
    )


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
        and all(
            isinstance(ci.get(key), (int, float)) and math.isfinite(ci[key])
            for key in ("mean", "lower_95", "upper_95")
        )
        and ci["lower_95"] <= ci["mean"] <= ci["upper_95"]
    )


def _valid_stratified_delta(strata: object, sample_count: int, expected_mean: float | None) -> bool:
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
            if type(entry["sample_count"]) is not int or entry["sample_count"] < 1:
                return False
            observed += entry["sample_count"]
            if not isinstance(entry["mean_delta"], (int, float)) or not math.isfinite(
                entry["mean_delta"]
            ):
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
        or not isinstance(primary_mean, (int, float))
        or not math.isfinite(primary_mean)
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
            or any(no_answer["strata"].values())
            or not _valid_stratified_delta(no_answer["strata"], 0, None)
        ):
            return False
    elif not isinstance(no_answer["mean_delta"], (int, float)) or not math.isfinite(
        no_answer["mean_delta"]
    ):
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


def _validate_phase_metrics(payload: object, where: str) -> dict:
    if not isinstance(payload, dict):
        raise RunError(f"{where} must be an object")
    system = payload.get("system")
    protocol_mode = "query_protocol" in payload
    system_key = "runner_binary_sha256" if system == "quanta" else "worker_sha256"
    if system == "quanta":
        expected_phases = {
            "discovery",
            "chunk",
            "model_provider_prepare",
            "embed_publish_seal_activate",
            "cold_query" if protocol_mode else "first_query",
            "warm_query",
            "unattributed",
        }
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
    if system == "semble":
        metric_keys.add("phase_boundaries_ns")
    if protocol_mode:
        metric_keys.update({"query_protocol", "warm_latencies_ms", "cold_latencies_ms"})
    metrics = _exact_keys(
        payload,
        metric_keys,
        where,
    )
    if metrics["schema_version"] != 1 or system not in ("quanta", "semble"):
        raise RunError(f"{where} has unknown schema/system")
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
            if (
                type(cold[route]) not in (int, float)
                or not math.isfinite(cold[route])
                or cold[route] < 0
            ):
                raise RunError(f"{where} cold latency is invalid")
            for task_id, values in by_task.items():
                if (
                    not isinstance(values, list)
                    or len(values) != metrics["measurement_repetitions"]
                ):
                    raise RunError(f"{where} warm latency count differs for {route}/{task_id}")
                for value in values:
                    if type(value) not in (int, float) or not math.isfinite(value) or value < 0:
                        raise RunError(f"{where} warm latency is invalid for {route}/{task_id}")
    phases = _exact_keys(metrics["phases_ms"], expected_phases, f"{where}.phases_ms")
    for key, value in phases.items():
        if type(value) not in (int, float) or not math.isfinite(value) or value < 0:
            raise RunError(f"{where}.phases_ms.{key} must be finite and nonnegative")
    total = metrics["total_ms"]
    if type(total) not in (int, float) or not math.isfinite(total) or total <= 0:
        raise RunError(f"{where}.total_ms must be finite and positive")
    if not math.isclose(sum(phases.values()), total, rel_tol=1e-9, abs_tol=0.01):
        raise RunError(f"{where} phase sum differs from total")
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
    return metrics


def _validate_resource_metrics(payload: object, where: str) -> dict:
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
    if not isinstance(payload, dict) or set(payload) not in (keys, keys | {"isolation"}):
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
    if (
        type(metrics["elapsed_ms"]) not in (int, float)
        or not math.isfinite(metrics["elapsed_ms"])
        or metrics["elapsed_ms"] <= 0
    ):
        raise RunError(f"{where}.elapsed_ms must be finite and positive")
    if type(metrics["peak_rss_bytes"]) is not int or metrics["peak_rss_bytes"] <= 0:
        raise RunError(f"{where}.peak_rss_bytes must be positive")
    if (
        type(metrics["peak_cpu_percent"]) not in (int, float)
        or not math.isfinite(metrics["peak_cpu_percent"])
        or metrics["peak_cpu_percent"] < 0
    ):
        raise RunError(f"{where}.peak_cpu_percent must be finite and nonnegative")
    processes = metrics["processes"]
    if not isinstance(processes, list) or not processes:
        raise RunError(f"{where}.processes must be nonempty")
    for index, process in enumerate(processes):
        row = _exact_keys(
            process,
            {"pid", "command", "peak_rss_bytes", "peak_cpu_percent", "samples"},
            f"{where}.processes[{index}]",
        )
        if type(row["pid"]) is not int or row["pid"] < 1:
            raise RunError(f"{where}.processes[{index}].pid must be positive")
        if not isinstance(row["command"], str) or not row["command"]:
            raise RunError(f"{where}.processes[{index}].command must be nonempty")
        if type(row["peak_rss_bytes"]) is not int or row["peak_rss_bytes"] <= 0:
            raise RunError(f"{where}.processes[{index}].peak_rss_bytes must be positive")
        if type(row["samples"]) is not int or row["samples"] < 1:
            raise RunError(f"{where}.processes[{index}].samples must be positive")
        if (
            type(row["peak_cpu_percent"]) not in (int, float)
            or not math.isfinite(row["peak_cpu_percent"])
            or row["peak_cpu_percent"] < 0
        ):
            raise RunError(f"{where}.processes[{index}].peak_cpu_percent is invalid")
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
    if type(metrics["samples"]) is not int or metrics["samples"] < 1:
        raise RunError(f"{where}.samples must be positive")
    if metrics["complete"] is not True or metrics["error"] is not None:
        raise RunError(f"{where} resource sampling is incomplete")
    if (
        metrics["cleanup_complete"] is not True
        or type(metrics["cleanup_escalated"]) is not bool
        or metrics["cleanup_error"] is not None
    ):
        raise RunError(f"{where} owned process cleanup is incomplete")
    isolation = metrics.get("isolation")
    if isolation is not None:
        proof = _exact_keys(
            isolation,
            {"backend", "profile_sha256", "proof_sha256"},
            f"{where}.isolation",
        )
        if proof["backend"] != ISOLATION_BACKEND:
            raise RunError(f"{where}.isolation backend mismatch")
        for key in ("profile_sha256", "proof_sha256"):
            if not _is_hex(proof[key], 64):
                raise RunError(f"{where}.isolation.{key} must be a lowercase sha256")
    return metrics


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
    proof = _exact_keys(
        payload,
        {
            "schema_version",
            "backend",
            "sandbox_exec",
            "profile_sha256",
            "denied_roots",
            "allowed_read_roots",
            "allowed_write_roots",
            "suite",
            "query_pack",
            "corpus_view",
            "runner_tools",
            "probes",
        },
        "isolation proof",
    )
    if proof["schema_version"] != 1 or proof["backend"] != ISOLATION_BACKEND:
        raise RunError("isolation proof schema/backend mismatch")
    backend = _exact_keys(proof["sandbox_exec"], {"path", "sha256"}, "isolation proof sandbox_exec")
    if backend["path"] != str(SANDBOX_EXEC) or not _is_hex(backend["sha256"], 64):
        raise RunError("isolation proof sandbox executable identity is malformed")
    if not SANDBOX_EXEC.is_file() or sha_file(SANDBOX_EXEC) != backend["sha256"]:
        raise RunError("isolation proof sandbox executable digest drifted")
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
    profile = _seatbelt_profile(roots, allowed_read_roots, allowed_write_roots)
    profile_sha = hashlib.sha256(profile.encode("utf-8")).hexdigest()
    if proof["profile_sha256"] != profile_sha:
        raise RunError("isolation proof profile digest mismatch")
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
    runner_tools = proof["runner_tools"]
    if not isinstance(runner_tools, list) or len(runner_tools) != 2:
        raise RunError("isolation proof runner_tools must bind two files")
    for index, entry in enumerate(runner_tools):
        row = _exact_keys(entry, {"path", "sha256"}, f"isolation proof runner_tools[{index}]")
        tool_path = _resolve_artifact(root, row["path"], "isolation proof runner tool")
        if not _is_hex(row["sha256"], 64) or sha_file(tool_path) != row["sha256"]:
            raise RunError("isolation proof runner tool digest mismatch")
    probes = _exact_keys(
        proof["probes"],
        {"suite_read_denied", "query_pack_read_allowed"},
        "isolation proof probes",
    )
    if probes != {"suite_read_denied": True, "query_pack_read_allowed": True}:
        raise RunError("isolation proof probes did not pass")
    if capture_paths["suite"].is_file() and capture_paths["query_pack"].is_file():
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
    return {
        "backend": ISOLATION_BACKEND,
        "profile_sha256": profile_sha,
        "proof_sha256": proof_sha,
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
        "phase_metrics",
        "resource_metrics",
    ):
        resolved[key] = [_resolve_artifact(root, ref, f"artifacts.{key}") for ref in artifacts[key]]
    for key in RECEIPT_KEYS:
        if key in artifacts:
            resolved[key] = _resolve_artifact(root, artifacts[key], f"artifacts.{key}")
    if manifest["scope"] == "qualified":
        resolved["driver_source_closure"] = _resolve_artifact(
            root, artifacts["driver_source_closure"], "artifacts.driver_source_closure"
        )
        for key in ("admission_manifest", "license_receipt", "adjudication_receipt"):
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
    host_profile = validate_host_profile(read_json(resolved["host_profile"]))
    if sha_file(resolved["host_profile"]) != provenance_claims["host"]["profile_digest"]:
        raise RunError("host profile artifact digest mismatch")
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
        suite, pack, _source = validate_suite(repo, suite_payload)
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
    if suite_digest != provenance_claims["suite"]["suite_digest"]:
        pair_note("suite_digest_mismatch", ("T01", "T12"))
    if pack_digest != provenance_claims["suite"]["query_pack_digest"]:
        pair_note("pack_digest_mismatch", ("T01", "T12"))
    if corpus_digest != provenance_claims["corpus"]["digest"]:
        pair_note("corpus_digest_mismatch", ("T00", "T12"), "corpus_mismatch")
    if mapping_digest != evidence["pair"]["mapping_proof_digest"]:
        pair_note("mapping_proof_digest_mismatch", ("T00", "T11"))
    protocol_payload = read_note(resolved["protocol_lock"], "protocol_lock", ("T12",))
    if isinstance(protocol_payload, dict):
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
        if manifest["scope"] == "qualified":
            closure = _validate_source_closure_shape(
                read_json(resolved["driver_source_closure"]), "driver source closure"
            )
            if closure["revision"] != provenance_claims["quanta"]["source_sha"]:
                pair_note("driver_source_closure_revision_drift", ("T12", "T17"))
            if closure["digest"] != provenance_claims["quanta"]["source_closure_digest"]:
                pair_note("driver_source_closure_digest_drift", ("T12", "T17"))
            if protocol_payload.get("driver_source_closure_digest") != closure["digest"]:
                pair_note("protocol_lock_source_closure_drift", ("T12", "T17"))
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
            run = _validate_single_record(repo, suite, pack, path)
            system, strategy = _record_identity(run, f"record {path.name}")
        except (RunError, ValueError) as exc:
            pair_note(f"record_invalid: {exc}", ("T03", "T12"))
            continue
        validated[str(path)] = {"run": run, "rep": rep, "system": system, "strategy": strategy}
        rep_records.setdefault(rep, []).append(str(path))
    if "rep-00" not in rep_records:
        pair_note("rep_00_missing", ("T12", "T13"))
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
                _suite, _pack, merged = merge_records(
                    repo,
                    resolved["suite"],
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
        comparison = (
            content.get("rank_metrics", {}).get("comparison", {})
            if isinstance(content.get("rank_metrics"), dict)
            else {}
        )
        try:
            rescored = evaluate(
                suite, pack, merged, comparison.get("baseline"), comparison.get("candidate")
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
                "stratified_primary_delta": rank_comparison["stratified_primary_delta"],
                "no_answer_abstention_delta": rank_comparison["no_answer_abstention_delta"],
                "report_sha": digest(canonical(content)),
            }
        )
    for strategy in quanta_by_strategy:
        if strategy not in {entry["strategy"] for entry in matched}:
            pair_note(f"strategy_without_report:{strategy}", ("T12", "T13"))

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

    # Record <-> capture-manifest binding.
    bound_records: set[str] = set()
    record_digests = {sha_file(Path(path)) for path in resolved["records"]}
    phase_record_digests: list[str] = []
    phase_by_record: dict[str, dict] = {}
    phase_ok = len(resolved["phase_metrics"]) == len(resolved["records"])
    expected_query_schedule = [task["task_id"] for task in pack["tasks"]]
    for path in resolved["phase_metrics"]:
        try:
            metrics = _validate_phase_metrics(read_json(Path(path)), f"phase metrics {path}")
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
        except (RunError, ValueError, OSError):
            phase_ok = False
    if set(phase_record_digests) != record_digests or len(phase_record_digests) != len(
        record_digests
    ):
        phase_ok = False

    resource_ok = len(resolved["resource_metrics"]) == len(resolved["records"])
    resource_subject_digests: list[str] = []
    resource_by_subject: dict[str, dict] = {}
    resource_isolation: list[dict | None] = []
    for path in resolved["resource_metrics"]:
        try:
            metrics = _validate_resource_metrics(read_json(Path(path)), f"resource metrics {path}")
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
    for path in resolved["semble_native"]:
        try:
            rep = _rep_segment(Path(path), root)
            semble_records = [
                record_path
                for record_path in rep_records.get(rep, [])
                if validated[record_path]["system"] == "semble"
            ]
            if len(semble_records) != 1:
                raise RunError("Semble native artifact lacks one record owner")
            metrics = resource_by_subject[sha_file(Path(semble_records[0]))]
            native = read_json(Path(path))
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
            for metric_key in ("phase_metrics", "resource_metrics"):
                metric_ref = run_entry.get(metric_key)
                metric_digest = run_entry.get(f"{metric_key}_digest")
                if not isinstance(metric_ref, str) or not _is_hex(metric_digest, 64):
                    if metric_key == "phase_metrics":
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
                    if metric_key == "phase_metrics":
                        phase_ok = False
                    else:
                        resource_ok = False
            ref = run_entry.get("record")
            want = run_entry.get("record_digest")
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
    mapping_diff = mapping_payload.get("diff_digest") if isinstance(mapping_payload, dict) else None
    for _path, entry in validated.items():
        if entry["system"] != "semble":
            continue
        _cid, capture = next(iter(entry["run"]["captures"].items()))
        if capture.get("receipt_digest") != mapping_diff:
            pair_note("semble_receipt_anchor_drift", ("T11", "T12"))

    quanta_binaries = set()
    for _path, entry in validated.items():
        if entry["system"] == "quanta":
            _cid, capture = next(iter(entry["run"]["captures"].items()))
            quanta_binaries.add(capture.get("runner_binary", {}).get("digest"))
    binary_digest = sorted(quanta_binaries)[0] if quanta_binaries else "0" * 64
    if len(quanta_binaries) != 1:
        pair_note("runner_binary_divergence", ("T12",))
    elif binary_digest != provenance_claims["quanta"]["binary_digest"]:
        pair_note("binary_digest_mismatch", ("T12",))

    admission_evidence = None
    admission_error = None
    if manifest["scope"] == "qualified":
        try:
            annotation_paths = resolved.get("annotation_receipts")
            if not isinstance(annotation_paths, list):
                raise RunError("qualified verdict lacks annotation receipts")
            quanta_model_revisions = {
                capture.get("model_revision")
                for entry in validated.values()
                if entry["rep"] == "rep-00" and entry["system"] == "quanta"
                for capture in entry["run"]["captures"].values()
                if capture.get("model_revision") != "not-applicable"
            }
            if len(quanta_model_revisions) != 1:
                raise RunError("qualified verdict requires one Quanta model revision")
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
                query_pack_path=resolved["query_pack"],
                lockfile_path=resolved["semble_lockfile"],
                host_profile_path=resolved["host_profile"],
                cache_regime=manifest["host"]["cache_regime"],
                receipt_paths=receipt_paths,
                quanta_model_revision=next(iter(quanta_model_revisions)),
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
            contract_authority = {
                "python": (
                    "retrieval-contract-python",
                    "python3 -m pytest tools/ci/tests/test_retrieval_benchmark.py -q",
                    "pytest-junit",
                    pytest_summary,
                ),
                "rust": (
                    "retrieval-contract-rust",
                    "./scripts/cargow nextest run -p quanta-index-retrieval-bench "
                    "--lib --test chunking_contract --all-features --locked",
                    "nextest-jsonl",
                    nextest_summary,
                ),
            }
            for side, (rail, command, role, producer) in contract_authority.items():
                receipt_ref = f"contract_{side}_receipt"
                results_ref = f"contract_{side}_results"
                raw_ref = f"contract_{side}_raw"
                if any(ref not in resolved for ref in (receipt_ref, results_ref, raw_ref)):
                    raise RunError(f"contract {side} artifacts missing")
                receipt = _validate_receipt_shape(
                    read_json(resolved[receipt_ref]), f"contract {side} receipt"
                )
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
                if receipt["rail"] != rail or receipt["command"] != command:
                    raise RunError(f"contract {side} receipt authority mismatch")
                if results["command"] != command:
                    raise RunError(f"contract {side} command mismatch")
                _verify_receipt_inputs(
                    receipt, {role: resolved[raw_ref]}, f"contract {side} receipt"
                )
                try:
                    rebuilt = producer(resolved[raw_ref])
                except SystemExit as exc:
                    raise RunError(f"contract {side} raw evidence refused: {exc}") from exc
                if rebuilt != results:
                    raise RunError(f"contract {side} summary is not reproducible")
                if receipt["revision"] != provenance_claims["quanta"]["source_sha"]:
                    raise RunError(f"contract {side} revision mismatch")
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

    sdk_ids = ["T05", "T06", "T07"]
    if "sdk_path" not in evidence:
        set_state("SDK_PATH_GREEN", "not_run", "no_evidence", None)
        missing.extend(sdk_ids)
    else:
        try:
            if "sdk_receipt" not in resolved or "sdk_results" not in resolved:
                raise RunError("sdk artifacts missing")
            sdk_receipt = _validate_receipt_shape(read_json(resolved["sdk_receipt"]), "sdk receipt")
            sdk_results = _validate_sdk_results_shape(
                read_json(resolved["sdk_results"]), "sdk results"
            )
            sdk_actual = sha_file(resolved["sdk_results"])
            if sdk_actual != sdk_receipt["evidence_sha256"]:
                raise RunError("sdk receipt digest mismatch")
            if sdk_actual != evidence["sdk_path"]["test_result_digest"]:
                raise RunError("sdk manifest digest mismatch")
            sdk_command = "just retrieval-sdk-proof"
            if (
                sdk_receipt["rail"] != "retrieval-sdk-proof"
                or sdk_receipt["command"] != sdk_command
            ):
                raise RunError("sdk receipt authority mismatch")
            if sdk_results["command"] != sdk_command:
                raise RunError("sdk results command mismatch")
            if "sdk_nextest_raw" not in resolved or "sdk_record_raw" not in resolved:
                raise RunError("sdk raw artifacts missing")
            if sha_file(resolved["sdk_nextest_raw"]) != evidence["sdk_path"]["nextest_digest"]:
                raise RunError("sdk nextest manifest digest mismatch")
            if sha_file(resolved["sdk_record_raw"]) != evidence["sdk_path"]["runner_record_digest"]:
                raise RunError("sdk record manifest digest mismatch")
            _verify_receipt_inputs(
                sdk_receipt,
                {
                    "nextest-jsonl": resolved["sdk_nextest_raw"],
                    "runner-record": resolved["sdk_record_raw"],
                },
                "sdk receipt",
            )
            try:
                rebuilt_sdk = build_summary_from_evidence(
                    resolved["sdk_record_raw"],
                    resolved["sdk_nextest_raw"],
                    binary_digest,
                )
            except SystemExit as exc:
                raise RunError(f"sdk raw evidence refused: {exc}") from exc
            if rebuilt_sdk != sdk_results:
                raise RunError("sdk summary is not reproducible")
            if sdk_receipt["revision"] != provenance_claims["quanta"]["source_sha"]:
                raise RunError("sdk revision mismatch")
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
            set_state(
                "SDK_PATH_GREEN", "pass", "sdk_proof_verified", digest(canonical(sdk_results))
            )

    set_state("PAIR_VALID", pair_state, pair_reason, pair_proof)
    if pair_state == "fail":
        missing.extend(pair_t_ids)
        classes.append(pair_class)

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
            elif not shared_protocol_ok:
                perf_fail = ("shared_warm_query_protocol_unimplemented", "provenance")
        if perf_fail is None:
            set_state(
                "PERF_QUALIFIED",
                "pass",
                "phase_and_process_tree_resources_verified",
                digest(
                    canonical(
                        {
                            "admission": admission_evidence,
                            "latency_matrix": rebuilt,
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
            if manifest["isolation_method"] != ISOLATION_BACKEND:
                raise RunError("manifest isolation method differs from the proof backend")
            if manifest["access_block_log"] != ("sha256:" + isolation_evidence["proof_sha256"]):
                raise RunError("manifest access_block_log does not bind the isolation proof")
            if len(resource_isolation) != len(resolved["records"]) or any(
                entry != isolation_evidence for entry in resource_isolation
            ):
                raise RunError("capture resources do not all bind the isolation profile")
        except (RunError, ValueError, OSError) as exc:
            isolation_error = str(exc)
    if not claims["quality"]:
        set_state("QUALITY_DELTA", "not_applicable", "no_quality_claim", None)
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
    elif provenance_claims["quanta"].get("embedder") != "potion-code":
        # T10: a quality claim over the hash-dev diagnostic control (or an
        # undeclared embedder) is not model-quality evidence.
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
    elif any(not _qualified_uncertainty(entry) for entry in matched):
        set_state("QUALITY_DELTA", "fail", "uncertainty_unqualified", None)
        classes.append("scoring")
    else:
        set_state(
            "QUALITY_DELTA",
            "pass",
            "blinded_graded_delta",
            digest(
                canonical(
                    {
                        "admission": admission_evidence,
                        "reports": sorted(entry["report_sha"] for entry in matched),
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
            results = _validate_parity_results_shape(read_json(resolved[ref]), f"{key} results")
            if sha_file(resolved[ref]) != evidence[key]["test_result_digest"]:
                raise RunError("manifest digest mismatch")
            if not (
                results["status"] == "pass"
                and results["failed"] == 0
                and results["executed"] >= 1
                and results["passed"] + results["failed"] == results["executed"]
            ):
                raise RunError("parity/incremental not proven")
        except (RunError, ValueError, OSError):
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


def _source_closure(repo_root: Path, command: str, path: Path | None = None) -> None:
    args = [sys.executable, str(repo_root / "tools/ci/source_closure.py"), command]
    if command == "capture":
        args.extend(("--profile", "retrieval", "--out", str(path)))
    elif command == "verify":
        args.extend(("--manifest", str(path)))
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
    scope = spec.get("scope", "exploratory")
    if scope == "qualified" and not isinstance(spec.get("admission"), dict):
        raise RunError("qualified pair capture requires spec.admission")
    if scope != "qualified" and "admission" in spec:
        raise RunError("spec.admission is valid only for a qualified capture")
    if scope == "qualified" and spec.get("claims", {}).get("speed") is True:
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
        raise RunError("pair requires spec.semble_lockfile naming the hash-pinned lockfile")
    if not spec.get("host_profile"):
        raise RunError("pair requires spec.host_profile naming the canonical host profile")
    if (
        spec.get("claims", {}).get("speed") is True
        and spec.get("embedder", "potion-code") == "potion-code"
        and not spec.get("quanta_model_dir")
    ):
        raise RunError("qualified speed capture with potion-code requires quanta_model_dir")
    if "quanta_model_dir" in spec and not Path(spec["quanta_model_dir"]).is_dir():
        raise RunError("quanta_model_dir must name an existing directory")
    out_root = preflight_capture(spec)
    stage = out_root.parent / (out_root.name + ".staging")
    if out_root.exists() or stage.exists():
        raise RunError("output root or staging dir already exists (refusing reuse)")
    stage.mkdir(parents=True)
    if scope == "qualified":
        closure_path = stage / "driver-source-closure.json"
        _source_closure(Path(__file__).resolve().parents[3], "capture", closure_path)
        spec = dict(spec, _driver_source_closure=str(closure_path))
    try:
        summary = _run_pair_staged(spec, stage)
    except Exception:
        # The stage is left for forensics, but the authoritative output
        # root is never promoted from a failed run.
        raise
    if out_root.exists():
        raise RunError("output root appeared during capture (refusing promotion)")
    os.rename(stage, out_root)
    summary["output_root"] = str(out_root)
    print(json.dumps(summary, indent=2))
    return 0


def _run_pair_staged(spec: dict, stage: Path) -> dict:
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
    semble_routes = [spec.get("semble_route", "semble-hybrid")]
    semble_pack = write_projected_pack(
        Path(spec["query_pack"]),
        Path(spec["suite"]),
        semble_routes,
        stage / "semble-pack.json",
    )
    rep_layouts: list[dict] = []
    semble_spec = dict(spec)
    # One shared model/Semble cache across reps: the model downloads once,
    # while each rep still rebuilds its index on a fresh corpus.
    semble_spec.setdefault("semble_cache_root", str(stage / "semble-cache"))
    for rep in range(repetitions):
        rep_order = order if (rep % 2 == 0 or not alternate) else list(reversed(order))
        rep_dir = stage / f"rep-{rep:02d}"
        rep_dir.mkdir(parents=True)
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
                spec.get("query_warmup_passes", spec.get("semble_warmup_passes", 1)),
                "spec.query_warmup_passes",
            ),
            _int(
                spec.get("query_repetitions_per_root", spec.get("semble_repetitions", 1)),
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
        rep_layouts.append(layout)
    host_end = host_probe()
    host_end["contention_override"] = override
    (stage / "host-end.json").write_text(
        json.dumps(host_end, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    # Every rep's records validate through the evaluator before any use:
    # each (strategy, semble) pair merges exactly like the scored join,
    # and each capture echoes the strategy it was invoked with.
    # Quality reports merge rep-0 records only.
    repo = source_repo
    suite_path = Path(spec["suite"])
    for layout in rep_layouts:
        for strategy, record in sorted(layout["quanta"].items()):
            payload = read_json(Path(record))
            if not isinstance(payload, dict):
                raise RunError(f"record is not an object: {record}")
            captures = payload.get("captures", {})
            if not isinstance(captures, dict) or len(captures) != 1:
                raise RunError(f"quanta record must carry exactly one capture: {record}")
            _capture_id, capture = next(iter(captures.items()))
            if not isinstance(capture, dict) or capture.get("chunk_strategy") != strategy:
                raise RunError(f"strategy echo mismatch for {record}: {strategy}")
            merge_records(repo, suite_path, [Path(record), Path(layout["semble"])])
    baseline = spec.get("baseline_route", semble_routes[0])
    rep0 = rep_layouts[0]
    reports = []
    for strategy, record in sorted(rep0["quanta"].items()):
        payload = read_json(Path(record))
        if not isinstance(payload, dict):
            raise RunError(f"record is not an object: {record}")
        suite, pack, combined = merge_records(
            repo, suite_path, [Path(record), Path(rep0["semble"])]
        )
        candidate_routes = sorted({row["route"] for row in payload["results"]})
        for candidate in candidate_routes:
            report = evaluate(suite, pack, combined, baseline, candidate)
            name = f"report-{baseline}-vs-{candidate}-{strategy}.json"
            (stage / name).write_text(
                json.dumps(report, indent=2, sort_keys=True) + "\n", encoding="utf-8"
            )
            reports.append(name)
    latency_path = stage / "latency-matrix.json"
    latency_path.write_text(
        json.dumps(build_latency_matrix(rep_layouts), indent=2, sort_keys=True) + "\n",
        encoding="utf-8",
    )
    driver_closure_digest = None
    if spec.get("scope", "exploratory") == "qualified":
        closure_path = Path(spec.get("_driver_source_closure", ""))
        _source_closure(Path(__file__).resolve().parents[3], "verify", closure_path)
        driver_closure = _validate_source_closure_shape(
            read_json(closure_path), "driver source closure"
        )
        driver_closure_digest = driver_closure["digest"]
    protocol_lock = {
        "suite_digest": sha_file(Path(spec["suite"])),
        "query_pack_digest": sha_file(stage / "query-pack.json"),
        "corpus_manifest_digest": sha_file(stage / "corpus-manifest.json"),
        "spec_digest": digest(canonical_bytes(spec)),
        "top_k": spec["top_k"],
        "strategies": [entry["name"] for entry in spec["strategies"]],
        "searchd_expected_sha256": spec["searchd_expected_sha256"],
        "semble_lockfile_sha256": spec["semble_lockfile_sha256"],
        "host_profile_digest": sha_file(Path(spec["host_profile"])),
        "admission_digest": (
            sha_file(Path(str(frozen_admission["manifest"]))) if frozen_admission else None
        ),
        "driver_source_closure_digest": driver_closure_digest,
        "repetitions": repetitions,
    }
    (stage / "protocol-lock.json").write_text(
        json.dumps(protocol_lock, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
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
    verdict = build_verdict(source_repo, suite_path, manifest_path)
    (stage / "verdict.json").write_text(
        json.dumps(verdict, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    return {
        "reports": reports,
        "repetitions": repetitions,
        "states": verdict["states"],
        "output_root": str(stage),
    }


ERROR_STATUSES = ("error", "timeout", "unavailable")


def _sample_value(value: object, where: str) -> float | None:
    """A matrix sample: finite number >= 0, or None when unknown. Never 0-filled."""
    if value is None:
        return None
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        raise RunError(f"{where} latency is not a number or null")
    if not math.isfinite(value) or value < 0:
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
        for route, task_id, status, timing in cell["rows"]:
            sys_key = f"{system}:{strategy}:{route}"
            bump(attempts, sys_key)
            sample_key = f"{sys_key}:{task_id}"
            value = _sample_value(timing, f"{sample_key}")
            timings_by_task[(route, task_id)] = value
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


def _validate_parity_results_shape(payload: object, where: str) -> dict:
    results = _exact_keys(
        payload, {"command", "status", "selected", "executed", "passed", "failed"}, where
    )
    if not isinstance(results["command"], str) or not results["command"]:
        raise RunError(f"{where}.command must be a nonempty string")
    if results["status"] not in ("pass", "fail"):
        raise RunError(f"{where}.status must be pass or fail")
    for key in ("selected", "executed", "passed", "failed"):
        value = results[key]
        if type(value) is not int or isinstance(value, bool) or value < 0:
            raise RunError(f"{where}.{key} must be an integer >= 0")
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
        if key not in receipts:
            continue
        source = Path(receipts[key])
        try:
            before = sha_file(source)
            target = target_dir / f"{key}{source.suffix or '.json'}"
            shutil.copyfile(source, target)
            after = sha_file(target)
        except OSError as exc:
            raise RunError(f"cannot freeze receipt artifact {key}: {exc}") from exc
        if before != after:
            raise RunError(f"receipt artifact changed during freeze: {key}")
        frozen[key] = str(target)
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
    admission = _exact_keys(raw, set(ADMISSION_KEYS), "spec.admission")
    target_dir = stage / "admission"
    target_dir.mkdir(parents=True, exist_ok=True)

    frozen: dict[str, object] = {}
    scalar_names = {
        "manifest": "admission.json",
        "license_receipt": "license-receipt.json",
        "adjudication_receipt": "adjudication-receipt.json",
    }
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
    quanta_manifests: list[str] = []
    phase_metrics: list[str] = []
    resource_metrics: list[str] = []
    for layout in rep_layouts:
        for record in sorted(layout["quanta"].values()):
            records.append(relative(Path(record)))
        records.append(relative(Path(layout["semble"])))
        natives.append(relative(Path(layout["semble"]).parent / "native.json"))
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
        for side in ("python", "rust"):
            _validate_counts_shape(
                read_json(Path(frozen[f"contract_{side}_results"])),
                f"contract {side} results",
            )
            receipt = _validate_receipt_shape(
                read_json(Path(frozen[f"contract_{side}_receipt"])),
                f"contract {side} receipt",
            )
            role = "pytest-junit" if side == "python" else "nextest-jsonl"
            _verify_receipt_inputs(
                receipt,
                {role: Path(frozen[f"contract_{side}_raw"])},
                f"contract {side} receipt",
            )
            evidence.setdefault("contract_suites", {})[side] = {
                "test_result_digest": sha_file(Path(frozen[f"contract_{side}_results"])),
                "raw_evidence_digest": sha_file(Path(frozen[f"contract_{side}_raw"])),
            }
    if any(key in frozen for key in SDK_EVIDENCE_KEYS):
        if not all(key in frozen for key in SDK_EVIDENCE_KEYS):
            raise RunError("incomplete frozen SDK receipt set")
        sdk_results = _validate_sdk_results_shape(
            read_json(Path(frozen["sdk_results"])), "sdk results"
        )
        sdk_receipt = _validate_receipt_shape(read_json(Path(frozen["sdk_receipt"])), "sdk receipt")
        _verify_receipt_inputs(
            sdk_receipt,
            {
                "nextest-jsonl": Path(frozen["sdk_nextest_raw"]),
                "runner-record": Path(frozen["sdk_record_raw"]),
            },
            "sdk receipt",
        )
        evidence["sdk_path"] = {
            "test_result_digest": sha_file(Path(frozen["sdk_results"])),
            "nextest_digest": sha_file(Path(frozen["sdk_nextest_raw"])),
            "runner_record_digest": sha_file(Path(frozen["sdk_record_raw"])),
            "separate_process": sdk_results["separate_process"],
            "sealed_receipt": True,
            "activation_ack": True,
            "empty_check": sdk_results["empty_check"],
        }
    if "model_parity_results" in frozen:
        _validate_parity_results_shape(
            read_json(Path(frozen["model_parity_results"])), "model parity results"
        )
        evidence["model_parity"] = {
            "test_result_digest": sha_file(Path(frozen["model_parity_results"]))
        }
    if "incremental_results" in frozen:
        _validate_parity_results_shape(
            read_json(Path(frozen["incremental_results"])), "incremental results"
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
    if scope == "qualified" and set(admission_files) != set(ADMISSION_KEYS):
        raise RunError("qualified run lacks the complete frozen admission bundle")
    if scope != "qualified" and admission_files:
        raise RunError("exploratory run cannot carry qualification admission authority")
    profile_path = Path(spec.get("host_profile", ""))
    validate_host_profile(read_json(profile_path))
    admission_digest = None
    if admission_files:
        annotation_refs = admission_files["annotation_receipts"]
        if not isinstance(annotation_refs, list):
            raise RunError("frozen admission annotation receipts are malformed")
        quanta_model_revisions = set()
        for record_path in rep0["quanta"].values():
            record = read_json(Path(record_path))
            if not isinstance(record, dict) or not isinstance(record.get("captures"), dict):
                raise RunError("qualified admission cannot inspect Quanta capture models")
            for capture in record["captures"].values():
                if not isinstance(capture, dict):
                    raise RunError("qualified admission found a malformed Quanta capture")
                revision = capture.get("model_revision")
                if isinstance(revision, str) and revision != "not-applicable":
                    quanta_model_revisions.add(revision)
        if len(quanta_model_revisions) != 1:
            raise RunError("qualified admission requires one Quanta model revision")
        admission = verify_admission_bundle(
            Path(str(admission_files["manifest"])),
            Path(str(admission_files["license_receipt"])),
            [Path(str(path)) for path in annotation_refs],
            Path(str(admission_files["adjudication_receipt"])),
            source_revision=source_sha,
            corpus_manifest_path=Path(spec["manifest"]),
            suite_path=Path(spec["suite"]),
            query_pack_path=Path(spec["query_pack"]),
            lockfile_path=Path(spec["semble_lockfile"]),
            host_profile_path=profile_path,
            cache_regime=spec.get("cache_regime", "undeclared"),
            receipt_paths={
                key: Path(frozen[key])
                for key in ("contract_python_receipt", "contract_rust_receipt", "sdk_receipt")
                if key in frozen
            },
            quanta_model_revision=next(iter(quanta_model_revisions)),
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
        "phase_metrics": sorted(phase_metrics),
        "resource_metrics": sorted(resource_metrics),
        "protocol_lock": "protocol-lock.json",
    }
    source_closure_digest = None
    if scope == "qualified":
        closure_path = Path(spec.get("_driver_source_closure", ""))
        closure = _validate_source_closure_shape(read_json(closure_path), "driver source closure")
        if closure["revision"] != source_sha:
            raise RunError("driver source closure revision differs from current HEAD")
        source_closure_digest = closure["digest"]
        for key in ("contract_python_receipt", "contract_rust_receipt", "sdk_receipt"):
            receipt = _validate_receipt_shape(read_json(Path(frozen[key])), key)
            if receipt["source_closure"]["digest"] != source_closure_digest:
                raise RunError(f"{key} source closure differs from the capture closure")
        artifacts["driver_source_closure"] = relative(closure_path)
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
            "cache_regime": spec.get("cache_regime", "undeclared"),
        },
        "artifacts": artifacts,
        "provenance": {
            "admission": {"manifest_digest": admission_digest},
            "quanta": {
                "source_sha": source_sha,
                "source_closure_digest": source_closure_digest,
                "binary_digest": runner_binary_digest,
                "embedder": spec.get("embedder", "potion-code"),
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
        spec.get("semble_cache_root", str(out_dir.parent / "semble-cache")),
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
        str(spec.get("query_repetitions_per_root", spec.get("semble_repetitions", 1))),
        "--warmup-passes",
        str(spec.get("query_warmup_passes", spec.get("semble_warmup_passes", 0))),
    ]
    if "_query_protocol" in spec:
        command += ["--query-protocol", spec["_query_protocol"]]
    if "_materialized_corpus" in spec:
        command += ["--materialized-corpus"]
    if "semble_model_revision" in spec:
        command += ["--model-revision", spec["semble_model_revision"]]
    command, isolation = sandbox_command(spec, command)
    evidence_root = out_dir.parent
    resource_path = evidence_root / "semble-resource-metrics.json"
    stdout_path = evidence_root / "semble-adapter.stdout.log"
    stderr_path = evidence_root / "semble-adapter.stderr.log"
    resource = run_monitored_process(
        command,
        stdout_path=stdout_path,
        stderr_path=stderr_path,
        resource_path=resource_path,
        timeout_secs=_int(spec.get("timeout_secs", 1800), "spec.timeout_secs"),
        subject_path=out_dir / "record.json",
        isolation=isolation,
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
    cache_root = Path(spec.get("semble_cache_root", str(out_dir.parent / "semble-cache")))
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


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    sub = parser.add_subparsers(dest="command", required=True)
    quanta = sub.add_parser("quanta", help="run the Rust runner per strategy")
    quanta.add_argument("--spec", required=True)
    pair = sub.add_parser("pair", help="sequential paired capture + scoring")
    pair.add_argument("--spec", required=True)
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
    return parser


def main(argv: list[str] | None = None) -> int:
    parser = build_parser()
    args = parser.parse_args(argv)
    if args.command == "merge":
        return cmd_merge(args)
    if args.command == "host-probe":
        return cmd_host_probe(args)
    if args.command == "host-profile":
        return cmd_host_profile(args)
    if args.command == "quanta":
        return cmd_quanta(args)
    if args.command == "verdict":
        return cmd_verdict(args)
    return cmd_pair(args)


if __name__ == "__main__":
    raise SystemExit(main())
