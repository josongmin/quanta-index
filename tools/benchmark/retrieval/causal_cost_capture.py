"""Run a clean-source tier with canonical trace or explicit Native Off observation.

The output root must be new and outside the checkout. A failed or timed-out
producer preserves its raw stdout/stderr and a FAILED execution record; it does
not create a successful causal report. Run tiers serially on the selected host.
"""

from __future__ import annotations

import argparse
import json
import os
import signal
import stat
import subprocess
import sys
import threading
import time
from pathlib import Path

from tools.benchmark.evidence import RawFile, _consume_regular_file
from tools.benchmark.producer_execution import ProducerExecutionError, _execute_owned
from tools.benchmark.retrieval import linux_process
from tools.benchmark.retrieval.causal_cost_build import build_scale, source_capture
from tools.benchmark.retrieval.causal_cost_canonical import canonical_profile
from tools.benchmark.retrieval.causal_cost_profile import (
    replay,
    validate_native_product_artifacts,
    validate_product_artifacts,
)
from tools.benchmark.retrieval.causal_cost_timeline import (
    MAX_CAPTURE_BYTES,
    MAX_CONFIGURED_CAPTURE_BYTES,
    chrome_export,
)
from tools.benchmark.retrieval.causal_cost_timeline import parse as parse_timeline
from tools.benchmark.retrieval.causal_cost_timeline import (
    parse_stream as parse_timeline_stream,
)
from tools.benchmark.retrieval.causal_cost_timeline import summarize as summarize_timeline
from tools.benchmark.retrieval.conditional_proof import canonical, load, sha
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


def _native_inputs(args: argparse.Namespace) -> dict | None:
    if type(getattr(args, "native_mixed_load", False)) is not bool:
        raise ValueError("native mixed load must be an explicit boolean selection")
    selected_trace_mode = getattr(args, "native_trace_mode", None)
    trace_limit = getattr(args, "trace_max_output_bytes", None)
    if trace_limit is not None and (
        type(trace_limit) is not int or not 6144 <= trace_limit <= MAX_CONFIGURED_CAPTURE_BYTES
    ):
        raise ValueError("trace-max-output-bytes must be in 6144..=1073741824")
    if selected_trace_mode not in (None, "off", "record"):
        raise ValueError("native trace mode must be off or record")
    if args.backend not in ("legacy", "index-native"):
        raise ValueError("unknown causal capture backend")
    if args.backend == "legacy":
        if (
            args.native_daemon_bin is not None
            or args.native_daemon_sha256 is not None
            or args.native_work_limits_json is not None
            or args.native_churn_cycles != 0
            or getattr(args, "native_mixed_load", False)
            or selected_trace_mode is not None
            or trace_limit is not None
            or getattr(args, "linux_cgroup_parent", None) is not None
        ):
            raise ValueError("native capture inputs require --backend index-native")
        return None
    if (
        args.client_timeout_ms is not None
        or args.history_max_bytes is not None
        or args.history_max_total_bytes is not None
        or args.disk_observation not in (None, "boundary-only")
    ):
        raise ValueError("native capture does not accept legacy policy overrides")
    if type(args.native_churn_cycles) is not int or not 0 <= args.native_churn_cycles <= 256:
        raise ValueError("native churn cycles must be in 0..=256")
    limits = {"reads": 1_000_000, "writes": 1_000_000, "bytes": 536_870_912, "mutations": 1_000_000}
    mode = "fixed"
    if args.native_work_limits_json is not None:
        supplied = load(args.native_work_limits_json.encode())
        if (
            not isinstance(supplied, dict)
            or set(supplied) != set(limits)
            or any(type(value) is not int or not 0 < value < 1 << 64 for value in supplied.values())
        ):
            raise ValueError("native work limits require exactly four positive u64 axes")
        limits, mode = supplied, "explicit"
    elif args.tier in ("large", "xlarge"):
        raise ValueError("fixed native funding does not admit Large/XL; declare explicit limits")
    if getattr(args, "native_mixed_load", False) and (
        args.tier != "large"
        or args.seed != 42
        or mode != "explicit"
        or limits
        != {
            "reads": 67_108_864,
            "writes": 67_108_864,
            "bytes": 549_755_813_888,
            "mutations": 67_108_864,
        }
    ):
        raise ValueError(
            "native mixed load requires Large seed42 and the fixed explicit work limits"
        )
    if getattr(args, "native_mixed_load", False):
        if selected_trace_mode == "record":
            raise ValueError("Native mixed load requires Off trace mode")
        if getattr(args, "timeline", False):
            raise ValueError("Native mixed Off cannot produce a canonical timeline viewer")
        if type(args.max_seconds) is not int or args.max_seconds <= 960:
            raise ValueError("Native mixed 60s+900s run requires max-seconds above 960")
    elif selected_trace_mode == "off" and getattr(args, "timeline", False):
        raise ValueError("Native Off cannot produce a canonical timeline viewer")
    if trace_limit is not None and _native_trace_mode(args) != "record":
        raise ValueError("trace-max-output-bytes requires Native Record mode")
    return {"mode": mode, "work_limits": limits}


def _native_trace_mode(args: argparse.Namespace) -> str:
    return (
        "off"
        if getattr(args, "native_mixed_load", False)
        else (getattr(args, "native_trace_mode", None) or "record")
    )


def _linux_cgroup_parent(args: argparse.Namespace) -> tuple[Path, tuple[int, int]] | None:
    """Freeze the explicit delegated parent before any managed build or producer launch."""
    supplied = getattr(args, "linux_cgroup_parent", None)
    if supplied is None:
        return None
    if args.backend != "index-native" or sys.platform != "linux":
        raise ValueError("Linux cgroup capture requires the Native backend on Linux")
    if threading.current_thread() is not threading.main_thread():
        raise ValueError("Linux cgroup capture requires the signal-owning main thread")
    if not isinstance(supplied, Path) or not supplied.is_absolute():
        raise ValueError("Linux cgroup parent must be an absolute path")
    canonical_parent = supplied.resolve(strict=True)
    if canonical_parent != supplied:
        raise ValueError("Linux cgroup parent must not contain symlinks")
    identity = supplied.stat()
    if not stat.S_ISDIR(identity.st_mode):
        raise ValueError("Linux cgroup parent is not a directory")
    return supplied, (identity.st_dev, identity.st_ino)


