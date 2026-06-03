#!/usr/bin/env python3
"""Orchestrate a cold-matrix DSL latency artifact via fresh Rust processes.

Layer-3 (query latency) cold-start measurement for the DSL-benchmarking model
defined in ``docs/plans/jun-2-dsl-hardening/RFC-DSL-Benchmarking.md``. Criterion
cannot measure true cold-start (it amortizes runtime boot across iterations), so
this orchestrator invokes a Rust harness binary once per (scenario, sample) in a
FRESH OS process to capture the first-query latency from a cold runtime.

The harness binary contract (built elsewhere; this script only orchestrates):

  <bin-cmd> --list
      prints a JSON array of scenario descriptors to stdout:
      [{"scenario_id": ..., "route_family": ..., "syntax": ..., "expected_shape": ...}, ...]

  <bin-cmd> --scenario <id>
      boots a fresh runtime, runs ONE query, prints ONE cold sample JSON object:
      {"scenario_id": ..., "route_family": ..., "syntax": ..., "mode": "cold",
       "result_shape": ..., "first_query_ms": <float-or-null>, "result_count": ...,
       "typed_error_code": ..., "engine_touched": [...], "early_stop_reason": null-or-str,
       "git_rev": ...}

The default bin-cmd runs the workspace harness through cargo; override with
``--bin-cmd`` (a shell-splittable prefix) to point at a prebuilt binary.
"""

from __future__ import annotations

import argparse
import json
import math
import shlex
import subprocess
import sys
from pathlib import Path

DEFAULT_SAMPLES = 3
DEFAULT_OUT = Path("artifacts/dsl-bench/cold-matrix.json")
DEFAULT_BIN_CMD = (
    "./scripts/cargow run -p quanta-index-searchd-harness --bin dsl_cold_matrix "
    "--quiet --locked --"
)


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--samples",
        type=int,
        default=DEFAULT_SAMPLES,
        help=(
            "number of fresh-process cold samples per scenario "
            f"(default: {DEFAULT_SAMPLES})"
        ),
    )
    parser.add_argument(
        "--out",
        type=Path,
        default=DEFAULT_OUT,
        help=f"output artifact path (default: {DEFAULT_OUT})",
    )
    parser.add_argument(
        "--bin-cmd",
        default=DEFAULT_BIN_CMD,
        help="shell-splittable command prefix for the harness binary",
    )
    parser.add_argument(
        "--git-rev",
        default=None,
        help="git revision to stamp into the artifact (default: `git rev-parse --short HEAD`)",
    )
    return parser.parse_args()


def percentile(samples: list[float], pct: float) -> float:
    """Nearest-rank percentile over a non-empty sample list.

    Sorts ascending, then index = ceil(pct/100 * n) - 1, clamped to [0, n-1].
    """
    if not samples:
        raise ValueError("percentile requires at least one sample")
    ordered = sorted(samples)
    n = len(ordered)
    index = math.ceil(pct / 100.0 * n) - 1
    index = max(0, min(index, n - 1))
    return ordered[index]


def resolve_git_rev(explicit: str | None) -> str:
    if explicit is not None:
        return explicit
    try:
        completed = subprocess.run(
            ["git", "rev-parse", "--short", "HEAD"],
            check=True,
            capture_output=True,
            text=True,
        )
    except (subprocess.CalledProcessError, OSError):
        return "unknown"
    rev = completed.stdout.strip()
    return rev or "unknown"


def run_bin(bin_cmd: list[str], *extra: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [*bin_cmd, *extra],
        check=False,
        capture_output=True,
        text=True,
    )


def discover_scenarios(bin_cmd: list[str]) -> list[dict[str, object]]:
    completed = run_bin(bin_cmd, "--list")
    if completed.returncode != 0:
        raise RuntimeError(f"--list failed (exit {completed.returncode}):\n{completed.stderr}")
    try:
        scenarios = json.loads(completed.stdout)
    except json.JSONDecodeError as exc:
        raise RuntimeError(f"--list emitted non-JSON stdout: {exc}") from exc
    if not isinstance(scenarios, list):
        raise RuntimeError("--list must emit a JSON array")
    return scenarios


