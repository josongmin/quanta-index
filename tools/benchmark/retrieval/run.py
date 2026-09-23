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
        TOKENIZER_BUDGET_VERSION,
        canonical,
        digest,
        evaluate,
        load_evidence,
        validate_suite,
        verify_repo,
    )
    from tools.benchmark.retrieval.evaluator import (
        read_json as read_evidence_json,
    )
except ImportError:  # direct script invocation: import the sibling module
    sys.path.insert(0, str(Path(__file__).resolve().parent))
    from evaluator import (  # noqa: E402
        TOKENIZER_BUDGET_VERSION,
        canonical,
        digest,
        evaluate,
        load_evidence,
        validate_suite,
        verify_repo,
    )
    from evaluator import (
        read_json as read_evidence_json,
    )

VERDICT_VERSION = 1
PILOT_OBSERVATIONS_FLOOR = 1000


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
    total = 0
    for dirpath, _dirnames, filenames in os.walk(root):
        for name in filenames:
            try:
                total += (Path(dirpath) / name).stat().st_size
            except OSError:
                continue
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
    """Validate each record via the evaluator and merge disjoint routes.

    Each system consumes a projected pack (same tasks/universe/commit,
    narrowed routes), so pack digests legitimately differ per record: the
    merge re-derives each projected pack from the frozen combined pack and
    requires an exact digest match, then validates the record against a
    projected suite with the evaluator itself. Returns (suite, pack,
    combined_record). Results concatenate in (task_id, route) order.
    """
    if not record_paths:
        raise RunError("merge needs at least one record")
    suite_payload = read_json(suite_path)
    if not isinstance(suite_payload, dict):
        raise RunError("suite must be an object")
    suite, pack, _source = validate_suite(repo, suite_payload)
    provenance: dict = {}
    results: dict[tuple[str, str], dict] = {}
    runners: list[dict] = []
    for path in record_paths:
        raw = read_json(path)
        if not isinstance(raw, dict):
            raise RunError(f"record is not an object: {path}")
        routes = sorted(raw.get("route_provenance", {}).keys())
        if not routes:
            raise RunError(f"record names no routes: {path}")
        projected_pack, projected_suite = project_pack_and_suite(pack, suite, routes)
        expected_sha = digest(canonical_bytes(projected_pack))
        if raw.get("query_pack_sha256") != expected_sha:
            raise RunError(
                f"record {path} pack digest does not match its projected pack"
            )
        with tempfile_record(projected_suite, suffix=".suite.json") as suite_file:
            _, _, run = load_evidence(repo, suite_file, path)
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
    merge_id = digest(
        canonical_bytes(sorted(entry["path"] for entry in runners))
    )[:16]
    combined = {
        "schema_version": 2,
        "query_pack_sha256": digest(canonical_bytes(pack)),
        "runner": {
            "name": "retrieval-pair-merge",
            "revision": "merge-v1",
            "run_id": f"merge:{merge_id}",
            "tokenizer": "qi-regex-v1",
            "tokenizer_budget_version": TOKENIZER_BUDGET_VERSION,
            "gold_access": False,
            "blinding": merged_blinding,
            "isolation_method": "merge of independently blinded records (weakest blinding wins)",
            "access_block_log": json.dumps(runners, sort_keys=True),
        },
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


def load_spec(path: Path) -> dict:
    spec = read_json(path)
    if not isinstance(spec, dict):
        raise RunError("spec must be an object")
    for key in ("repo", "manifest", "suite", "query_pack", "top_k", "output_root"):
        if key not in spec:
            raise RunError(f"spec lacks required key: {key}")
    return spec


def preflight_capture(spec: dict) -> Path:
    """Refuse dirty/wrong-HEAD inputs and output inside the frozen checkout."""
    manifest = read_json(Path(spec["manifest"]))
    if not isinstance(manifest, dict) or not isinstance(manifest.get("repository_commit"), str):
        raise RunError("manifest must pin repository_commit")
    try:
        repo = verify_repo(Path(spec["repo"]), manifest["repository_commit"])
    except ValueError as exc:
        raise RunError(f"pinned repository proof failed: {exc}") from exc
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
    if "runner_revision" in spec:
        raise RunError("runner_revision is derived from the Rust runner binary; do not supply it")
    try:
        runner_binary_sha256 = sha_file(Path(runner_bin))
    except OSError as exc:
        raise RunError(f"cannot hash Rust runner binary: {exc}") from exc
    strategies = spec.get("strategies")
    if not isinstance(strategies, list) or not strategies:
        raise RunError("spec.strategies must be a nonempty list")
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
    if name not in ("whole_file", "fixed_window", "syntax"):
        raise RunError(f"unknown strategy: {name}")
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
    if "searchd_binary" in spec:
        command += ["--searchd-bin", spec["searchd_binary"]]
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
    return {
        "strategy": name,
        "strategy_config": strategy,
        "record": str(record_path),
        "record_digest": sha_file(record_path),
        "runner_binary_sha256": runner_binary_sha256,
        "driver_ms": elapsed_ms,
        "index_bytes": tree_size(state_root),
        "state_root": str(state_root),
    }


def cmd_verdict(args: argparse.Namespace) -> int:
    try:
        verdict = build_verdict(
            repo=Path(args.repo),
            suite_path=Path(args.suite),
            record_paths=[Path(p) for p in args.records],
            run_manifest=read_json(Path(args.run_manifest)),
            baseline_route=args.baseline_route,
            candidate_route=args.candidate_route,
        )
    except (RunError, ValueError) as exc:
        print(f"ERROR: {exc}", file=sys.stderr)
        return 2
    Path(args.out).write_text(
        json.dumps(verdict, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    print(json.dumps(verdict["states"], indent=2, sort_keys=True))
    return 0


def build_verdict(
    repo: Path,
    suite_path: Path,
    record_paths: list[Path],
    run_manifest: object,
    baseline_route: str,
    candidate_route: str,
) -> dict:
    """Re-score immutable records and emit the TEST-PLAN §8 verdict."""
    if not isinstance(run_manifest, dict):
        raise RunError("run manifest must be an object")
    suite, pack, combined = merge_records(repo, suite_path, record_paths)
    report = evaluate(suite, pack, combined, baseline_route, candidate_route)
    evidence = run_manifest.get("evidence", {})
    if not isinstance(evidence, dict):
        raise RunError("run manifest evidence must be an object")

    states: dict[str, str] = {}
    missing: list[str] = []
    not_applicable: list[str] = []
    failure_class = "none"

    def state(name: str, value: str, t_ids: list[str], fail_class: str = "") -> None:
        nonlocal failure_class
        states[name] = value
        if value == "fail":
            missing.extend(t for t in t_ids if evidence.get(t, {}).get("status") != "pass")
            if failure_class == "none" and fail_class:
                failure_class = fail_class
        elif value == "not_run":
            missing.extend(t_ids)
        elif value == "not_applicable":
            not_applicable.extend(t_ids)

    # CONTRACT_GREEN: owner test suites referenced by the manifest.
    contract = evidence.get("contract_suites", {})
    if (
        isinstance(contract, dict)
        and contract.get("python", {}).get("failed", 1) == 0
        and contract.get("python", {}).get("passed", 0) > 0
        and contract.get("rust", {}).get("failed", 1) == 0
        and contract.get("rust", {}).get("passed", 0) > 0
    ):
        state("CONTRACT_GREEN", "pass", [])
    elif contract:
        state("CONTRACT_GREEN", "fail", ["T01", "T02", "T03", "T04", "T08", "T09"], "scoring")
    else:
        state("CONTRACT_GREEN", "not_run", ["T01", "T02", "T03", "T04", "T08", "T09"])

    # SDK_PATH_GREEN: separate daemon + receipt/activation + SDK query.
    sdk = evidence.get("sdk_path", {})
    if (
        isinstance(sdk, dict)
        and sdk.get("separate_process") is True
        and sdk.get("sealed_receipt") is True
        and sdk.get("activation_ack") is True
        and sdk.get("empty_check") is True
    ):
        state("SDK_PATH_GREEN", "pass", [])
    elif sdk:
        state("SDK_PATH_GREEN", "fail", ["T05", "T06", "T07"], "provenance")
    else:
        state("SDK_PATH_GREEN", "not_run", ["T05", "T06", "T07"])

    # PAIR_VALID: merge already proved same pack + disjoint complete routes;
    # the manifest must additionally bind commit/files/host equality.
    pair = evidence.get("pair", {})
    if (
        isinstance(pair, dict)
        and pair.get("mapping_proof_clean") is True
        and pair.get("same_commit") is True
        and pair.get("same_files") is True
        and pair.get("same_host") is True
    ):
        state("PAIR_VALID", "pass", [])
    elif pair:
        state("PAIR_VALID", "fail", ["T00", "T11", "T12"], "corpus_mismatch")
    else:
        state("PAIR_VALID", "not_run", ["T00", "T11", "T12"])

    # PERF_QUALIFIED: only when a speed claim is made.
    claims = run_manifest.get("claims", {})
    host = run_manifest.get("host", {})
    perf = evidence.get("perf", {})
    observations = perf.get("observations_per_route", 0) if isinstance(perf, dict) else 0

    def probe_clean(probe: object) -> bool:
        return (
            isinstance(probe, dict)
            and probe.get("concurrent_processes", {}) in ({}, {"none": []})
            and probe.get("contention_override") is not True
        )

    if isinstance(host, dict) and ("start" in host or "end" in host):
        host_clean = all(
            probe_clean(host.get(edge, {})) for edge in ("start", "end")
        )
    else:
        host_clean = probe_clean(host)
    if not claims.get("speed"):
        state("PERF_QUALIFIED", "not_applicable", [])
    elif (
        isinstance(perf, dict)
        and observations >= PILOT_OBSERVATIONS_FLOOR
        and perf.get("phase_boundaries") is True
        and perf.get("resource_accounting") is True
        and host_clean
    ):
        state("PERF_QUALIFIED", "pass", [])
    elif isinstance(perf, dict) and perf:
        if not host_clean:
            state("PERF_QUALIFIED", "fail", [], "host")
        else:
            state("PERF_QUALIFIED", "fail", [], "provenance")
    else:
        state("PERF_QUALIFIED", "not_run", [])

    # QUALITY_DELTA: blinded, graded, in-scope quality only.
    declared_blinding = run_manifest.get("blinding", "attested")
    recorded_blinding = combined["runner"]["blinding"]
    blinding = (
        "isolated"
        if declared_blinding == "isolated" and recorded_blinding == "isolated"
        else "attested"
    )
    scope = run_manifest.get("scope", "exploratory")
    if not claims.get("quality"):
        state("QUALITY_DELTA", "not_applicable", [])
    elif blinding != "isolated":
        state("QUALITY_DELTA", "not_applicable", [])
        not_applicable.append("blinding:attested_only")
    elif scope == "exploratory":
        state("QUALITY_DELTA", "not_applicable", [])
        not_applicable.append("scope:exploratory_only")
    elif not report.get("graded"):
        state("QUALITY_DELTA", "fail", [], "scoring")
    else:
        state("QUALITY_DELTA", "pass", [])

    # Conditional IDs.
    if claims.get("same_model"):
        parity = evidence.get("model_parity", {})
        if isinstance(parity, dict) and parity.get("status") == "pass":
            pass
        else:
            missing.append("T15")
            if failure_class == "none":
                failure_class = "model"
    else:
        not_applicable.append("T15")
    if claims.get("incremental"):
        incremental = evidence.get("incremental", {})
        if isinstance(incremental, dict) and incremental.get("status") == "pass":
            pass
        else:
            missing.append("T16")
            if failure_class == "none":
                failure_class = "infra"
    else:
        not_applicable.append("T16")

    provenance = run_manifest.get("provenance", {})
    if not isinstance(provenance, dict):
        raise RunError("run manifest provenance must be an object")
    counts = {
        "selected": report.get("sample_count", 0),
        "executed": len(combined["results"]),
        "passed": sum(1 for row in combined["results"] if row["status"] in ("success", "capped")),
        "failed": sum(1 for row in combined["results"] if row["status"] not in ("success", "capped", "abstained")),
    }
    return {
        "verdict_version": VERDICT_VERSION,
        "states": states,
        "blinding": blinding,
        "isolation_method": run_manifest.get("isolation_method", ""),
        "access_block_log": run_manifest.get("access_block_log", ""),
        "missing_t_ids": sorted(set(missing)),
        "not_applicable_t_ids": sorted(set(not_applicable)),
        "failure_class": failure_class,
        "provenance": provenance,
        "counts": counts,
        "primary_metric": report.get("primary_metric"),
        "primary_delta": report.get("rank_metrics", {}).get("comparison", {}).get("primary_delta"),
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
    Partial output is never resumed: rerun from a fresh output root.
    """
    lockfile_sha = spec.get("semble_lockfile_sha256")
    if not isinstance(lockfile_sha, str) or len(lockfile_sha) != 64 or any(
        c not in "0123456789abcdef" for c in lockfile_sha
    ):
        raise RunError("pair requires a pinned semble_lockfile_sha256")
    out_root = preflight_capture(spec)
    if out_root.exists():
        raise RunError(f"output root already exists (refusing reuse): {out_root}")
    out_root.mkdir(parents=True)
    order = spec.get("order", ["quanta", "semble"])
    if sorted(order) != ["quanta", "semble"]:
        raise RunError("spec.order must list quanta and semble exactly once")
    repetitions = _int(spec.get("repetitions", 1), "spec.repetitions")
    if repetitions <= 0:
        raise RunError("spec.repetitions must be positive")
    alternate = spec.get("alternate_order", True)
    host_start = host_probe()
    (out_root / "host-start.json").write_text(
        json.dumps(host_start, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    semble_routes = [spec.get("semble_route", "semble-hybrid")]
    semble_pack = write_projected_pack(
        Path(spec["query_pack"]),
        Path(spec["suite"]),
        semble_routes,
        out_root / "semble-pack.json",
    )
    rep_layouts: list[dict] = []
    semble_spec = dict(spec)
    # One shared model/Semble cache across reps: the model downloads once,
    # while each rep still rebuilds its index on a fresh corpus.
    semble_spec.setdefault("semble_cache_root", str(out_root / "semble-cache"))
    for rep in range(repetitions):
        rep_order = order if (rep % 2 == 0 or not alternate) else list(reversed(order))
        rep_dir = out_root / f"rep-{rep:02d}"
        rep_dir.mkdir(parents=True)
        layout: dict = {"rep": rep, "order": rep_order, "quanta": {}, "semble": ""}
        for system in rep_order:
            if system == "quanta":
                quanta_spec = dict(spec)
                quanta_spec["output_root"] = str(rep_dir / "quanta")
                quanta_spec["run_id"] = f"{spec.get('run_id', 'run')}-r{rep}"
                if run_quanta(quanta_spec, Path(".")) != 0:
                    raise RunError(f"quanta capture failed at rep {rep}")
                manifest = read_json(rep_dir / "quanta" / "quanta-manifest.json")
                if not isinstance(manifest, dict):
                    raise RunError("quanta manifest is not an object")
                for run in manifest["runs"]:
                    layout["quanta"][run["strategy"]] = run["record"]
            else:
                semble_out = rep_dir / "semble"
                run_semble_capture(
                    semble_spec, semble_out, semble_pack, semble_routes[0], rep=rep
                )
                layout["semble"] = str(semble_out / "record.json")
        rep_layouts.append(layout)
    host_end = host_probe()
    (out_root / "host-end.json").write_text(
        json.dumps(host_end, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    # Every rep's records validate through the evaluator before any use:
    # each (strategy, semble) pair merges exactly like the scored join.
    # Quality reports merge rep-0 records only.
    repo = Path(spec["repo"])
    suite_path = Path(spec["suite"])
    for layout in rep_layouts:
        for record in sorted(layout["quanta"].values()):
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
            (out_root / name).write_text(
                json.dumps(report, indent=2, sort_keys=True) + "\n", encoding="utf-8"
            )
            reports.append(name)
    latency_path = out_root / "latency-matrix.json"
    latency_path.write_text(
        json.dumps(build_latency_matrix(rep_layouts), indent=2, sort_keys=True) + "\n",
        encoding="utf-8",
    )
    manifest_path = out_root / "run-manifest.json"
    manifest_path.write_text(
        json.dumps(
            build_run_manifest(spec, out_root, rep_layouts, host_start, host_end, reports),
            indent=2,
            sort_keys=True,
        )
        + "\n",
        encoding="utf-8",
    )
    print(json.dumps(
        {"reports": reports, "repetitions": repetitions, "output_root": str(out_root)},
        indent=2,
    ))
    return 0


def build_latency_matrix(rep_layouts: list[dict]) -> dict:
    """Aggregate per (system, strategy, route, task) latencies across reps."""
    samples: dict[str, list[float]] = {}

    def add(key: str, value: object) -> None:
        if isinstance(value, (int, float)) and math.isfinite(value) and value >= 0:
            samples.setdefault(key, []).append(float(value))

    for layout in rep_layouts:
        for strategy, record in layout["quanta"].items():
            payload = read_json(Path(record))
            assert isinstance(payload, dict)
            for row in payload["results"]:
                add(
                    f"quanta:{strategy}:{row['route']}:{row['task_id']}",
                    row.get("timings", {}).get("query_latency_ms"),
                )
        semble_record = read_json(Path(layout["semble"]))
        assert isinstance(semble_record, dict)
        for row in semble_record["results"]:
            add(
                f"semble:{row['route']}:{row['task_id']}",
                row.get("timings", {}).get("query_latency_ms"),
            )
        native_path = Path(layout["semble"]).parent / "native.json"
        native = read_json(native_path)
        if isinstance(native, dict):
            latencies = native.get("latencies_ms", {})
            if isinstance(latencies, dict):
                route = semble_record["results"][0]["route"] if semble_record["results"] else "?"
                for task_id, values in latencies.items():
                    if isinstance(values, list):
                        for value in values[1:]:  # rep-0 sample already counted
                            add(f"semble:{route}:{task_id}", value)
    summary = {key: latency_summary(values) for key, values in sorted(samples.items())}
    per_route: dict[str, int] = {}
    for key, values in samples.items():
        route = key.split(":")[2] if key.startswith("quanta:") else key.split(":")[1]
        per_route[route] = per_route.get(route, 0) + len(values)
    return {
        "samples": {key: samples[key] for key in sorted(samples)},
        "summary": summary,
        "observations_per_route": per_route,
        "observations_floor": min(per_route.values()) if per_route else 0,
    }


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


def build_run_manifest(
    spec: dict,
    out_root: Path,
    rep_layouts: list[dict],
    host_start: dict,
    host_end: dict,
    reports: list[str],
) -> dict:
    """Emit the driver-observed run manifest consumed by `verdict`.

    The driver records what it observed (pair binding, host, provenance,
    latency floors). Contract/SDK evidence arrives via
    `spec["evidence"]` passthrough from actual CI/test runs; absent keys
    honestly yield `not_run` verdict states.
    """
    rep0 = rep_layouts[0]
    checkout = Path(__file__).resolve().parent.parent.parent.parent
    manifest_payload = read_json(Path(spec["manifest"]))
    pack_payload = read_json(Path(spec["query_pack"]))
    same_commit = (
        isinstance(manifest_payload, dict)
        and isinstance(pack_payload, dict)
        and manifest_payload.get("repository_commit") == pack_payload.get("repository_commit")
    )
    mapping = read_json(Path(rep0["semble"]).parent / "mapping-proof.json")
    mapping_clean = mapping_matches_manifest(mapping, manifest_payload)
    latency = read_json(out_root / "latency-matrix.json")
    observations = latency.get("observations_floor", 0) if isinstance(latency, dict) else 0
    adapter_manifest = read_json(Path(rep0["semble"]).parent / "adapter-manifest.json")
    semble_info: dict = {}
    if isinstance(adapter_manifest, dict):
        semble_info = {
            key: adapter_manifest.get(key)
            for key in ("semble_version", "lockfile_digest", "model_id", "model_revision")
        }
    evidence: dict = {
        "pair": {
            "mapping_proof_clean": mapping_clean,
            "same_commit": same_commit,
            "same_files": bool(mapping_clean and same_commit),
            "same_host": True,
        },
        "perf": {
            "observations_per_route": observations,
            "phase_boundaries": False,
            "phase_note": "driver wall + index samples recorded; structured phase "
            "fragments need runner-manifest support",
            "resource_accounting": True,
        },
    }
    passthrough = spec.get("evidence", {})
    if not isinstance(passthrough, dict):
        raise RunError("spec.evidence must be an object")
    reserved = set(passthrough) & {"pair", "perf"}
    if reserved:
        raise RunError(
            f"spec.evidence cannot override driver-observed evidence: {sorted(reserved)}"
        )
    for key, value in passthrough.items():
        evidence[key] = value
    claims = spec.get("claims", {})
    if not isinstance(claims, dict):
        claims = {}
    return {
        "blinding": spec.get("blinding", "attested"),
        "isolation_method": spec.get("isolation_method", "attested-only"),
        "access_block_log": spec.get("access_block_log", "attested-only"),
        "scope": spec.get("scope", "exploratory"),
        "claims": {
            "quality": bool(claims.get("quality", False)),
            "speed": bool(claims.get("speed", False)),
            "same_model": bool(claims.get("same_model", False)),
            "incremental": bool(claims.get("incremental", False)),
        },
        "evidence": evidence,
        "host": {"start": host_start, "end": host_end},
        "provenance": {
            "quanta_source_sha": git_head_sha(checkout),
            "spec_digest": digest(canonical_bytes(spec)),
            "suite_digest": sha_file(Path(spec["suite"])),
            "pack_digest": sha_file(Path(spec["query_pack"])),
            "manifest_digest": sha_file(Path(spec["manifest"])),
            "runner_binary_digest": sha_file(Path(spec["runner_binary"])),
            "searchd_binary_digest": (
                sha_file(Path(spec["searchd_binary"])) if spec.get("searchd_binary") else "resolved-at-runtime"
            ),
            "semble": semble_info,
            "reports": sorted(reports),
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
    verdict = sub.add_parser("verdict", help="re-score and emit verdict")
    verdict.add_argument("--repo", required=True)
    verdict.add_argument("--suite", required=True)
    verdict.add_argument("--records", nargs="+", required=True)
    verdict.add_argument("--run-manifest", required=True)
    verdict.add_argument("--baseline-route", required=True)
    verdict.add_argument("--candidate-route", required=True)
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
