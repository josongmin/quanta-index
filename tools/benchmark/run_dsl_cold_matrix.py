#!/usr/bin/env python3
"""Orchestrate a cold-matrix DSL latency artifact via fresh Rust processes.

Layer-3 (query latency) cold-start measurement for the DSL-benchmarking model
in ``docs/adr/JUN-08-001-verification-hellgate-and-benchmark-separation.md``. Criterion
cannot measure true cold-start (it amortizes runtime boot across iterations), so
this orchestrator invokes a Rust harness binary once per (scenario, sample) in a
FRESH OS process to capture the first-query latency from a cold runtime, then
hands every sample to the same binary's ``--assemble`` subcommand, which
aggregates the percentiles and writes the one ``BenchArtifactV1`` (QI-BB-010):
the exact head of a clean worktree, the fixture corpus digest, the run
configuration digest, the host and the peak RSS. This script never writes an
artifact and never stamps a head; it only orchestrates processes.

The harness binary contract (built elsewhere; this script only orchestrates):

  <bin-cmd> --list
      prints a JSON array of scenario descriptors to stdout:
      [{"scenario_id": ..., "route_family": ..., "syntax": ..., "expected_shape": ...}, ...]

  <bin-cmd> --scenario <id>
      boots a fresh runtime, runs ONE query, prints ONE cold sample JSON object:
      {"scenario_id": ..., "route_family": ..., "syntax": ..., "mode": "cold",
       "result_shape": ..., "first_query_ms": <float-or-null>, "result_count": ...,
       "typed_error_code": ..., "engine_touched": [...], "early_stop_reason": null-or-str,
       "model_revision": null-or-str}

  <bin-cmd> --assemble --out <path> --samples <n>
      reads a JSON array of samples on stdin and writes the artifact to <path>.

The default bin-cmd runs the workspace harness through cargo; override with
``--bin-cmd`` (a shell-splittable prefix) to point at a prebuilt binary.
"""

from __future__ import annotations

import argparse
import json
import shlex
import subprocess
import sys
from pathlib import Path

DEFAULT_SAMPLES = 20
DEFAULT_OUT = Path("artifacts/dsl-bench/cold-matrix.json")
DEFAULT_BUILD_CMD = (
    "env CARGO_NET_OFFLINE=true ./scripts/cargow --lane bench-lane build -p quanta-index-searchd-harness "
    "--bin dsl_cold_matrix --profile bench --quiet --locked"
)
TARGET_DIR_CMD = (
    "export QUANTA_INDEX_BUILD_LANE=bench-lane; "
    "source scripts/quanta-index-env.sh; "
    'printf "%s" "$CARGO_TARGET_DIR"'
)


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--samples",
        type=int,
        default=DEFAULT_SAMPLES,
        help=(f"number of fresh-process cold samples per scenario (default: {DEFAULT_SAMPLES})"),
    )
    parser.add_argument(
        "--out",
        type=Path,
        default=DEFAULT_OUT,
        help=f"output artifact path (default: {DEFAULT_OUT})",
    )
    parser.add_argument(
        "--bin-cmd",
        default=None,
        help=(
            "shell-splittable command prefix for the harness binary; when omitted, "
            "the script prebuilds dsl_cold_matrix once and invokes the binary directly"
        ),
    )
    return parser.parse_args()


def resolve_default_bin_cmd() -> list[str]:
    build = subprocess.run(
        shlex.split(DEFAULT_BUILD_CMD),
        check=False,
        capture_output=True,
        text=True,
    )
    if build.returncode != 0:
        raise RuntimeError(
            f"cold-matrix build failed (exit {build.returncode}):\n{build.stderr or build.stdout}"
        )
    target_dir = subprocess.run(
        ["bash", "-lc", TARGET_DIR_CMD],
        check=False,
        capture_output=True,
        text=True,
    )
    if target_dir.returncode != 0:
        raise RuntimeError(
            "cold-matrix target-dir probe failed "
            f"(exit {target_dir.returncode}):\n{target_dir.stderr or target_dir.stdout}"
        )
    path = Path(target_dir.stdout.strip()) / "release" / "dsl_cold_matrix"
    if not path.is_file():
        raise RuntimeError(f"cold-matrix binary missing after build: {path}")
    return [str(path)]