def _linux_execution_observation(
    result: linux_process.ProcessResult,
    parent: Path,
    identity: tuple[int, int],
) -> dict:
    """Keep sampled RSS distinct from kernel-accounted cgroup memory.peak."""
    current = parent.stat()
    if (current.st_dev, current.st_ino) != identity:
        raise ValueError("delegated cgroup parent identity changed during capture")
    if (
        not isinstance(result, linux_process.ProcessResult)
        or not result.ownership_complete
        or result.cgroup_path is None
        or Path(result.cgroup_path).parent != parent
        or type(result.root_exit_code) is not int
        or type(result.timed_out) is not bool
        or type(result.samples) is not int
        or result.samples < 1
    ):
        raise ValueError("qualified Linux capture lacks complete process ownership evidence")
    return {
        "owner": "linux-process-owner-v1",
        "scope": "scale_harness_and_owned_descendants_until_cgroup_empty",
        "cgroup_parent": str(parent),
        "cgroup_parent_dev": identity[0],
        "cgroup_parent_ino": identity[1],
        "cgroup_path": result.cgroup_path,
        "root_pid": result.root.pid,
        "root_start_ticks": result.root.start_ticks,
        "sample_interval_ms": result.sample_interval_ms,
        "sample_count": result.samples,
        "aggregate_rss_sampled_max_bytes": result.peak_tree_rss_bytes,
        "aggregate_rss_method": result.rss_method,
        "cgroup_memory_peak_bytes": result.peak_cgroup_memory_bytes,
        "cgroup_memory_method": "cgroup_v2_memory.peak_kernel_accounted_memory_not_rss",
        "cgroup_cpu_usage_ns": result.cgroup_cpu_usage_ns,
        "ownership_complete": True,
        "migration_caveat": "unobserved_cgroup_migration_requires_external_prevention",
        "continuous_rss_peak": False,
        "allocated_disk_peak": None,
    }


def _native_environment(workload: dict, churn: int, *, mixed: bool = False) -> dict[str, str]:
    selected = {"QUANTA_INDEX_NATIVE_RESOURCE_SAMPLING_V1": "1"}
    if workload["mode"] == "explicit":
        selected["QUANTA_INDEX_NATIVE_WORK_LIMITS_JSON"] = canonical(
            workload["work_limits"]
        ).decode()
    if churn:
        selected["QUANTA_INDEX_NATIVE_CHURN_CYCLES"] = str(churn)
    if mixed:
        selected["QUANTA_INDEX_NATIVE_MIXED_LOAD_V1"] = "1"
    for key in (
        "QUANTA_INDEX_NATIVE_WORK_LIMITS_JSON",
        "QUANTA_INDEX_NATIVE_CHURN_CYCLES",
        "QUANTA_INDEX_NATIVE_RESOURCE_SAMPLING_V1",
        "QUANTA_INDEX_NATIVE_MIXED_LOAD_V1",
        "QUANTA_INDEX_NATIVE_RECORD_CAPTURE_V1",
    ):
        inherited = os.environ.get(key)
        if inherited is not None and inherited != selected.get(key):
            raise ValueError(f"inherited {key} differs from the explicit native capture profile")
    return selected


def _native_child_inputs(
    summary: bytes, out_root: Path, tier: str, *, trace_max_output_bytes: int | None = None
) -> dict[str, dict]:
    """Only consume the two fixed process paths owned by this invocation."""
    directory = out_root / f"causal-timeline.jsonl.native-{tier}-children"
    parsed = load(summary)
    if not isinstance(parsed, dict) or not isinstance(parsed.get("detail"), dict):
        raise ValueError("native summary detail is missing")
    detail = parsed["detail"].get("child_trace_files")
    if not isinstance(detail, dict) or detail.get("directory") != str(directory):
        raise ValueError("native raw directory differs from this capture invocation")
    rows = detail.get("captures")
    if not isinstance(rows, list) or len(rows) != 2:
        raise ValueError("native capture requires two raw process epochs")
    result = {}
    for row in rows:
        if not isinstance(row, dict) or not isinstance(row.get("stderr_tail"), dict):
            raise ValueError("native raw process record is malformed")
        role = row.get("role")
        if role not in ("first", "reopen") or role in result:
            raise ValueError("native raw capture role is invalid or repeated")
        trace_path, stderr_path = directory / f"{role}.jsonl", directory / f"{role}.stderr"
        if row.get("path") != str(trace_path) or row.get("stderr_tail", {}).get("path") != str(
            stderr_path
        ):
            raise ValueError("native raw paths differ from the fixed process epoch")
        child_limit = trace_max_output_bytes if trace_max_output_bytes is not None else 67_108_864
        if trace_path.lstat().st_size > child_limit:
            raise ValueError("native child trace exceeds its declared byte bound")
        trace_file = RawFile.capture(trace_path)
        if trace_file.size > child_limit:
            raise ValueError("native child trace exceeds its declared byte bound")
        trace = trace_file.consume_seekable(
            lambda stream, limit=child_limit: parse_timeline_stream(stream, max_capture_bytes=limit)
        )
        result[role] = {
            "trace_file": trace_file,
            "trace": trace,
            "stderr_file": RawFile.capture(stderr_path),
        }
    return result


