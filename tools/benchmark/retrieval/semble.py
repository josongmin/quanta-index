#!/usr/bin/env python3
"""Pinned Semble same-corpus comparison adapter (RB-04).

Runs a pinned Semble 0.6.0 install (an outside-the-checkout virtualenv)
against the exact admitted file universe and blind query pack, then
normalizes its native results into a v3 runner record for the single
Quanta-owned evaluator. Semble ranking is never reimplemented here.

Layout contract (all outside the source checkout):
  <output-root>/
    corpus/                 # isolated corpus: admitted files only
    worker.py               # exact spawned worker (auditable)
    native.json             # Semble-native results + timings + observed files
    mapping-proof.json      # path map + both-side path+SHA diff
    record.json             # current runner record (schema v3)
    lockfile.txt            # external hash-pinned lockfile copy (+ digest)

A common-universe pair requires a clean mapping proof: every admitted file
observed in Semble's indexed chunks with matching bytes. Anything else is a
typed refusal, never a silent subset.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import os
import subprocess
import sys
from pathlib import Path

try:
    from tools.benchmark.retrieval.evaluator import (
        TOKEN_RE,
        TOKENIZER,
        TOKENIZER_BUDGET_VERSION,
        canonical,
        digest,
        validate_comparison_contract,
        verify_repo,
    )
except ImportError:  # direct script invocation: import the sibling module
    sys.path.insert(0, str(Path(__file__).resolve().parent))
    from evaluator import (  # noqa: E402
        TOKEN_RE,
        TOKENIZER,
        TOKENIZER_BUDGET_VERSION,
        canonical,
        digest,
        validate_comparison_contract,
        verify_repo,
    )

SEMBLE_PINNED_VERSION = "0.6.0"

WORKER_TEMPLATE = '''"""Spawned Semble worker (pinned env only). Reads SPEC_JSON, writes NATIVE_JSON."""
import json
import os
import sys
import time

def peak_resident_bytes() -> int:
    if sys.platform == "win32":
        import ctypes

        class ProcessMemoryCounters(ctypes.Structure):
            _fields_ = [
                ("cb", ctypes.c_uint32),
                ("PageFaultCount", ctypes.c_uint32),
                ("PeakWorkingSetSize", ctypes.c_size_t),
                ("WorkingSetSize", ctypes.c_size_t),
                ("QuotaPeakPagedPoolUsage", ctypes.c_size_t),
                ("QuotaPagedPoolUsage", ctypes.c_size_t),
                ("QuotaPeakNonPagedPoolUsage", ctypes.c_size_t),
                ("QuotaNonPagedPoolUsage", ctypes.c_size_t),
                ("PagefileUsage", ctypes.c_size_t),
                ("PeakPagefileUsage", ctypes.c_size_t),
            ]

        kernel = ctypes.WinDLL("kernel32", use_last_error=True)
        psapi = ctypes.WinDLL("psapi", use_last_error=True)
        kernel.GetCurrentProcess.restype = ctypes.c_void_p
        psapi.GetProcessMemoryInfo.argtypes = [
            ctypes.c_void_p,
            ctypes.POINTER(ProcessMemoryCounters),
            ctypes.c_uint32,
        ]
        psapi.GetProcessMemoryInfo.restype = ctypes.c_int32
        counters = ProcessMemoryCounters()
        counters.cb = ctypes.sizeof(counters)
        if not psapi.GetProcessMemoryInfo(
            kernel.GetCurrentProcess(), ctypes.byref(counters), counters.cb
        ):
            raise OSError(ctypes.get_last_error(), "GetProcessMemoryInfo failed")
        observed = int(counters.PeakWorkingSetSize)
        if observed <= 0:
            raise RuntimeError("Windows peak working set is not positive")
        return observed

    import resource

    unit = 1 if sys.platform == "darwin" else 1024
    observed = int(resource.getrusage(resource.RUSAGE_SELF).ru_maxrss) * unit
    if observed <= 0:
        raise RuntimeError("peak resident set is not positive")
    return observed

def main() -> int:
    worker_started_ns = time.monotonic_ns()
    spec_path = os.environ["SPEC_JSON"]
    out_path = os.environ["NATIVE_JSON"]
    with open(spec_path, encoding="utf-8") as handle:
        spec = json.load(handle)
    discovery_end_ns = time.monotonic_ns()
    from semble import SembleIndex
    model_prepare_end_ns = time.monotonic_ns()

    rss_before_index = peak_resident_bytes()
    index = SembleIndex.from_path(spec["corpus_dir"], show_progress_bar=False)
    index_end_ns = time.monotonic_ns()
    rss_after_index = peak_resident_bytes()
    index_resident_bytes = max(rss_after_index - rss_before_index, 0)
    if index_resident_bytes == 0:
        raise SystemExit("worker could not attribute positive resident bytes to the index")
    observed = sorted({chunk.file_path for chunk in index.chunks})
    stats = {
        "indexed_files": int(index.stats.indexed_files),
        "total_chunks": int(index.stats.total_chunks),
        "languages": {str(k): int(v) for k, v in dict(index.stats.languages).items()},
        "index_resident_bytes": index_resident_bytes,
        "index_measurement": "process_peak_rss_delta_v1",
    }
    queries = [(task["task_id"], task["query"]) for task in spec["tasks"]]
    if len({task_id for task_id, _ in queries}) != len(queries):
        raise SystemExit("worker refuses a spec with duplicate task_ids")
    top_k = int(spec["top_k"])
    seed = int(spec.get("seed", 0))
    warmup = int(spec.get("warmup_passes", 1))
    repetitions = int(spec.get("repetitions", 1))
    protocol = spec.get("query_protocol")
    query_by_id = dict(queries)
    cold_latency_ms = None
    cold_query_start_ns = index_end_ns
    cold_query_end_ns = index_end_ns
    if protocol is not None:
        cold_query_start_ns = time.monotonic_ns()
        index.search(query_by_id[protocol["cold_probe_task_id"]], top_k=top_k)
        cold_query_end_ns = time.monotonic_ns()
        cold_latency_ms = (cold_query_end_ns - cold_query_start_ns) / 1_000_000.0
        warmup_schedules = protocol["warmup_schedules"]
        measurement_schedules = protocol["measurement_schedules"]
    else:
        warmup_schedules = [[task_id for task_id, _ in queries] for _ in range(warmup)]
        measurement_schedules = [[task_id for task_id, _ in queries] for _ in range(repetitions)]
    for schedule in warmup_schedules:
        for task_id in schedule:
            index.search(query_by_id[task_id], top_k=top_k)
    warmup_end_ns = time.monotonic_ns()
    native = []
    latencies = {}
    query_started_ns = time.monotonic_ns()
    first_query_ms = None
    first_query_start_ns = None
    first_query_end_ns = None
    for rep, schedule in enumerate(measurement_schedules):
        for task_id in schedule:
            query = query_by_id[task_id]
            t0 = time.monotonic_ns()
            results = index.search(query, top_k=top_k)
            ended_ns = time.monotonic_ns()
            elapsed_ms = (ended_ns - t0) / 1_000_000.0
            if first_query_ms is None:
                first_query_ms = elapsed_ms
                first_query_start_ns = t0
                first_query_end_ns = ended_ns
            latencies.setdefault(task_id, []).append(elapsed_ms)
            if rep == 0:
                native.append(
                    {
                        "task_id": task_id,
                        "results": [
                            {
                                "file_path": r.chunk.file_path,
                                "start_line": int(r.chunk.start_line),
                                "end_line": int(r.chunk.end_line),
                                "score": float(r.score),
                            }
                            for r in results
                        ],
                    }
                )
    query_end_ns = time.monotonic_ns()
    first_query_ms = first_query_ms or 0.0
    if first_query_start_ns is None or first_query_end_ns is None:
        raise SystemExit("worker did not execute a first measured query")
    emitted = sorted(row["task_id"] for row in native)
    expected = sorted(task_id for task_id, _ in queries)
    if emitted != expected:
        raise SystemExit("worker output task set differs from the spec task set")
    worker_end_ns = time.monotonic_ns()
    query_ms = (query_end_ns - query_started_ns) / 1_000_000.0
    phase_boundaries_ns = {
        "worker_start": worker_started_ns,
        "discovery_end": discovery_end_ns,
        "model_provider_prepare_end": model_prepare_end_ns,
        "index_end": index_end_ns,
        "warmup_end": warmup_end_ns,
        "query_start": query_started_ns,
        "first_query_start": first_query_start_ns,
        "first_query_end": first_query_end_ns,
        "cold_query_start": cold_query_start_ns,
        "cold_query_end": cold_query_end_ns,
        "query_end": query_end_ns,
        "worker_end": worker_end_ns,
    }
    payload = {
        "semble_index_ms": (index_end_ns - model_prepare_end_ns) / 1_000_000.0,
        "discovery_ms": (discovery_end_ns - worker_started_ns) / 1_000_000.0,
        "model_provider_prepare_ms": (
            model_prepare_end_ns - discovery_end_ns
        ) / 1_000_000.0,
        "warmup_ms": (
            warmup_end_ns - (cold_query_end_ns if protocol is not None else index_end_ns)
        ) / 1_000_000.0,
        "first_query_ms": first_query_ms,
        "warm_query_ms": max(query_ms - first_query_ms, 0.0),
        "cold_query_ms": cold_latency_ms,
        "protocol_warm_query_ms": query_ms if protocol is not None else None,
        "worker_total_ms": (worker_end_ns - worker_started_ns) / 1_000_000.0,
        "phase_boundaries_ns": phase_boundaries_ns,
        "configured_model_name": os.environ["SEMBLE_MODEL_NAME"],
        "observed_files": observed,
        "stats": stats,
        "query_schedule": [task_id for task_id, _ in queries],
        "query_protocol": protocol,
        "cold_latency_ms": cold_latency_ms,
        "native": native,
        "latencies_ms": latencies,
        "timing_layer": "worker_wall_per_query_ms",
        "worker_pid": os.getpid(),
        "repetitions": repetitions,
        "warmup_passes": warmup,
        "seed": seed,
    }
    with open(out_path, "w", encoding="utf-8") as handle:
        json.dump(payload, handle, indent=2, sort_keys=True)
        handle.write("\\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
'''


class AdapterError(ValueError):
    """Semble adapter evidence is absent, inconsistent or ineligible."""


def _int(value: object, label: str) -> int:
    try:
        return int(str(value))
    except (TypeError, ValueError) as exc:
        raise AdapterError(f"{label} must be an integer: {value!r}") from exc


def read_json(path: Path) -> object:
    def unique_object(pairs: list[tuple[str, object]]) -> dict:
        value = {}
        for key, item in pairs:
            if key in value:
                raise AdapterError(f"duplicate JSON key: {key}")
            value[key] = item
        return value

    def reject_constant(value: str) -> object:
        raise AdapterError(f"non-finite JSON number: {value}")

    try:
        return json.loads(
            path.read_text(encoding="utf-8"),
            object_pairs_hook=unique_object,
            parse_constant=reject_constant,
        )
    except (OSError, UnicodeError, json.JSONDecodeError) as exc:
        raise AdapterError(f"cannot read JSON {path}: {exc}") from exc


def sha_file(path: Path) -> str:
    digestor = hashlib.sha256()
    with path.open("rb") as handle:
        for block in iter(lambda: handle.read(65536), b""):
            digestor.update(block)
    return digestor.hexdigest()


def validate_query_protocol(payload: object, task_ids: list[str]) -> dict:
    if not isinstance(payload, dict):
        raise AdapterError("query protocol must be an object")
    expected_keys = {
        "schema_version",
        "seed",
        "task_ids",
        "cold_probe_task_id",
        "warmup_schedules",
        "measurement_schedules",
        "sha256",
    }
    if set(payload) != expected_keys:
        raise AdapterError("query protocol keys differ from the closed schema")
    if payload["schema_version"] != 1 or type(payload["seed"]) is not int:
        raise AdapterError("query protocol version or seed is invalid")
    if payload["task_ids"] != task_ids or payload["cold_probe_task_id"] not in task_ids:
        raise AdapterError("query protocol task ids differ from the query pack")
    expected = set(task_ids)
    for key in ("warmup_schedules", "measurement_schedules"):
        schedules = payload[key]
        if not isinstance(schedules, list) or (key == "measurement_schedules" and not schedules):
            raise AdapterError(f"query protocol {key} has an invalid schedule list")
        if any(
            not isinstance(schedule, list)
            or len(schedule) != len(task_ids)
            or any(not isinstance(task_id, str) for task_id in schedule)
            or set(schedule) != expected
            for schedule in schedules
        ):
            raise AdapterError(f"query protocol {key} must contain exact task permutations")
    core = {key: value for key, value in payload.items() if key != "sha256"}
    observed = hashlib.sha256(
        json.dumps(core, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode("utf-8")
    ).hexdigest()
    if payload["sha256"] != observed:
        raise AdapterError("query protocol digest mismatch")
    return payload


def load_manifest(path: Path) -> tuple[str, list[tuple[str, str]]]:
    payload = read_json(path)
    if not isinstance(payload, dict):
        raise AdapterError("manifest must be an object")
    commit = payload.get("repository_commit")
    files = payload.get("files")
    if (
        not isinstance(commit, str)
        or len(commit) != 40
        or any(c not in "0123456789abcdef" for c in commit)
    ):
        raise AdapterError("manifest lacks repository_commit")
    if not isinstance(files, list) or not files:
        raise AdapterError("manifest admits no files")
    rows = []
    for entry in files:
        if not isinstance(entry, dict):
            raise AdapterError("manifest entry must be an object")
        name, sha = entry.get("path"), entry.get("file_sha256")
        if not isinstance(name, str) or not name or not isinstance(sha, str):
            raise AdapterError("manifest entry lacks path/file_sha256")
        if (
            name.startswith("/")
            or "\\" in name
            or any(part in ("", ".", "..") for part in name.split("/"))
        ):
            raise AdapterError(f"unsafe manifest path: {name}")
        if len(sha) != 64 or any(c not in "0123456789abcdef" for c in sha):
            raise AdapterError(f"manifest entry has invalid file_sha256: {name}")
        rows.append((name, sha))
    if len({name for name, _ in rows}) != len(rows):
        raise AdapterError("manifest holds duplicate paths")
    return commit, rows


def load_query_pack(path: Path) -> dict:
    payload = read_json(path)
    if not isinstance(payload, dict):
        raise AdapterError("query pack must be an object")
    if payload.get("schema_version") != 3:
        raise AdapterError("query pack schema_version must be 3")
    expected = {
        "schema_version",
        "suite_id",
        "suite_commitment_sha256",
        "repository_commit",
        "tokenizer",
        "tokenizer_budget_version",
        "routes",
        "file_universe",
        "file_universe_digest",
        "comparison_contract",
        "tasks",
    }
    if set(payload) != expected:
        raise AdapterError("query pack holds unexpected or missing top-level keys")
    try:
        validate_comparison_contract(payload.get("comparison_contract"), "pack.comparison_contract")
    except ValueError as exc:
        raise AdapterError(f"query pack comparison contract invalid: {exc}") from exc
    tasks = payload.get("tasks")
    if not isinstance(tasks, list) or not tasks:
        raise AdapterError("query pack holds no tasks")
    seen = set()
    for task in tasks:
        if not isinstance(task, dict):
            raise AdapterError("query pack task must be an object")
        if set(task) != {"task_id", "query", "query_sha256"}:
            raise AdapterError(
                "query pack task holds unexpected keys (gold/grade smuggling refused)"
            )
        for key in ("task_id", "query", "query_sha256"):
            if not isinstance(task.get(key), str) or not task[key]:
                raise AdapterError(f"query pack task lacks {key}")
        if task["task_id"] in seen:
            raise AdapterError(f"query pack holds a duplicate task_id: {task['task_id']!r}")
        seen.add(task["task_id"])
    return payload


def verify_lockfile(lockfile_bytes: bytes, expected_sha256: str, freeze_text: str) -> str:
    """Check the external hash-pinned lockfile against the observed env.

    The authoritative pin is the external lockfile (path + digest), never the
    observed ``pip freeze`` output: freeze is the env observation, the lockfile
    is the expectation. The env must carry every lockfile line plus the exact
    pinned Semble line; anything else is env drift and refuses.
    """
    if not isinstance(expected_sha256, str) or len(expected_sha256) != 64:
        raise AdapterError("lockfile pin must be a lowercase sha256")
    observed = hashlib.sha256(bytes(lockfile_bytes)).hexdigest()
    if observed != expected_sha256:
        raise AdapterError("external lockfile digest differs from the spec pin")
    locked = {line.strip() for line in lockfile_bytes.decode("utf-8", "strict").splitlines()} - {
        "",
        "#",
    }
    locked = {line for line in locked if not line.startswith("#")}
    frozen = {line.strip() for line in freeze_text.splitlines() if line.strip()}
    if f"semble=={SEMBLE_PINNED_VERSION}" not in frozen:
        raise AdapterError("observed freeze lacks the pinned Semble line")
    missing = sorted(locked - frozen)
    if missing:
        raise AdapterError(f"observed freeze lacks {len(missing)} locked lines")
    return observed


def check_semble_env(python: Path) -> dict:
    """Verify the pinned Semble interpreter: exact version, imports, identity."""
    if not python.is_file():
        raise AdapterError(f"Semble python is not a file: {python}")
    probe = (
        "import hashlib, importlib.metadata, json, pathlib, sys; "
        "import semble; "
        "from semble import SembleIndex; "
        "pkg = pathlib.Path(semble.__file__).resolve().parent; "
        "infos = sorted(pkg.parent.glob('semble-*.dist-info')); "
        "info = infos[0] if infos else None; "
        "record = (info / 'RECORD').read_bytes() if info and (info / 'RECORD').is_file() else None; "
        "direct = (info / 'direct_url.json').read_bytes() if info and (info / 'direct_url.json').is_file() else None; "
        "print(json.dumps({"
        "'semble_version': importlib.metadata.version('semble'), "
        "'python_version': sys.version, "
        "'has_from_path': hasattr(SembleIndex, 'from_path'), "
        "'has_search': hasattr(SembleIndex, 'search'), "
        "'dist_info': info.name if info else None, "
        "'record_sha256': hashlib.sha256(record).hexdigest() if record else None, "
        "'direct_url_sha256': hashlib.sha256(direct).hexdigest() if direct else None}))"
    )
    try:
        completed = subprocess.run(
            [str(python), "-c", probe],
            check=True,
            capture_output=True,
            text=True,
            timeout=120,
        )
    except (OSError, subprocess.CalledProcessError, subprocess.TimeoutExpired) as exc:
        raise AdapterError(f"Semble env probe failed: {exc}") from exc
    try:
        report = json.loads(completed.stdout)
    except json.JSONDecodeError as exc:
        raise AdapterError(f"Semble env probe is not JSON: {exc}") from exc
    if not report.get("has_from_path") or not report.get("has_search"):
        raise AdapterError("pinned Semble lacks the from_path/search API")
    observed_version = report.get("semble_version")
    if observed_version != SEMBLE_PINNED_VERSION:
        raise AdapterError(
            f"Semble {SEMBLE_PINNED_VERSION} is pinned but the env holds {observed_version!r}"
        )
    if not isinstance(report.get("python_version"), str) or not report["python_version"]:
        raise AdapterError("Semble env probe lacks the interpreter version")
    installed = {
        "dist_info": report.get("dist_info"),
        "record_sha256": report.get("record_sha256"),
        "direct_url_sha256": report.get("direct_url_sha256"),
    }
    if not isinstance(installed["dist_info"], str) or not installed["dist_info"]:
        raise AdapterError("installed Semble lacks dist-info identity proof")
    record_digest = installed["record_sha256"]
    if (
        not isinstance(record_digest, str)
        or len(record_digest) != 64
        or any(c not in "0123456789abcdef" for c in record_digest)
    ):
        raise AdapterError("installed Semble lacks a RECORD digest proof")
    direct_digest = installed["direct_url_sha256"]
    if direct_digest is not None and (
        not isinstance(direct_digest, str)
        or len(direct_digest) != 64
        or any(c not in "0123456789abcdef" for c in direct_digest)
    ):
        raise AdapterError("installed Semble holds a malformed direct_url digest")
    report["installed_distribution"] = installed
    try:
        freeze = subprocess.run(
            [str(python), "-m", "pip", "freeze"],
            check=False,
            capture_output=True,
            text=True,
            timeout=120,
        )
    except (OSError, subprocess.TimeoutExpired) as exc:
        raise AdapterError(f"Semble environment pip freeze failed: {exc}") from exc
    if freeze.returncode != 0 or not freeze.stdout.strip():
        raise AdapterError("Semble environment pip freeze failed or is empty")
    # Freeze is the env observation, never the authoritative pin: the external
    # hash-pinned lockfile is the expectation (see verify_lockfile).
    report["observed_freeze"] = freeze.stdout
    report["observed_freeze_sha256"] = hashlib.sha256(freeze.stdout.encode("utf-8")).hexdigest()
    resolved = python.resolve()
    try:
        interpreter_digest = sha_file(resolved)
    except OSError as exc:
        raise AdapterError(f"cannot hash the Semble interpreter: {exc}") from exc
    report["interpreter"] = {
        "path": str(python),
        "realpath": str(resolved),
        "version": report.pop("python_version"),
        "digest": interpreter_digest,
    }
    return report


def build_isolated_corpus(
    repo: Path, manifest_rows: list[tuple[str, str]], corpus_dir: Path
) -> tuple[list[tuple[str, str]], int]:
    """Copy admitted bytes to an isolated dir. Returns rows + max bytes."""
    if corpus_dir.exists():
        raise AdapterError(f"corpus dir already exists (refusing reuse): {corpus_dir}")
    corpus_dir.mkdir(parents=True)
    rows: list[tuple[str, str]] = []
    max_bytes = 0
    for name, expected in manifest_rows:
        source = repo / name
        if source.is_symlink() or not source.is_file() or repo not in source.resolve().parents:
            raise AdapterError(f"admitted file is not a regular file: {name}")
        data = source.read_bytes()
        observed = hashlib.sha256(data).hexdigest()
        if observed != expected:
            raise AdapterError(f"admitted file hash mismatch: {name}")
        target = corpus_dir / name
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_bytes(data)
        rows.append((name, observed))
        max_bytes = max(max_bytes, len(data))
    return rows, max_bytes


def verify_materialized_corpus(repo: Path, manifest_rows: list[tuple[str, str]]) -> Path:
    """Prove a Git-free directory contains exactly the admitted file universe."""
    repo = repo.resolve()
    if not repo.is_dir() or (repo / ".git").exists():
        raise AdapterError("materialized corpus must be a Git-free directory")
    observed: list[tuple[str, str]] = []
    for dirpath, dirnames, filenames in os.walk(repo, followlinks=False):
        base = Path(dirpath)
        if any((base / name).is_symlink() for name in dirnames):
            raise AdapterError("materialized corpus contains a symlinked directory")
        for name in filenames:
            path = base / name
            if path.is_symlink() or not path.is_file():
                raise AdapterError("materialized corpus contains a non-regular file")
            observed.append((path.relative_to(repo).as_posix(), sha_file(path)))
    if sorted(observed) != sorted(manifest_rows):
        raise AdapterError("materialized corpus differs from the admitted universe")
    return repo


def read_hf_revision(hf_home: Path, model_id: str) -> str | None:
    """Best-effort pinned model revision from the HF cache refs."""
    slug = "models--" + model_id.replace("/", "--")
    ref = hf_home / "hub" / slug / "refs" / "main"
    try:
        text = ref.read_text(encoding="utf-8").strip()
    except OSError:
        return None
    return text or None


def mapping_proof(
    admitted: list[tuple[str, str]],
    observed: list[str],
    corpus_dir: Path,
) -> tuple[dict, str]:
    """Path map + both-side path+SHA diff. Returns (proof, diff_digest)."""
    if not isinstance(observed, list) or any(not isinstance(name, str) for name in observed):
        raise AdapterError("Semble observed files must be a path list")
    if len(observed) != len(set(observed)):
        raise AdapterError("Semble observed files contain a duplicate path")
    corpus_root = corpus_dir.resolve()
    for name in observed:
        if (
            not name
            or name.startswith("/")
            or "\\" in name
            or any(part in ("", ".", "..") for part in name.split("/"))
        ):
            raise AdapterError(f"unsafe Semble observed path: {name!r}")
        target = corpus_dir / name
        if corpus_root not in target.resolve().parents:
            raise AdapterError(f"Semble observed path escapes the corpus: {name!r}")
        current = corpus_dir
        for part in name.split("/"):
            current = current / part
            if current.is_symlink():
                raise AdapterError(f"Semble observed path uses a symlink: {name!r}")
    admitted_names = [name for name, _ in admitted]
    admitted_set = set(admitted_names)
    observed_set = set(observed)
    quanta_side = sorted(
        ({"path": n, "file_sha256": s} for n, s in admitted),
        key=lambda row: row["path"],
    )
    semble_side = []
    for name in sorted(observed_set):
        target = corpus_dir / name
        try:
            data = target.read_bytes() if target.is_file() else None
        except OSError:
            data = None
        semble_side.append(
            {
                "path": name,
                "file_sha256": hashlib.sha256(data).hexdigest() if data is not None else None,
                "readable": data is not None,
            }
        )
    observed_by_path = {row["path"]: row for row in semble_side}
    per_file = []
    mismatched = []
    for name, sha in sorted(admitted):
        if name not in observed_set:
            status = "skipped"
        else:
            side = observed_by_path[name]
            if not side["readable"]:
                status = "unreadable"
            elif side["file_sha256"] != sha:
                status = "hash_mismatch"
            else:
                status = "indexed"
            if status in ("unreadable", "hash_mismatch"):
                mismatched.append(name)
        per_file.append({"path": name, "file_sha256": sha, "status": status})
    for name in sorted(observed_set - admitted_set):
        per_file.append({"path": name, "file_sha256": None, "status": "extra"})
    proof = {
        "path_map": [
            {"semble_path": name, "canonical_path": name} for name in sorted(observed_set)
        ],
        "quanta_side": quanta_side,
        "semble_side": semble_side,
        "per_file": sorted(per_file, key=lambda row: row["path"]),
        "admitted_count": len(admitted_set),
        "observed_count": len(observed_set),
        "skipped": sorted(admitted_set - observed_set),
        "extra": sorted(observed_set - admitted_set),
        "mismatched": mismatched,
    }
    diff_digest = digest(
        json.dumps(
            {"quanta_side": quanta_side, "semble_side": semble_side},
            sort_keys=True,
            separators=(",", ":"),
            ensure_ascii=False,
        ).encode("utf-8")
    )
    proof["diff_digest"] = diff_digest
    return proof, diff_digest


def count_tokens(text: str) -> int:
    return len(TOKEN_RE.findall(text))


def normalize_record(
    pack: dict,
    pack_sha256: str,
    native: list[dict],
    latencies: dict[str, list[float]],
    repo: Path,
    file_shas: dict[str, str],
    file_lines: dict[str, list[bytes]],
    contract: dict,
    run_id: str,
    blinding: str,
    isolation_method: str,
    access_block_log: str,
    model: str,
    model_revision: str,
    route: str,
    capture_id: str,
    receipt_digest: str,
    worker_digest: str,
) -> dict:
    """Native Semble hits -> v3 runner record. Order preserved, spans proven.

    Unknown latency is null, never 0. The capture binds the Semble-owned
    chunker, the exact worker bytes, and the mapping-proof anchor.
    """
    if not isinstance(contract, dict) or pack.get("comparison_contract") != contract:
        raise AdapterError("pack comparison contract differs from the capture contract")
    top_k = contract.get("top_k")
    if not isinstance(top_k, int) or isinstance(top_k, bool) or top_k <= 0:
        raise AdapterError("capture contract top_k must be a positive integer")
    if not isinstance(capture_id, str) or not capture_id.strip():
        raise AdapterError("capture_id must be a nonempty string")
    for label, value in (("receipt_digest", receipt_digest), ("worker_digest", worker_digest)):
        if (
            not isinstance(value, str)
            or len(value) != 64
            or any(c not in "0123456789abcdef" for c in value)
        ):
            raise AdapterError(f"{label} must be a lowercase sha256")
    if not isinstance(native, list):
        raise AdapterError("Semble native rows must be a list")
    by_task: dict[str, object] = {}
    expected_task_ids = {task["task_id"] for task in pack["tasks"]}
    for row in native:
        if not isinstance(row, dict) or set(row) != {"task_id", "results"}:
            raise AdapterError("Semble native row must hold exactly task_id and results")
        task_key = row["task_id"]
        if not isinstance(task_key, str) or not task_key:
            raise AdapterError("Semble native row lacks a task_id")
        if task_key not in expected_task_ids:
            raise AdapterError(f"Semble emitted an unexpected native task: {task_key}")
        if task_key in by_task:
            raise AdapterError(f"Semble emitted a duplicate native row: {task_key}")
        by_task[task_key] = row["results"]
    if not isinstance(latencies, dict) or any(
        task_id not in expected_task_ids for task_id in latencies
    ):
        raise AdapterError("Semble latencies contain an unexpected task or invalid mapping")
    results = []
    for task in pack["tasks"]:
        task_id = task["task_id"]
        if task_id not in by_task:
            results.append(
                {
                    "task_id": task_id,
                    "route": route,
                    "status": "error",
                    "timings": {"query_latency_ms": None},
                    "candidates": [],
                    "error": {
                        "code": "semble_missing_query",
                        "message": "Semble emitted no row for this query",
                    },
                }
            )
            continue
        samples = latencies.get(task_id, [])
        latency = samples[0] if samples else None
        hits = by_task[task_id]
        if not isinstance(hits, list):
            raise AdapterError(f"Semble native row is not a list: {task_id}")
        if len(hits) > top_k:
            raise AdapterError(f"Semble exceeded top_k for {task_id}")
        if not hits:
            results.append(
                {
                    "task_id": task_id,
                    "route": route,
                    "status": "abstained",
                    "candidates": [],
                    "timings": {"query_latency_ms": latency},
                    "error": None,
                }
            )
            continue
        candidates = []
        hit_error = None
        for rank, hit in enumerate(hits, start=1):
            # Per-hit content failures become error rows (pair-incomplete at
            # verdict) instead of silently clamped spans or fabricated bytes.
            path = hit.get("file_path") if isinstance(hit, dict) else None
            start = hit.get("start_line") if isinstance(hit, dict) else None
            end = hit.get("end_line") if isinstance(hit, dict) else None
            if not isinstance(path, str) or path not in file_shas:
                hit_error = {
                    "code": "semble_hit_outside_universe",
                    "message": f"Semble hit outside admitted universe: {path!r}",
                }
                break
            if (
                not isinstance(start, int)
                or not isinstance(end, int)
                or isinstance(start, bool)
                or isinstance(end, bool)
                or start < 1
                or end < start
            ):
                hit_error = {
                    "code": "semble_hit_bad_span",
                    "message": f"Semble hit has a bad span: {path}:{start}-{end}",
                }
                break
            lines = file_lines[path]
            if end > len(lines):
                hit_error = {
                    "code": "semble_hit_beyond_eof",
                    "message": f"Semble hit spans beyond EOF: {path}:{start}-{end}",
                }
                break
            block = b"".join(lines[start - 1 : end])
            try:
                text = block.decode("utf-8")
            except UnicodeDecodeError:
                hit_error = {
                    "code": "semble_hit_not_utf8",
                    "message": f"Semble hit block is not UTF-8: {path}",
                }
                break
            tokens = count_tokens(text)
            if tokens == 0:
                hit_error = {
                    "code": "semble_hit_no_tokens",
                    "message": f"Semble hit holds no tokens: {path}:{start}-{end}",
                }
                break
            start_byte = sum(len(line) for line in lines[: start - 1])
            candidates.append(
                {
                    "path": path,
                    "start_byte": start_byte,
                    "end_byte": start_byte + len(block),
                    "start_line": start,
                    "end_line": end,
                    "file_sha256": file_shas[path],
                    "block_sha256": hashlib.sha256(block).hexdigest(),
                    "tokens": tokens,
                    "rank": rank,
                }
            )
        if hit_error is not None:
            results.append(
                {
                    "task_id": task_id,
                    "route": route,
                    "status": "error",
                    "timings": {"query_latency_ms": latency},
                    "candidates": [],
                    "error": hit_error,
                }
            )
            continue
        results.append(
            {
                "task_id": task_id,
                "route": route,
                "status": "success",
                "candidates": candidates,
                "timings": {"query_latency_ms": latency},
                "error": None,
            }
        )
    ordered_native = sorted(native, key=lambda row: str(row.get("task_id")))
    return {
        "schema_version": 3,
        "query_pack_sha256": pack_sha256,
        "comparison_contract": contract,
        "runner": {
            "name": "semble-adapter",
            "revision": run_id,
            "run_id": run_id,
            "tokenizer": TOKENIZER,
            "tokenizer_budget_version": TOKENIZER_BUDGET_VERSION,
            "gold_access": False,
            "blinding": blinding,
            "isolation_method": isolation_method,
            "access_block_log": access_block_log,
        },
        "captures": {
            capture_id: {
                "system": "semble",
                "chunk_strategy": "semble_native",
                "chunk_config": {},
                "runner_binary": {"name": "semble-worker", "digest": worker_digest},
                "searchd_binary": None,
                "generation": 0,
                "receipt_digest": receipt_digest,
                "activation_digest": digest(canonical(ordered_native)),
                "model": model,
                "model_revision": model_revision,
            }
        },
        "route_provenance": {route: {"capture_id": capture_id}},
        "results": results,
    }


def cmd_check(args: argparse.Namespace) -> int:
    try:
        report = check_semble_env(Path(args.python))
    except AdapterError as exc:
        print(f"ERROR: {exc}", file=sys.stderr)
        return 2
    print(json.dumps(report, indent=2, sort_keys=True))
    return 0


def cmd_run(args: argparse.Namespace) -> int:
    try:
        return run_adapter(args)
    except AdapterError as exc:
        print(f"ERROR: {exc}", file=sys.stderr)
        return 2


def validate_worker_phase_timings(
    native_payload: dict, *, protocol: bool
) -> tuple[dict[str, int | float], int | float]:
    """Reject non-finite worker timings before emitting any normalized artifact."""
    phases = {
        "discovery": native_payload.get("discovery_ms"),
        "model_provider_prepare": native_payload.get("model_provider_prepare_ms"),
        "index": native_payload.get("semble_index_ms"),
        "warmup": native_payload.get("warmup_ms"),
    }
    if protocol:
        phases["cold_query"] = native_payload.get("cold_query_ms")
        phases["warm_query"] = native_payload.get("protocol_warm_query_ms")
    else:
        phases["first_query"] = native_payload.get("first_query_ms")
        phases["warm_query"] = native_payload.get("warm_query_ms")

    def finite_nonnegative(value: object) -> bool:
        try:
            return type(value) in (int, float) and value >= 0 and math.isfinite(value)
        except OverflowError:
            return False

    if any(not finite_nonnegative(value) for value in phases.values()):
        raise AdapterError("Semble worker omitted finite nonnegative phase timings")
    phase_sum = sum(phases.values())
    total = native_payload.get("worker_total_ms")
    if not finite_nonnegative(phase_sum) or not finite_nonnegative(total) or total < phase_sum:
        raise AdapterError("Semble worker total timing is inconsistent with phases")
    phases["unattributed"] = total - phase_sum
    return phases, total


def run_adapter(args: argparse.Namespace) -> int:
    repo = Path(args.repo)
    out_root = Path(args.output_root)
    commit, manifest_rows = load_manifest(Path(args.manifest))
    if args.materialized_corpus:
        repo = verify_materialized_corpus(repo, manifest_rows)
    else:
        try:
            repo = verify_repo(repo, commit)
        except ValueError as exc:
            raise AdapterError(f"pinned repository proof failed: {exc}") from exc
    if repo in out_root.resolve().parents or out_root.resolve() == repo:
        raise AdapterError("output root must be outside the frozen repository")
    cache_root = Path(args.cache_root)
    if repo in cache_root.resolve().parents or cache_root.resolve() == repo:
        raise AdapterError("cache root must be outside the frozen repository")
    if out_root.exists():
        raise AdapterError(f"output root already exists (refusing reuse): {out_root}")
    out_root.mkdir(parents=True)
    pack = load_query_pack(Path(args.query_pack))
    if pack.get("repository_commit") != commit:
        raise AdapterError("manifest commit differs from query-pack commit")
    top_k = _int(args.top_k, "top_k")
    if top_k <= 0:
        raise AdapterError("top_k must be positive")
    if pack["comparison_contract"]["top_k"] != top_k:
        raise AdapterError("CLI top_k differs from the query-pack comparison contract")
    if args.blinding not in ("isolated", "attested"):
        raise AdapterError("blinding must be isolated or attested")

    env_report = check_semble_env(Path(args.python))
    try:
        external_lock = Path(args.lockfile).read_bytes()
    except OSError as exc:
        raise AdapterError(f"cannot read the external lockfile: {exc}") from exc
    lockfile_digest = verify_lockfile(
        external_lock, args.lockfile_sha256, env_report["observed_freeze"]
    )
    semble_version = env_report["semble_version"]
    lockfile = out_root / "lockfile.txt"
    lockfile.write_bytes(external_lock)
    assert sha_file(lockfile) == lockfile_digest

    corpus_dir = out_root / "corpus"
    admitted_rows, max_bytes = build_isolated_corpus(repo, manifest_rows, corpus_dir)
    # Semble skips files above its size cap; raise the cap to cover the
    # admitted universe explicitly and record the override (never silent).
    max_file_bytes = max(max_bytes + 1024, 1024 * 1024)

    worker_path = out_root / "worker.py"
    worker_path.write_text(WORKER_TEMPLATE, encoding="utf-8")
    worker_digest = sha_file(worker_path)
    repetitions = _int(args.repetitions, "repetitions")
    warmup_passes = _int(args.warmup_passes, "warmup_passes")
    seed = _int(args.seed, "seed")
    if repetitions <= 0 or warmup_passes < 0:
        raise AdapterError("repetitions must be positive and warmup_passes non-negative")
    task_ids = [task["task_id"] for task in pack["tasks"]]
    query_protocol = None
    if args.query_protocol is not None:
        query_protocol = validate_query_protocol(read_json(Path(args.query_protocol)), task_ids)
        if len(query_protocol["warmup_schedules"]) != warmup_passes:
            raise AdapterError("query protocol warmup count differs from CLI")
        if len(query_protocol["measurement_schedules"]) != repetitions:
            raise AdapterError("query protocol repetition count differs from CLI")
        if query_protocol["seed"] != seed:
            raise AdapterError("query protocol seed differs from CLI")
    spec = {
        "corpus_dir": str(corpus_dir),
        "tasks": [{"task_id": task["task_id"], "query": task["query"]} for task in pack["tasks"]],
        "top_k": top_k,
        "seed": seed,
        "warmup_passes": warmup_passes,
        "repetitions": repetitions,
        "query_protocol": query_protocol,
    }
    spec_path = out_root / "spec.json"
    spec_path.write_text(json.dumps(spec, indent=2, sort_keys=True), encoding="utf-8")
    native_path = out_root / "native.json"
    cache_root.mkdir(parents=True, exist_ok=True)
    env = dict(os.environ)
    env["SPEC_JSON"] = str(spec_path)
    env["NATIVE_JSON"] = str(native_path)
    env["SEMBLE_CACHE_LOCATION"] = str(cache_root / "semble")
    env["HF_HOME"] = str(cache_root / "hf")
    env["SEMBLE_MODEL_NAME"] = args.model_id
    env["SEMBLE_MAX_FILE_BYTES"] = str(max_file_bytes)
    try:
        completed = subprocess.run(
            [str(Path(args.python)), str(worker_path)],
            check=True,
            timeout=_int(args.timeout_secs, "timeout_secs"),
            env=env,
            capture_output=True,
            text=True,
        )
    except subprocess.TimeoutExpired as exc:
        raise AdapterError(f"Semble worker timed out: {exc}") from exc
    except (OSError, subprocess.CalledProcessError) as exc:
        tail = ""
        if isinstance(exc, subprocess.CalledProcessError):
            (out_root / "worker.stdout.log").write_text(exc.stdout or "", encoding="utf-8")
            (out_root / "worker.stderr.log").write_text(exc.stderr or "", encoding="utf-8")
            tail = (exc.stderr or "")[-2000:]
        raise AdapterError(f"Semble worker failed: {exc}\nstderr tail: {tail}") from exc
    (out_root / "worker.stdout.log").write_text(completed.stdout or "", encoding="utf-8")
    (out_root / "worker.stderr.log").write_text(completed.stderr or "", encoding="utf-8")
    native_payload = read_json(native_path)
    if not isinstance(native_payload, dict):
        raise AdapterError("Semble native output must be an object")
    if native_payload.get("configured_model_name") != args.model_id:
        raise AdapterError("Semble worker model configuration differs from requested model")
    observed = native_payload.get("observed_files", [])
    proof, diff_digest = mapping_proof(admitted_rows, observed, corpus_dir)
    (out_root / "mapping-proof.json").write_text(
        json.dumps(proof, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    if proof["skipped"] or proof["extra"] or proof["mismatched"]:
        raise AdapterError(
            "common-universe pair ineligible: "
            f"skipped={proof['skipped']} extra={proof['extra']} "
            f"mismatched={proof['mismatched']} "
            f"(see {out_root / 'mapping-proof.json'})"
        )

    # Source bytes for span proofs (pinned checkout, not the copied corpus).
    file_shas: dict[str, str] = {}
    file_lines: dict[str, list[bytes]] = {}
    for name, sha in admitted_rows:
        data = (repo / name).read_bytes()
        if hashlib.sha256(data).hexdigest() != sha:
            raise AdapterError(f"pinned source drifted during the run: {name}")
        file_shas[name] = sha
        file_lines[name] = data.splitlines(keepends=True)

    pack_canonical = json.dumps(
        json.loads(Path(args.query_pack).read_text(encoding="utf-8")),
        sort_keys=True,
        separators=(",", ":"),
        ensure_ascii=False,
    )
    pack_sha256 = digest(pack_canonical.encode("utf-8"))
    model_id = args.model_id
    model_revision, model_asset = resolve_model_revision(
        cache_root / "hf", model_id, args.model_revision
    )
    record = normalize_record(
        pack,
        pack_sha256,
        native_payload.get("native", []),
        native_payload.get("latencies_ms", {}),
        repo,
        file_shas,
        file_lines,
        pack["comparison_contract"],
        args.run_id,
        args.blinding,
        args.isolation_method,
        args.access_block_log,
        model_id,
        model_revision,
        args.route,
        args.run_id,
        diff_digest,
        worker_digest,
    )
    phase_values, worker_total_ms = validate_worker_phase_timings(
        native_payload, protocol=query_protocol is not None
    )
    phase_boundaries_ns = native_payload.get("phase_boundaries_ns")
    if not isinstance(phase_boundaries_ns, dict):
        raise AdapterError("Semble worker omitted monotonic phase boundaries")
    record_path = out_root / "record.json"
    record_path.write_text(json.dumps(record, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    phase_metrics = {
        "schema_version": 1,
        "system": "semble",
        "timing_layer": "worker_monotonic_wall_v1",
        "strategy": "native",
        "record_sha256": sha_file(record_path),
        "worker_sha256": worker_digest,
        "task_count": len(pack["tasks"]),
        "route_count": 1,
        "file_count": native_payload.get("stats", {}).get("indexed_files"),
        "chunk_count": native_payload.get("stats", {}).get("total_chunks"),
        "query_schedule": native_payload.get("query_schedule"),
        "warmup_passes": warmup_passes,
        "measurement_repetitions": repetitions,
        "phases_ms": phase_values,
        "phase_boundaries_ns": phase_boundaries_ns,
        "total_ms": worker_total_ms,
    }
    if query_protocol is not None:
        phase_metrics["query_protocol"] = query_protocol
        phase_metrics["warm_latencies_ms"] = {args.route: native_payload.get("latencies_ms")}
        phase_metrics["cold_latencies_ms"] = {args.route: native_payload.get("cold_latency_ms")}
    (out_root / "phase-metrics.json").write_text(
        json.dumps(phase_metrics, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    manifest_out = {
        "semble_version": semble_version,
        "semble_python": str(Path(args.python)),
        "interpreter": env_report["interpreter"],
        "worker_digest": worker_digest,
        "lockfile_digest": lockfile_digest,
        "observed_freeze_digest": env_report["observed_freeze_sha256"],
        "installed_distribution": env_report["installed_distribution"],
        "model_id": model_id,
        "model_revision": model_revision,
        "model_asset_digest": model_asset,
        "record_digest": sha_file(record_path),
        "timing_layer": "library",
        "semble_index_ms": native_payload.get("semble_index_ms"),
        "index_stats": native_payload.get("stats"),
        "repetitions": repetitions,
        "warmup_passes": warmup_passes,
        "seed": seed,
        "query_protocol_sha256": (query_protocol["sha256"] if query_protocol is not None else None),
        "semble_max_file_bytes": max_file_bytes,
        "path_sha_diff_digest": diff_digest,
        "top_k": top_k,
    }
    (out_root / "adapter-manifest.json").write_text(
        json.dumps(manifest_out, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    print(json.dumps(manifest_out, indent=2, sort_keys=True))
    return 0


def model_asset_digest(hf_home: Path, model_id: str, revision: str) -> str:
    """Digest every byte of the pinned model snapshot. Symlinks refused."""
    slug = "models--" + model_id.replace("/", "--")
    snapshot = hf_home / "hub" / slug / "snapshots" / revision
    if not snapshot.is_dir():
        raise AdapterError(f"model snapshot unavailable in HF cache: {model_id}@{revision}")
    digestor = hashlib.sha256()
    members = []
    for path in sorted(snapshot.rglob("*")):
        if path.is_symlink():
            raise AdapterError(f"model snapshot holds a symlink: {path}")
        if path.is_file():
            members.append(path)
    if not members:
        raise AdapterError(f"model snapshot holds no files: {model_id}@{revision}")
    for path in members:
        try:
            data = path.read_bytes()
        except OSError as exc:
            raise AdapterError(f"cannot read model asset {path}: {exc}") from exc
        digestor.update(path.relative_to(snapshot).as_posix().encode("utf-8"))
        digestor.update(b"\0")
        digestor.update(data)
        digestor.update(b"\0")
    return digestor.hexdigest()


def resolve_model_revision(hf_home: Path, model_id: str, pinned: str | None) -> tuple[str, str]:
    """Return (revision, model_asset_digest) for the pinned model snapshot."""
    observed = read_hf_revision(hf_home, model_id)
    if (
        observed is None
        or len(observed) != 40
        or any(c not in "0123456789abcdef" for c in observed)
    ):
        raise AdapterError(f"model revision unavailable or invalid in HF cache: {model_id}")
    if pinned and observed != pinned:
        raise AdapterError(f"model revision drift: pinned {pinned} but cache holds {observed}")
    revision = pinned or observed
    return revision, model_asset_digest(hf_home, model_id, revision)


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    sub = parser.add_subparsers(dest="command", required=True)
    check = sub.add_parser("check", help="verify the pinned Semble env")
    check.add_argument("--python", required=True)
    run = sub.add_parser("run", help="run Semble and emit record + mapping proof")
    run.add_argument("--repo", required=True)
    run.add_argument("--manifest", required=True)
    run.add_argument("--query-pack", required=True)
    run.add_argument("--top-k", required=True)
    run.add_argument("--python", required=True)
    run.add_argument("--lockfile", required=True)
    run.add_argument("--lockfile-sha256", required=True)
    run.add_argument("--cache-root", required=True)
    run.add_argument("--output-root", required=True)
    run.add_argument("--model-id", default="minishlab/potion-code-16M-v2")
    run.add_argument("--model-revision", default=None)
    run.add_argument("--route", default="semble-hybrid")
    run.add_argument("--run-id", required=True)
    run.add_argument("--blinding", required=True)
    run.add_argument("--isolation-method", required=True)
    run.add_argument("--access-block-log", required=True)
    run.add_argument("--seed", default="0")
    run.add_argument("--warmup-passes", default="1")
    run.add_argument("--repetitions", default="1")
    run.add_argument("--query-protocol", default=None)
    run.add_argument("--timeout-secs", default="1800")
    run.add_argument("--materialized-corpus", action="store_true")
    return parser


def main(argv: list[str] | None = None) -> int:
    parser = build_parser()
    args = parser.parse_args(argv)
    if args.command == "check":
        return cmd_check(args)
    return cmd_run(args)


if __name__ == "__main__":
    raise SystemExit(main())
