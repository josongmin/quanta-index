"""Run one clean-source scale tier with opt-in causal markers and replay it.

The output root must be new and outside the checkout. A failed or timed-out
producer preserves its raw stdout/stderr and a FAILED execution record; it does
not create a successful causal report. Run tiers serially on the selected host.
"""

from __future__ import annotations

import argparse
import os
import subprocess
import time
from pathlib import Path

from tools.benchmark.evidence import RawFile
from tools.benchmark.retrieval.causal_cost_profile import replay
from tools.benchmark.retrieval.conditional_proof import canonical, sha
from tools.benchmark.retrieval.tool_custody import capture_executable


def _git(cwd: Path, *args: str) -> str:
    result = subprocess.run(
        ["git", "-C", str(cwd), *args], capture_output=True, text=True, check=True
    )
    return result.stdout.strip()


def _history_inputs(pair: int | None, total: int | None) -> dict:
    for label, value in (("history-max-bytes", pair), ("history-max-total-bytes", total)):
        if value is not None and (type(value) is not int or not 1 <= value <= (1 << 64) - 1):
            raise ValueError(f"{label} must be a positive u64")
    if total is not None and pair is None:
        raise ValueError("total history override requires an explicit pair bound")
    if pair is not None and pair > (total if total is not None else 2 * 1024 * 1024 * 1024):
        raise ValueError("pair history bound exceeds effective total history bound")
    return {
        "history_policy_id": (
            "explicit-pair-total-diagnostic-v1"
            if total is not None
            else "explicit-pair-default-total-v1"
            if pair is not None
            else "scale-supported-v1"
        ),
        "requested_history_max_bytes": pair,
        "history_max_bytes": pair if pair is not None else 1024 * 1024 * 1024,
        "requested_history_max_total_bytes": total,
        "history_max_total_bytes": total if total is not None else 2 * 1024 * 1024 * 1024,
    }


def _scale_command(args: argparse.Namespace, binary: Path, artifact_dir: Path) -> list[str]:
    command = [
        str(binary),
        "--tier",
        args.tier,
        "--seed",
        str(args.seed),
        "--out-dir",
        str(artifact_dir),
    ]
    if args.client_timeout_ms is not None:
        command.extend(["--client-timeout-ms", str(args.client_timeout_ms)])
    if args.history_max_bytes is not None:
        command.extend(["--history-max-bytes", str(args.history_max_bytes)])
    if args.history_max_total_bytes is not None:
        command.extend(["--history-max-total-bytes", str(args.history_max_total_bytes)])
    return command


