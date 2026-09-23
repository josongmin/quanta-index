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
import shutil
import subprocess
import sys
import time
from pathlib import Path

try:
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
except ImportError:  # direct script invocation: import the sibling module
    sys.path.insert(0, str(Path(__file__).resolve().parent))
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

VERDICT_VERSION = 2
MANIFEST_VERSION = 1
PILOT_OBSERVATIONS_FLOOR = 1000
FRESH_ROOTS_FLOOR = 5
RUNNABLE_STRATEGIES = tuple(s for s in CHUNK_STRATEGIES if s != "semble_native")
SEMBLE_PINNED_VERSION = "0.6.0"


class RunError(ValueError):
    """Paired-run evidence is absent, inconsistent or ineligible."""


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
        record["rustc"] = (
            completed.stdout.strip() if completed.returncode == 0 else "unavailable"
        )
    except (OSError, subprocess.SubprocessError):
        record["rustc"] = "unavailable"
    record["concurrent_processes"] = find_competing_processes()
    record["thermal"] = read_thermal()
    record["frequency"] = read_frequency()
    return record


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
        return read_sysctl(
            ["machdep.cpu.brand_string", "machdep.cpu.thermal_state", "hw.cpufrequency"]
        )
    if sys.platform.startswith("linux"):
        out: dict = {}
        for zone in sorted(Path("/sys/class/thermal").glob("thermal_zone*/temp")):
            try:
                out[zone.parent.name] = zone.read_text().strip()
            except OSError:
                continue
        return out or {"zones": "unavailable"}
    return {"platform": "unsupported"}


def read_frequency() -> dict:
    if sys.platform == "darwin":
        return read_sysctl(["hw.cpufrequency", "hw.cpufrequency_max"])
    if sys.platform.startswith("linux"):
        out: dict = {}
        for node in sorted(
            Path("/sys/devices/system/cpu").glob("cpu[0-9]*/cpufreq/scaling_cur_freq")
        ):
            try:
                out[node.parent.parent.name] = node.read_text().strip()
            except OSError:
                continue
        return out or {"scaling": "unavailable"}
    return {"platform": "unsupported"}


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


def project_pack_and_suite(
    pack: dict, suite: dict, routes: list[str]
) -> tuple[dict, dict]:
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
    return json.dumps(
        payload, sort_keys=True, separators=(",", ":"), ensure_ascii=False
    ).encode("utf-8")


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
                "merged records disagree on the comparison contract: "
                f"{differing}; refusing merge"
            )
    merged_blinding = (
        "isolated" if all(r["runner"].get("blinding") == "isolated" for r in runners) else "attested"
    )
    expected_routes = set(suite["routes"])
    if set(provenance) != expected_routes:
        missing = sorted(expected_routes - set(provenance))
        extra = sorted(set(provenance) - expected_routes)
        raise RunError(f"merged routes {sorted(provenance)} != suite routes "
                       f"(missing={missing} extra={extra})")
    ordered = [results[key] for key in sorted(results)]
    runners.sort(key=lambda entry: entry["path"])
    content_digests = sorted(
        digest(canonical_bytes(read_json(path))) for path in record_paths
    )
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
    "semble_repetitions",
    "baseline_route",
    "candidate_route",
    "host_profile",
    "claims",
    "receipts",
    "contention_override",
)
RECEIPT_KEYS = (
    "contract_python_receipt",
    "contract_python_results",
    "contract_rust_receipt",
    "contract_rust_results",
    "sdk_receipt",
    "sdk_results",
    "model_parity_results",
    "incremental_results",
)


def _is_hex(value: object, length: int) -> bool:
    return (
        isinstance(value, str)
        and len(value) == length
        and all(c in "0123456789abcdef" for c in value)
    )


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
    for key in ("repo", "manifest", "suite", "query_pack", "output_root", "runner_binary",
                "searchd_binary"):
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
        unknown_entry = sorted(set(entry) - {"name", "window_bytes", "overlap_bytes",
                                              "max_item_bytes"})
        if unknown_entry:
            raise RunError(f"strategy has unknown keys: {unknown_entry}")
        if entry.get("name") not in RUNNABLE_STRATEGIES:
            raise RunError(f"unknown strategy: {entry.get('name')}")
        for key, minimum in (("window_bytes", 1), ("overlap_bytes", 0), ("max_item_bytes", 1)):
            if key in entry and (
                type(entry[key]) is not int or isinstance(entry[key], bool)
                or entry[key] < minimum
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
        "true_process_cold", "warm_cache", "undeclared",
    ):
        raise RunError("spec.cache_regime must be true_process_cold, warm_cache or undeclared")
    for key, minimum in (("generation", 0), ("seed", 0), ("timeout_secs", 1),
                         ("repetitions", 1), ("semble_repetitions", 1)):
        if key in spec:
            _spec_int(spec, key, minimum)
    if "alternate_order" in spec and type(spec["alternate_order"]) is not bool:
        raise RunError("spec.alternate_order must be a boolean")
    for key in ("isolation_method", "access_block_log", "run_id", "runner_name", "repo_id",
                "revision_id", "semble_route", "semble_python", "semble_cache_root",
                "semble_lockfile", "baseline_route", "candidate_route", "host_profile"):
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
    if "contention_override" in spec and type(spec["contention_override"]) is not bool:
        raise RunError("spec.contention_override must be a boolean")
    return spec