def _scale_command(
    args: argparse.Namespace, binary: Path, artifact_dir: Path, *, native_daemon: Path | None = None
) -> list[str]:
    trace_mode = _native_trace_mode(args)
    command = [
        str(binary),
        "--tier",
        args.tier,
        "--seed",
        str(args.seed),
        "--out-dir",
        str(artifact_dir),
        "--trace-mode",
        trace_mode,
    ]
    if trace_mode == "record":
        command.extend(["--trace-out", str(artifact_dir.parent / "causal-timeline.jsonl")])
        trace_limit = getattr(args, "trace_max_output_bytes", None)
        if trace_limit is not None:
            command.extend(["--trace-max-output-bytes", str(trace_limit)])
    command.extend(["--disk-observation", _disk_observation(args.disk_observation)])
    if args.client_timeout_ms is not None:
        command.extend(["--client-timeout-ms", str(args.client_timeout_ms)])
    if args.history_max_bytes is not None:
        command.extend(["--history-max-bytes", str(args.history_max_bytes)])
    if args.history_max_total_bytes is not None:
        command.extend(["--history-max-total-bytes", str(args.history_max_total_bytes)])
    if args.backend == "index-native":
        if native_daemon is None:
            raise ValueError("native capture command has no bound daemon")
        command.extend(["--backend", "index-native", "--native-daemon-bin", str(native_daemon)])
    elif native_daemon is not None:
        raise ValueError("legacy capture cannot execute a native daemon")
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
    raw_file: RawFile | None = None,
    max_capture_bytes: int | None = None,
    expected_absent: bool = False,
    absent_scope: str = "Native mixed Off",
) -> dict:
    """Export an immutable raw input without promoting partial evidence to PASS."""
    max_capture_bytes = MAX_CAPTURE_BYTES if max_capture_bytes is None else max_capture_bytes
    if (
        type(max_capture_bytes) is not int
        or not 0 < max_capture_bytes <= MAX_CONFIGURED_CAPTURE_BYTES
    ):
        raise ValueError("causal capture byte bound must be in 1..=1073741824")
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
                "product_status",
                "source_revision",
                "source_revision_after",
                "dirty_after",
                "binary_sha256",
                "binary_sha256_after",
                "backend",
                "native_daemon_sha256",
                "native_daemon_sha256_after",
                "native_foreground_workload",
                "native_churn_cycles",
                "native_mixed_load",
                "native_trace_mode",
                "native_child_inputs",
                "summary_sha256",
                "tier_manifest_sha256",
                "timeline_sha256",
                "timeline_capture_id",
                "build_binding",
                "build_receipt_sha256",
                "command",
                "history_policy",
                "disk_observation_mode",
                "exit_code",
                "timed_out",
                "producer_error",
                "elapsed_seconds",
                "stdout_sha256",
                "stderr_sha256",
            )
        }
        if execution.get("backend") == "index-native":
            status["execution"]["native_temporary_root"] = execution.get(
                "native_temporary_root"
            )
            status["execution"]["linux_resource_observation"] = execution.get(
                "linux_resource_observation"
            )
        if "trace_max_output_bytes" in execution:
            status["execution"]["trace_max_output_bytes"] = execution["trace_max_output_bytes"]
        # execution.json owns the final verdict after export has finished.
        # This envelope carries only input facts available before that step.
        if (
            execution.get("build_binding") == "OBSERVED_LOCAL_BUILD"
            and execution.get("status")
            in ("PARTIAL_DIAGNOSTIC", "VERIFIED_DIAGNOSTIC", "VERIFIED_PRODUCT_OBSERVATION")
            and execution.get("product_status") == "PRODUCT_ARTIFACT_VERIFIED"
            and isinstance(execution.get("source_revision"), str)
            and isinstance(execution.get("binary_sha256"), str)
            and execution.get("source_revision_after") == execution.get("source_revision")
            and execution.get("binary_sha256_after") == execution.get("binary_sha256")
            and execution.get("dirty_after") is False
            and (
                execution.get("backend") != "index-native"
                or (
                    execution.get("native_daemon_sha256") is not None
                    and execution.get("native_daemon_sha256_after")
                    == execution.get("native_daemon_sha256")
                    and (
                        (
                            execution.get("native_trace_mode") == "off"
                            and "native_child_inputs" not in execution
                        )
                        or (
                            execution.get("native_trace_mode") == "record"
                            and isinstance(execution.get("native_child_inputs"), dict)
                            and set(execution["native_child_inputs"]) == {"first", "reopen"}
                        )
                    )
                )
            )
        ):
            status["source_binding"] = "observed_local_build_and_execution"
    try:
        if expected_absent:
            if input_path.exists():
                status["capture_status"] = "invalid"
                status["raw_placeholder"] = False
                raise ValueError(f"{absent_scope} unexpectedly produced a canonical timeline")
            input_path.touch(exist_ok=False)
            status["raw_placeholder"] = True
            raise ValueError(f"{absent_scope} has no canonical timeline")
        if raw_input is None and raw_file is None and not input_path.exists():
            # Keep the fixed artifact path, while declaring that these empty
            # bytes were not emitted by the producer.
            input_path.touch(exist_ok=False)
            status["raw_placeholder"] = True
            raise ValueError("canonical timeline was not produced")
        status["raw_placeholder"] = False
        if raw_input is not None and raw_file is not None:
            raise ValueError("timeline export has conflicting raw input custody")
        if raw_file is None and raw_input is None:
            raw_file = RawFile.capture(input_path)
        if raw_file is not None:
            if raw_file.path != input_path.absolute() or raw_file.size > max_capture_bytes:
                raise ValueError(
                    "causal timeline exceeds offline replay byte bound or differs from input"
                )
            status["input_sha256"] = raw_file.sha256.removeprefix("sha256:")
            capture = raw_file.consume_seekable(
                lambda stream: parse_timeline_stream(stream, max_capture_bytes=max_capture_bytes)
            )
            if capture.get("sha256") != status["input_sha256"]:
                raise ValueError("parsed timeline differs from committed raw bytes")
        else:
            if len(raw_input) > max_capture_bytes:
                raise ValueError("causal timeline exceeds offline replay byte bound")
            status["input_sha256"] = sha(raw_input)
            capture = parse_timeline(raw_input, max_capture_bytes=max_capture_bytes)
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