def capture(args: argparse.Namespace) -> dict:
    history_policy = _history_inputs(args.history_max_bytes, args.history_max_total_bytes)
    cwd = args.cwd.resolve(strict=True)
    binary = args.binary.absolute()
    out_root = Path(os.path.realpath(args.out_root))
    if out_root == cwd or cwd in out_root.parents:
        raise ValueError("causal output root must be outside the checkout")
    if _git(cwd, "status", "--porcelain"):
        raise ValueError("causal producer requires a clean source checkout")
    head = _git(cwd, "rev-parse", "HEAD")
    if head != args.source_revision:
        raise ValueError("source revision does not match the clean checkout")
    binary_epoch = capture_executable(binary)
    binary_input = RawFile.capture(binary)
    binary_sha = binary_epoch["sha256"]
    if binary_sha != args.binary_sha256:
        raise ValueError("scale binary digest does not match declared input")
    if binary_input.sha256 != "sha256:" + binary_sha:
        raise ValueError("scale binary changed while preparing replay input")
    out_root.mkdir(mode=0o700, parents=False, exist_ok=False)
    artifact_dir = out_root / "artifact"
    command = _scale_command(args, binary, artifact_dir)
    environment = os.environ.copy()
    environment["QUANTA_INDEX_CAUSAL_PROFILE_V1"] = "1"
    if capture_executable(binary) != binary_epoch:
        raise ValueError("scale executable epoch changed before execution")
    started = time.monotonic()
    with (out_root / "stdout").open("xb") as stdout, (out_root / "stderr").open("xb") as stderr:
        try:
            result = subprocess.run(
                command,
                cwd=cwd,
                env=environment,
                stdout=stdout,
                stderr=stderr,
                timeout=args.max_seconds,
                check=False,
            )
            exit_code = result.returncode
            timed_out = False
        except subprocess.TimeoutExpired:
            exit_code = None
            timed_out = True
    elapsed_seconds = time.monotonic() - started
    trace_input = RawFile.capture(out_root / "stderr")
    stdout_input = RawFile.capture(out_root / "stdout")
    execution = {
        "schema_version": 1,
        "status": "FAILED",
        "source_revision": head,
        "source_revision_after": None,
        "dirty_after": None,
        "binary_sha256": binary_sha,
        "binary_sha256_after": None,
        "command": command,
        "history_policy": history_policy,
        "exit_code": exit_code,
        "timed_out": timed_out,
        "elapsed_seconds": elapsed_seconds,
        "stdout_sha256": stdout_input.sha256.removeprefix("sha256:"),
        "stderr_sha256": trace_input.sha256.removeprefix("sha256:"),
    }
    profile_path = out_root / "causal-profile.json"
    profile_created = False
    try:
        post_head = _git(cwd, "rev-parse", "HEAD")
        post_dirty = bool(_git(cwd, "status", "--porcelain"))
        post_binary_epoch = capture_executable(binary)
        execution.update(
            source_revision_after=post_head,
            dirty_after=post_dirty,
            binary_sha256_after=post_binary_epoch["sha256"],
        )
        if (
            exit_code != 0
            or timed_out
            or post_head != head
            or post_dirty
            or post_binary_epoch != binary_epoch
        ):
            raise ValueError("producer failed, timed out or source/binary changed")
        summary_input = RawFile.capture(artifact_dir / "summary.json")
        manifest_input = RawFile.capture(artifact_dir / "tier_manifest.json")
        inputs = (binary_input, trace_input, stdout_input, summary_input, manifest_input)

        def verify_inputs() -> None:
            if _git(cwd, "rev-parse", "HEAD") != head or _git(cwd, "status", "--porcelain"):
                raise ValueError("source changed during causal replay")
            if capture_executable(binary) != binary_epoch:
                raise ValueError("scale executable epoch changed during causal replay")
            for prepared in inputs:
                if RawFile.capture(prepared.path) != prepared:
                    raise ValueError(f"causal replay input changed: {prepared.path}")

        profile = replay(
            summary_input.read_control(),
            trace_input.consume_seekable(lambda stream: stream.read()),
            binary_input.consume_seekable(lambda stream: stream.read()),
            manifest_input.read_control(),
            source_revision=head,
            expected_tier=args.tier,
            expected_seed=args.seed,
            requested_client_timeout_ms=args.client_timeout_ms,
            requested_history_max_bytes=args.history_max_bytes,
            requested_history_max_total_bytes=args.history_max_total_bytes,
        )
        if any(
            profile["runtime_config"].get(key) != value for key, value in history_policy.items()
        ):
            raise ValueError("captured history policy differs from replayed scale policy")
        profile["scope"]["binary_source_binding"] = (
            "executable epoch and replay bytes are pinned through publication; fresh-build source "
            "custody must be established separately"
        )
        verify_inputs()
        profile_raw = canonical(profile) + b"\n"
        with profile_path.open("xb") as stream:
            profile_created = True
            stream.write(profile_raw)
        verify_inputs()
        if RawFile.capture(profile_path).sha256 != "sha256:" + sha(profile_raw):
            raise ValueError("causal profile changed during publication")
        execution["status"] = "VERIFIED_DIAGNOSTIC"
        execution["profile_sha256"] = sha(profile_raw)
    except (ValueError, OSError, KeyError, TypeError, subprocess.CalledProcessError) as error:
        execution["reason"] = str(error)
        if profile_created:
            try:
                profile_path.unlink(missing_ok=True)
            except OSError as cleanup_error:
                execution["cleanup_error"] = str(cleanup_error)
    (out_root / "execution.json").write_bytes(canonical(execution) + b"\n")
    return execution


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cwd", type=Path, required=True)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--source-revision", required=True)
    parser.add_argument("--binary-sha256", required=True)
    parser.add_argument("--tier", choices=("small", "medium", "large", "xlarge"), required=True)
    parser.add_argument("--seed", type=int, required=True)
    parser.add_argument("--client-timeout-ms", type=int)
    parser.add_argument("--history-max-bytes", type=int)
    parser.add_argument("--history-max-total-bytes", type=int)
    parser.add_argument("--max-seconds", type=int, required=True)
    parser.add_argument("--out-root", type=Path, required=True)
    args = parser.parse_args()
    try:
        if args.seed < 0 or args.seed >= 1 << 64 or args.max_seconds <= 0:
            raise ValueError("seed or max-seconds is out of range")
        if args.client_timeout_ms is not None and not 1 <= args.client_timeout_ms <= 600_000:
            raise ValueError("client-timeout-ms must be in 1..=600000")
        _history_inputs(args.history_max_bytes, args.history_max_total_bytes)
        if len(args.source_revision) != 40 or any(
            c not in "0123456789abcdef" for c in args.source_revision
        ):
            raise ValueError("source revision must be a full lowercase commit SHA")
        if len(args.binary_sha256) != 64 or any(
            c not in "0123456789abcdef" for c in args.binary_sha256
        ):
            raise ValueError("binary digest must be lowercase SHA-256")
        execution = capture(args)
    except (ValueError, OSError, subprocess.CalledProcessError) as error:
        parser.exit(2, f"causal capture refused: {error}\n")
    return 0 if execution["status"] == "VERIFIED_DIAGNOSTIC" else 2


if __name__ == "__main__":
    raise SystemExit(main())
