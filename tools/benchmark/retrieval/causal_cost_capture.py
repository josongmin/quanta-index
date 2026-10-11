"""Run one clean-source scale tier with a canonical causal capture.

The output root must be new and outside the checkout. A failed or timed-out
producer preserves its raw stdout/stderr and a FAILED execution record; it does
not create a successful causal report. Run tiers serially on the selected host.
"""

from __future__ import annotations

import argparse
import os
import shutil
import subprocess
import time
from pathlib import Path

from tools.benchmark.evidence import RawFile
from tools.benchmark.retrieval.causal_cost_profile import replay, validate_product_artifacts
from tools.benchmark.retrieval.causal_cost_timeline import MAX_CAPTURE_BYTES, chrome_export
from tools.benchmark.retrieval.causal_cost_timeline import parse as parse_timeline
from tools.benchmark.retrieval.causal_cost_timeline import summarize as summarize_timeline
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
        "--trace-mode",
        "record",
        "--trace-out",
        str(artifact_dir.parent / "causal-timeline.jsonl"),
        "--disk-observation",
        _disk_observation(args.disk_observation),
    ]
    if args.client_timeout_ms is not None:
        command.extend(["--client-timeout-ms", str(args.client_timeout_ms)])
    if args.history_max_bytes is not None:
        command.extend(["--history-max-bytes", str(args.history_max_bytes)])
    if args.history_max_total_bytes is not None:
        command.extend(["--history-max-total-bytes", str(args.history_max_total_bytes)])
    return command


def _disk_observation(requested: str | None) -> str:
    if requested is not None and requested not in ("boundary-only", "periodic"):
        raise ValueError("disk-observation must be boundary-only or periodic")
    return requested or "boundary-only"


def _write_new(path: Path, value: dict) -> None:
    with path.open("xb") as stream:
        stream.write(canonical(value) + b"\n")


def _unavailable_timeline_summary(status: dict) -> dict:
    """Keep the summary artifact explicit when no valid events can be replayed."""
    return {
        "schema_version": None,
        "status": status["capture_status"],
        "capture_id": None,
        "stream_sha256": status["input_sha256"],
        "event_count": None,
        "truncated_tail": None,
        "orphan_events": None,
        "spans": [],
        "sync_aggregates": [],
        "object_kind_aggregates": [],
        "adoption_group_aggregates": [],
        "bm25_census_aggregates": [],
        "bm25_boundary_aggregates": [],
        "bm25_live_build_aggregates": [],
        "links": [],
        "verified_flow_count": None,
        "ambiguous_link_count": None,
        "health": [],
        "loss": None,
        "unavailable_reason": status["reason"],
    }