def preflight_capture(spec: dict) -> Path:
    """Refuse dirty/wrong-HEAD inputs, contract drift and unpinned binaries."""
    manifest = read_json(Path(spec["manifest"]))
    if not isinstance(manifest, dict) or not isinstance(manifest.get("repository_commit"), str):
        raise RunError("manifest must pin repository_commit")
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


def write_projected_pack(
    pack_path: Path, suite_path: Path, routes: list[str], out: Path
) -> Path:
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
    if spec.get("blinding", "attested") != "attested":
        raise RunError(
            "the Rust CLI proves no process isolation: only blinding=attested is "
            "allowed (isolated needs sandboxed runner support)"
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
    command = [
        spec["runner_binary"],
        "run",
        "--repo", spec["repo"],
        "--manifest", spec["manifest"],
        "--query-pack", str(pack_path),
        "--strategy", name,
        "--routes", ",".join(routes),
        "--top-k", str(spec["top_k"]),
        "--state-root", str(state_root),
        "--repo-id", spec.get("repo_id", "bench-repo"),
        "--revision-id", spec.get("revision_id", "bench-rev"),
        "--generation", str(spec.get("generation", 7)),
        "--embedder", spec.get("embedder", "potion-code"),
        "--runner-name", spec.get("runner_name", "quanta-sdk-runner"),
        "--runner-revision", f"sha256:{runner_binary_sha256}",
        "--run-id", f"{spec.get('run_id', 'run')}-{name}",
        "--blinding", "attested",
        "--isolation-method", spec.get("isolation_method", "attested-only: same-checkout pack consumer"),
        "--access-block-log", spec.get("access_block_log", "attested-only: no suite path is passed to the runner; pack blindness verified by freeze"),
        "--out", str(record_path),
    ]
    command += ["--searchd-bin", spec["searchd_binary"]]
    command += ["--searchd-expected-sha256", spec["searchd_expected_sha256"]]
    for key, flag in (
        ("window_bytes", "--window-bytes"),
        ("overlap_bytes", "--overlap-bytes"),
        ("max_item_bytes", "--max-item-bytes"),
    ):
        if key in strategy:
            command += [flag, str(strategy[key])]
    started = time.monotonic()
    try:
        completed = subprocess.run(
            command,
            capture_output=True,
            text=True,
            timeout=_int(spec.get("timeout_secs", 1800), "spec.timeout_secs"),
        )
    except subprocess.TimeoutExpired as exc:
        raise RunError(f"Rust runner timed out for {name}: {exc}") from exc
    elapsed_ms = (time.monotonic() - started) * 1000.0
    (run_dir / "runner.stdout.log").write_text(completed.stdout, encoding="utf-8")
    (run_dir / "runner.stderr.log").write_text(completed.stderr, encoding="utf-8")
    if completed.returncode != 0:
        raise RunError(
            f"Rust runner failed for {name} (exit {completed.returncode}): "
            f"{completed.stderr[-2000:]}"
        )
    index_bytes = tree_size(state_root)
    return {
        "strategy": name,
        "strategy_config": strategy,
        # Rename-safe: paths stay relative to the quanta output root so a
        # staged tree can be atomically promoted without rebinding.
        "record": record_path.relative_to(out_abs).as_posix(),
        "record_digest": sha_file(record_path),
        "runner_binary_sha256": runner_binary_sha256,
        "driver_ms": elapsed_ms,
        "index_bytes": index_bytes,
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
        {"manifest_version", "blinding", "isolation_method", "access_block_log",
         "scope", "claims", "repetitions", "evidence", "host", "artifacts",
         "provenance"},
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
        manifest["claims"], {"quality", "speed", "same_model", "incremental"},
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
    if not {"pair", "perf"} <= set(evidence) <= {
        "pair", "perf", "contract_suites", "sdk_path", "model_parity", "incremental"
    }:
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
            claim = _exact_keys(suites[side], {"test_result_digest"}, f"manifest {side} claim")
            if not _is_hex(claim["test_result_digest"], 64):
                raise RunError(f"manifest {side} test_result_digest must be a lowercase sha256")
    if "sdk_path" in evidence:
        sdk = _exact_keys(
            evidence["sdk_path"],
            {"test_result_digest", "separate_process", "sealed_receipt", "activation_ack",
             "empty_check"},
            "manifest sdk evidence",
        )
        if not _is_hex(sdk["test_result_digest"], 64):
            raise RunError("manifest sdk test_result_digest must be a lowercase sha256")
        for key in ("separate_process", "sealed_receipt", "activation_ack", "empty_check"):
            if type(sdk[key]) is not bool:
                raise RunError(f"manifest sdk {key} must be a strict boolean")
    for key in ("model_parity", "incremental"):
        if key in evidence:
            claim = _exact_keys(evidence[key], {"test_result_digest"}, f"manifest {key} claim")
            if not _is_hex(claim["test_result_digest"], 64):
                raise RunError(f"manifest {key} test_result_digest must be a lowercase sha256")
    host = _exact_keys(
        manifest["host"], {"start_digest", "end_digest", "cache_regime"}, "manifest host")
    for key in ("start_digest", "end_digest"):
        if not _is_hex(host[key], 64):
            raise RunError(f"manifest host {key} must be a lowercase sha256")
    if host["cache_regime"] not in ("true_process_cold", "warm_cache", "undeclared"):
        raise RunError("manifest host cache_regime must be a frozen regime")
    artifacts = manifest["artifacts"]
    if not isinstance(artifacts, dict):
        raise RunError("run manifest artifacts must be an object")
    required_artifacts = {"suite", "query_pack", "corpus_manifest", "mapping_proof",
                          "latency_matrix", "host_start", "host_end", "records",
                          "reports", "quanta_manifests", "semble_adapter_manifest",
                          "semble_lockfile", "semble_native", "protocol_lock"}
    if not required_artifacts <= set(artifacts) <= required_artifacts | set(RECEIPT_KEYS):
        raise RunError("run manifest artifacts hold missing/unknown keys")
    for key in required_artifacts | set(RECEIPT_KEYS):
        if key not in artifacts:
            continue
        value = artifacts[key]
        if key in ("records", "reports", "quanta_manifests", "semble_native"):
            if not isinstance(value, list) or not all(
                isinstance(ref, str) and ref for ref in value
            ):
                raise RunError(f"manifest artifacts.{key} must be a path list")
        elif not isinstance(value, str) or not value:
            raise RunError(f"manifest artifacts.{key} must be a nonempty path")
    if not artifacts["records"]:
        raise RunError("run manifest artifacts.records must be nonempty")
    provenance = _exact_keys(
        manifest["provenance"], {"quanta", "semble", "corpus", "suite", "host"},
        "run manifest provenance",
    )
    quanta = _exact_keys(
        provenance["quanta"], {"source_sha", "binary_digest", "embedder"}, "manifest quanta")
    if not _is_hex(quanta["source_sha"], 40) or not _is_hex(quanta["binary_digest"], 64):
        raise RunError("manifest quanta provenance digests malformed")
    if quanta["embedder"] not in ("potion-code", "hash-dev"):
        raise RunError("manifest quanta embedder must be a frozen embedder")
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
        provenance["suite"], {"suite_digest", "query_pack_digest", "tokenizer_budget_version"},
        "manifest suite",
    )
    for key in ("suite_digest", "query_pack_digest"):
        if not _is_hex(suite_prov[key], 64):
            raise RunError(f"manifest suite {key} must be a lowercase sha256")
    if suite_prov["tokenizer_budget_version"] != TOKENIZER_BUDGET_VERSION:
        raise RunError("manifest suite tokenizer budget version mismatch")
    host_prov = _exact_keys(
        provenance["host"], {"profile", "check_record_digest"}, "manifest host provenance"
    )
    if not isinstance(host_prov["profile"], str) or not host_prov["profile"]:
        raise RunError("manifest host profile must be a nonempty string")
    if not _is_hex(host_prov["check_record_digest"], 64):
        raise RunError("manifest host check_record_digest must be a lowercase sha256")
    return manifest


def _validate_single_record(
    repo: Path, suite: dict, pack: dict, path: Path
) -> dict:
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


def _probe_clean(probe: object) -> bool:
    return (
        isinstance(probe, dict)
        and probe.get("concurrent_processes", {}) in ({}, {"none": []})
        and probe.get("contention_override") is not True
    )


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
    for key in ("suite", "query_pack", "corpus_manifest", "mapping_proof",
                "latency_matrix", "host_start", "host_end",
                "semble_adapter_manifest", "semble_lockfile", "protocol_lock"):
        resolved[key] = _resolve_artifact(root, artifacts[key], f"artifacts.{key}")
    for key in ("records", "reports", "quanta_manifests", "semble_native"):
        resolved[key] = [
            _resolve_artifact(root, ref, f"artifacts.{key}") for ref in artifacts[key]
        ]
    for key in RECEIPT_KEYS:
        if key in artifacts:
            resolved[key] = _resolve_artifact(root, artifacts[key], f"artifacts.{key}")
    try:
        cli_suite_bytes = suite_path.read_bytes()
    except OSError as exc:
        raise RunError(f"cannot read CLI suite: {exc}") from exc
    if digest(cli_suite_bytes) != sha_file(resolved["suite"]):
        raise RunError("CLI suite differs from the frozen manifest suite")

    evidence = manifest["evidence"]
    claims = manifest["claims"]
    provenance_claims = manifest["provenance"]
    pair_notes: list[tuple[str, list[str], str]] = []

    def pair_note(reason: str, t_ids: tuple[str, ...] = (), fail_class: str = "provenance") -> None:
        pair_notes.append((reason, list(t_ids), fail_class))

    def read_note(path: Path, where: str, t_ids: tuple[str, ...]) -> object:
        try:
            return read_json(path)
        except ValueError as exc:
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
    if isinstance(corpus_payload, dict) and isinstance(mapping_payload, dict):
        if not mapping_matches_manifest(mapping_payload, corpus_payload):
            pair_note("mapping_proof_not_clean", ("T00", "T11", "T12"), "corpus_mismatch")
        if mapping_payload.get("diff_digest") != provenance_claims["corpus"]["path_sha_diff_digest"]:
            pair_note("path_sha_diff_digest_mismatch", ("T00", "T11"))
        pack_commit = pack_payload.get("repository_commit") if isinstance(pack_payload, dict) else None
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
                    repo, resolved["suite"],
                    [Path(quanta_by_strategy[strategy]), Path(semble_rep0)],
                )
            except (RunError, ValueError) as exc:
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
        comparison = content.get("rank_metrics", {}).get("comparison", {}) \
            if isinstance(content.get("rank_metrics"), dict) else {}
        try:
            rescored = evaluate(
                suite, pack, merged, comparison.get("baseline"), comparison.get("candidate")
            )
        except (RunError, ValueError, TypeError) as exc:
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
        matched.append({
            "strategy": strategy,
            "baseline_route": rank_comparison["baseline"],
            "candidate_route": rank_comparison["candidate"],
            "primary_metric": rank_comparison["primary_metric"],
            "primary_delta": primary_delta,
            "record_digest": merged_digest,
            "report_digest": report_digest,
            "graded": bool(rescored.get("graded")),
            "report_sha": digest(canonical(content)),
        })
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
    resource_ok = True
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
        if adapter.get("interpreter", {}).get("digest") != provenance_claims["semble"]["interpreter_digest"]:
            pair_note("interpreter_digest_mismatch", ("T11",))
        if adapter.get("model_asset_digest") != provenance_claims["semble"]["model_asset_digest"]:
            pair_note("model_asset_digest_mismatch", ("T11",))
    mapping_diff = mapping_payload.get("diff_digest") if isinstance(mapping_payload, dict) else None
    for path, entry in validated.items():
        if entry["system"] != "semble":
            continue
        _cid, capture = next(iter(entry["run"]["captures"].items()))
        if capture.get("receipt_digest") != mapping_diff:
            pair_note("semble_receipt_anchor_drift", ("T11", "T12"))

    quanta_binaries = set()
    for path, entry in validated.items():
        if entry["system"] == "quanta":
            _cid, capture = next(iter(entry["run"]["captures"].items()))
            quanta_binaries.add(capture.get("runner_binary", {}).get("digest"))
    binary_digest = sorted(quanta_binaries)[0] if quanta_binaries else "0" * 64
    if len(quanta_binaries) != 1:
        pair_note("runner_binary_divergence", ("T12",))
    elif binary_digest != provenance_claims["quanta"]["binary_digest"]:
        pair_note("binary_digest_mismatch", ("T12",))

    # T12: error/timeout/unavailable rows are incomplete observations — the
    # pair never silently compares a system that failed to observe a query.
    for path, entry in validated.items():
        rows = entry["run"].get("results", [])
        bad = sorted({row["task_id"] for row in rows
                      if row.get("status") in ("error", "timeout", "unavailable")})
        if bad:
            pair_note(
                f"incomplete_observation:{entry['rep']}:{entry['system']}"
                f":{entry['strategy']}:{','.join(bad)}",
                ("T12",),
            )

    pair_t_ids: list[str] = []
    for _reason, t_ids, _class in pair_notes:
        pair_t_ids.extend(t_ids)
    pair_proof = digest(canonical({
        "mapping": mapping_payload,
        "reports": sorted(entry["report_sha"] for entry in matched),
    })) if not pair_notes else None
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

    contract_ids = ["T01", "T02", "T03", "T04", "T08", "T09"]
    if "contract_suites" not in evidence:
        set_state("CONTRACT_GREEN", "not_run", "no_evidence", None)
        missing.extend(contract_ids)
    else:
        try:
            proofs: dict[str, dict] = {}
            for side, marker in (("python", "test_retrieval_benchmark"),
                                 ("rust", "quanta-index-retrieval-bench")):
                receipt_ref = f"contract_{side}_receipt"
                results_ref = f"contract_{side}_results"
                if receipt_ref not in resolved or results_ref not in resolved:
                    raise RunError(f"contract {side} artifacts missing")
                receipt = _validate_receipt_shape(read_json(resolved[receipt_ref]), f"contract {side} receipt")
                results = _validate_counts_shape(read_json(resolved[results_ref]), f"contract {side} results")
                actual = sha_file(resolved[results_ref])
                if actual != receipt["evidence_sha256"]:
                    raise RunError(f"contract {side} receipt digest mismatch")
                if actual != evidence["contract_suites"][side]["test_result_digest"]:
                    raise RunError(f"contract {side} manifest digest mismatch")
                if marker not in receipt["command"] or marker not in results["command"]:
                    raise RunError(f"contract {side} command mismatch")
                if not suite["repository_commit"].startswith(receipt["revision"]):
                    raise RunError(f"contract {side} revision mismatch")
                if not (results["failed"] == 0 and results["passed"] > 0
                        and results["passed"] + results["failed"] == results["executed"]
                        and results["executed"] <= results["selected"]):
                    raise RunError(f"contract {side} counts inconsistent")
                proofs[side] = results
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
            sdk_results = _validate_sdk_results_shape(read_json(resolved["sdk_results"]), "sdk results")
            sdk_actual = sha_file(resolved["sdk_results"])
            if sdk_actual != sdk_receipt["evidence_sha256"]:
                raise RunError("sdk receipt digest mismatch")
            if sdk_actual != evidence["sdk_path"]["test_result_digest"]:
                raise RunError("sdk manifest digest mismatch")
            if "retrieval-sdk-proof" not in sdk_receipt["command"]:
                raise RunError("sdk receipt command mismatch")
            if "retrieval-sdk-proof" not in sdk_results["command"]:
                raise RunError("sdk results command mismatch")
            if not suite["repository_commit"].startswith(sdk_receipt["revision"]):
                raise RunError("sdk revision mismatch")
            claimed = evidence["sdk_path"]
            for key in ("separate_process", "empty_check"):
                if claimed[key] is not True or sdk_results[key] is not True:
                    raise RunError(f"sdk {key} not proven")
            if claimed["sealed_receipt"] is not True or claimed["activation_ack"] is not True:
                raise RunError("sdk receipt/ack not claimed")
            if sdk_results["binary_digest"] != binary_digest:
                raise RunError("sdk binary differs from pair captures")
            if not (sdk_results["failed"] == 0 and sdk_results["passed"] > 0
                    and sdk_results["passed"] + sdk_results["failed"] == sdk_results["executed"]
                    and sdk_results["executed"] <= sdk_results["selected"]):
                raise RunError("sdk counts inconsistent")
        except (RunError, ValueError, OSError) as exc:
            set_state("SDK_PATH_GREEN", "fail", f"sdk_refused: {exc}", None)
            missing.extend(sdk_ids)
            classes.append("provenance")
        else:
            set_state("SDK_PATH_GREEN", "pass", "sdk_proof_verified", digest(canonical(sdk_results)))

    set_state("PAIR_VALID", pair_state, pair_reason, pair_proof)
    if pair_state == "fail":
        missing.extend(pair_t_ids)
        classes.append(pair_class)

    # PERF_QUALIFIED: matrix re-derivation + floors + host, only on speed claims.
    if not claims["speed"]:
        set_state("PERF_QUALIFIED", "not_applicable", "no_speed_claim", None)
    else:
        perf_fail: tuple[str, str] | None = None
        try:
            cells = []
            for rep in sorted(rep_records, key=_rep_sort_key):
                quanta_paths = sorted(
                    (p for p in rep_records[rep] if validated[p]["system"] == "quanta"),
                    key=lambda p: (validated[p]["strategy"], p),
                )
                for path in quanta_paths:
                    entry = validated[path]
                    cells.append(_cell_from_record("quanta", entry["strategy"], entry["run"]))
                semble_paths = [p for p in rep_records[rep] if validated[p]["system"] == "semble"]
                if len(semble_paths) != 1:
                    raise RunError(f"rep {rep} lacks exactly one semble record")
                native_paths = native_reps.get(rep, [])
                if len(native_paths) != 1:
                    raise RunError(f"rep {rep} lacks exactly one native file")
                native_content = read_json(Path(native_paths[0]))
                if not isinstance(native_content, dict):
                    raise RunError(f"rep {rep} native output is not an object")
                scell = _cell_from_record("semble", "native", validated[semble_paths[0]]["run"])
                routes = {row["route"] for row in validated[semble_paths[0]]["run"].get("results", [])}
                if len(routes) != 1:
                    raise RunError("semble record must carry exactly one route")
                scell["native_latencies"] = native_content.get("latencies_ms", {})
                scell["native_route"] = next(iter(routes))
                cells.append(scell)
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
                and _probe_clean(host_start_payload)
                and _probe_clean(host_end_payload)
            ):
                perf_fail = ("host_contended", "host")
            elif manifest.get("host", {}).get("cache_regime", "undeclared") == "undeclared":
                # T12: a speed claim without a declared cache regime (true
                # process-cold vs warm cache) cannot be compared or repeated.
                perf_fail = ("cache_regime_undeclared", "host")
            else:
                # No runner emits phase fragments yet, so no speed claim
                # can pass until phase artifacts exist and are verified.
                # Everything above held; the frontier is explicit.
                perf_fail = ("phases_unimplemented", "provenance")
        if perf_fail is None:  # pragma: no cover - chain above is exhaustive
            raise RunError("verdict invariant broken: perf undecided")
        set_state("PERF_QUALIFIED", "fail", perf_fail[0], None)
        classes.append(perf_fail[1])

    # QUALITY_DELTA: blinded, graded, in-scope quality only.
    all_isolated = manifest["blinding"] == "isolated"
    if all_isolated:
        for path in resolved["records"]:
            try:
                payload = read_json(path)
                blinding = payload.get("runner", {}).get("blinding") if isinstance(payload, dict) else None
            except ValueError:
                blinding = None
            if blinding != "isolated":
                all_isolated = False
                break
    if not claims["quality"]:
        set_state("QUALITY_DELTA", "not_applicable", "no_quality_claim", None)
    elif not matched:
        set_state("QUALITY_DELTA", "not_run", "reports_unmatched", None)
    elif provenance_claims["quanta"].get("embedder") != "potion-code":
        # T10: a quality claim over the hash-dev diagnostic control (or an
        # undeclared embedder) is not model-quality evidence.
        set_state("QUALITY_DELTA", "fail", "model_quality_embedder", None)
        classes.append("model")
    elif not all_isolated:
        set_state("QUALITY_DELTA", "not_applicable", "attested_only", None)
        not_applicable.append("blinding:attested_only")
    elif manifest["scope"] != "qualified":
        set_state("QUALITY_DELTA", "not_applicable", "exploratory_only", None)
        not_applicable.append("scope:exploratory_only")
    elif not all(entry["graded"] for entry in matched):
        set_state("QUALITY_DELTA", "fail", "reports_ungraded", None)
        classes.append("scoring")
    else:
        set_state(
            "QUALITY_DELTA", "pass", "blinded_graded_delta",
            digest(canonical(sorted(entry["report_sha"] for entry in matched))),
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
            if not (results["status"] == "pass" and results["failed"] == 0
                    and results["executed"] >= 1
                    and results["passed"] + results["failed"] == results["executed"]):
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
        row for path, entry in validated.items()
        if entry["rep"] == "rep-00" for row in entry["run"].get("results", [])
    ]
    counts = {
        "selected": len(rep0_rows),
        "executed": len(total_rows),
        "passed": sum(1 for row in total_rows if row.get("status") in ("success", "capped")),
        "failed": sum(1 for row in total_rows if row.get("status") not in ("success", "capped", "abstained")),
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
        for entry in sorted(matched, key=lambda e: (e["strategy"], e["baseline_route"], e["candidate_route"]))
    ]
    diff_digest = mapping_diff if _is_hex(mapping_diff, 64) else "0" * 64
    provenance = {
        "quanta": {"source_sha": suite["repository_commit"], "binary_digest": binary_digest,
                   "embedder": provenance_claims["quanta"].get("embedder", "undeclared")},
        "semble": {"revision": SEMBLE_PINNED_VERSION, "lockfile_digest": lockfile_digest or "0" * 64},
        "corpus": {"digest": corpus_digest or "0" * 64, "path_sha_diff_digest": diff_digest},
        "suite": {
            "suite_digest": suite_digest or "0" * 64,
            "query_pack_digest": pack_digest or "0" * 64,
            "tokenizer_budget_version": TOKENIZER_BUDGET_VERSION,
        },
        "host": {
            "profile": provenance_claims["host"]["profile"],
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


def run_pair(spec: dict) -> int:
    """Sequential Quanta + Semble capture with merged scoring and verdict.

    External repetitions re-run both systems on fresh state (fresh index
    samples); system order alternates per repetition unless disabled.
    Quality merges rep-0 records; every rep feeds the latency matrix.
    Everything builds in a sibling staging directory; only a complete
    tree (manifest + verdict) is atomically renamed onto the output
    root. Partial output is never resumed: rerun from a fresh root.
    """
    lockfile_sha = spec.get("semble_lockfile_sha256")
    if not _is_hex(lockfile_sha, 64):
        raise RunError("pair requires a pinned semble_lockfile_sha256")
    if not spec.get("semble_python"):
        raise RunError("pair requires spec.semble_python")
    if not spec.get("semble_lockfile"):
        raise RunError("pair requires spec.semble_lockfile naming the hash-pinned lockfile")
    if not spec.get("host_profile"):
        raise RunError("pair requires spec.host_profile naming the canonical host profile")
    out_root = preflight_capture(spec)
    stage = out_root.parent / (out_root.name + ".staging")
    if out_root.exists() or stage.exists():
        raise RunError("output root or staging dir already exists (refusing reuse)")
    stage.mkdir(parents=True)
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
    frozen_inputs = freeze_inputs(spec, stage)
    frozen_receipts = freeze_receipts(spec, stage)
    spec = dict(spec, **frozen_inputs)
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
        layout: dict = {"rep": rep, "order": rep_order, "quanta": {}, "semble": ""}
        for system in rep_order:
            if system == "quanta":
                quanta_out = rep_dir / "quanta"
                quanta_spec = dict(spec)
                quanta_spec["output_root"] = str(quanta_out)
                quanta_spec["run_id"] = f"{spec.get('run_id', 'run')}-r{rep}"
                if run_quanta(quanta_spec, Path(".")) != 0:
                    raise RunError(f"quanta capture failed at rep {rep}")
                quanta_manifest_path = quanta_out / "quanta-manifest.json"
                quanta_manifest = read_json(quanta_manifest_path)
                if not isinstance(quanta_manifest, dict):
                    raise RunError("quanta manifest is not an object")
                for run in quanta_manifest["runs"]:
                    layout["quanta"][run["strategy"]] = str(quanta_out / run["record"])
                layout["quanta_manifest"] = str(quanta_manifest_path)
            else:
                semble_out = rep_dir / "semble"
                run_semble_capture(
                    semble_spec, semble_out, semble_pack, semble_routes[0], rep=rep
                )
                layout["semble"] = str(semble_out / "record.json")
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
    repo = Path(spec["repo"])
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
    protocol_lock = {
        "suite_digest": sha_file(stage / "suite.json"),
        "query_pack_digest": sha_file(stage / "query-pack.json"),
        "corpus_manifest_digest": sha_file(stage / "corpus-manifest.json"),
        "spec_digest": digest(canonical_bytes(spec)),
        "top_k": spec["top_k"],
        "strategies": [entry["name"] for entry in spec["strategies"]],
        "searchd_expected_sha256": spec["searchd_expected_sha256"],
        "semble_lockfile_sha256": spec["semble_lockfile_sha256"],
        "host_profile": spec["host_profile"],
        "repetitions": repetitions,
    }
    (stage / "protocol-lock.json").write_text(
        json.dumps(protocol_lock, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    manifest = build_run_manifest(
        spec, stage, rep_layouts, host_start, host_end, reports, frozen_receipts
    )
    manifest_path = stage / "run-manifest.json"
    manifest_path.write_text(
        json.dumps(manifest, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    verdict = build_verdict(repo, suite_path, manifest_path)
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
        native = cell.get("native_latencies")
        if native is not None:
            if not isinstance(native, dict):
                raise RunError("native latencies must be an object")
            route = cell.get("native_route")
            if not isinstance(route, str) or not route:
                raise RunError("native latencies need their record route")
            for task_id, values in native.items():
                if not isinstance(values, list):
                    raise RunError(f"native latencies for {task_id} are not a list")
                sample_key = f"{system}:{cell['strategy']}:{route}:{task_id}"
                sys_key = f"{system}:{cell['strategy']}:{route}"
                if (route, task_id) not in timings_by_task:
                    raise RunError(f"native task without a record row: {task_id}")
                if not values:
                    continue
                recorded = timings_by_task[(route, task_id)]
                extras: list[object] = list(values)
                if recorded is not None:
                    first = _sample_value(values[0], f"{sample_key}[native]")
                    if first != recorded:
                        raise RunError(
                            f"native timing and normalized timing disagree for {task_id}"
                        )
                    extras = values[1:]
                for value in extras:
                    parsed = _sample_value(value, f"{sample_key}[native]")
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
    return {"system": system, "strategy": strategy, "rows": rows,
            "native_latencies": None, "native_route": None}


def build_latency_matrix(rep_layouts: list[dict]) -> dict:
    """Aggregate per (system, strategy, route, task) latencies across reps."""
    if not rep_layouts:
        raise RunError("latency matrix needs at least one rep")
    cells = []
    for layout in rep_layouts:
        for strategy, record in sorted(layout["quanta"].items()):
            payload = read_json(Path(record))
            if not isinstance(payload, dict):
                raise RunError(f"record is not an object: {record}")
            cells.append(_cell_from_record("quanta", strategy, payload))
        semble_record = read_json(Path(layout["semble"]))
        if not isinstance(semble_record, dict):
            raise RunError(f"semble record is not an object: {layout['semble']}")
        cell = _cell_from_record("semble", "native", semble_record)
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
    if mapping.get("skipped") != [] or mapping.get("extra") != [] or mapping.get("mismatched") != []:
        return False
    if any(row.get("status") != "indexed" for row in per_file):
        return False
    if any(row.get("readable") is not True for row in semble):
        return False
    try:
        expected = sorted(files, key=lambda row: row["path"])
    except (KeyError, TypeError):
        return False
    observed = [
        {"path": row.get("path"), "file_sha256": row.get("file_sha256")}
        for row in semble
    ]
    return bool(expected) and quanta == expected == observed and len(per_file) == len(expected)


def _exact_keys(payload: object, keys: set[str], where: str) -> dict:
    if not isinstance(payload, dict) or set(payload) != keys:
        raise RunError(f"{where} must hold exactly {sorted(keys)}")
    return payload


def _validate_receipt_shape(payload: object, where: str) -> dict:
    """Mirror of tools/ci/verification-receipt.schema.json (locked by test)."""
    receipt = _exact_keys(
        payload,
        {"schema_version", "revision", "rail", "tier", "command", "evidence_path",
         "evidence_sha256", "test_event_count"},
        where,
    )
    if receipt["schema_version"] != 1:
        raise RunError(f"{where}.schema_version must be 1")
    if not isinstance(receipt["revision"], str) or len(receipt["revision"]) < 7:
        raise RunError(f"{where}.revision must be a string of length >= 7")
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
    return receipt


def _validate_counts_shape(payload: object, where: str) -> dict:
    results = _exact_keys(
        payload, {"command", "selected", "executed", "passed", "failed"}, where
    )
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
        {"command", "separate_process", "sealed_receipt_digest", "activation_ack_digest",
         "empty_check", "binary_digest", "sdk_route", "selected", "executed", "passed",
         "failed"},
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


def freeze_inputs(spec: dict, stage: Path) -> dict:
    """Copy capture inputs into the stage root and return byte-verified paths.

    Captures consume these frozen copies, so the verdict re-verifies the
    exact bytes used rather than whatever the external paths hold later.
    """
    frozen = {}
    for key, name in (("suite", "suite.json"), ("query_pack", "query-pack.json"),
                      ("manifest", "corpus-manifest.json"),
                      ("semble_lockfile", "semble-lockfile.txt")):
        source = Path(spec[key])
        try:
            before = sha_file(source)
            target = stage / name
            shutil.copyfile(source, target)
            after = sha_file(target)
        except OSError as exc:
            raise RunError(f"cannot freeze capture input {key}: {exc}") from exc
        if before != after:
            raise RunError(f"capture input changed during freeze: {key}")
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
    for layout in rep_layouts:
        for record in sorted(layout["quanta"].values()):
            records.append(relative(Path(record)))
        records.append(relative(Path(layout["semble"])))
        natives.append(relative(Path(layout["semble"]).parent / "native.json"))
        if "quanta_manifest" not in layout:
            raise RunError("rep layout lacks the quanta manifest path")
        quanta_manifests.append(relative(Path(layout["quanta_manifest"])))
    for name in ("suite", "query_pack", "manifest"):
        if Path(spec[name]).resolve().parent != out_root.resolve():
            raise RunError(f"spec.{name} must name the frozen stage copy")
    host_start_path = out_root / "host-start.json"
    host_end_path = out_root / "host-end.json"
    evidence: dict = {
        "pair": {"mapping_proof_digest": sha_file(mapping_path)},
        "perf": {
            "observations_floor": latency["observations_floor"],
            "fresh_roots": latency["fresh_roots"],
            "phase_boundaries": False,
            "resource_accounting": True,
        },
    }
    frozen = frozen_receipts or {}
    unknown_frozen = sorted(set(frozen) - set(RECEIPT_KEYS))
    if unknown_frozen:
        raise RunError(f"frozen receipts hold unknown keys: {unknown_frozen}")
    receipt_artifacts: dict[str, str] = {}
    for key, path in sorted(frozen.items()):
        receipt_artifacts[key] = relative(Path(path))
    contract_keys = ("contract_python_receipt", "contract_python_results",
                     "contract_rust_receipt", "contract_rust_results")
    if any(key in frozen for key in contract_keys):
        if not all(key in frozen for key in contract_keys):
            raise RunError("incomplete frozen contract receipt set")
        for side in ("python", "rust"):
            results = _validate_counts_shape(
                read_json(Path(frozen[f"contract_{side}_results"])),
                f"contract {side} results",
            )
            _validate_receipt_shape(
                read_json(Path(frozen[f"contract_{side}_receipt"])),
                f"contract {side} receipt",
            )
            evidence.setdefault("contract_suites", {})[side] = {
                "test_result_digest": sha_file(Path(frozen[f"contract_{side}_results"]))
            }
    if any(key in frozen for key in ("sdk_receipt", "sdk_results")):
        if not all(key in frozen for key in ("sdk_receipt", "sdk_results")):
            raise RunError("incomplete frozen SDK receipt set")
        sdk_results = _validate_sdk_results_shape(
            read_json(Path(frozen["sdk_results"])), "sdk results"
        )
        _validate_receipt_shape(read_json(Path(frozen["sdk_receipt"])), "sdk receipt")
        evidence["sdk_path"] = {
            "test_result_digest": sha_file(Path(frozen["sdk_results"])),
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
    profile = spec.get("host_profile")
    if not isinstance(profile, str) or not profile:
        raise RunError("pair requires spec.host_profile naming the canonical host profile")
    artifacts = {
        "suite": relative(Path(spec["suite"])),
        "query_pack": relative(Path(spec["query_pack"])),
        "corpus_manifest": relative(Path(spec["manifest"])),
        "mapping_proof": relative(mapping_path),
        "latency_matrix": relative(latency_path),
        "host_start": relative(host_start_path),
        "host_end": relative(host_end_path),
        "records": sorted(records),
        "reports": sorted(reports),
        "quanta_manifests": sorted(quanta_manifests),
        "semble_adapter_manifest": relative(adapter_path),
        "semble_lockfile": relative(lockfile_path),
        "semble_native": sorted(natives),
        "protocol_lock": "protocol-lock.json",
    }
    artifacts.update(receipt_artifacts)
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
            "quanta": {"source_sha": source_sha, "binary_digest": runner_binary_digest,
                       "embedder": spec.get("embedder", "potion-code")},
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
                "profile": profile,
                "check_record_digest": digest(
                    canonical({"start": host_start, "end": host_end})
                ),
            },
        },
    }


def run_semble_capture(
    spec: dict, out_dir: Path, pack_path: Path, route: str, rep: int = 0
) -> None:
    adapter = Path(__file__).resolve().parent / "semble.py"
    command = [
        sys.executable,
        str(adapter),
        "run",
        "--repo", spec["repo"],
        "--manifest", spec["manifest"],
        "--query-pack", str(pack_path),
        "--top-k", str(spec["top_k"]),
        "--python", spec["semble_python"],
        "--lockfile", spec["semble_lockfile"],
        "--lockfile-sha256", spec["semble_lockfile_sha256"],
        "--cache-root", spec.get("semble_cache_root", str(out_dir.parent / "semble-cache")),
        "--output-root", str(out_dir),
        "--route", route,
        "--run-id", f"{spec.get('run_id', 'run')}-semble-r{rep}",
        "--seed", str(_int(spec.get("seed", 0), "spec.seed") + rep),
        "--blinding", spec.get("blinding", "attested"),
        "--isolation-method", spec.get("isolation_method", "attested-only: worker sees pack+corpus only"),
        "--access-block-log", spec.get("access_block_log", "attested-only: no suite path is passed to the worker"),
        "--repetitions", str(spec.get("semble_repetitions", 1)),
    ]
    if "semble_model_revision" in spec:
        command += ["--model-revision", spec["semble_model_revision"]]
    try:
        completed = subprocess.run(
            command,
            capture_output=True,
            text=True,
            timeout=_int(spec.get("timeout_secs", 1800), "spec.timeout_secs"),
        )
    except subprocess.TimeoutExpired as exc:
        raise RunError(f"Semble capture timed out: {exc}") from exc
    if completed.returncode != 0:
        raise RunError(f"Semble capture failed: {completed.stderr[-2000:]}")


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
    return parser


def main(argv: list[str] | None = None) -> int:
    parser = build_parser()
    args = parser.parse_args(argv)
    if args.command == "merge":
        return cmd_merge(args)
    if args.command == "host-probe":
        return cmd_host_probe(args)
    if args.command == "quanta":
        return cmd_quanta(args)
    if args.command == "verdict":
        return cmd_verdict(args)
    return cmd_pair(args)


if __name__ == "__main__":
    raise SystemExit(main())
