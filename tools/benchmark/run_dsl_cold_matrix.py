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
configuration digest, the host and the peak RSS. The Rust producer writes the
artifact and supplies its source identity.

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

The default prebuilds the workspace harness. For a frozen Legacy release,
pass its prebuilt binary, checkout, commit and binary SHA-256 together.
With ``--bin-mode warm``, the pinned warm binary takes ``--out`` directly.
Native SDK diagnostics use a pinned runner, daemon and reopened History
fixture. Their raw scenario reports have a separate schema from the DSL
authority artifact.
"""

from __future__ import annotations

import argparse
import json
import math
import os
import shlex
import subprocess
import sys
from collections.abc import Callable
from functools import partial
from pathlib import Path
from tempfile import TemporaryDirectory

DEFAULT_SAMPLES = 20
DEFAULT_OUT = Path("artifacts/dsl-bench/cold-matrix.json")
DEFAULT_BUILD_CMD = (
    "env CARGO_NET_OFFLINE=true ./scripts/cargow --lane bench-lane build -p quanta-index-searchd-harness "
    "--bin dsl_cold_matrix --features legacy-bench --profile bench --quiet --locked"
)
TARGET_DIR_CMD = (
    "export QUANTA_INDEX_BUILD_LANE=bench-lane; "
    "source scripts/quanta-index-env.sh; "
    'printf "%s" "$CARGO_TARGET_DIR"'
)
BinRunner = Callable[..., subprocess.CompletedProcess[str]]


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--samples",
        type=int,
        help=(f"number of fresh-process cold samples per scenario (default: {DEFAULT_SAMPLES})"),
    )
    parser.add_argument(
        "--out",
        type=Path,
        help=f"output artifact path (workspace default: {DEFAULT_OUT})",
    )
    parser.add_argument(
        "--bin-cmd",
        default=None,
        help=(
            "shell-splittable command prefix for the harness binary; when omitted, "
            "the script prebuilds dsl_cold_matrix once and invokes the binary directly"
        ),
    )
    parser.add_argument("--bin-cwd", type=Path, help="frozen Legacy producer checkout")
    parser.add_argument("--bin-head", help="expected full commit at --bin-cwd")
    parser.add_argument("--bin-sha256", help="expected sha256:<hex> of the absolute producer binary")
    parser.add_argument(
        "--bin-mode", choices=("cold", "warm"), default="cold",
        help="use the warm binary for a frozen release, or the default cold matrix",
    )
    parser.add_argument("--native-runner", type=Path)
    parser.add_argument(
        "--native-artifact", action="store_true",
        help="assemble the captured Native samples with the pinned Rust BenchArtifactV1 writer",
    )
    parser.add_argument("--native-daemon", type=Path)
    parser.add_argument("--native-daemon-sha256")
    parser.add_argument("--native-history-fixture", type=Path)
    parser.add_argument("--native-history-sha256")
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
    bin_cmd: list[str], *extra: str, stdin: str | None = None, cwd: Path | None = None,
    env: dict[str, str] | None = None,
) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [*bin_cmd, *extra],
        check=False,
        capture_output=True,
        text=True,
        input=stdin,
        cwd=cwd,
        env=env,
    )


def discover_scenarios(run: BinRunner) -> list[dict[str, object]]:
    completed = run("--list")
    if completed.returncode != 0:
        raise RuntimeError(f"--list failed (exit {completed.returncode}):\n{completed.stderr}")
    try:
        scenarios = json.loads(completed.stdout)
    except json.JSONDecodeError as exc:
        raise RuntimeError(f"--list emitted non-JSON stdout: {exc}") from exc
    if not isinstance(scenarios, list):
        raise RuntimeError("--list must emit a JSON array")
    return scenarios


def sample_scenario(run: BinRunner, scenario_id: str) -> dict[str, object]:
    completed = run("--scenario", scenario_id)
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


def collect_samples(run: BinRunner, scenario_id: str, samples: int) -> list[dict[str, object]]:
    """Every cold sample of one scenario, one fresh process each.

    An early-stopped scenario is recorded from its first sample alone: the
    assembler writes it as an unmeasured row, never a guessed latency.
    """
    first = sample_scenario(run, scenario_id)
    if first.get("early_stop_reason") is not None:
        print(
            f"  {scenario_id}: early_stop ({first['early_stop_reason']}) — recording unmeasured row",
            file=sys.stderr,
        )
        return [first]
    collected = [first]
    for index in range(1, samples):
        sample = sample_scenario(run, scenario_id)
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
    run: BinRunner, out: Path, samples: int, collected: list[dict[str, object]]
) -> None:
    out.parent.mkdir(parents=True, exist_ok=True)
    completed = run(
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


def measure(run: BinRunner, samples: int, out: Path) -> int:
    scenarios = discover_scenarios(run)
    print(f"discovered {len(scenarios)} scenario(s); samples={samples}", file=sys.stderr)
    collected: list[dict[str, object]] = []
    for descriptor in scenarios:
        scenario_id = descriptor["scenario_id"]
        print(f"measuring {scenario_id} ...", file=sys.stderr)
        collected.extend(collect_samples(run, scenario_id, samples))
    assemble(run, out, samples, collected)
    return len(collected)


def measure_frozen(args: argparse.Namespace, bin_cmd: list[str]) -> int:
    from benchctl import require_frozen_source
    from compare_dsl_bench import ArtifactRefused, load_artifact
    from evidence import EvidenceError, RawFile

    if not (args.bin_cwd and args.bin_head and args.bin_sha256):
        raise RuntimeError("frozen capture requires --bin-cwd, --bin-head and --bin-sha256")
    if len(bin_cmd) != 1 or not Path(bin_cmd[0]).is_absolute():
        raise RuntimeError("frozen capture requires one absolute executable path in --bin-cmd")
    if args.out is None:
        raise RuntimeError("frozen capture requires an explicit --out")
    checkout = args.bin_cwd.resolve(strict=True)
    out = args.out.absolute()
    if out.resolve().is_relative_to(checkout):
        raise RuntimeError("frozen capture output must stay outside the producer checkout")
    require_frozen_source(checkout, args.bin_head)

    try:
        binary = RawFile.capture(Path(bin_cmd[0]))
        if binary.sha256 != args.bin_sha256:
            raise RuntimeError("frozen producer binary SHA-256 differs")
        out.parent.mkdir(parents=True, exist_ok=True)
        with TemporaryDirectory(dir=out.parent) as scratch:
            staged = Path(scratch) / out.name
            env = dict(os.environ, QUANTA_INDEX_BENCH_DISABLE_QUERY_OBS="1")
            run = partial(run_bin, bin_cmd, cwd=checkout, env=env)
            sample_count = None
            if args.bin_mode == "warm":
                completed = run("--out", str(staged))
                if completed.returncode:
                    raise RuntimeError(
                        f"warm producer failed (exit {completed.returncode}):\n{completed.stderr}"
                    )
            else:
                sample_count = measure(run, args.samples, staged)
            artifact = load_artifact(staged, role=f"frozen {args.bin_mode} producer")
            if artifact.git_head != args.bin_head or artifact.mode != args.bin_mode:
                raise RuntimeError("frozen producer emitted an artifact for a different commit or mode")
            require_frozen_source(checkout, args.bin_head)
            if RawFile.capture(binary.path) != binary:
                raise RuntimeError("frozen producer binary changed during capture")
            staged.replace(out)
            return len(artifact.rows) if sample_count is None else sample_count
    except (ArtifactRefused, EvidenceError) as exc:
        raise RuntimeError(f"frozen {args.bin_mode} producer refused: {exc}") from exc


def native_json(run: BinRunner, description: str, *argv: str) -> dict:
    from evidence import parse_json

    completed = run(*argv)
    if completed.returncode:
        raise RuntimeError(f"{description} failed (exit {completed.returncode}):\n{completed.stderr}")
    if not completed.stdout.strip():
        raise RuntimeError(f"missing {description}")
    payload = parse_json(completed.stdout)
    if not isinstance(payload, dict):
        raise RuntimeError(f"{description} must be a JSON object")
    return payload


def validate_native_report(
    payload: dict, kind: str, scenario: str, mode: str, daemon_sha: str, history_config_sha: str,
) -> None:
    if kind == "history":
        report = payload.get("history")
        if payload.get("schema") != f"native-dsl-history-process-{mode}-v1":
            raise RuntimeError(f"wrong Native History report mode for {scenario}")
        if payload.get("config_sha256") != history_config_sha:
            raise RuntimeError(f"Native History config SHA-256 differs for {scenario}")
    else:
        report = payload
    if payload.get("daemon_sha256") != daemon_sha.removeprefix("sha256:"):
        raise RuntimeError(f"Native DSL daemon SHA-256 differs for {scenario}")
    if (
        not isinstance(report, dict)
        or report.get("schema") != f"native-dsl-{kind}-samples-v1"
        or report.get("scenario_id") != scenario
    ):
        raise RuntimeError(f"wrong Native SDK scenario report for {scenario}")
    passes, per_pass, prime = (report.get(key) for key in ("passes", "samples_per_pass", "prime_queries"))
    samples = report.get("samples_ms")
    if (
        any(type(value) is not int for value in (passes, per_pass, prime))
        or passes < 1 or per_pass < 1 or prime < 0
        or not isinstance(samples, list) or len(samples) != passes * per_pass
        or any(type(value) not in (int, float) or not 0 <= value < math.inf for value in samples)
    ):
        raise RuntimeError(f"invalid Native SDK samples for {scenario}")
    if mode == "cold" and (passes, per_pass, prime) != (1, 1, 0):
        raise RuntimeError(f"cold sample must contain one unprimed SDK query for {scenario}")


def assemble_native(run: BinRunner, matrix: Path, out: Path, mode: str, head: str) -> None:
    from compare_dsl_bench import ArtifactRefused, load_artifact
    from evidence import RawFile, _read_control_file, parse_json

    commitment = RawFile.capture(matrix)
    completed = run("--assemble-native", "--input", str(matrix), "--out", str(out))
    if completed.returncode:
        raise RuntimeError(f"Native DSL artifact producer failed (exit {completed.returncode}):\n{completed.stderr}")
    try:
        content = _read_control_file(out).decode("utf-8")
        payload = parse_json(content)
        artifact = load_artifact(out, role="Native DSL artifact", content=content)
    except (ArtifactRefused, OSError, ValueError) as exc:
        raise RuntimeError(f"Native DSL artifact refused: {exc}") from exc
    if (
        artifact.mode != mode or artifact.git_head != head
        or artifact.qualification != "diagnostic"
        or payload["detail"].get("native_matrix_sha256") != commitment.sha256
        or RawFile.capture(matrix) != commitment
    ):
        raise RuntimeError("Native DSL artifact differs from the collected source, mode or matrix")


def measure_native(args: argparse.Namespace, bin_cmd: list[str]) -> int:
    from benchctl import require_frozen_source
    from evidence import CONTROL_DOCUMENT_BYTES, DIGEST_RE, RawFile, parse_json

    if len(bin_cmd) != 1 or args.out is None:
        raise RuntimeError("Native DSL requires one --bin-cmd executable and an explicit --out")
    if bool(args.bin_cwd) != bool(args.bin_head):
        raise RuntimeError("Native DSL source pin requires both --bin-cwd and --bin-head")
    if args.native_artifact and not args.bin_cwd:
        raise RuntimeError("Native DSL artifact requires a frozen --bin-cwd and --bin-head")
    cwd = args.bin_cwd.resolve(strict=True) if args.bin_cwd else None
    out = args.out.resolve()
    checkout = cwd or Path(__file__).resolve().parents[2]
    if out.is_relative_to(checkout):
        raise RuntimeError("Native DSL output must stay outside the producer checkout")
    if cwd is not None:
        require_frozen_source(cwd, args.bin_head)

    def pin(path: Path | None, expected: str | None, name: str) -> RawFile:
        if path is None or not path.is_absolute() or expected is None:
            raise RuntimeError(f"Native DSL requires an absolute {name} path and SHA-256")
        pinned = RawFile.capture(path)
        if pinned.sha256 != expected:
            raise RuntimeError(f"Native DSL {name} SHA-256 differs")
        if pinned.path.resolve() == out:
            raise RuntimeError(f"Native DSL output overlaps the {name} input")
        return pinned

    runner = pin(Path(bin_cmd[0]), args.bin_sha256, "runner")
    daemon = pin(args.native_daemon, args.native_daemon_sha256, "daemon")
    history = pin(args.native_history_fixture, args.native_history_sha256, "History fixture")
    if history.size > CONTROL_DOCUMENT_BYTES:
        raise RuntimeError("Native DSL History fixture exceeds the control file byte limit")
    fixture = history.consume_seekable(lambda stream: parse_json(stream.read().decode("utf-8")))
    if not isinstance(fixture, dict):
        raise RuntimeError("Native DSL History fixture must be a JSON object")
    history_daemon = fixture.get("daemon")
    if (
        not isinstance(history_daemon, str)
        or not Path(history_daemon).is_absolute()
        or Path(history_daemon).resolve() != daemon.path.resolve()
        or fixture.get("daemon_sha256") != daemon.sha256.removeprefix("sha256:")
    ):
        raise RuntimeError("Native DSL History fixture targets a different daemon")
    history_config_sha = fixture.get("config_sha256")
    if not isinstance(history_config_sha, str) or not DIGEST_RE.fullmatch(f"sha256:{history_config_sha}"):
        raise RuntimeError("Native DSL History fixture requires a config SHA-256")

    run = partial(run_bin, [str(runner.path)], cwd=cwd, env=dict(os.environ, QUANTA_INDEX_BENCH_DISABLE_QUERY_OBS="1"))
    roster = native_json(run, "Native DSL roster", "--list-native")
    if set(roster) != {"text", "history"}:
        raise RuntimeError("Native DSL roster requires exactly Text and History")

    seen = set()
    for kind in ("text", "history"):
        scenarios = roster[kind]
        if not isinstance(scenarios, list) or len(scenarios) != 7:
            raise RuntimeError(f"Native DSL {kind.title()} roster requires exactly 7 scenarios")
        for scenario in scenarios:
            if not isinstance(scenario, str) or not scenario or scenario in seen:
                raise RuntimeError(f"invalid or duplicate Native DSL scenario {scenario!r}")
            seen.add(scenario)

    mode = args.bin_mode
    cold = mode == "cold"
    routes = {
        "text": ("--native-text", daemon.path),
        "history": ("--native-history" if cold else "--native-history-reopen", history.path),
    }
    rows = []
    for kind, (flag, source) in routes.items():
        for scenario in roster[kind]:
            reports = []
            for _ in range(args.samples if cold else 1):
                report = native_json(run, f"SDK report for {scenario}", flag, str(source), scenario)
                validate_native_report(report, kind, scenario, mode, daemon.sha256, history_config_sha)
                reports.append(report)
            rows.append({"scenario_id": scenario, "kind": kind, "reports": reports})

    if cwd is not None:
        require_frozen_source(cwd, args.bin_head)
    for name, pinned in (("runner", runner), ("daemon", daemon), ("History fixture", history)):
        if RawFile.capture(pinned.path) != pinned:
            raise RuntimeError(f"Native DSL {name} changed during capture")
    artifact = {
        "schema": "native-dsl-matrix-v1",
        "qualification": "diagnostic",
        "mode": mode,
        "git_head": args.bin_head,
        "runner_sha256": runner.sha256,
        "daemon_sha256": daemon.sha256,
        "history_fixture_sha256": history.sha256,
        "scenarios": rows,
    }
    out.parent.mkdir(parents=True, exist_ok=True)
    with TemporaryDirectory(dir=out.parent) as scratch:
        staged = Path(scratch) / out.name
        staged.write_text(json.dumps(artifact, allow_nan=False) + "\n", encoding="utf-8")
        if args.native_artifact:
            assembled = Path(scratch) / "assembled.json"
            assemble_native(run, staged, assembled, mode, args.bin_head)
            require_frozen_source(cwd, args.bin_head)
            for name, pinned in (("runner", runner), ("daemon", daemon), ("History fixture", history)):
                if RawFile.capture(pinned.path) != pinned:
                    raise RuntimeError(f"Native DSL {name} changed during assembly")
            staged = assembled
        staged.replace(out)
    return len(rows)


def main() -> int:
    args = parse_args()

    if args.bin_mode == "warm" and args.samples is not None:
        print("ERROR: --samples is only valid for the cold matrix", file=sys.stderr)
        return 2
    if args.samples is None:
        args.samples = DEFAULT_SAMPLES
    if args.samples < 1:
        print("ERROR: --samples must be >= 1", file=sys.stderr)
        return 2

    try:
        bin_cmd = shlex.split(args.bin_cmd) if args.bin_cmd is not None else []
        native_inputs = (
            args.native_runner, args.native_artifact, args.native_daemon, args.native_daemon_sha256,
            args.native_history_fixture, args.native_history_sha256,
        )
        if any(native_inputs):
            if args.native_runner is not None:
                if bin_cmd:
                    raise RuntimeError("Native DSL accepts only one of --native-runner and --bin-cmd")
                bin_cmd = [str(args.native_runner)]
            count = measure_native(args, bin_cmd)
            print(f"wrote {count} Native SDK scenarios to {args.out}", file=sys.stderr)
            return 0
        if args.bin_mode == "warm" or any((args.bin_cwd, args.bin_head, args.bin_sha256)):
            count = measure_frozen(args, bin_cmd)
        else:
            args.out = args.out or DEFAULT_OUT
            run = partial(run_bin, bin_cmd or resolve_default_bin_cmd())
            count = measure(run, args.samples, args.out)
    except (RuntimeError, OSError, ValueError, KeyError) as exc:
        print(f"ERROR: {args.bin_mode}-matrix measurement failed: {exc}", file=sys.stderr)
        return 2

    if args.bin_mode == "warm":
        print(f"wrote {count} warm scenario(s) to {args.out}", file=sys.stderr)
    else:
        print(f"assembled {count} sample(s) into {args.out}", file=sys.stderr)
    return 0


if __name__ == "__main__":
    sys.exit(main())