def export_timeline(
    input_path: Path,
    out_root: Path,
    *,
    viewer: bool,
    execution: dict | None = None,
    legacy_aggregate: dict | None = None,
    raw_input: bytes | None = None,
) -> dict:
    """Export an immutable raw input without promoting partial evidence to PASS."""
    status: dict = {
        "schema_version": 1,
        "capture_status": "unavailable",
        "viewer_available": False,
        "clock_mapping": "single_process_capture_origin_only",
        "source_binding": "offline_unverified" if execution is None else "UNPROVEN",
        "input_sha256": None,
        "coverage": None,
        "reason": None,
    }
    summary = None
    if execution is not None:
        status["execution"] = {
            key: execution.get(key)
            for key in (
                "status",
                "product_status",
                "source_revision",
                "source_revision_after",
                "dirty_after",
                "binary_sha256",
                "binary_sha256_after",
                "summary_sha256",
                "tier_manifest_sha256",
                "timeline_sha256",
                "timeline_capture_id",
                "build_binding",
                "command",
                "history_policy",
                "disk_observation_mode",
                "exit_code",
                "timed_out",
                "elapsed_seconds",
                "stdout_sha256",
                "stderr_sha256",
            )
        }
    try:
        if raw_input is None and not input_path.exists():
            # Keep the fixed artifact path, while declaring that these empty
            # bytes were not emitted by the producer.
            input_path.touch(exist_ok=False)
            status["raw_placeholder"] = True
            raise ValueError("canonical timeline was not produced")
        status["raw_placeholder"] = False
        if raw_input is None:
            if input_path.stat().st_size > MAX_CAPTURE_BYTES:
                raise ValueError("causal timeline exceeds offline replay byte bound")
            raw = input_path.read_bytes()
        else:
            raw = raw_input
        if len(raw) > MAX_CAPTURE_BYTES:
            raise ValueError("causal timeline exceeds offline replay byte bound")
        status["input_sha256"] = sha(raw)
        capture = parse_timeline(raw)
        status.update(
            capture_status=capture["status"],
            capture_id=capture["capture_id"],
            stream_id=capture["stream_id"],
            clock_domain=capture["clock_domain"],
            coverage=capture["header"]["coverage"] if capture["status"] == "complete" else None,
            event_count=capture["event_count"],
            truncated_tail=capture["truncated_tail"],
            loss=None
            if capture["footer"] is None
            else {
                key: capture["footer"][key]
                for key in ("events_dropped", "events_unwritten", "open_spans")
            },
        )
        summary = summarize_timeline(capture, legacy_aggregate, execution_binding=execution)
        if viewer:
            rendered = chrome_export(capture, legacy_aggregate, execution_binding=execution)
            rendered["captureExecution"] = status.get("execution")
            output = canonical(rendered) + b"\n"
            with (out_root / "causal-waterfall.json").open("xb") as stream:
                stream.write(output)
            status["viewer_available"] = True
            status["viewer_sha256"] = sha(output)
    except (OSError, ValueError, KeyError, TypeError) as error:
        status["reason"] = str(error)
        if (
            status["capture_status"] == "unavailable"
            and input_path.exists()
            and not status.get("raw_placeholder", False)
        ):
            status["capture_status"] = "invalid"
    _write_new(
        out_root / "timeline-summary.json",
        summary if summary is not None else _unavailable_timeline_summary(status),
    )
    _write_new(out_root / "timeline-status.json", status)
    return status


def offline_export(input_path: Path, out_root: Path, *, viewer: bool = True) -> dict:
    out_root.mkdir(mode=0o700, parents=False, exist_ok=False)
    preserved = out_root / "causal-timeline.jsonl"
    if input_path.exists():
        with input_path.open("rb") as source, preserved.open("xb") as target:
            shutil.copyfileobj(source, target)
    status = export_timeline(preserved, out_root, viewer=viewer)
    _write_new(
        out_root / "execution.json",
        {
            "schema_version": 1,
            "status": "OFFLINE_EXPORT_UNVERIFIED",
            "product_status": "UNVERIFIED",
            "diagnostic_status": "UNVERIFIED",
            "capture_status": status["capture_status"],
            "producer_executed": False,
            "source_binding": "offline_unverified",
            "timeline_sha256": status["input_sha256"],
            "viewer_available": status["viewer_available"],
        },
    )
    return status