def offline_export(
    input_path: Path, out_root: Path, *, viewer: bool = True, max_capture_bytes: int | None = None
) -> dict:
    explicitly_configured = max_capture_bytes is not None
    max_capture_bytes = MAX_CAPTURE_BYTES if max_capture_bytes is None else max_capture_bytes
    if (
        type(max_capture_bytes) is not int
        or not 0 < max_capture_bytes <= MAX_CONFIGURED_CAPTURE_BYTES
    ):
        raise ValueError("offline capture byte bound must be in 1..=1073741824")
    raw_file = None
    if input_path.is_symlink():
        raise ValueError("offline timeline must be a regular file")
    if input_path.exists():

        def read_bounded(source) -> None:
            if os.fstat(source.fileno()).st_size > max_capture_bytes:
                raise ValueError("offline timeline exceeds replay byte bound")
            count = 0
            while block := source.read(min(1024 * 1024, max_capture_bytes - count + 1)):
                count += len(block)
                if count > max_capture_bytes:
                    raise ValueError("offline timeline exceeds replay byte bound")

        _consume_regular_file(input_path, read_bounded)
        raw_file = RawFile.capture(input_path)
        if raw_file.size > max_capture_bytes:
            raise ValueError("offline timeline exceeds replay byte bound")
    out_root.mkdir(mode=0o700, parents=False, exist_ok=False)
    preserved = out_root / "causal-timeline.jsonl"
    if raw_file is not None:
        copied = raw_file.copy_to(preserved)
        if copied.sha256 != raw_file.sha256 or copied.size != raw_file.size:
            raise ValueError("offline copy differs from bounded input")
    status = export_timeline(
        preserved,
        out_root,
        viewer=viewer,
        raw_file=copied if raw_file is not None else None,
        max_capture_bytes=max_capture_bytes,
    )
    execution = {
        "schema_version": 1,
        "status": "OFFLINE_EXPORT_UNVERIFIED",
        "product_status": "UNVERIFIED",
        "diagnostic_status": "UNVERIFIED",
        "capture_status": status["capture_status"],
        "producer_executed": False,
        "source_binding": "offline_unverified",
        "timeline_sha256": status["input_sha256"],
        "viewer_available": status["viewer_available"],
    }
    if explicitly_configured:
        execution["trace_max_output_bytes"] = max_capture_bytes
    _write_new(out_root / "execution.json", execution)
    return status


def _preflight_native_socket_path(out_root: Path) -> None:
    # Native daemon state lives in Rust tempfile::TempDir::new(), whose
    # default directory name is .tmp followed by six random characters.
    # sockaddr_un.sun_path also needs one byte for the trailing NUL.
    limit = 103 if sys.platform == "darwin" else 107 if sys.platform.startswith("linux") else None
    if limit is None:
        return
    socket_path = out_root / "native-tmp" / ".tmpXXXXXX" / "native.sock"
    path_bytes = len(os.fsencode(socket_path))
    if path_bytes > limit:
        raise ValueError(
            f"Native capture output root is too long for Unix socket pathname "
            f"({path_bytes} > {limit} bytes); choose a shorter out-root"
        )