def run_bin(
    bin_cmd: list[str], *extra: str, stdin: str | None = None
) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [*bin_cmd, *extra],
        check=False,
        capture_output=True,
        text=True,
        input=stdin,
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
    if sample.get("scenario_id") != scenario_id:
        raise RuntimeError(f"--scenario {scenario_id} answered for {sample.get('scenario_id')!r}")
    return sample


def collect_samples(bin_cmd: list[str], scenario_id: str, samples: int) -> list[dict[str, object]]:
    """Every cold sample of one scenario, one fresh process each.

    An early-stopped scenario is recorded from its first sample alone: the
    assembler writes it as an unmeasured row, never a guessed latency.
    """
    first = sample_scenario(bin_cmd, scenario_id)
    if first.get("early_stop_reason") is not None:
        print(
            f"  {scenario_id}: early_stop ({first['early_stop_reason']}) — recording unmeasured row",
            file=sys.stderr,
        )
        return [first]
    collected = [first]
    for index in range(1, samples):
        sample = sample_scenario(bin_cmd, scenario_id)
        if sample.get("early_stop_reason") is not None:
            print(
                f"  {scenario_id}: early_stop ({sample['early_stop_reason']}) on sample {index + 1}"
                " — recording unmeasured row",
                file=sys.stderr,
            )
            return [sample]
        if sample.get("first_query_ms") is None:
            raise RuntimeError(
                f"--scenario {scenario_id} returned null first_query_ms without early_stop_reason"
            )
        collected.append(sample)
    return collected


def assemble(
    bin_cmd: list[str], out: Path, samples: int, collected: list[dict[str, object]]
) -> None:
    out.parent.mkdir(parents=True, exist_ok=True)
    completed = run_bin(
        bin_cmd,
        "--assemble",
        "--out",
        str(out),
        "--samples",
        str(samples),
        stdin=json.dumps(collected),
    )
    if completed.returncode != 0:
        raise RuntimeError(f"--assemble failed (exit {completed.returncode}):\n{completed.stderr}")
    if not out.is_file():
        raise RuntimeError(f"--assemble exited 0 but wrote no artifact at {out}")


def main() -> int:
    args = parse_args()

    if args.samples < 1:
        print("ERROR: --samples must be >= 1", file=sys.stderr)
        return 2

    bin_cmd = shlex.split(args.bin_cmd) if args.bin_cmd is not None else []
    if not bin_cmd:
        try:
            bin_cmd = resolve_default_bin_cmd()
        except (RuntimeError, OSError) as exc:
            print(f"ERROR: default cold-matrix binary resolution failed: {exc}", file=sys.stderr)
            return 2

    try:
        scenarios = discover_scenarios(bin_cmd)
    except (RuntimeError, OSError) as exc:
        print(f"ERROR: scenario discovery failed: {exc}", file=sys.stderr)
        return 2

    print(f"discovered {len(scenarios)} scenario(s); samples={args.samples}", file=sys.stderr)

    collected: list[dict[str, object]] = []
    try:
        for descriptor in scenarios:
            scenario_id = descriptor["scenario_id"]
            print(f"measuring {scenario_id} ...", file=sys.stderr)
            collected.extend(collect_samples(bin_cmd, scenario_id, args.samples))
        assemble(bin_cmd, args.out, args.samples, collected)
    except (RuntimeError, OSError, KeyError) as exc:
        print(f"ERROR: cold-matrix measurement failed: {exc}", file=sys.stderr)
        return 2

    print(f"assembled {len(collected)} sample(s) into {args.out}", file=sys.stderr)
    return 0


if __name__ == "__main__":
    sys.exit(main())