def sample_scenario(bin_cmd: list[str], scenario_id: str) -> dict[str, object]:
    completed = run_bin(bin_cmd, "--scenario", scenario_id)
    if completed.returncode != 0:
        raise RuntimeError(
            f"--scenario {scenario_id} failed (exit {completed.returncode}):\n{completed.stderr}"
        )
    try:
        sample = json.loads(completed.stdout)
    except json.JSONDecodeError as exc:
        raise RuntimeError(f"--scenario {scenario_id} emitted non-JSON stdout: {exc}") from exc
    if not isinstance(sample, dict):
        raise RuntimeError(f"--scenario {scenario_id} must emit a JSON object")
    return sample


def build_row(scenario_id: str, samples: int, last_sample: dict[str, object]) -> dict[str, object]:
    """Build an unmeasured (early-stop) row from a single probe sample."""
    return {
        "scenario_id": scenario_id,
        "route_family": last_sample["route_family"],
        "syntax": last_sample["syntax"],
        "mode": "cold",
        "result_shape": last_sample["result_shape"],
        "latency_p50_ms": None,
        "latency_p95_ms": None,
        "latency_p99_ms": None,
        "samples": samples,
        "result_count": last_sample.get("result_count"),
        "typed_error_code": last_sample.get("typed_error_code"),
        "engine_touched": last_sample.get("engine_touched"),
        "early_stop_reason": last_sample.get("early_stop_reason"),
        "git_rev": last_sample.get("git_rev"),
    }


def measure_scenario(bin_cmd: list[str], scenario_id: str, samples: int) -> dict[str, object]:
    # Probe once first: an early-stop scenario is recorded as one unmeasured row.
    first = sample_scenario(bin_cmd, scenario_id)
    if first.get("early_stop_reason") is not None:
        print(
            f"  {scenario_id}: early_stop ({first['early_stop_reason']}) — recording unmeasured row",
            file=sys.stderr,
        )
        return build_row(scenario_id, samples, first)

    latencies: list[float] = []
    last = first
    for i in range(samples):
        sample = first if i == 0 else sample_scenario(bin_cmd, scenario_id)
        last = sample
        if sample.get("early_stop_reason") is not None:
            print(
                f"  {scenario_id}: early_stop ({sample['early_stop_reason']}) on sample {i + 1}"
                " — recording unmeasured row",
                file=sys.stderr,
            )
            return build_row(scenario_id, samples, sample)
        first_query_ms = sample.get("first_query_ms")
        if first_query_ms is None:
            raise RuntimeError(
                f"--scenario {scenario_id} returned null first_query_ms without early_stop_reason"
            )
        latencies.append(float(first_query_ms))

    return {
        "scenario_id": scenario_id,
        "route_family": last["route_family"],
        "syntax": last["syntax"],
        "mode": "cold",
        "result_shape": last["result_shape"],
        "latency_p50_ms": percentile(latencies, 50),
        "latency_p95_ms": percentile(latencies, 95),
        "latency_p99_ms": percentile(latencies, 99),
        "samples": samples,
        "result_count": last.get("result_count"),
        "typed_error_code": last.get("typed_error_code"),
        "engine_touched": last.get("engine_touched"),
        "early_stop_reason": None,
        "git_rev": last.get("git_rev"),
    }


def main() -> int:
    args = parse_args()

    if args.samples < 1:
        print("ERROR: --samples must be >= 1", file=sys.stderr)
        return 2

    bin_cmd = shlex.split(args.bin_cmd)
    if not bin_cmd:
        print("ERROR: --bin-cmd resolved to an empty command", file=sys.stderr)
        return 2

    git_rev = resolve_git_rev(args.git_rev)

    try:
        scenarios = discover_scenarios(bin_cmd)
    except (RuntimeError, OSError) as exc:
        print(f"ERROR: scenario discovery failed: {exc}", file=sys.stderr)
        return 2

    print(f"discovered {len(scenarios)} scenario(s); samples={args.samples}", file=sys.stderr)

    rows: list[dict[str, object]] = []
    try:
        for descriptor in scenarios:
            scenario_id = descriptor["scenario_id"]
            print(f"measuring {scenario_id} ...", file=sys.stderr)
            rows.append(measure_scenario(bin_cmd, scenario_id, args.samples))
    except (RuntimeError, OSError, KeyError) as exc:
        print(f"ERROR: cold-matrix measurement failed: {exc}", file=sys.stderr)
        return 2

    artifact = {
        "schema_version": 1,
        "mode": "cold",
        "git_rev": git_rev,
        "rows": rows,
    }

    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(json.dumps(artifact, indent=2) + "\n", encoding="utf-8")
    print(f"wrote {len(rows)} row(s) to {args.out}", file=sys.stderr)
    return 0


if __name__ == "__main__":
    sys.exit(main())