def capture(args: argparse.Namespace) -> dict:
    history_policy = _history_inputs(args.history_max_bytes, args.history_max_total_bytes)
    native_workload = _native_inputs(args)
    linux_parent = _linux_cgroup_parent(args)
    native_environment = (
        {}
        if native_workload is None
        else _native_environment(
            native_workload,
            args.native_churn_cycles,
            mixed=getattr(args, "native_mixed_load", False),
        )
    )
    mixed = bool(getattr(args, "native_mixed_load", False))
    trace_mode = _native_trace_mode(args)
    trace_limit = getattr(args, "trace_max_output_bytes", None)
    parser_limit = trace_limit if trace_limit is not None else MAX_CAPTURE_BYTES
    off = native_workload is not None and trace_mode == "off"
    if native_workload is not None:
        history_policy = None
    disk_observation = _disk_observation(args.disk_observation)
    cwd = args.cwd.resolve(strict=True)
    out_root = Path(os.path.realpath(args.out_root))
    if out_root == cwd or cwd in out_root.parents:
        raise ValueError("causal output root must be outside the checkout")
    if native_workload is not None:
        _preflight_native_socket_path(out_root)
    if _git(cwd, "status", "--porcelain"):
        raise ValueError("causal producer requires a clean source checkout")
    head = _git(cwd, "rev-parse", "HEAD")
    if head != args.source_revision:
        raise ValueError("source revision does not match the clean checkout")
    managed = getattr(args, "managed_build", False)
    build = None
    build_input = None
    if managed:
        if (
            args.binary is not None
            or args.binary_sha256 is not None
            or args.native_daemon_bin is not None
            or args.native_daemon_sha256 is not None
        ):
            raise ValueError("managed build cannot accept a caller-supplied binary/digest")
        out_root.mkdir(mode=0o700, parents=False, exist_ok=False)
        build = build_scale(cwd, out_root / "build", backend=args.backend)
        if build["source_revision"] != head or build["status"] != "OBSERVED_LOCAL_BUILD":
            raise ValueError("managed build source/status differs from capture source")
        build_input = RawFile.capture(out_root / "build/build-receipt.json")
        if json.loads(build_input.read_control()) != build:
            raise ValueError("managed build receipt differs from observed build")
        binary = Path(build["binary"])
        expected_binary_sha = build["binary_sha256"]
    else:
        binary = args.binary.absolute()
        expected_binary_sha = args.binary_sha256
    native_daemon = None
    daemon_epoch = None
    daemon_input = None
    if native_workload is not None:
        if build is not None:
            if build.get("backend") != "index-native" or build.get("build_profile") != "bench":
                raise ValueError("native capture requires an observed bench-profile binary pair")
            daemon_build = build["native_daemon"]
            native_daemon = Path(daemon_build["binary"])
            expected_daemon_sha = daemon_build["binary_sha256"]
        else:
            if args.native_daemon_bin is None or args.native_daemon_sha256 is None:
                raise ValueError("native capture requires a daemon/digest or a managed binary pair")
            native_daemon = args.native_daemon_bin.absolute()
            expected_daemon_sha = args.native_daemon_sha256
        daemon_epoch = capture_executable(native_daemon)
        daemon_input = RawFile.capture(native_daemon)
        if (
            daemon_epoch["sha256"] != expected_daemon_sha
            or daemon_input.sha256 != "sha256:" + expected_daemon_sha
            or (build is not None and daemon_epoch != daemon_build["binary_epoch"])
        ):
            raise ValueError("native daemon differs from its declared or observed build binding")
    binary_epoch = capture_executable(binary)
    binary_input = RawFile.capture(binary)
    binary_sha = binary_epoch["sha256"]
    if binary_sha != expected_binary_sha:
        raise ValueError("scale binary digest does not match declared input")
    if build is not None and (
        binary_epoch != build["binary_epoch"]
        or source_capture(cwd, head) != build["source_identity"]
    ):
        raise ValueError("managed build source/executable epoch changed before execution")
    if binary_input.sha256 != "sha256:" + binary_sha:
        raise ValueError("scale binary changed while preparing replay input")
    if not managed:
        out_root.mkdir(mode=0o700, parents=False, exist_ok=False)
    native_temporary_root = None
    if native_workload is not None:
        native_temporary_root = out_root / "native-tmp"
        native_temporary_root.mkdir(mode=0o700, exist_ok=False)
    artifact_dir = out_root / "artifact"
    timeline_path = out_root / "causal-timeline.jsonl"
    command = _scale_command(args, binary, artifact_dir, native_daemon=native_daemon)
    environment = os.environ.copy()
    # A canonical capture must not enable the legacy stderr marker pipeline,
    # even when the launching shell has opted into that older diagnostic path.
    environment.pop("QUANTA_INDEX_CAUSAL_PROFILE_V1", None)
    environment.update(native_environment)
    if native_temporary_root is not None:
        environment["TMPDIR"] = str(native_temporary_root)
    if capture_executable(binary) != binary_epoch:
        raise ValueError("scale executable epoch changed before execution")
    if native_daemon is not None and capture_executable(native_daemon) != daemon_epoch:
        raise ValueError("native daemon executable epoch changed before execution")
    started = time.monotonic()
    producer_error = None
    interrupted = None
    linux_observation = None
    if linux_parent is not None:
        parent, identity = linux_parent
        stdout_path, stderr_path = out_root / "stdout", out_root / "stderr"
        terminated = False

        def on_terminate(_signum, _frame):
            nonlocal terminated
            if not terminated:
                terminated = True
                raise ProducerExecutionError("benchmark producer interrupted by SIGTERM")

        try:
            previous_handler = signal.getsignal(signal.SIGTERM)
            try:
                signal.signal(signal.SIGTERM, on_terminate)
                owned = linux_process.run(
                    command,
                    cwd=str(cwd),
                    env=environment,
                    timeout_secs=args.max_seconds,
                    stdout_path=str(stdout_path),
                    stderr_path=str(stderr_path),
                    qualified=True,
                    cgroup_parent=str(parent),
                    absolute_deadline=started + args.max_seconds,
                )
            finally:
                signal.signal(signal.SIGTERM, previous_handler)
            linux_observation = _linux_execution_observation(owned, parent, identity)
            exit_code, timed_out = owned.root_exit_code, owned.timed_out
        except (
            linux_process.ProcessError,
            ProducerExecutionError,
            OSError,
            ValueError,
            subprocess.TimeoutExpired,
        ) as error:
            exit_code = None
            timed_out = False
            producer_error = (
                str(error) if terminated else f"qualified Linux process ownership failed: {error}"
            )
        except KeyboardInterrupt as error:
            exit_code = None
            timed_out = False
            producer_error = "benchmark producer interrupted by SIGINT"
            interrupted = error
        # Cgroup setup can fail before linux_process.run opens either raw sink.
        # Retain explicit empty files for that failed execution, never success.
        for path in (stdout_path, stderr_path):
            if not path.exists():
                with path.open("xb"):
                    pass
    else:
        with (out_root / "stdout").open("xb") as stdout, (out_root / "stderr").open("xb") as stderr:
            try:
                result = _execute_owned(
                    command,
                    cwd=cwd,
                    env=environment,
                    sinks=(stdout, stderr),
                    timeout=args.max_seconds,
                )
                exit_code = result["exit_code"]
                timed_out = False
            except ProducerExecutionError as error:
                exit_code = None
                timed_out = isinstance(error.__cause__, subprocess.TimeoutExpired)
                producer_error = str(error)
                if isinstance(error.__cause__, KeyboardInterrupt):
                    interrupted = error.__cause__
                    producer_error = f"benchmark producer interrupted by SIGINT; {producer_error}"
            except KeyboardInterrupt as error:
                exit_code = None
                timed_out = False
                producer_error = "benchmark producer interrupted by SIGINT"
                interrupted = error
    elapsed_seconds = time.monotonic() - started
    trace_input = RawFile.capture(out_root / "stderr")
    stdout_input = RawFile.capture(out_root / "stdout")
    execution = {
        "schema_version": 1,
        "status": "FAILED",
        "backend": args.backend,
        "product_status": "UNVERIFIED",
        "diagnostic_status": "NOT_RUN",
        "source_revision": head,
        "source_revision_after": None,
        "dirty_after": None,
        "binary_sha256": binary_sha,
        "binary_sha256_after": None,
        "native_daemon_sha256": None if daemon_epoch is None else daemon_epoch["sha256"],
        "native_daemon_sha256_after": None,
        "native_foreground_workload": native_workload,
        "native_churn_cycles": args.native_churn_cycles,
        "native_mixed_load": mixed,
        "native_trace_mode": trace_mode if native_workload is not None else None,
        "native_environment": native_environment,
        "summary_sha256": None,
        "tier_manifest_sha256": None,
        "build_binding": "UNPROVEN" if build is None else build["status"],
        "build_receipt_sha256": None if build_input is None else build_input.sha256,
        "command": command,
        "history_policy": history_policy,
        "requested_disk_observation_mode": args.disk_observation,
        "disk_observation_mode": disk_observation,
        "exit_code": exit_code,
        "timed_out": timed_out,
        "producer_error": producer_error,
        "linux_resource_observation": linux_observation,
        "elapsed_seconds": elapsed_seconds,
        "stdout_sha256": stdout_input.sha256.removeprefix("sha256:"),
        "stderr_sha256": trace_input.sha256.removeprefix("sha256:"),
        "timeline_sha256": None,
        "timeline_capture_id": None,
        "waterfall_sha256": None,
    }
    if native_temporary_root is not None:
        execution["native_temporary_root"] = str(native_temporary_root)
    if trace_limit is not None:
        execution["trace_max_output_bytes"] = trace_limit
    timeline = None
    timeline_input = None
    profile = None
    verifying_inputs = False
    child_inputs = {}
    try:
        post_head = _git(cwd, "rev-parse", "HEAD")
        post_dirty = bool(_git(cwd, "status", "--porcelain"))
        post_binary_epoch = capture_executable(binary)
        post_daemon_epoch = None if native_daemon is None else capture_executable(native_daemon)
        execution.update(
            source_revision_after=post_head,
            dirty_after=post_dirty,
            binary_sha256_after=post_binary_epoch["sha256"],
            native_daemon_sha256_after=(
                None if post_daemon_epoch is None else post_daemon_epoch["sha256"]
            ),
        )
        if producer_error is not None:
            raise ValueError(producer_error)
        if (
            exit_code != 0
            or timed_out
            or post_head != head
            or post_dirty
            or post_binary_epoch != binary_epoch
            or post_daemon_epoch != daemon_epoch
        ):
            raise ValueError("producer failed, timed out or source/binary changed")
        if build is not None and source_capture(cwd, head) != build["source_identity"]:
            raise ValueError("managed source inventory changed during execution")
        if build_input is not None and RawFile.capture(build_input.path) != build_input:
            raise ValueError("managed build receipt changed during execution")
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
            except (ValueError, OSError):
                execution["product_status"] = "UNVERIFIED"
                raise
            if current_binary_epoch != binary_epoch:
                execution["product_status"] = "UNVERIFIED"
                raise ValueError("scale executable epoch changed during causal replay")
            if native_daemon is not None:
                try:
                    current_daemon_epoch = capture_executable(native_daemon)
                except (ValueError, OSError):
                    execution["product_status"] = "UNVERIFIED"
                    raise
                if current_daemon_epoch != daemon_epoch:
                    execution["product_status"] = "UNVERIFIED"
                    raise ValueError("native daemon executable epoch changed during causal replay")
            raw_children = [
                record[key]
                for record in child_inputs.values()
                for key in ("trace_file", "stderr_file")
            ]
            prepared_inputs = [binary_input, summary_input, manifest_input, *raw_children]
            if daemon_input is not None:
                prepared_inputs.append(daemon_input)
            if off:
                # Off has no canonical replay. Its product observation is the
                # terminal proof, so freeze every reported raw input here.
                prepared_inputs.extend((stdout_input, trace_input))
                if build_input is not None:
                    prepared_inputs.append(build_input)
            for prepared in prepared_inputs:
                try:
                    current_input = RawFile.capture(prepared.path)
                except (ValueError, OSError):
                    execution["product_status"] = "UNVERIFIED"
                    raise
                if current_input != prepared:
                    execution["product_status"] = "UNVERIFIED"
                    raise ValueError(f"causal replay input changed: {prepared.path}")

        if native_workload is None:
            product = validate_product_artifacts(
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
        else:
            child_directory = out_root / f"causal-timeline.jsonl.native-{args.tier}-children"
            if off and os.path.lexists(child_directory):
                raise ValueError("Native Off unexpectedly produced a child capture directory")
            child_inputs = (
                {}
                if off
                else _native_child_inputs(
                    summary_input.read_control(),
                    out_root,
                    args.tier,
                    trace_max_output_bytes=trace_limit,
                )
            )
            product = validate_native_product_artifacts(
                summary_input.read_control(),
                manifest_input.read_control(),
                source_revision=head,
                expected_tier=args.tier,
                expected_seed=args.seed,
                expected_daemon_sha256=daemon_epoch["sha256"],
                expected_foreground_workload=native_workload,
                expected_churn_cycles=args.native_churn_cycles,
                expected_mixed_load=mixed,
                expected_trace_mode=trace_mode,
                expected_trace_max_output_bytes=trace_limit,
                expected_resource_sampling=True,
                child_records=child_inputs,
                linux_resource_observation=linux_observation,
            )
        verify_product_inputs()
        if off and timeline_path.exists():
            scope = "Native mixed Off" if mixed else "Native ordinary Off"
            raise ValueError(f"{scope} unexpectedly produced a canonical timeline")
        execution["product_status"] = "PRODUCT_ARTIFACT_VERIFIED"
        if child_inputs:
            execution["native_child_inputs"] = {
                role: {
                    "trace_path": str(record["trace_file"].path),
                    "trace_sha256": record["trace_file"].sha256.removeprefix("sha256:"),
                    "trace_bytes": record["trace_file"].size,
                    "stderr_path": str(record["stderr_file"].path),
                    "stderr_sha256": record["stderr_file"].sha256.removeprefix("sha256:"),
                    "stderr_bytes": record["stderr_file"].size,
                    "capture_id": record["trace"]["capture_id"],
                }
                for role, record in child_inputs.items()
            }

        if off:
            execution["status"] = (
                "VERIFIED_PRODUCT_OBSERVATION" if build is not None else "PARTIAL_DIAGNOSTIC"
            )
            execution["diagnostic_status"] = "NOT_APPLICABLE"
            execution["canonical_profile_status"] = "NOT_APPLICABLE_OFF_NO_CAPTURE"
            if mixed:
                execution["reason"] = (
                    "Native mixed Off product observation verified; canonical trace, child capture "
                    "and Perfetto diagnostic not applicable; performance qualification NOT_RUN"
                    if build is not None
                    else "Native mixed Off product artifact verified; prebuilt source/build binding "
                    "UNPROVEN; canonical diagnostic not applicable; performance qualification NOT_RUN"
                )
            else:
                execution["reason"] = (
                    "Native ordinary Off product observation verified; canonical trace and child "
                    "capture not applicable; performance qualification NOT_RUN"
                    if build is not None
                    else "Native ordinary Off product artifact verified; prebuilt source/build binding "
                    "UNPROVEN; canonical diagnostic not applicable; performance qualification NOT_RUN"
                )
        else:
            timeline_input = RawFile.capture(timeline_path)
            execution["timeline_sha256"] = timeline_input.sha256.removeprefix("sha256:")
            if timeline_input.size > parser_limit:
                raise ValueError("causal timeline exceeds offline replay byte bound")
            timeline = timeline_input.consume_seekable(
                lambda stream: parse_timeline_stream(stream, max_capture_bytes=parser_limit)
            )
            execution["timeline_capture_id"] = timeline["capture_id"]
            if timeline["status"] != "complete":
                raise ValueError("canonical timeline is partial")

            def verify_inputs() -> None:
                nonlocal verifying_inputs
                verifying_inputs = True
                verify_product_inputs()
                if build_input is not None and RawFile.capture(build_input.path) != build_input:
                    raise ValueError("managed build receipt changed during causal replay")
                for prepared in (trace_input, stdout_input, timeline_input):
                    if RawFile.capture(prepared.path) != prepared:
                        raise ValueError(f"causal replay input changed: {prepared.path}")
                verifying_inputs = False

            legacy_error = None if native_workload is None else "NOT_APPLICABLE_NATIVE"
            try:
                if native_workload is None:
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
                    legacy_error = (
                        "canonical phase identity/status differs from strict legacy profile"
                    )
                    profile = None
                elif sum(
                    span["stage"] == "harness_phase" for span in timeline["spans"].values()
                ) != len(expected_phases):
                    legacy_error = "canonical timeline has repeated or unnamed phase spans"
                    profile = None
            if profile is not None:
                if any(
                    profile["runtime_config"].get(key) != value
                    for key, value in history_policy.items()
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
            verify_inputs()
            execution["status"] = "PARTIAL_DIAGNOSTIC"
            execution["diagnostic_status"] = "PARTIAL"
            execution["reason"] = (
                "canonical sync/BM25/process-I/O/owner-duration parity with legacy aggregates "
                "is not established; causal-profile.json withheld"
                if native_workload is None
                else "Native parent phases and independent child captures require canonical validation"
            )
            if legacy_error is not None:
                execution["legacy_replay_error"] = legacy_error
            # Legacy timer equivalence is a separate claim. A canonical-only profile
            # can be validated using its typed metrics and product resource oracle.
            try:
                canonical_result = canonical_profile(timeline, product, execution_binding=execution)
            except (ValueError, KeyError, TypeError) as error:
                execution["canonical_profile_status"] = "UNAVAILABLE"
                execution["canonical_profile_reason"] = str(error)
                verify_inputs()
            else:
                verify_inputs()
                if trace_limit is not None:
                    if product.get("trace_max_output_bytes") != trace_limit:
                        raise ValueError(
                            "canonical profile trace cap differs from product contract"
                        )
                    canonical_result["trace_max_output_bytes"] = trace_limit
                _write_new(out_root / "canonical-profile.json", canonical_result)
                execution["canonical_profile_status"] = "VERIFIED"
                execution["canonical_profile_sha256"] = RawFile.capture(
                    out_root / "canonical-profile.json"
                ).sha256
                if build is not None:
                    execution["status"] = "VERIFIED_DIAGNOSTIC"
                    execution["diagnostic_status"] = "CANONICAL_PROFILE_VERIFIED"
                    execution["reason"] = (
                        "bound canonical diagnostic only; legacy timer equivalence and performance qualification NOT_RUN"
                        if native_workload is None
                        else "bound Native binary pair and independent-clock diagnostic; performance qualification NOT_RUN"
                    )
                else:
                    execution["reason"] = (
                        "canonical diagnostic verified; prebuilt source/build binding UNPROVEN; "
                        "performance qualification NOT_RUN"
                    )
    except (ValueError, OSError, KeyError, TypeError, subprocess.CalledProcessError) as error:
        execution["reason"] = str(error)
        execution["diagnostic_status"] = "PARTIAL" if timeline is not None else "UNAVAILABLE"
        if verifying_inputs:
            execution["status"] = "FAILED"
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
    timeline = None
    status = export_timeline(
        timeline_path,
        out_root,
        viewer=getattr(args, "timeline", False),
        execution=execution,
        legacy_aggregate=profile,
        raw_file=timeline_input,
        max_capture_bytes=parser_limit,
        expected_absent=off,
        absent_scope="Native mixed Off" if mixed else "Native ordinary Off",
    )
    execution["timeline_sha256"] = status["input_sha256"]
    execution["viewer_available"] = status["viewer_available"]
    if status.get("viewer_sha256"):
        execution["waterfall_sha256"] = status["viewer_sha256"]
    expected_off_absence = (
        off
        and status.get("raw_placeholder") is True
        and status["capture_status"] == "unavailable"
        and status["reason"]
        == ("Native mixed Off" if mixed else "Native ordinary Off") + " has no canonical timeline"
    )
    if (
        status["capture_status"] != "complete"
        or status["reason"] is not None
        or (getattr(args, "timeline", False) and not status["viewer_available"])
    ) and not expected_off_absence:
        execution["diagnostic_status"] = "PARTIAL"
        if execution["status"] in (
            "PARTIAL_DIAGNOSTIC",
            "VERIFIED_DIAGNOSTIC",
            "VERIFIED_PRODUCT_OBSERVATION",
        ):
            execution["status"] = "PARTIAL_DIAGNOSTIC"
            execution["reason"] = status["reason"] or "canonical timeline is partial"
    (out_root / "execution.json").write_bytes(canonical(execution) + b"\n")
    if interrupted is not None:
        raise interrupted
    return execution


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cwd", type=Path)
    parser.add_argument("--binary", type=Path)
    parser.add_argument("--source-revision")
    parser.add_argument("--binary-sha256")
    parser.add_argument("--backend", choices=("legacy", "index-native"), default="legacy")
    parser.add_argument("--native-daemon-bin", type=Path)
    parser.add_argument("--native-daemon-sha256")
    parser.add_argument(
        "--linux-cgroup-parent",
        type=Path,
        help="Native Linux diagnostic only: explicit writable delegated cgroup v2 parent",
    )
    parser.add_argument("--native-work-limits-json")
    parser.add_argument("--native-churn-cycles", type=int, default=0)
    parser.add_argument("--native-trace-mode", choices=("off", "record"))
    parser.add_argument(
        "--trace-max-output-bytes",
        type=int,
        help="explicit Native Record output and replay cap, 6144..=1073741824 bytes",
    )
    parser.add_argument(
        "--native-mixed-load",
        action="store_true",
        help="run the fixed Large42 mixed-load Off observation without canonical capture",
    )
    parser.add_argument(
        "--managed-build",
        action="store_true",
        help="build the selected backend through cargow in a fresh external target before capturing",
    )
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
            if args.trace_max_output_bytes is not None and not (
                6144 <= args.trace_max_output_bytes <= MAX_CONFIGURED_CAPTURE_BYTES
            ):
                raise ValueError("trace-max-output-bytes must be in 6144..=1073741824")
            if args.managed_build:
                raise ValueError("offline export cannot build a producer")
            if (
                args.backend != "legacy"
                or args.linux_cgroup_parent is not None
                or args.native_churn_cycles != 0
                or args.native_mixed_load
                or args.native_trace_mode is not None
                or any(
                    value is not None
                    for value in (
                        args.native_daemon_bin,
                        args.native_daemon_sha256,
                        args.native_work_limits_json,
                    )
                )
            ):
                raise ValueError("offline export cannot take native execution inputs")
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
            status = offline_export(
                args.offline_input,
                args.out_root,
                viewer=True,
                max_capture_bytes=args.trace_max_output_bytes,
            )
            return 0 if status["viewer_available"] else 2
        if any(
            value is None
            for value in (
                args.cwd,
                args.source_revision,
                args.tier,
                args.seed,
                args.max_seconds,
            )
        ):
            raise ValueError(
                "capture requires cwd, binary, source revision/digest, tier, seed and timeout"
            )
        if not args.managed_build and (args.binary is None or args.binary_sha256 is None):
            raise ValueError("capture requires a binary/digest or --managed-build")
        _native_inputs(args)
        if args.seed < 0 or args.seed >= 1 << 64 or args.max_seconds <= 0:
            raise ValueError("seed or max-seconds is out of range")
        if args.client_timeout_ms is not None and not 1 <= args.client_timeout_ms <= 600_000:
            raise ValueError("client-timeout-ms must be in 1..=600000")
        _history_inputs(args.history_max_bytes, args.history_max_total_bytes)
        if len(args.source_revision) != 40 or any(
            c not in "0123456789abcdef" for c in args.source_revision
        ):
            raise ValueError("source revision must be a full lowercase commit SHA")
        if not args.managed_build and (
            len(args.binary_sha256) != 64
            or any(c not in "0123456789abcdef" for c in args.binary_sha256)
        ):
            raise ValueError("binary digest must be lowercase SHA-256")
        if args.backend == "index-native" and not args.managed_build:
            if args.native_daemon_bin is None or args.native_daemon_sha256 is None:
                raise ValueError("native capture requires a daemon/digest or --managed-build")
            if len(args.native_daemon_sha256) != 64 or any(
                c not in "0123456789abcdef" for c in args.native_daemon_sha256
            ):
                raise ValueError("native daemon digest must be lowercase SHA-256")
        execution = capture(args)
    except (ValueError, OSError, subprocess.CalledProcessError) as error:
        parser.exit(2, f"causal capture refused: {error}\n")
    return (
        0
        if execution["status"] == "VERIFIED_DIAGNOSTIC"
        or (
            args.backend == "index-native"
            and _native_trace_mode(args) == "off"
            and execution["status"] == "VERIFIED_PRODUCT_OBSERVATION"
            and execution["product_status"] == "PRODUCT_ARTIFACT_VERIFIED"
            and execution["diagnostic_status"] == "NOT_APPLICABLE"
            and execution["canonical_profile_status"] == "NOT_APPLICABLE_OFF_NO_CAPTURE"
            and execution["build_binding"] == "OBSERVED_LOCAL_BUILD"
            and execution["timeline_sha256"] is None
            and execution["viewer_available"] is False
        )
        else 2
    )


if __name__ == "__main__":
    raise SystemExit(main())
