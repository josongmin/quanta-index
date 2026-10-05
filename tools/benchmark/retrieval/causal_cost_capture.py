"""Run one clean-source scale tier with opt-in causal markers and replay it.

The output root must be new and outside the checkout. A failed or timed-out
producer preserves its raw stdout/stderr and a FAILED execution record; it does
not create a successful causal report. Run tiers serially on the selected host.
"""

from __future__ import annotations

import argparse
import hashlib
import os
import stat
import subprocess
import time
from pathlib import Path

from tools.benchmark.retrieval.causal_cost_profile import replay
from tools.benchmark.retrieval.conditional_proof import canonical, sha


def _git(cwd: Path, *args: str) -> str:
    result = subprocess.run(
        ["git", "-C", str(cwd), *args], capture_output=True, text=True, check=True
    )
    return result.stdout.strip()


def _binary_sha(path: Path) -> str:
    if not stat.S_ISREG(path.lstat().st_mode):
        raise ValueError("scale binary must be a regular file without a symlink")
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def capture(args: argparse.Namespace) -> dict:
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
    binary_sha = _binary_sha(binary)
    if binary_sha != args.binary_sha256:
        raise ValueError("scale binary digest does not match declared input")
    out_root.mkdir(mode=0o700, parents=False, exist_ok=False)
    artifact_dir = out_root / "artifact"
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
    environment = os.environ.copy()
    environment["QUANTA_INDEX_CAUSAL_PROFILE_V1"] = "1"
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
    post_head = _git(cwd, "rev-parse", "HEAD")
    post_dirty = bool(_git(cwd, "status", "--porcelain"))
    post_binary_sha = _binary_sha(binary)
    trace_raw = (out_root / "stderr").read_bytes()
    stdout_raw = (out_root / "stdout").read_bytes()
    execution = {
        "schema_version": 1,
        "status": "FAILED",
        "source_revision": head,
        "source_revision_after": post_head,
        "dirty_after": post_dirty,
        "binary_sha256": binary_sha,
        "binary_sha256_after": post_binary_sha,
        "command": command,
        "exit_code": exit_code,
        "timed_out": timed_out,
        "elapsed_seconds": elapsed_seconds,
        "stdout_sha256": sha(stdout_raw),
        "stderr_sha256": sha(trace_raw),
    }
    try:
        if (
            exit_code != 0
            or timed_out
            or post_head != head
            or post_dirty
            or post_binary_sha != binary_sha
        ):
            raise ValueError("producer failed, timed out or source/binary changed")
        summary_raw = (artifact_dir / "summary.json").read_bytes()
        manifest_raw = (artifact_dir / "tier_manifest.json").read_bytes()
        profile = replay(
            summary_raw, trace_raw, binary.read_bytes(), manifest_raw,
            source_revision=head,
            expected_tier=args.tier,
            expected_seed=args.seed,
            requested_client_timeout_ms=args.client_timeout_ms,
            requested_history_max_bytes=args.history_max_bytes,
        )
        profile["scope"]["binary_source_binding"] = (
            "binary bytes are pinned before and after execution; matching fresh-build source "
            "custody must be established separately"
        )
        (out_root / "causal-profile.json").write_bytes(canonical(profile) + b"\n")
        execution["status"] = "VERIFIED_DIAGNOSTIC"
        execution["profile_sha256"] = sha((out_root / "causal-profile.json").read_bytes())
    except (ValueError, OSError, KeyError, TypeError) as error:
        execution["reason"] = str(error)
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
    parser.add_argument("--max-seconds", type=int, required=True)
    parser.add_argument("--out-root", type=Path, required=True)
    args = parser.parse_args()
    try:
        if args.seed < 0 or args.seed >= 1 << 64 or args.max_seconds <= 0:
            raise ValueError("seed or max-seconds is out of range")
        if args.client_timeout_ms is not None and not 1 <= args.client_timeout_ms <= 600_000:
            raise ValueError("client-timeout-ms must be in 1..=600000")
        if args.history_max_bytes is not None and not 1 <= args.history_max_bytes <= 256 * 1024 * 1024:
            raise ValueError("history-max-bytes must be in 1..=268435456")
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
