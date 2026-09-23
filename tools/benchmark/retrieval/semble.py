#!/usr/bin/env python3
"""Pinned Semble same-corpus comparison adapter (RB-04).

Runs a pinned Semble install (an outside-the-checkout virtualenv) against
the exact admitted file universe and blind query pack, then normalizes its
native results into a v2 runner record for the single Quanta-owned
evaluator. Semble ranking is never reimplemented here.

Layout contract (all outside the source checkout):
  <output-root>/
    corpus/                 # isolated corpus: admitted files only
    worker.py               # exact spawned worker (auditable)
    native.json             # Semble-native results + timings + observed files
    mapping-proof.json      # path map + both-side path+SHA diff
    record.json             # v2 runner record
    lockfile.txt            # pip freeze of the Semble env (+ digest)

A common-universe pair requires a clean mapping proof: every admitted file
observed in Semble's indexed chunks with matching bytes. Anything else is a
typed refusal, never a silent subset.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

try:
    from tools.benchmark.retrieval.evaluator import (
        TOKENIZER,
        TOKENIZER_BUDGET_VERSION,
        TOKEN_RE,
        digest,
    )
except ImportError:  # direct script invocation: import the sibling module
    sys.path.insert(0, str(Path(__file__).resolve().parent))
    from evaluator import (  # noqa: E402
        TOKENIZER,
        TOKENIZER_BUDGET_VERSION,
        TOKEN_RE,
        digest,
    )

WORKER_TEMPLATE = '''"""Spawned Semble worker (pinned env only). Reads SPEC_JSON, writes NATIVE_JSON."""
import json
import os
import sys
import time

def main() -> int:
    spec_path = os.environ["SPEC_JSON"]
    out_path = os.environ["NATIVE_JSON"]
    with open(spec_path, encoding="utf-8") as handle:
        spec = json.load(handle)
    from semble import SembleIndex

    t0 = time.monotonic()
    index = SembleIndex.from_path(spec["corpus_dir"], show_progress_bar=False)
    index_ms = (time.monotonic() - t0) * 1000.0
    observed = sorted({chunk.file_path for chunk in index.chunks})
    stats = {
        "indexed_files": int(index.stats.indexed_files),
        "total_chunks": int(index.stats.total_chunks),
        "languages": {str(k): int(v) for k, v in dict(index.stats.languages).items()},
    }
    queries = [(task["task_id"], task["query"]) for task in spec["tasks"]]
    top_k = int(spec["top_k"])
    seed = int(spec.get("seed", 0))
    warmup = int(spec.get("warmup_passes", 1))
    repetitions = int(spec.get("repetitions", 1))
    import random

    order = list(range(len(queries)))
    for _ in range(warmup):
        for task_id, query in queries:
            index.search(query, top_k=top_k)
    native = []
    latencies = {}
    for rep in range(repetitions):
        rng = random.Random(seed + rep)
        rng.shuffle(order)
        for position in order:
            task_id, query = queries[position]
            t0 = time.monotonic()
            results = index.search(query, top_k=top_k)
            elapsed_ms = (time.monotonic() - t0) * 1000.0
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
    payload = {
        "semble_index_ms": index_ms,
        "observed_files": observed,
        "stats": stats,
        "native": native,
        "latencies_ms": latencies,
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
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeError, json.JSONDecodeError) as exc:
        raise AdapterError(f"cannot read JSON {path}: {exc}") from exc


def sha_file(path: Path) -> str:
    digestor = hashlib.sha256()
    with path.open("rb") as handle:
        for block in iter(lambda: handle.read(65536), b""):
            digestor.update(block)
    return digestor.hexdigest()


def load_manifest(path: Path) -> tuple[str, list[tuple[str, str]]]:
    payload = read_json(path)
    if not isinstance(payload, dict):
        raise AdapterError("manifest must be an object")
    commit = payload.get("repository_commit")
    files = payload.get("files")
    if not isinstance(commit, str) or not commit:
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
        rows.append((name, sha))
    if len({name for name, _ in rows}) != len(rows):
        raise AdapterError("manifest holds duplicate paths")
    return commit, rows


def load_query_pack(path: Path) -> dict:
    payload = read_json(path)
    if not isinstance(payload, dict):
        raise AdapterError("query pack must be an object")
    if payload.get("schema_version") != 2:
        raise AdapterError("query pack schema_version must be 2")
    tasks = payload.get("tasks")
    if not isinstance(tasks, list) or not tasks:
        raise AdapterError("query pack holds no tasks")
    for task in tasks:
        if not isinstance(task, dict):
            raise AdapterError("query pack task must be an object")
        for key in ("task_id", "query", "query_sha256"):
            if not isinstance(task.get(key), str) or not task[key]:
                raise AdapterError(f"query pack task lacks {key}")
    return payload


def check_semble_env(python: Path) -> dict:
    """Verify the pinned Semble interpreter: version, imports, model id."""
    if not python.is_file():
        raise AdapterError(f"Semble python is not a file: {python}")
    probe = (
        "import importlib.metadata, json; "
        "from semble import SembleIndex; "
        "print(json.dumps({"
        "'semble_version': importlib.metadata.version('semble'), "
        "'has_from_path': hasattr(SembleIndex, 'from_path'), "
        "'has_search': hasattr(SembleIndex, 'search')}))"
    )
    try:
        completed = subprocess.run(
            [str(python), "-c", probe],
            check=True,
            capture_output=True,
            text=True,
            timeout=120,
        )
    except (OSError, subprocess.CalledProcessError) as exc:
        raise AdapterError(f"Semble env probe failed: {exc}") from exc
    try:
        report = json.loads(completed.stdout)
    except json.JSONDecodeError as exc:
        raise AdapterError(f"Semble env probe is not JSON: {exc}") from exc
    if not report.get("has_from_path") or not report.get("has_search"):
        raise AdapterError("pinned Semble lacks the from_path/search API")
    freeze = subprocess.run(
        [str(python), "-m", "pip", "freeze"],
        check=False,
        capture_output=True,
        text=True,
        timeout=120,
    )
    report["lockfile"] = freeze.stdout if freeze.returncode == 0 else ""
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
        if source.is_symlink() or not source.is_file():
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
    per_file = []
    for name, sha in sorted(admitted):
        if name not in observed_set:
            status = "skipped"
        else:
            status = "indexed" if (corpus_dir / name).is_file() else "unreadable"
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
    top_k: int,
    run_id: str,
    blinding: str,
    isolation_method: str,
    access_block_log: str,
    model: str,
    model_revision: str,
    route: str,
) -> dict:
    """Native Semble hits -> v2 runner record. Order preserved, spans proven."""
    by_task = {row["task_id"]: row.get("results", []) for row in native}
    results = []
    for task in pack["tasks"]:
        task_id = task["task_id"]
        if task_id not in by_task:
            results.append(
                {
                    "task_id": task_id,
                    "route": route,
                    "status": "error",
                    "candidates": [],
                    "timings": {"query_latency_ms": 0.0},
                    "error": {
                        "code": "semble_missing_query",
                        "message": "Semble emitted no row for this query",
                    },
                }
            )
            continue
        samples = latencies.get(task_id, [])
        latency = samples[0] if samples else 0.0
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
        for rank, hit in enumerate(hits, start=1):
            path = hit.get("file_path")
            start = hit.get("start_line")
            end = hit.get("end_line")
            if not isinstance(path, str) or path not in file_shas:
                raise AdapterError(f"Semble hit outside admitted universe: {path!r}")
            if (
                not isinstance(start, int)
                or not isinstance(end, int)
                or start < 1
                or end < start
            ):
                raise AdapterError(f"Semble hit has a bad span: {path}:{start}-{end}")
            lines = file_lines[path]
            if end > len(lines):
                raise AdapterError(f"Semble hit spans beyond EOF: {path}:{start}-{end}")
            block = b"".join(lines[start - 1 : end])
            try:
                text = block.decode("utf-8")
            except UnicodeDecodeError as exc:
                raise AdapterError(f"Semble hit block is not UTF-8: {path}") from exc
            tokens = count_tokens(text)
            if tokens == 0:
                raise AdapterError(f"Semble hit holds no tokens: {path}:{start}-{end}")
            candidates.append(
                {
                    "path": path,
                    "start_line": start,
                    "end_line": end,
                    "file_sha256": file_shas[path],
                    "block_sha256": hashlib.sha256(block).hexdigest(),
                    "tokens": tokens,
                    "rank": rank,
                }
            )
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
    return {
        "schema_version": 2,
        "query_pack_sha256": pack_sha256,
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
        "route_provenance": {
            route: {
                "system": "semble",
                "model": model,
                "model_revision": model_revision,
            }
        },
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


def run_adapter(args: argparse.Namespace) -> int:
    repo = Path(args.repo)
    out_root = Path(args.output_root)
    if out_root.exists():
        raise AdapterError(f"output root already exists (refusing reuse): {out_root}")
    out_root.mkdir(parents=True)
    commit, manifest_rows = load_manifest(Path(args.manifest))
    pack = load_query_pack(Path(args.query_pack))
    if pack.get("repository_commit") != commit:
        raise AdapterError("manifest commit differs from query-pack commit")
    top_k = _int(args.top_k, "top_k")
    if top_k <= 0:
        raise AdapterError("top_k must be positive")
    if args.blinding not in ("isolated", "attested"):
        raise AdapterError("blinding must be isolated or attested")

    env_report = check_semble_env(Path(args.python))
    semble_version = env_report["semble_version"]
    lockfile = out_root / "lockfile.txt"
    lockfile.write_text(env_report.get("lockfile", ""), encoding="utf-8")
    lockfile_digest = sha_file(lockfile)

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
    spec = {
        "corpus_dir": str(corpus_dir),
        "tasks": [
            {"task_id": task["task_id"], "query": task["query"]} for task in pack["tasks"]
        ],
        "top_k": top_k,
        "seed": seed,
        "warmup_passes": warmup_passes,
        "repetitions": repetitions,
    }
    spec_path = out_root / "spec.json"
    spec_path.write_text(json.dumps(spec, indent=2, sort_keys=True), encoding="utf-8")
    native_path = out_root / "native.json"
    cache_root = Path(args.cache_root)
    cache_root.mkdir(parents=True, exist_ok=True)
    env = dict(os.environ)
    env["SPEC_JSON"] = str(spec_path)
    env["NATIVE_JSON"] = str(native_path)
    env["SEMBLE_CACHE_LOCATION"] = str(cache_root / "semble")
    env["HF_HOME"] = str(cache_root / "hf")
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
    observed = native_payload.get("observed_files", [])
    proof, diff_digest = mapping_proof(admitted_rows, observed, corpus_dir)
    (out_root / "mapping-proof.json").write_text(
        json.dumps(proof, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    if proof["skipped"] or proof["extra"]:
        raise AdapterError(
            "common-universe pair ineligible: "
            f"skipped={proof['skipped']} extra={proof['extra']} "
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
    model_revision = resolve_model_revision(cache_root / "hf", model_id, args.model_revision)
    record = normalize_record(
        pack,
        pack_sha256,
        native_payload.get("native", []),
        native_payload.get("latencies_ms", {}),
        repo,
        file_shas,
        file_lines,
        top_k,
        args.run_id,
        args.blinding,
        args.isolation_method,
        args.access_block_log,
        model_id,
        model_revision,
        args.route,
    )
    (out_root / "record.json").write_text(
        json.dumps(record, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    manifest_out = {
        "semble_version": semble_version,
        "semble_python": str(Path(args.python)),
        "worker_digest": worker_digest,
        "lockfile_digest": lockfile_digest,
        "model_id": model_id,
        "model_revision": model_revision,
        "timing_layer": "library",
        "semble_index_ms": native_payload.get("semble_index_ms"),
        "index_stats": native_payload.get("stats"),
        "repetitions": repetitions,
        "warmup_passes": warmup_passes,
        "seed": seed,
        "semble_max_file_bytes": max_file_bytes,
        "path_sha_diff_digest": diff_digest,
        "top_k": top_k,
    }
    (out_root / "adapter-manifest.json").write_text(
        json.dumps(manifest_out, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    print(json.dumps(manifest_out, indent=2, sort_keys=True))
    return 0


def resolve_model_revision(hf_home: Path, model_id: str, pinned: str | None) -> str:
    observed = read_hf_revision(hf_home, model_id)
    if pinned:
        if observed is not None and observed != pinned:
            raise AdapterError(
                f"model revision drift: pinned {pinned} but cache holds {observed}"
            )
        return pinned
    if observed is None:
        return "unresolved"
    return observed


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
    run.add_argument("--timeout-secs", default="1800")
    return parser


def main(argv: list[str] | None = None) -> int:
    parser = build_parser()
    args = parser.parse_args(argv)
    if args.command == "check":
        return cmd_check(args)
    return cmd_run(args)


if __name__ == "__main__":
    raise SystemExit(main())