def capture(args: argparse.Namespace) -> dict:
    history_policy = _history_inputs(args.history_max_bytes, args.history_max_total_bytes)
    disk_observation = _disk_observation(args.disk_observation)
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
    timeline_path = out_root / "causal-timeline.jsonl"
    command = _scale_command(args, binary, artifact_dir)
    environment = os.environ.copy()
    # A canonical capture must not enable the legacy stderr marker pipeline,
    # even when the launching shell has opted into that older diagnostic path.
    environment.pop("QUANTA_INDEX_CAUSAL_PROFILE_V1", None)
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
        "product_status": "UNVERIFIED",
        "diagnostic_status": "NOT_RUN",
        "source_revision": head,
        "source_revision_after": None,
        "dirty_after": None,
        "binary_sha256": binary_sha,
        "binary_sha256_after": None,
        "summary_sha256": None,
        "tier_manifest_sha256": None,
        "build_binding": "UNPROVEN",
        "command": command,
        "history_policy": history_policy,
        "requested_disk_observation_mode": args.disk_observation,
        "disk_observation_mode": disk_observation,
        "exit_code": exit_code,
        "timed_out": timed_out,
        "elapsed_seconds": elapsed_seconds,
        "stdout_sha256": stdout_input.sha256.removeprefix("sha256:"),
        "stderr_sha256": trace_input.sha256.removeprefix("sha256:"),
        "timeline_sha256": None,
        "timeline_capture_id": None,
        "waterfall_sha256": None,
    }
    timeline = None
    timeline_bytes = None
    profile = None
    verifying_inputs = False
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
        execution["product_status"] = "EXIT_ZERO_UNQUALIFIED"
        summary_input = RawFile.capture(artifact_dir / "summary.json")
        manifest_input = RawFile.capture(artifact_dir / "tier_manifest.json")
        execution["summary_sha256"] = summary_input.sha256.removeprefix("sha256:")
        execution["tier_manifest_sha256"] = manifest_input.sha256.removeprefix("sha256:")

        def verify_product_inputs() -> None:
            if _git(cwd, "rev-parse", "HEAD") != head or _git(cwd, "status", "--porcelain"):
                execution["product_status"] = "UNVERIFIED"
                raise ValueError("source changed during causal replay")
            try:
                current_binary_epoch = capture_executable(binary)
            except OSError:
                execution["product_status"] = "UNVERIFIED"
                raise
            if current_binary_epoch != binary_epoch:
                execution["product_status"] = "UNVERIFIED"
                raise ValueError("scale executable epoch changed during causal replay")
            for prepared in (binary_input, summary_input, manifest_input):
                try:
                    current_input = RawFile.capture(prepared.path)
                except OSError:
                    execution["product_status"] = "UNVERIFIED"
                    raise
                if current_input != prepared:
                    execution["product_status"] = "UNVERIFIED"
                    raise ValueError(f"causal replay input changed: {prepared.path}")

        validate_product_artifacts(
            summary_input.read_control(),
            manifest_input.read_control(),
            source_revision=head,
            expected_tier=args.tier,
            expected_seed=args.seed,
            requested_client_timeout_ms=args.client_timeout_ms,
            requested_history_max_bytes=args.history_max_bytes,
            requested_history_max_total_bytes=args.history_max_total_bytes,
            requested_disk_observation_mode=args.disk_observation,
        )
        verify_product_inputs()
        execution["product_status"] = "PRODUCT_ARTIFACT_VERIFIED"

        timeline_input = RawFile.capture(timeline_path)
        execution["timeline_sha256"] = timeline_input.sha256.removeprefix("sha256:")
        if timeline_input.size > MAX_CAPTURE_BYTES:
            raise ValueError("causal timeline exceeds offline replay byte bound")
        timeline_bytes = timeline_input.consume_seekable(
            lambda stream: stream.read(MAX_CAPTURE_BYTES + 1)
        )
        if len(timeline_bytes) > MAX_CAPTURE_BYTES:
            raise ValueError("causal timeline exceeds offline replay byte bound")
        timeline = parse_timeline(timeline_bytes)
        execution["timeline_capture_id"] = timeline["capture_id"]
        if timeline["status"] != "complete":
            raise ValueError("canonical timeline is partial")

        def verify_inputs() -> None:
            verify_product_inputs()
            for prepared in (trace_input, stdout_input, timeline_input):
                if RawFile.capture(prepared.path) != prepared:
                    raise ValueError(f"causal replay input changed: {prepared.path}")

        legacy_error = None
        try:
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
                requested_disk_observation_mode=args.disk_observation,
            )
        except (ValueError, KeyError, TypeError) as error:
            legacy_error = str(error)
        if profile is not None:
            expected_phases = set(profile["phases"])
            observed_phases = {
                span["phase"]: span["outcome"]
                for span in timeline["spans"].values()
                if span["stage"] == "harness_phase" and "phase" in span
            }
            if set(observed_phases) != expected_phases or any(
                outcome != "ok" for outcome in observed_phases.values()
            ):
                legacy_error = "canonical phase identity/status differs from strict legacy profile"
                profile = None
            elif sum(
                span["stage"] == "harness_phase" for span in timeline["spans"].values()
            ) != len(expected_phases):
                legacy_error = "canonical timeline has repeated or unnamed phase spans"
                profile = None
        if profile is not None:
            if any(
                profile["runtime_config"].get(key) != value for key, value in history_policy.items()
            ):
                raise ValueError("captured history policy differs from replayed scale policy")
            if (
                profile["runtime_config"].get("requested_disk_observation_mode")
                != args.disk_observation
                or profile["runtime_config"].get("disk_observation_mode") != disk_observation
            ):
                raise ValueError(
                    "captured disk observation mode differs from replayed scale policy"
                )
            profile["timeline_sha256"] = timeline_input.sha256.removeprefix("sha256:")
            profile["timeline_capture_id"] = timeline["capture_id"]
            profile["tier_manifest_sha256"] = manifest_input.sha256.removeprefix("sha256:")
            profile["stderr_sha256"] = trace_input.sha256.removeprefix("sha256:")
            profile["phase_identity_parity"] = "verified"
            profile["scope"]["legacy_aggregate_source"] = "stderr QI_CAUSAL_V1 markers"
        verifying_inputs = True
        verify_inputs()
        verifying_inputs = False
        execution["status"] = "PARTIAL_DIAGNOSTIC"
        execution["diagnostic_status"] = "PARTIAL"
        execution["reason"] = (
            "canonical sync/BM25/process-I/O/owner-duration parity with legacy aggregates "
            "is not established; causal-profile.json withheld"
        )
        if legacy_error is not None:
            execution["legacy_replay_error"] = legacy_error
    except (ValueError, OSError, KeyError, TypeError, subprocess.CalledProcessError) as error:
        execution["reason"] = str(error)
        execution["diagnostic_status"] = "PARTIAL" if timeline is not None else "UNAVAILABLE"
        if execution["product_status"] == "PRODUCT_ARTIFACT_VERIFIED" and not verifying_inputs:
            try:
                verify_product_inputs()
            except (ValueError, OSError, subprocess.CalledProcessError) as binding_error:
                execution["product_status"] = "UNVERIFIED"
                execution["reason"] = str(binding_error)
        if (
            exit_code == 0
            and not timed_out
            and not verifying_inputs
            and execution["product_status"] == "PRODUCT_ARTIFACT_VERIFIED"
        ):
            execution["status"] = "PARTIAL_DIAGNOSTIC"
        profile = None
    status = export_timeline(
        timeline_path,
        out_root,
        viewer=getattr(args, "timeline", False),
        execution=execution,
        legacy_aggregate=profile,
        raw_input=timeline_bytes if timeline_bytes is not None else None,
    )
    execution["timeline_sha256"] = status["input_sha256"]
    execution["viewer_available"] = status["viewer_available"]
    if status.get("viewer_sha256"):
        execution["waterfall_sha256"] = status["viewer_sha256"]
    if status["capture_status"] != "complete" or (
        getattr(args, "timeline", False) and not status["viewer_available"]
    ):
        execution["diagnostic_status"] = "PARTIAL"
        if execution["status"] == "PARTIAL_DIAGNOSTIC":
            execution["reason"] = status["reason"] or "canonical timeline is partial"
    (out_root / "execution.json").write_bytes(canonical(execution) + b"\n")
    return execution


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cwd", type=Path)
    parser.add_argument("--binary", type=Path)
    parser.add_argument("--source-revision")
    parser.add_argument("--binary-sha256")
    parser.add_argument("--tier", choices=("small", "medium", "large", "xlarge"))
    parser.add_argument("--seed", type=int)
    parser.add_argument("--client-timeout-ms", type=int)
    parser.add_argument("--history-max-bytes", type=int)
    parser.add_argument("--history-max-total-bytes", type=int)
    parser.add_argument("--disk-observation", choices=("boundary-only", "periodic"))
    parser.add_argument("--max-seconds", type=int)
    parser.add_argument("--out-root", type=Path, required=True)
    parser.add_argument("--timeline", action="store_true", help="write Chrome waterfall viewer")
    parser.add_argument(
        "--offline-input", type=Path, help="export existing canonical JSONL without executing"
    )
    args = parser.parse_args()
    try:
        if args.offline_input is not None:
            if any(
                value is not None
                for value in (
                    args.cwd,
                    args.binary,
                    args.source_revision,
                    args.binary_sha256,
                    args.tier,
                    args.seed,
                    args.max_seconds,
                )
            ):
                raise ValueError("offline export cannot take producer execution inputs")
            status = offline_export(args.offline_input, args.out_root, viewer=True)
            return 0 if status["viewer_available"] else 2
        if any(
            value is None
            for value in (
                args.cwd,
                args.binary,
                args.source_revision,
                args.binary_sha256,
                args.tier,
                args.seed,
                args.max_seconds,
            )
        ):
            raise ValueError(
                "capture requires cwd, binary, source revision/digest, tier, seed and timeout"
            )
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
