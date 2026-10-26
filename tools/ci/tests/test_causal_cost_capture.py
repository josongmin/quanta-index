"""Frontdoor history policy flags and refusal before creating an output root."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import shutil
import signal
import socket
import subprocess
import sys
import tempfile
import textwrap
from dataclasses import replace
from pathlib import Path
from types import SimpleNamespace

import pytest

from tools.benchmark import producer_execution
from tools.benchmark.producer_execution import _execute_owned
from tools.benchmark.retrieval import causal_cost_capture, linux_process
from tools.benchmark.retrieval.causal_cost_capture import (
    _history_inputs,
    _scale_command,
    capture,
    offline_export,
)
from tools.ci.tests.test_causal_cost_profile import (
    NATIVE_PHASE_GOLD,
    _manifest,
    _native_mixed_fixture,
    _native_off_fixture,
    _native_product_fixture,
)


def _timeline(phases: tuple[str, ...] | None = None) -> bytes:
    capture_id = "0" * 32
    phases = phases or (
        "full_ingest_seal",
        "full_activate",
        "delta_ingest_seal",
        "delta_activate",
        "noop_seal",
        "noop_activate",
    )
    rows = [
        {
            "record_type": "header",
            "schema_version": 1,
            "capture_id": capture_id,
            "stream_id": capture_id,
            "clock_domain": capture_id,
            "process_instance": capture_id,
            "started_ns": 0,
            "max_record_bytes": 4096,
            "max_counters_per_event": 1,
            "coverage": ["harness"],
        }
    ]
    sequence = 0
    for span_id, phase in enumerate(phases, 1):
        for ts, kind in (
            (
                span_id * 10,
                {
                    "type": "span_begin",
                    "span_id": span_id,
                    "root_kind": "operation",
                    "stage": "harness_phase",
                    "lane_id": 1,
                },
            ),
            (span_id * 10 + 1, {"type": "phase_identity", "span_id": span_id, "phase": phase}),
            (span_id * 10 + 2, {"type": "span_end", "span_id": span_id, "outcome": "ok"}),
        ):
            sequence += 1
            rows.append(
                {
                    "record_type": "event",
                    "output_seq": sequence,
                    "capture_id": capture_id,
                    "ts_ns": ts,
                    "kind": kind,
                }
            )
    rows.append(
        {
            "record_type": "footer",
            "schema_version": 1,
            "capture_id": capture_id,
            "closed_ns": max(100, len(phases) * 10 + 10),
            "status": "complete",
            "close_reason": "normal",
            "last_output_seq": sequence,
            "producer_leases": 0,
            "events_attempted": sequence,
            "events_accepted": sequence,
            "events_dropped": 0,
            "events_written": sequence,
            "events_unwritten": 0,
            "open_spans": 0,
        }
    )
    return b"".join(json.dumps(row, separators=(",", ":")).encode() + b"\n" for row in rows)


def _qualified_process_result(parent: Path, *, exit_code: int = 0, timed_out: bool = False):
    return linux_process.ProcessResult(
        root=linux_process.ProcessIdentity(321, 98765),
        root_exit_code=exit_code,
        timed_out=timed_out,
        elapsed_ms=100.0,
        sample_interval_ms=50,
        samples=3,
        peak_tree_rss_bytes=4096,
        total_user_cpu_ns=100,
        total_kernel_cpu_ns=200,
        processes=(),
        escaped=(),
        sampling_complete=True,
        cleanup_complete=True,
        stdout_path=None,
        stderr_path=None,
        backend="cgroup-v2",
        cgroup_path=str(parent / "quanta-retrieval-fixture"),
        peak_cgroup_memory_bytes=8192,
        cgroup_cpu_usage_ns=300,
    )


@pytest.fixture
def capture_fixture(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> SimpleNamespace:
    monkeypatch.setenv("QUANTA_INDEX_CAUSAL_PROFILE_V1", "1")
    source = tmp_path / "source"
    source.mkdir()
    binary = tmp_path / "scale_matrix"
    binary.write_bytes(b"fixed executable fixture\n")
    binary.chmod(0o700)
    state = {
        "head": "a" * 40,
        "dirty": "",
        "replays": 0,
        "product_validations": 0,
        "timeline": _timeline(),
    }
    args = argparse.Namespace(
        backend="legacy",
        native_daemon_bin=None,
        native_daemon_sha256=None,
        native_work_limits_json=None,
        native_churn_cycles=0,
        native_mixed_load=False,
        native_trace_mode=None,
        cwd=source,
        binary=binary,
        out_root=tmp_path / "capture",
        source_revision=state["head"],
        binary_sha256=hashlib.sha256(binary.read_bytes()).hexdigest(),
        tier="small",
        seed=7,
        client_timeout_ms=None,
        history_max_bytes=None,
        history_max_total_bytes=None,
        disk_observation=None,
        max_seconds=10,
        timeline=True,
    )

    def git(_cwd: Path, *command: str) -> str:
        return state["head"] if command == ("rev-parse", "HEAD") else state["dirty"]

    def run(_command: list[str], **kwargs: object) -> dict:
        assert "QUANTA_INDEX_CAUSAL_PROFILE_V1" not in kwargs["env"]
        artifact = args.out_root / "artifact"
        artifact.mkdir()
        (artifact / "summary.json").write_bytes(b'{"fixture":"summary"}\n')
        (artifact / "tier_manifest.json").write_bytes(b'{"fixture":"manifest"}\n')
        kwargs["sinks"][0].write(b"fixed stdout\n")
        kwargs["sinks"][1].write(b"fixed trace\n")
        (args.out_root / "causal-timeline.jsonl").write_bytes(state["timeline"])
        return {"exit_code": 0}

    def replay(
        summary: bytes, trace: bytes, executable: bytes, manifest: bytes, **options: object
    ) -> dict:
        state["replays"] += 1
        assert summary == b'{"fixture":"summary"}\n'
        assert trace == b"fixed trace\n"
        assert executable == b"fixed executable fixture\n"
        assert manifest == b'{"fixture":"manifest"}\n'
        return {
            "status": "diagnostic_unqualified",
            "source_revision": options["source_revision"],
            "binary_sha256": hashlib.sha256(executable).hexdigest(),
            "summary_sha256": hashlib.sha256(summary).hexdigest(),
            "trace_sha256": hashlib.sha256(trace).hexdigest(),
            "runtime_config": {
                "history_policy_id": "scale-supported-v1",
                "requested_history_max_bytes": None,
                "history_max_bytes": 1_073_741_824,
                "requested_history_max_total_bytes": None,
                "history_max_total_bytes": 2_147_483_648,
                "requested_disk_observation_mode": args.disk_observation,
                "disk_observation_mode": args.disk_observation or "boundary-only",
            },
            "scope": {},
            "phases": {
                name: {
                    "status": "ok",
                    "sync": {},
                    "exact_live_token_scan": {"calls": 0, "elapsed_ns": 0},
                    "bm25_census_build": {"calls": 0, "elapsed_ns": 0},
                    "bm25_boundary": {},
                    "bm25_live_build": [],
                    "process_write_io": {},
                    "process_write_io_unavailable_reason": None,
                }
                for name in (
                    "full_ingest_seal",
                    "full_activate",
                    "delta_ingest_seal",
                    "delta_activate",
                    "noop_seal",
                    "noop_activate",
                )
            },
        }

    def validate_product(*_values: object, **_options: object) -> dict:
        state["product_validations"] += 1
        return {"status": "product_artifact_verified"}

    monkeypatch.setattr(causal_cost_capture, "_git", git)
    monkeypatch.setattr(causal_cost_capture, "_execute_owned", run)
    monkeypatch.setattr(causal_cost_capture, "replay", replay)
    monkeypatch.setattr(causal_cost_capture, "validate_product_artifacts", validate_product)
    return SimpleNamespace(args=args, state=state, replay=replay)


@pytest.fixture
def native_capture_fixture(capture_fixture, tmp_path, monkeypatch, request) -> SimpleNamespace:
    """Fake only execution/build; replay the fixed Rust-wire gold with real validators."""
    args, state = capture_fixture.args, capture_fixture.state
    short_root = tempfile.TemporaryDirectory(prefix="qcc-", dir="/tmp")
    request.addfinalizer(short_root.cleanup)
    args.out_root = Path(os.path.realpath(short_root.name)) / "capture"
    for key in (
        "QUANTA_INDEX_NATIVE_WORK_LIMITS_JSON",
        "QUANTA_INDEX_NATIVE_CHURN_CYCLES",
        "QUANTA_INDEX_NATIVE_RESOURCE_SAMPLING_V1",
        "QUANTA_INDEX_NATIVE_MIXED_LOAD_V1",
        "QUANTA_INDEX_NATIVE_RECORD_CAPTURE_V1",
    ):
        monkeypatch.delenv(key, raising=False)
    args.backend = "index-native"
    daemon = tmp_path / "quanta-index-searchd"
    daemon.write_bytes(b"fixed Native daemon executable fixture\n")
    daemon.chmod(0o700)
    args.native_daemon_bin = daemon
    args.native_daemon_sha256 = hashlib.sha256(daemon.read_bytes()).hexdigest()
    fixture_root = tmp_path / "native-fixture"
    fixture_root.mkdir()
    summary, _children, _workload = _native_product_fixture(fixture_root, sampling=True)
    summary["detail"]["daemon_sha256"] = args.native_daemon_sha256
    state.update(native_after_run=None, native_command=None, native_environment=None)

    def run(command, **kwargs):
        assert "QUANTA_INDEX_CAUSAL_PROFILE_V1" not in kwargs["env"]
        state["native_command"] = command
        state["native_environment"] = kwargs["env"]
        artifact = args.out_root / "artifact"
        artifact.mkdir()
        if args.native_mixed_load:
            mixed_root = tmp_path / "mixed-fixture"
            mixed_root.mkdir()
            mixed_summary, _workload = _native_mixed_fixture(mixed_root)
            mixed_summary["detail"]["daemon_sha256"] = hashlib.sha256(
                daemon.read_bytes()
            ).hexdigest()
            state["mixed_summary"] = mixed_summary
            (artifact / "summary.json").write_text(json.dumps(mixed_summary))
        elif args.native_trace_mode == "off":
            off_root = tmp_path / "off-fixture"
            off_root.mkdir()
            off_summary, _workload = _native_off_fixture(off_root)
            off_summary["detail"]["daemon_sha256"] = hashlib.sha256(daemon.read_bytes()).hexdigest()
            if args.tier == "large":
                off_summary["detail"].update(tier="large", seed=42, file_count=4096)
                off_summary["detail"]["foreground_workload"] = {
                    "mode": "explicit",
                    "work_limits": {
                        "reads": 67_108_864,
                        "writes": 67_108_864,
                        "bytes": 549_755_813_888,
                        "mutations": 67_108_864,
                    },
                }
            state["off_summary"] = off_summary
            (artifact / "summary.json").write_text(json.dumps(off_summary))
        else:
            children = args.out_root / "causal-timeline.jsonl.native-small-children"
            shutil.copytree(fixture_root / "children", children)
            custody = summary["detail"]["child_trace_files"]
            custody["directory"] = str(children)
            for row in custody["captures"]:
                row["path"] = str(children / f"{row['role']}.jsonl")
                row["stderr_tail"]["path"] = str(children / f"{row['role']}.stderr")
                metadata = Path(row["stderr_tail"]["path"]).stat()
                row["stderr_tail"]["file_device"] = metadata.st_dev
                row["stderr_tail"]["file_inode"] = metadata.st_ino
            (artifact / "summary.json").write_text(json.dumps(summary))
            (args.out_root / "causal-timeline.jsonl").write_bytes(
                _timeline(tuple(f"native_{phase}" for phase in NATIVE_PHASE_GOLD))
            )
        (artifact / "tier_manifest.json").write_text(json.dumps(_manifest()))
        kwargs["sinks"][0].write(b"Native fixed producer fixture\n")
        kwargs["sinks"][1].write(b"Native parent diagnostics; no legacy timers\n")
        if state["native_after_run"] is not None:
            state["native_after_run"]()
        return {"exit_code": 0}

    original_harness = args.binary.read_bytes()
    original_daemon = daemon.read_bytes()

    def managed_build(cwd, out, *, backend):
        assert cwd == args.cwd and backend == "index-native"
        out.mkdir()
        pair = {}
        for name, data in (
            ("scale_matrix", original_harness),
            ("quanta-index-searchd", original_daemon),
        ):
            binary = out / name
            binary.write_bytes(data)
            binary.chmod(0o500)
            pair[name] = {
                "binary": str(binary),
                "binary_sha256": hashlib.sha256(data).hexdigest(),
                "binary_epoch": causal_cost_capture.capture_executable(binary),
            }
        receipt = {
            "status": "OBSERVED_LOCAL_BUILD",
            "source_revision": state["head"],
            "source_identity": {"fixture": "Native source inventory"},
            "backend": "index-native",
            "build_profile": "bench",
            **pair["scale_matrix"],
            "native_daemon": pair["quanta-index-searchd"],
        }
        (out / "build-receipt.json").write_text(json.dumps(receipt))
        return receipt

    monkeypatch.setattr(causal_cost_capture, "_execute_owned", run)
    monkeypatch.setattr(causal_cost_capture, "build_scale", managed_build)
    monkeypatch.setattr(
        causal_cost_capture, "source_capture", lambda *_args: {"fixture": "Native source inventory"}
    )
    return capture_fixture


def _use_native_managed_pair(args) -> None:
    args.managed_build = True
    args.binary = args.binary_sha256 = None
    args.native_daemon_bin = args.native_daemon_sha256 = None


def _select_native_mixed(args) -> None:
    args.native_mixed_load = True
    args.tier = "large"
    args.seed = 42
    args.max_seconds = 1_200
    args.timeline = False
    args.native_work_limits_json = json.dumps(
        {
            "reads": 67_108_864,
            "writes": 67_108_864,
            "bytes": 549_755_813_888,
            "mutations": 67_108_864,
        }
    )


def _native_mixed_argv(args, *, managed: bool) -> list[str]:
    command = [
        "causal-cost-capture",
        "--cwd",
        str(args.cwd),
        "--source-revision",
        args.source_revision,
        "--backend",
        "index-native",
        "--tier",
        args.tier,
        "--seed",
        str(args.seed),
        "--native-work-limits-json",
        args.native_work_limits_json,
        "--native-mixed-load",
        "--max-seconds",
        str(args.max_seconds),
        "--out-root",
        str(args.out_root),
    ]
    if managed:
        command.append("--managed-build")
    else:
        command.extend(
            [
                "--binary",
                str(args.binary),
                "--binary-sha256",
                args.binary_sha256,
                "--native-daemon-bin",
                str(args.native_daemon_bin),
                "--native-daemon-sha256",
                args.native_daemon_sha256,
            ]
        )
    return command


def _native_ordinary_off_argv(args, *, managed: bool) -> list[str]:
    command = [
        "causal-cost-capture",
        "--cwd",
        str(args.cwd),
        "--source-revision",
        args.source_revision,
        "--backend",
        "index-native",
        "--tier",
        args.tier,
        "--seed",
        str(args.seed),
        "--native-trace-mode",
        "off",
        "--max-seconds",
        str(args.max_seconds),
        "--out-root",
        str(args.out_root),
    ]
    if managed:
        command.append("--managed-build")
    else:
        command.extend(
            [
                "--binary",
                str(args.binary),
                "--binary-sha256",
                args.binary_sha256,
                "--native-daemon-bin",
                str(args.native_daemon_bin),
                "--native-daemon-sha256",
                args.native_daemon_sha256,
            ]
        )
    return command


def _qualified_native_fixture(native_capture_fixture, tmp_path, monkeypatch):
    args = native_capture_fixture.args
    parent = tmp_path / "delegated-cgroup"
    parent.mkdir()
    identity = parent.stat()
    args.linux_cgroup_parent = parent
    monkeypatch.setattr(
        causal_cost_capture,
        "_linux_cgroup_parent",
        lambda _args: (parent, (identity.st_dev, identity.st_ino)),
    )
    legacy_owner = causal_cost_capture._execute_owned
    calls = []

    def qualified_owner(command, **kwargs):
        calls.append((command, kwargs))
        assert kwargs["qualified"] is True
        assert kwargs["cgroup_parent"] == str(parent)
        assert kwargs["timeout_secs"] == args.max_seconds
        assert (
            0
            < kwargs["absolute_deadline"] - causal_cost_capture.time.monotonic()
            <= args.max_seconds
        )
        assert kwargs["cwd"] == str(args.cwd)
        with (
            Path(kwargs["stdout_path"]).open("xb") as stdout,
            Path(kwargs["stderr_path"]).open("xb") as stderr,
        ):
            legacy_owner(
                command,
                cwd=args.cwd,
                env=kwargs["env"],
                sinks=(stdout, stderr),
                timeout=args.max_seconds,
            )
        return _qualified_process_result(parent)

    monkeypatch.setattr(causal_cost_capture.linux_process, "run", qualified_owner)
    return args, parent, calls


def test_native_linux_cgroup_uses_one_qualified_owner_and_keeps_memory_scope(
    native_capture_fixture, tmp_path, monkeypatch
):
    args, parent, calls = _qualified_native_fixture(native_capture_fixture, tmp_path, monkeypatch)
    validator = causal_cost_capture.validate_native_product_artifacts
    seen_observations = []

    def validate_with_observation(*values, **options):
        seen_observations.append(options["linux_resource_observation"])
        return validator(*values, **options)

    monkeypatch.setattr(
        causal_cost_capture, "validate_native_product_artifacts", validate_with_observation
    )
    result = capture(args)
    assert len(calls) == 1
    assert result["product_status"] == "PRODUCT_ARTIFACT_VERIFIED"
    assert result["linux_resource_observation"] == {
        "owner": "linux-process-owner-v1",
        "scope": "scale_harness_and_owned_descendants_until_cgroup_empty",
        "cgroup_parent": str(parent),
        "cgroup_parent_dev": parent.stat().st_dev,
        "cgroup_parent_ino": parent.stat().st_ino,
        "cgroup_path": str(parent / "quanta-retrieval-fixture"),
        "root_pid": 321,
        "root_start_ticks": 98765,
        "sample_interval_ms": 50,
        "sample_count": 3,
        "aggregate_rss_sampled_max_bytes": 4096,
        "aggregate_rss_method": "sampled_smaps_rollup_rss_sum_shared_pages_may_repeat",
        "cgroup_memory_peak_bytes": 8192,
        "cgroup_memory_method": "cgroup_v2_memory.peak_kernel_accounted_memory_not_rss",
        "cgroup_cpu_usage_ns": 300,
        "ownership_complete": True,
        "migration_caveat": "unobserved_cgroup_migration_requires_external_prevention",
        "continuous_rss_peak": False,
        "allocated_disk_peak": None,
    }
    assert seen_observations
    assert all(observation == result["linux_resource_observation"] for observation in seen_observations)
    status = json.loads((args.out_root / "timeline-status.json").read_text())
    assert status["execution"]["linux_resource_observation"] == result["linux_resource_observation"]
    assert status["execution"]["native_temporary_root"] == result["native_temporary_root"]
    assert json.loads((args.out_root / "execution.json").read_text()) == result


@pytest.mark.parametrize("trace_mode", ("off", "record"))
def test_native_capture_overrides_inherited_tmpdir_only_for_owned_child(
    native_capture_fixture: SimpleNamespace,
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
    trace_mode: str,
) -> None:
    args, state = native_capture_fixture.args, native_capture_fixture.state
    inherited = tmp_path / "inherited-tmp"
    inherited.mkdir()
    monkeypatch.setenv("TMPDIR", str(inherited))
    args.native_trace_mode = trace_mode
    if trace_mode == "off":
        args.timeline = False

    result = capture(args)
    native_temp = args.out_root / "native-tmp"
    assert result["product_status"] == "PRODUCT_ARTIFACT_VERIFIED"
    assert native_temp.is_dir()
    assert native_temp.stat().st_mode & 0o777 == 0o700
    assert result["native_temporary_root"] == str(native_temp)
    assert state["native_environment"]["TMPDIR"] == str(native_temp)
    assert os.environ["TMPDIR"] == str(inherited)
    if sys.platform == "darwin" or sys.platform.startswith("linux"):
        daemon_state = native_temp / ".tmpABCDEF"
        daemon_state.mkdir()
        with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as listener:
            listener.bind(str(daemon_state / "native.sock"))
    status = json.loads((args.out_root / "timeline-status.json").read_text())
    assert status["execution"]["native_temporary_root"] == str(native_temp)


@pytest.mark.parametrize("platform,limit", (("darwin", 103), ("linux", 107)))
def test_native_socket_preflight_exact_byte_boundary(
    monkeypatch: pytest.MonkeyPatch, platform: str, limit: int
) -> None:
    monkeypatch.setattr(causal_cost_capture.sys, "platform", platform)
    suffix_bytes = len(os.fsencode("/native-tmp/.tmpXXXXXX/native.sock"))
    root = Path("/" + "a" * (limit - suffix_bytes - 1))
    causal_cost_capture._preflight_native_socket_path(root)
    with pytest.raises(ValueError, match="choose a shorter out-root"):
        causal_cost_capture._preflight_native_socket_path(Path(str(root) + "a"))


def test_native_socket_preflight_counts_unicode_bytes(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr(causal_cost_capture.sys, "platform", "darwin")
    root = Path("/" + "é" * 36)
    assert len(str(root / "native-tmp" / ".tmpXXXXXX" / "native.sock")) <= 103
    with pytest.raises(ValueError, match="choose a shorter out-root"):
        causal_cost_capture._preflight_native_socket_path(root)


def test_native_socket_preflight_refuses_before_managed_build_or_output(
    native_capture_fixture: SimpleNamespace, monkeypatch: pytest.MonkeyPatch
) -> None:
    args, state = native_capture_fixture.args, native_capture_fixture.state
    _use_native_managed_pair(args)
    args.out_root = args.out_root.with_name("x" * 100)
    monkeypatch.setattr(
        causal_cost_capture,
        "build_scale",
        lambda *_args, **_options: pytest.fail("managed build started before socket preflight"),
    )
    with pytest.raises(ValueError, match="choose a shorter out-root"):
        capture(args)
    assert not args.out_root.exists()
    assert state["native_command"] is None


def test_legacy_capture_preserves_inherited_tmpdir(
    capture_fixture: SimpleNamespace, monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    inherited = tmp_path / "inherited-tmp"
    inherited.mkdir()
    monkeypatch.setenv("TMPDIR", str(inherited))
    captured = []
    original = causal_cost_capture._execute_owned

    def run(command, **options):
        captured.append(options["env"]["TMPDIR"])
        return original(command, **options)

    monkeypatch.setattr(causal_cost_capture, "_execute_owned", run)
    result = capture(capture_fixture.args)
    assert captured == [str(inherited)]
    assert "native_temporary_root" not in result
    assert not (capture_fixture.args.out_root / "native-tmp").exists()
    assert os.environ["TMPDIR"] == str(inherited)


def test_native_mixed_linux_outer_rss_exceedance_refuses_product(
    native_capture_fixture: SimpleNamespace,
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    args = native_capture_fixture.args
    _select_native_mixed(args)
    args, _parent, _calls = _qualified_native_fixture(native_capture_fixture, tmp_path, monkeypatch)
    original = causal_cost_capture.linux_process.run

    def over_cap(command, **options):
        return replace(
            original(command, **options), peak_tree_rss_bytes=2 * 1024**3 + 1
        )

    monkeypatch.setattr(causal_cost_capture.linux_process, "run", over_cap)
    result = capture(args)
    assert result["status"] == "FAILED"
    assert result["product_status"] == "EXIT_ZERO_UNQUALIFIED"
    assert result["linux_resource_observation"]["aggregate_rss_sampled_max_bytes"] == (
        2 * 1024**3 + 1
    )
    assert "outer Linux sampled aggregate RSS exceeds 2 GiB" in result["reason"]
    status = json.loads((args.out_root / "timeline-status.json").read_text())
    assert status["execution"]["linux_resource_observation"] == result["linux_resource_observation"]


@pytest.mark.parametrize(
    "failure", ["setup", "owner_wait_timeout", "missing_ownership", "timed_out"]
)
def test_native_linux_cgroup_failure_retains_raw_and_never_promotes(
    native_capture_fixture, tmp_path, monkeypatch, failure
):
    args, parent, _calls = _qualified_native_fixture(native_capture_fixture, tmp_path, monkeypatch)
    original = causal_cost_capture.linux_process.run

    def fail(command, **kwargs):
        if failure == "setup":
            raise linux_process.ProcessError("delegation unavailable")
        if failure == "owner_wait_timeout":
            raise subprocess.TimeoutExpired(["owned-child"], 5)
        result = original(command, **kwargs)
        if failure == "missing_ownership":
            return replace(result, cleanup_complete=False)
        return replace(result, timed_out=True, root_exit_code=-9)

    monkeypatch.setattr(causal_cost_capture.linux_process, "run", fail)
    result = capture(args)
    assert result["status"] == "FAILED"
    assert result["product_status"] == "UNVERIFIED"
    assert (args.out_root / "stdout").is_file()
    assert (args.out_root / "stderr").is_file()
    assert json.loads((args.out_root / "execution.json").read_text()) == result
    if failure in ("setup", "owner_wait_timeout"):
        assert (args.out_root / "stdout").read_bytes() == b""
        if failure == "setup":
            assert result["producer_error"] == (
                "qualified Linux process ownership failed: delegation unavailable"
            )
        else:
            assert "timed out" in result["producer_error"]
        assert result["linux_resource_observation"] is None
    elif failure == "missing_ownership":
        assert "lacks complete process ownership" in result["producer_error"]
        assert result["linux_resource_observation"] is None
    else:
        assert result["timed_out"] is True
        assert result["linux_resource_observation"]["cgroup_memory_peak_bytes"] == 8192


def test_native_linux_cgroup_sigterm_restores_handler_and_preserves_failed_record(
    native_capture_fixture, tmp_path, monkeypatch
):
    args, _parent, _calls = _qualified_native_fixture(native_capture_fixture, tmp_path, monkeypatch)
    previous = signal.getsignal(signal.SIGTERM)

    def interrupted(_command, **_kwargs):
        os.kill(os.getpid(), signal.SIGTERM)
        pytest.fail("SIGTERM did not interrupt the qualified owner")

    monkeypatch.setattr(causal_cost_capture.linux_process, "run", interrupted)
    result = capture(args)
    assert signal.getsignal(signal.SIGTERM) == previous
    assert result["status"] == "FAILED"
    assert result["producer_error"] == "benchmark producer interrupted by SIGTERM"
    assert result["linux_resource_observation"] is None
    assert (args.out_root / "stdout").read_bytes() == b""
    assert json.loads((args.out_root / "execution.json").read_text()) == result


def test_native_linux_cgroup_parent_identity_drift_refuses_product(
    native_capture_fixture, tmp_path, monkeypatch
):
    args, parent, _calls = _qualified_native_fixture(native_capture_fixture, tmp_path, monkeypatch)
    original = causal_cost_capture.linux_process.run

    def drift(command, **kwargs):
        result = original(command, **kwargs)
        parent.rename(tmp_path / "original-cgroup-parent")
        parent.mkdir()
        return result

    monkeypatch.setattr(causal_cost_capture.linux_process, "run", drift)
    result = capture(args)
    assert result["status"] == "FAILED"
    assert result["product_status"] == "UNVERIFIED"
    assert result["linux_resource_observation"] is None
    assert "parent identity changed" in result["producer_error"]
    assert (args.out_root / "stdout").read_bytes() == b"Native fixed producer fixture\n"


@pytest.mark.parametrize("signum", [signal.SIGTERM, signal.SIGINT])
def test_owned_interrupt_after_pipe_read_retains_consumed_stdout_block(
    capture_fixture, monkeypatch, signum
):
    args = capture_fixture.args
    args.binary.write_text(
        f"#!{sys.executable}\n"
        "import time\n"
        "print('read-before-interrupt-gold', flush=True)\n"
        "time.sleep(600)\n"
    )
    args.binary_sha256 = hashlib.sha256(args.binary.read_bytes()).hexdigest()
    monkeypatch.setattr(causal_cost_capture, "_execute_owned", _execute_owned)
    read = producer_execution.os.read
    fired = False

    def interrupt_after_read(fd, count):
        nonlocal fired
        block = read(fd, count)
        if b"read-before-interrupt-gold" in block and not fired:
            fired = True
            os.kill(os.getpid(), signum)
        return block

    monkeypatch.setattr(producer_execution.os, "read", interrupt_after_read)
    previous = signal.getsignal(signum)
    if signum == signal.SIGINT:
        with pytest.raises(KeyboardInterrupt):
            capture(args)
        result = json.loads((args.out_root / "execution.json").read_text())
    else:
        result = capture(args)
    assert fired is True
    assert signal.getsignal(signum) == previous
    assert result["status"] == "FAILED"
    assert result["producer_error"] == f"benchmark producer interrupted by {signum.name}"
    assert b"read-before-interrupt-gold" in (args.out_root / "stdout").read_bytes()


def test_linux_cgroup_selection_refuses_non_linux_or_non_native_before_output(
    capture_fixture, tmp_path, monkeypatch
):
    args = capture_fixture.args
    args.linux_cgroup_parent = tmp_path
    with pytest.raises(ValueError, match="native capture inputs"):
        capture(args)
    assert not args.out_root.exists()
    args.backend = "index-native"
    monkeypatch.setattr(causal_cost_capture.sys, "platform", "darwin")
    with pytest.raises(ValueError, match="Native backend on Linux"):
        capture(args)
    assert not args.out_root.exists()


@pytest.mark.parametrize("managed", [False, True])
def test_native_ordinary_off_keeps_product_and_build_binding_without_trace(
    native_capture_fixture: SimpleNamespace,
    monkeypatch: pytest.MonkeyPatch,
    managed: bool,
) -> None:
    args, state = native_capture_fixture.args, native_capture_fixture.state
    args.native_trace_mode = "off"
    args.timeline = False
    monkeypatch.setattr(sys, "argv", _native_ordinary_off_argv(args, managed=managed))
    assert causal_cost_capture.main() == (0 if managed else 2)
    result = json.loads((args.out_root / "execution.json").read_bytes())
    assert result["native_trace_mode"] == "off"
    assert result["native_mixed_load"] is False
    assert result["status"] == ("VERIFIED_PRODUCT_OBSERVATION" if managed else "PARTIAL_DIAGNOSTIC")
    assert result["product_status"] == "PRODUCT_ARTIFACT_VERIFIED"
    assert result["diagnostic_status"] == "NOT_APPLICABLE"
    assert result["canonical_profile_status"] == "NOT_APPLICABLE_OFF_NO_CAPTURE"
    assert result["build_binding"] == ("OBSERVED_LOCAL_BUILD" if managed else "UNPROVEN")
    assert result["timeline_sha256"] is None
    assert result["viewer_available"] is False
    assert "native_child_inputs" not in result
    assert not (args.out_root / "canonical-profile.json").exists()
    assert (args.out_root / "causal-timeline.jsonl").read_bytes() == b""
    assert (args.out_root / "stdout").read_bytes() == b"Native fixed producer fixture\n"
    assert (
        args.out_root / "stderr"
    ).read_bytes() == b"Native parent diagnostics; no legacy timers\n"
    command = state["native_command"]
    assert command[command.index("--trace-mode") + 1] == "off"
    assert "--trace-out" not in command
    assert "QUANTA_INDEX_NATIVE_MIXED_LOAD_V1" not in state["native_environment"]
    status = json.loads((args.out_root / "timeline-status.json").read_bytes())
    assert status["source_binding"] == (
        "observed_local_build_and_execution" if managed else "UNPROVEN"
    )
    assert status["raw_placeholder"] is True
    assert status["reason"] == "Native ordinary Off has no canonical timeline"
    assert status["execution"]["native_trace_mode"] == "off"


@pytest.mark.parametrize(
    "fault",
    (
        "unexpected_timeline",
        "unexpected_child_directory",
        "record_claim",
        "child_object_io",
        "partial_publication",
        "cold_reopen",
        "daemon_mutation",
        "binary_mutation",
        "source_mutation",
    ),
)
def test_native_ordinary_off_refuses_partial_record_or_product_forgery(
    native_capture_fixture: SimpleNamespace, fault: str
) -> None:
    args, state = native_capture_fixture.args, native_capture_fixture.state
    args.native_trace_mode = "off"
    args.timeline = False

    def corrupt() -> None:
        if fault == "unexpected_timeline":
            (args.out_root / "causal-timeline.jsonl").write_bytes(_timeline())
        elif fault == "unexpected_child_directory":
            (args.out_root / "causal-timeline.jsonl.native-small-children").mkdir()
        elif fault == "daemon_mutation":
            args.native_daemon_bin.write_bytes(b"changed daemon\n")
        elif fault == "binary_mutation":
            args.binary.write_bytes(b"changed harness\n")
        elif fault == "source_mutation":
            state["dirty"] = " M changed.rs"
        else:
            path = args.out_root / "artifact/summary.json"
            summary = json.loads(path.read_bytes())
            detail = summary["detail"]
            if fault == "record_claim":
                detail["child_trace_verified"] = True
            elif fault == "child_object_io":
                detail["child_object_io"] = {"epochs": {}}
            elif fault == "partial_publication":
                detail["publication_outcomes"].pop()
            else:
                detail["phases"]["reopen"]["completed"] = False
            path.write_text(json.dumps(summary))

    state["native_after_run"] = corrupt
    result = capture(args)
    assert result["status"] == "FAILED"
    assert result["product_status"] != "PRODUCT_ARTIFACT_VERIFIED"
    assert result["native_trace_mode"] == "off"
    assert "canonical_profile_sha256" not in result
    assert not (args.out_root / "canonical-profile.json").exists()


@pytest.mark.parametrize("mixed", [False, True])
@pytest.mark.parametrize("managed", [False, True])
@pytest.mark.parametrize("raw_name", ["stdout", "stderr", "build-receipt.json"])
def test_native_off_refuses_raw_or_build_receipt_drift_during_product_validation(
    native_capture_fixture: SimpleNamespace,
    monkeypatch: pytest.MonkeyPatch,
    mixed: bool,
    managed: bool,
    raw_name: str,
) -> None:
    if raw_name == "build-receipt.json" and not managed:
        pytest.skip("prebuilt capture has no managed build receipt")
    args = native_capture_fixture.args
    args.native_trace_mode = "off"
    args.timeline = False
    if mixed:
        _select_native_mixed(args)
    if managed:
        _use_native_managed_pair(args)
    validate = causal_cost_capture.validate_native_product_artifacts

    def mutate_after_validation(*values: object, **options: object) -> dict:
        product = validate(*values, **options)
        path = (
            args.out_root / "build/build-receipt.json"
            if raw_name == "build-receipt.json"
            else args.out_root / raw_name
        )
        path.write_bytes(path.read_bytes() + b"changed after producer\n")
        return product

    monkeypatch.setattr(
        causal_cost_capture, "validate_native_product_artifacts", mutate_after_validation
    )
    result = capture(args)
    assert result["status"] == "FAILED"
    assert result["product_status"] == "UNVERIFIED"
    assert "causal replay input changed" in result["reason"]
    assert (args.out_root / "execution.json").is_file()


def test_native_ordinary_large_off_preserves_explicit_funding_and_full_product_gold(
    native_capture_fixture: SimpleNamespace,
) -> None:
    args, state = native_capture_fixture.args, native_capture_fixture.state
    args.native_trace_mode = "off"
    args.timeline = False
    args.tier = "large"
    args.seed = 42
    args.native_work_limits_json = json.dumps(
        {
            "reads": 67_108_864,
            "writes": 67_108_864,
            "bytes": 549_755_813_888,
            "mutations": 67_108_864,
        }
    )
    result = capture(args)
    assert result["product_status"] == "PRODUCT_ARTIFACT_VERIFIED"
    assert result["status"] == "PARTIAL_DIAGNOSTIC"
    assert result["native_trace_mode"] == "off"
    assert result["native_foreground_workload"] == {
        "mode": "explicit",
        "work_limits": {
            "reads": 67_108_864,
            "writes": 67_108_864,
            "bytes": 549_755_813_888,
            "mutations": 67_108_864,
        },
    }
    assert state["native_command"][state["native_command"].index("--tier") + 1] == "large"
    assert "--trace-out" not in state["native_command"]
    assert json.loads(state["native_environment"]["QUANTA_INDEX_NATIVE_WORK_LIMITS_JSON"]) == {
        "reads": 67_108_864,
        "writes": 67_108_864,
        "bytes": 549_755_813_888,
        "mutations": 67_108_864,
    }
    assert "QUANTA_INDEX_NATIVE_MIXED_LOAD_V1" not in state["native_environment"]
    assert result["timeline_sha256"] is None


@pytest.mark.parametrize("fault", ("legacy", "offline", "mixed_record", "off_viewer"))
def test_native_trace_mode_refuses_incompatible_selection_before_output(
    native_capture_fixture: SimpleNamespace,
    monkeypatch: pytest.MonkeyPatch,
    fault: str,
) -> None:
    args, state = native_capture_fixture.args, native_capture_fixture.state
    command = _native_ordinary_off_argv(args, managed=False)
    if fault == "legacy":
        command[command.index("--backend") + 1] = "legacy"
    elif fault == "offline":
        command.extend(["--offline-input", str(args.cwd / "input.jsonl")])
    elif fault == "mixed_record":
        command.append("--native-mixed-load")
        command[command.index("--native-trace-mode") + 1] = "record"
    else:
        command.append("--timeline")
    monkeypatch.setattr(sys, "argv", command)
    with pytest.raises(SystemExit) as refusal:
        causal_cost_capture.main()
    assert refusal.value.code == 2
    assert state["native_command"] is None
    assert not args.out_root.exists()


def test_explicit_native_record_never_falls_back_to_off_when_trace_is_missing(
    native_capture_fixture: SimpleNamespace,
) -> None:
    args, state = native_capture_fixture.args, native_capture_fixture.state
    args.native_trace_mode = "record"

    def remove_parent_trace() -> None:
        (args.out_root / "causal-timeline.jsonl").unlink()

    state["native_after_run"] = remove_parent_trace
    result = capture(args)
    assert result["native_trace_mode"] == "record"
    assert result["status"] != "VERIFIED_PRODUCT_OBSERVATION"
    assert result.get("canonical_profile_status") != "NOT_APPLICABLE_OFF_NO_CAPTURE"
    assert result["diagnostic_status"] != "NOT_APPLICABLE"


@pytest.mark.parametrize("managed", [False, True])
def test_native_mixed_off_cli_validates_actual_product_shape_without_canonical_capture(
    native_capture_fixture: SimpleNamespace,
    monkeypatch: pytest.MonkeyPatch,
    managed: bool,
) -> None:
    args, state = native_capture_fixture.args, native_capture_fixture.state
    _select_native_mixed(args)
    monkeypatch.setattr(sys, "argv", _native_mixed_argv(args, managed=managed))
    assert causal_cost_capture.main() == (0 if managed else 2)
    result = json.loads((args.out_root / "execution.json").read_bytes())
    assert result["status"] == ("VERIFIED_PRODUCT_OBSERVATION" if managed else "PARTIAL_DIAGNOSTIC")
    assert result["product_status"] == "PRODUCT_ARTIFACT_VERIFIED"
    assert result["diagnostic_status"] == "NOT_APPLICABLE"
    assert result["canonical_profile_status"] == "NOT_APPLICABLE_OFF_NO_CAPTURE"
    assert result["native_mixed_load"] is True
    assert result["timeline_sha256"] is None
    assert result["timeline_capture_id"] is None
    assert result["viewer_available"] is False
    assert "native_child_inputs" not in result
    assert "canonical_profile_sha256" not in result
    assert not (args.out_root / "canonical-profile.json").exists()
    assert not (args.out_root / "causal-waterfall.json").exists()
    assert (args.out_root / "causal-timeline.jsonl").read_bytes() == b""
    assert result["native_environment"]["QUANTA_INDEX_NATIVE_MIXED_LOAD_V1"] == "1"
    assert result["native_environment"]["QUANTA_INDEX_NATIVE_RESOURCE_SAMPLING_V1"] == "1"
    assert state["native_environment"]["QUANTA_INDEX_NATIVE_MIXED_LOAD_V1"] == "1"
    assert "QUANTA_INDEX_CAUSAL_PROFILE_V1" not in state["native_environment"]
    command = state["native_command"]
    assert command[command.index("--trace-mode") + 1] == "off"
    assert "--trace-out" not in command
    status = json.loads((args.out_root / "timeline-status.json").read_bytes())
    assert status["capture_status"] == "unavailable"
    assert status["raw_placeholder"] is True
    assert status["viewer_available"] is False
    assert status["execution"]["native_mixed_load"] is True
    assert status["execution"]["product_status"] == "PRODUCT_ARTIFACT_VERIFIED"
    assert state["replays"] == state["product_validations"] == 0


@pytest.mark.parametrize("fault", ["partial_publisher", "unexpected_timeline"])
def test_native_mixed_off_refuses_partial_product_or_unexpected_canonical_trace(
    native_capture_fixture: SimpleNamespace, fault: str
) -> None:
    args, state = native_capture_fixture.args, native_capture_fixture.state
    _select_native_mixed(args)

    def corrupt() -> None:
        if fault == "partial_publisher":
            artifact = args.out_root / "artifact/summary.json"
            summary = json.loads(artifact.read_bytes())
            summary["detail"]["mixed_load"]["measure"]["publisher"]["publication_records"].pop()
            artifact.write_text(json.dumps(summary))
        else:
            (args.out_root / "causal-timeline.jsonl").write_bytes(_timeline())

    state["native_after_run"] = corrupt
    result = capture(args)
    assert result["status"] == "FAILED"
    assert result["product_status"] == "EXIT_ZERO_UNQUALIFIED"
    assert "canonical_profile_sha256" not in result
    assert not (args.out_root / "canonical-profile.json").exists()
    status = json.loads((args.out_root / "timeline-status.json").read_bytes())
    if fault == "unexpected_timeline":
        assert status["capture_status"] == "invalid"
        assert status["raw_placeholder"] is False
        assert status["viewer_available"] is False
        assert (args.out_root / "causal-timeline.jsonl").read_bytes() == _timeline()
    else:
        assert status["capture_status"] == "unavailable"
        assert status["raw_placeholder"] is True


@pytest.mark.parametrize("failure", ["timeout", "interrupt"])
def test_native_mixed_off_owned_execution_failure_preserves_raw_and_never_verifies_product(
    native_capture_fixture: SimpleNamespace,
    monkeypatch: pytest.MonkeyPatch,
    failure: str,
) -> None:
    args = native_capture_fixture.args
    _select_native_mixed(args)

    def incomplete(command: list[str], **kwargs: object) -> dict:
        kwargs["sinks"][0].write(b"owned mixed stdout\n")
        kwargs["sinks"][1].write(b"owned mixed stderr\n")
        cause = (
            subprocess.TimeoutExpired(command, args.max_seconds)
            if failure == "timeout"
            else KeyboardInterrupt()
        )
        raise causal_cost_capture.ProducerExecutionError("owned producer incomplete") from cause

    monkeypatch.setattr(causal_cost_capture, "_execute_owned", incomplete)
    if failure == "interrupt":
        with pytest.raises(KeyboardInterrupt):
            capture(args)
    else:
        capture(args)
    result = json.loads((args.out_root / "execution.json").read_bytes())
    assert result["status"] == "FAILED"
    assert result["product_status"] == "UNVERIFIED"
    assert result["timed_out"] is (failure == "timeout")
    assert result["diagnostic_status"] == "UNAVAILABLE"
    assert result["native_mixed_load"] is True
    assert (args.out_root / "stdout").read_bytes() == b"owned mixed stdout\n"
    assert (args.out_root / "stderr").read_bytes() == b"owned mixed stderr\n"
    assert not (args.out_root / "canonical-profile.json").exists()
    status = json.loads((args.out_root / "timeline-status.json").read_bytes())
    assert status["capture_status"] == "unavailable"
    assert status["execution"]["product_status"] == "UNVERIFIED"


@pytest.mark.parametrize(
    "invalid",
    ["legacy", "wrong_seed", "wrong_funding", "short_timeout", "viewer", "ambient_drift"],
)
def test_native_mixed_cli_refuses_invalid_profile_before_output(
    native_capture_fixture: SimpleNamespace,
    monkeypatch: pytest.MonkeyPatch,
    invalid: str,
) -> None:
    args, state = native_capture_fixture.args, native_capture_fixture.state
    _select_native_mixed(args)
    command = _native_mixed_argv(args, managed=True)
    if invalid == "legacy":
        command[command.index("--backend") + 1] = "legacy"
    elif invalid == "wrong_seed":
        command[command.index("--seed") + 1] = "41"
    elif invalid == "wrong_funding":
        altered = json.loads(args.native_work_limits_json)
        altered["reads"] -= 1
        command[command.index("--native-work-limits-json") + 1] = json.dumps(altered)
    elif invalid == "short_timeout":
        command[command.index("--max-seconds") + 1] = "960"
    elif invalid == "viewer":
        command.append("--timeline")
    else:
        monkeypatch.setenv("QUANTA_INDEX_NATIVE_MIXED_LOAD_V1", "0")
    monkeypatch.setattr(sys, "argv", command)
    with pytest.raises(SystemExit) as refusal:
        causal_cost_capture.main()
    assert refusal.value.code == 2
    assert state["native_command"] is None
    assert not args.out_root.exists()


@pytest.mark.parametrize("managed", [False, True])
def test_native_capture_replays_real_product_and_canonical_validators(
    native_capture_fixture, managed
):
    args, state = native_capture_fixture.args, native_capture_fixture.state
    if managed:
        _use_native_managed_pair(args)
    result = capture(args)
    assert result["status"] == ("VERIFIED_DIAGNOSTIC" if managed else "PARTIAL_DIAGNOSTIC")
    assert result["canonical_profile_status"] == "VERIFIED"
    assert result["product_status"] == "PRODUCT_ARTIFACT_VERIFIED"
    if not managed:
        assert result["reason"] == (
            "canonical diagnostic verified; prebuilt source/build binding UNPROVEN; "
            "performance qualification NOT_RUN"
        )
    assert result["native_daemon_sha256"] == result["native_daemon_sha256_after"]
    assert result["legacy_replay_error"] == "NOT_APPLICABLE_NATIVE"
    assert state["replays"] == state["product_validations"] == 0
    command = state["native_command"]
    assert command[command.index("--backend") + 1] == "index-native"
    bound_daemon = Path(command[command.index("--native-daemon-bin") + 1])
    assert bound_daemon.is_absolute()
    assert hashlib.sha256(bound_daemon.read_bytes()).hexdigest() == result["native_daemon_sha256"]
    assert state["native_environment"]["QUANTA_INDEX_NATIVE_RESOURCE_SAMPLING_V1"] == "1"
    assert "QUANTA_INDEX_NATIVE_WORK_LIMITS_JSON" not in state["native_environment"]
    assert "QUANTA_INDEX_NATIVE_CHURN_CYCLES" not in state["native_environment"]
    profile = json.loads((args.out_root / "canonical-profile.json").read_text())
    assert profile["source"] == "native_parent_and_independent_children"
    assert set(profile["phases"]) == set(NATIVE_PHASE_GOLD)
    assert profile["scope"]["performance_qualification"] == "NOT_RUN"
    assert profile["native_resource_observation"]["mode"] == "opt_in_diagnostic_v2"
    assert set(result["native_child_inputs"]) == {"first", "reopen"}
    for role, expected_id in (("first", "1" * 32), ("reopen", "2" * 32)):
        child = result["native_child_inputs"][role]
        assert child["capture_id"] == expected_id
        assert (
            hashlib.sha256(Path(child["trace_path"]).read_bytes()).hexdigest()
            == child["trace_sha256"]
        )
        assert (
            hashlib.sha256(Path(child["stderr_path"]).read_bytes()).hexdigest()
            == child["stderr_sha256"]
        )
    status = json.loads((args.out_root / "timeline-status.json").read_text())
    assert status["source_binding"] == (
        "observed_local_build_and_execution" if managed else "UNPROVEN"
    )
    assert status["execution"]["native_child_inputs"] == result["native_child_inputs"]
    assert not (args.out_root / "causal-profile.json").exists()


@pytest.mark.parametrize(
    "fault", ["daemon", "foreign_trace", "missing_stderr", "missing_sampling", "failed_sampler"]
)
def test_native_capture_refuses_changed_execution_or_foreign_raw_path(
    native_capture_fixture, fault
):
    args, state = native_capture_fixture.args, native_capture_fixture.state

    def corrupt():
        if fault == "daemon":
            args.native_daemon_bin.write_bytes(b"different daemon bytes\n")
        elif fault == "missing_stderr":
            (args.out_root / "causal-timeline.jsonl.native-small-children/first.stderr").unlink()
        else:
            summary_path = args.out_root / "artifact/summary.json"
            summary = json.loads(summary_path.read_text())
            if fault == "foreign_trace":
                summary["detail"]["child_trace_files"]["captures"][0]["path"] = (
                    "/unowned/never-read.jsonl"
                )
            elif fault == "missing_sampling":
                del summary["detail"]["native_resource_observation"]
            else:
                summary["detail"]["native_resource_observation"]["epochs"][0]["worker_failure"] = (
                    "observer_thread_panicked"
                )
            summary_path.write_text(json.dumps(summary))

    state["native_after_run"] = corrupt
    result = capture(args)
    assert result["status"] == "FAILED"
    assert result["product_status"] != "PRODUCT_ARTIFACT_VERIFIED"
    if fault == "foreign_trace":
        assert result["reason"] == "native raw paths differ from the fixed process epoch"
    elif fault == "daemon":
        assert result["reason"] == "producer failed, timed out or source/binary changed"
    elif fault == "missing_stderr":
        assert "first.stderr" in result["reason"]
    else:
        assert "sampling" in result["reason"] or "resource" in result["reason"]
    assert not (args.out_root / "canonical-profile.json").exists()


@pytest.mark.parametrize("fault", ["daemon", "trace", "stderr", "receipt", "trace_fifo"])
def test_native_capture_refuses_binding_drift_after_canonical_replay(
    native_capture_fixture, monkeypatch, fault
):
    args = native_capture_fixture.args
    _use_native_managed_pair(args)
    actual_canonical = causal_cost_capture.canonical_profile

    def replay_then_mutate(*values, **options):
        profile = actual_canonical(*values, **options)
        path = {
            "daemon": args.out_root / "build/quanta-index-searchd",
            "trace": args.out_root / "causal-timeline.jsonl.native-small-children/first.jsonl",
            "trace_fifo": args.out_root / "causal-timeline.jsonl.native-small-children/first.jsonl",
            "stderr": args.out_root / "causal-timeline.jsonl.native-small-children/reopen.stderr",
            "receipt": args.out_root / "build/build-receipt.json",
        }[fault]
        if fault == "trace_fifo":
            path.unlink()
            os.mkfifo(path, 0o600)
            return profile
        path.chmod(0o600)
        with path.open("ab") as output:
            output.write(b"changed after canonical replay\n")
        return profile

    monkeypatch.setattr(causal_cost_capture, "canonical_profile", replay_then_mutate)
    result = capture(args)
    assert result["status"] == "FAILED"
    assert result["product_status"] == (
        "PRODUCT_ARTIFACT_VERIFIED" if fault == "receipt" else "UNVERIFIED"
    )
    assert not (args.out_root / "canonical-profile.json").exists()
    status = json.loads((args.out_root / "timeline-status.json").read_text())
    assert status["source_binding"] == "UNPROVEN"


@pytest.mark.parametrize(
    ("field", "value"),
    [
        ("native_churn_cycles", True),
        ("native_churn_cycles", 257),
        ("native_work_limits_json", '{"reads":true,"writes":1,"bytes":1,"mutations":1}'),
        ("native_work_limits_json", '{"reads":1,"writes":1,"bytes":1}'),
        ("native_work_limits_json", '{"reads":1,"reads":2,"writes":1,"bytes":1,"mutations":1}'),
        ("history_max_bytes", 1024),
        ("disk_observation", "periodic"),
        ("tier", "large"),
    ],
)
def test_native_profile_refuses_invalid_inputs_before_output(native_capture_fixture, field, value):
    args = native_capture_fixture.args
    setattr(args, field, value)
    with pytest.raises(ValueError):
        capture(args)
    assert not args.out_root.exists()
    assert native_capture_fixture.state["native_command"] is None


def test_native_capture_refuses_inherited_mixed_profile_before_output(
    native_capture_fixture, monkeypatch
):
    args = native_capture_fixture.args
    monkeypatch.setenv("QUANTA_INDEX_NATIVE_MIXED_LOAD_V1", "1")
    with pytest.raises(ValueError, match="inherited QUANTA_INDEX_NATIVE_MIXED_LOAD_V1"):
        capture(args)
    assert not args.out_root.exists()


def test_capture_accepts_one_unchanged_executable_and_input_epoch(
    capture_fixture: SimpleNamespace,
) -> None:
    result = capture(capture_fixture.args)
    assert result["status"] == "PARTIAL_DIAGNOSTIC"
    assert result["diagnostic_status"] == "PARTIAL"
    assert result["product_status"] == "PRODUCT_ARTIFACT_VERIFIED"
    assert result["binary_sha256"] == result["binary_sha256_after"]
    assert result["build_binding"] == "UNPROVEN"
    assert result["summary_sha256"] == hashlib.sha256(b'{"fixture":"summary"}\n').hexdigest()
    assert result["tier_manifest_sha256"] == hashlib.sha256(b'{"fixture":"manifest"}\n').hexdigest()
    assert capture_fixture.state["replays"] == 1
    assert capture_fixture.state["product_validations"] == 1
    assert result["requested_disk_observation_mode"] is None
    assert result["disk_observation_mode"] == "boundary-only"
    assert result["command"][-2:] == ["--disk-observation", "boundary-only"]
    assert not (capture_fixture.args.out_root / "causal-profile.json").exists()
    assert (capture_fixture.args.out_root / "causal-waterfall.json").is_file()
    status = json.loads((capture_fixture.args.out_root / "timeline-status.json").read_text())
    assert status["capture_status"] == "complete"
    assert status["coverage"] == ["harness"]
    assert status["viewer_available"] is True
    assert status["source_binding"] == "UNPROVEN"
    assert status["execution"]["summary_sha256"] == result["summary_sha256"]
    assert status["execution"]["tier_manifest_sha256"] == result["tier_manifest_sha256"]
    assert status["execution"]["stderr_sha256"] == result["stderr_sha256"]
    assert status["execution"]["timeline_capture_id"] == "0" * 32
    summary = json.loads((capture_fixture.args.out_root / "timeline-summary.json").read_text())
    assert summary["legacy_reconciliation"]["phase_parity"] == "verified"
    waterfall = json.loads((capture_fixture.args.out_root / "causal-waterfall.json").read_text())
    assert waterfall["quantaIndexSummary"]["legacy_reconciliation"]["phase_parity"] == "verified"
    assert "legacyAggregate" not in waterfall


def test_managed_build_refuses_a_caller_supplied_binary_before_execution(
    capture_fixture: SimpleNamespace,
) -> None:
    capture_fixture.args.managed_build = True
    with pytest.raises(ValueError, match="caller-supplied"):
        capture(capture_fixture.args)
    assert not capture_fixture.args.out_root.exists()
    assert capture_fixture.state["product_validations"] == 0


@pytest.mark.parametrize("canonical_available", [False, True])
@pytest.mark.parametrize("export_error", [False, True])
def test_managed_capture_requires_both_observed_build_and_canonical_profile(
    capture_fixture: SimpleNamespace,
    monkeypatch: pytest.MonkeyPatch,
    canonical_available: bool,
    export_error: bool,
) -> None:
    original_binary = capture_fixture.args.binary
    capture_fixture.args.managed_build = True
    capture_fixture.args.binary = None
    capture_fixture.args.binary_sha256 = None

    def managed_build(cwd: Path, out: Path, *, backend: str) -> dict:
        assert backend == "legacy"
        assert cwd == capture_fixture.args.cwd
        out.mkdir()
        binary = out / "scale_matrix"
        binary.write_bytes(original_binary.read_bytes())
        binary.chmod(0o500)
        result = {
            "status": "OBSERVED_LOCAL_BUILD",
            "source_revision": capture_fixture.state["head"],
            "binary": str(binary),
            "binary_sha256": hashlib.sha256(binary.read_bytes()).hexdigest(),
            "binary_epoch": causal_cost_capture.capture_executable(binary),
            "source_identity": {"fixture": "source inventory"},
        }
        (out / "build-receipt.json").write_text(json.dumps(result))
        return result

    def canonical(_timeline: dict, _product: dict, *, execution_binding: dict) -> dict:
        assert execution_binding["product_status"] == "PRODUCT_ARTIFACT_VERIFIED"
        assert execution_binding["build_binding"] == "OBSERVED_LOCAL_BUILD"
        if not canonical_available:
            raise ValueError("missing typed metrics")
        return {"status": "diagnostic_unqualified", "source": "canonical_timeline"}

    monkeypatch.setattr(causal_cost_capture, "build_scale", managed_build)
    monkeypatch.setattr(
        causal_cost_capture, "source_capture", lambda *_args: {"fixture": "source inventory"}
    )
    monkeypatch.setattr(causal_cost_capture, "canonical_profile", canonical)
    if export_error:
        capture_fixture.args.timeline = False

        def unavailable_summary(*_args: object, **_kwargs: object) -> dict:
            raise ValueError("timeline summary export failed")

        monkeypatch.setattr(causal_cost_capture, "summarize_timeline", unavailable_summary)
    result = capture(capture_fixture.args)
    assert result["status"] == (
        "VERIFIED_DIAGNOSTIC" if canonical_available and not export_error else "PARTIAL_DIAGNOSTIC"
    )
    assert result["canonical_profile_status"] == (
        "VERIFIED" if canonical_available else "UNAVAILABLE"
    )
    assert (
        capture_fixture.args.out_root / "canonical-profile.json"
    ).exists() == canonical_available
    assert not (capture_fixture.args.out_root / "causal-profile.json").exists()
    status = json.loads((capture_fixture.args.out_root / "timeline-status.json").read_text())
    assert status["source_binding"] == "observed_local_build_and_execution"
    if export_error:
        assert status["capture_status"] == "complete"
        assert status["reason"] == "timeline summary export failed"
        assert "status" not in status["execution"]
        assert result["diagnostic_status"] == "PARTIAL"
        assert result["reason"] == "timeline summary export failed"
        if canonical_available:
            capture_fixture.args.out_root = capture_fixture.args.out_root.with_name("second")
            monkeypatch.setattr(
                "sys.argv",
                [
                    "causal_cost_capture.py",
                    "--cwd",
                    str(capture_fixture.args.cwd),
                    "--managed-build",
                    "--source-revision",
                    capture_fixture.args.source_revision,
                    "--tier",
                    "small",
                    "--seed",
                    "7",
                    "--max-seconds",
                    "10",
                    "--out-root",
                    str(capture_fixture.args.out_root),
                ],
            )
            assert causal_cost_capture.main() == 2


@pytest.mark.parametrize(
    "fault", ["receipt_before", "epoch_before", "receipt_during", "source_during"]
)
def test_managed_capture_refuses_build_execution_drift(capture_fixture, monkeypatch, fault):
    args = capture_fixture.args
    original = args.binary.read_bytes()
    args.managed_build = True
    args.binary = None
    args.binary_sha256 = None
    source_calls = 0

    def builder(_cwd, out, *, backend):
        assert backend == "legacy"
        out.mkdir()
        binary = out / "scale_matrix"
        binary.write_bytes(original)
        binary.chmod(0o500)
        receipt = {
            "status": "OBSERVED_LOCAL_BUILD",
            "source_revision": capture_fixture.state["head"],
            "binary": str(binary),
            "binary_sha256": hashlib.sha256(original).hexdigest(),
            "binary_epoch": causal_cost_capture.capture_executable(binary),
            "source_identity": {"fixture": "source inventory"},
        }
        if fault == "epoch_before":
            receipt["binary_epoch"] = {"stale": True}
        recorded = {} if fault == "receipt_before" else receipt
        (out / "build-receipt.json").write_text(json.dumps(recorded))
        return receipt

    def source(*_args):
        nonlocal source_calls
        source_calls += 1
        return {
            "fixture": "changed"
            if fault == "source_during" and source_calls > 1
            else "source inventory"
        }

    original_run = causal_cost_capture._execute_owned

    def run(*values, **kwargs):
        result = original_run(*values, **kwargs)
        if fault == "receipt_during":
            (args.out_root / "build/build-receipt.json").write_text("changed")
        return result

    monkeypatch.setattr(causal_cost_capture, "build_scale", builder)
    monkeypatch.setattr(causal_cost_capture, "source_capture", source)
    monkeypatch.setattr(causal_cost_capture, "_execute_owned", run)
    if fault.endswith("before"):
        with pytest.raises(ValueError, match="receipt|epoch"):
            capture(args)
    else:
        result = capture(args)
        assert result["status"] == "FAILED"
        assert "changed during execution" in result["reason"]
        status = json.loads((args.out_root / "timeline-status.json").read_text())
        assert status["source_binding"] == "UNPROVEN"
    assert capture_fixture.state["product_validations"] == 0
    assert not (args.out_root / "canonical-profile.json").exists()


@pytest.mark.parametrize("drift_point", ["replay", "canonical_profile", "canonical_profile_error"])
def test_managed_receipt_drift_during_replay_never_binds_failed_capture(
    capture_fixture: SimpleNamespace, monkeypatch: pytest.MonkeyPatch, drift_point: str
) -> None:
    args = capture_fixture.args
    original = args.binary.read_bytes()
    args.managed_build = True
    args.binary = None
    args.binary_sha256 = None

    def builder(_cwd: Path, out: Path, *, backend: str) -> dict:
        assert backend == "legacy"
        out.mkdir()
        binary = out / "scale_matrix"
        binary.write_bytes(original)
        binary.chmod(0o500)
        receipt = {
            "status": "OBSERVED_LOCAL_BUILD",
            "source_revision": capture_fixture.state["head"],
            "binary": str(binary),
            "binary_sha256": hashlib.sha256(original).hexdigest(),
            "binary_epoch": causal_cost_capture.capture_executable(binary),
            "source_identity": {"fixture": "source inventory"},
        }
        (out / "build-receipt.json").write_text(json.dumps(receipt))
        return receipt

    def drift_receipt() -> None:
        (args.out_root / "build/build-receipt.json").write_text("changed receipt\n")

    def replay_then_drift(*values: bytes, **options: object) -> dict:
        profile = capture_fixture.replay(*values, **options)
        if drift_point == "replay":
            drift_receipt()
        return profile

    canonical_calls = 0

    def canonical_then_drift(_timeline: dict, _product: dict, *, execution_binding: dict) -> dict:
        nonlocal canonical_calls
        canonical_calls += 1
        assert execution_binding["product_status"] == "PRODUCT_ARTIFACT_VERIFIED"
        if drift_point in ("canonical_profile", "canonical_profile_error"):
            drift_receipt()
        if drift_point == "canonical_profile_error":
            raise ValueError("injected canonical profile failure")
        return {"status": "diagnostic_unqualified", "source": "canonical_timeline"}

    monkeypatch.setattr(causal_cost_capture, "build_scale", builder)
    monkeypatch.setattr(
        causal_cost_capture, "source_capture", lambda *_args: {"fixture": "source inventory"}
    )
    monkeypatch.setattr(causal_cost_capture, "replay", replay_then_drift)
    monkeypatch.setattr(causal_cost_capture, "canonical_profile", canonical_then_drift)
    result = capture(args)
    assert result["status"] == "FAILED"
    assert result["reason"] == "managed build receipt changed during causal replay"
    assert result["product_status"] == "PRODUCT_ARTIFACT_VERIFIED"
    assert capture_fixture.state["product_validations"] == 1
    assert canonical_calls == (drift_point != "replay")
    assert not (args.out_root / "canonical-profile.json").exists()
    saved = json.loads((args.out_root / "execution.json").read_text())
    assert saved["status"] == "FAILED"
    status = json.loads((args.out_root / "timeline-status.json").read_text())
    assert status["capture_status"] == "complete"
    assert status["source_binding"] == "UNPROVEN"
    assert status["execution"]["build_binding"] == "OBSERVED_LOCAL_BUILD"


def test_export_uses_the_committed_timeline_snapshot_after_path_changes(tmp_path: Path) -> None:
    raw = _timeline()
    input_path = tmp_path / "causal-timeline.jsonl"
    output = tmp_path / "export"
    input_path.write_bytes(raw)
    output.mkdir()
    input_path.write_bytes(b"changed after input commitment\n")

    status = causal_cost_capture.export_timeline(
        input_path,
        output,
        viewer=True,
        raw_input=raw,
    )

    expected_sha = hashlib.sha256(raw).hexdigest()
    assert status["capture_status"] == "complete"
    assert status["input_sha256"] == expected_sha
    summary = json.loads((output / "timeline-summary.json").read_bytes())
    waterfall = json.loads((output / "causal-waterfall.json").read_bytes())
    assert summary["stream_sha256"] == expected_sha
    assert waterfall["quantaIndexSummary"]["stream_sha256"] == expected_sha


def test_export_replays_bound_raw_bytes_and_rejects_forged_parse(tmp_path: Path) -> None:
    from tools.benchmark.evidence import RawFile

    source = tmp_path / "causal-timeline.jsonl"
    source.write_bytes(b"not canonical JSONL\n")
    raw_file = RawFile.capture(source)
    output = tmp_path / "export"
    output.mkdir()
    forged = {"status": "complete", "sha256": raw_file.sha256.removeprefix("sha256:")}
    with pytest.raises(TypeError, match="parsed_capture"):
        causal_cost_capture.export_timeline(
            source, output, viewer=False, raw_file=raw_file, parsed_capture=forged
        )
    status = causal_cost_capture.export_timeline(source, output, viewer=False, raw_file=raw_file)
    assert status["capture_status"] == "invalid"
    assert status["input_sha256"] == raw_file.sha256.removeprefix("sha256:")


@pytest.mark.parametrize("entrypoint", ["export", "standalone"])
@pytest.mark.parametrize("source_kind", ["oversized", "device", "symlink"])
def test_timeline_readers_refuse_unsafe_input_before_decoding(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    entrypoint: str,
    source_kind: str,
) -> None:
    from tools.benchmark.retrieval import causal_cost_timeline

    source = tmp_path / "raw.jsonl"
    if source_kind == "oversized":
        with source.open("wb") as stream:
            stream.truncate(65)
    elif source_kind == "device":
        source = Path("/dev/zero")
    else:
        target = tmp_path / "target.jsonl"
        target.write_bytes(_timeline())
        source.symlink_to(target)
    monkeypatch.setattr(causal_cost_capture, "MAX_CAPTURE_BYTES", 64)
    monkeypatch.setattr(causal_cost_timeline, "MAX_CAPTURE_BYTES", 64)

    def unexpected_decode(*_args, **_kwargs):
        pytest.fail("unsafe input reached the timeline decoder")

    monkeypatch.setattr(causal_cost_capture, "parse_timeline_stream", unexpected_decode)
    monkeypatch.setattr(causal_cost_timeline, "parse_stream", unexpected_decode)
    if entrypoint == "export":
        output = tmp_path / "export"
        output.mkdir()
        status = causal_cost_capture.export_timeline(source, output, viewer=True)
        assert status["capture_status"] == "invalid"
        assert status["input_sha256"] is None
        assert status["viewer_available"] is False
        assert not (output / "causal-waterfall.json").exists()
    else:
        output = tmp_path / "viewer.json"
        monkeypatch.setattr("sys.argv", ["timeline", "--input", str(source), "--out", str(output)])
        with pytest.raises(ValueError, match="limit|regular file|symlink"):
            causal_cost_timeline.main()
        assert not output.exists()


def test_capture_refuses_binary_mutation_after_post_execution_epoch(
    capture_fixture: SimpleNamespace, monkeypatch: pytest.MonkeyPatch
) -> None:
    original = causal_cost_capture.capture_executable
    calls = 0

    def capture_then_mutate(path: Path) -> dict:
        nonlocal calls
        epoch = original(path)
        calls += 1
        if calls == 3:
            path.write_bytes(b"changed executable fixture\n")
        return epoch

    monkeypatch.setattr(causal_cost_capture, "capture_executable", capture_then_mutate)
    result = capture(capture_fixture.args)
    assert result["status"] == "FAILED"
    assert capture_fixture.state["replays"] == 0
    assert not (capture_fixture.args.out_root / "causal-profile.json").exists()


def test_failed_producer_preserves_raw_timeline_and_exports_viewer(
    capture_fixture: SimpleNamespace, monkeypatch: pytest.MonkeyPatch
) -> None:
    original = causal_cost_capture._execute_owned

    def failed(command: list[str], **kwargs: object) -> dict:
        original(command, **kwargs)
        return {"exit_code": 7}

    monkeypatch.setattr(causal_cost_capture, "_execute_owned", failed)
    result = capture(capture_fixture.args)
    assert result["status"] == "FAILED"
    assert (capture_fixture.args.out_root / "causal-timeline.jsonl").is_file()
    assert (capture_fixture.args.out_root / "causal-waterfall.json").is_file()
    assert not (capture_fixture.args.out_root / "causal-profile.json").exists()


@pytest.mark.parametrize("outcome", ["timeout", "cancel", "interrupt", "failure", "exit_zero"])
def test_capture_owns_descendants_and_retains_execution_failure(
    capture_fixture: SimpleNamespace,
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
    outcome: str,
) -> None:
    """Exercise the capture front door with a real child and background daemon."""
    args = capture_fixture.args
    pid_file = tmp_path / "daemon.pid"
    child_script = (
        "import os,time;from pathlib import Path;"
        f"Path({str(pid_file)!r}).write_text(str(os.getpid()));"
        "print('daemon stdout',flush=True);time.sleep(600)"
    )
    args.binary.write_text(
        f"#!{sys.executable}\n"
        + textwrap.dedent(
            f"""\
            import os,signal,subprocess,sys,time
            from pathlib import Path
            child=subprocess.Popen([sys.executable,'-c',{child_script!r}])
            while not Path({str(pid_file)!r}).exists():
                time.sleep(0.01)
            print('harness stdout',flush=True)
            print('harness stderr',file=sys.stderr,flush=True)
            if {outcome!r} == 'cancel':
                os.kill({os.getpid()},signal.SIGTERM)
            if {outcome!r} == 'interrupt':
                os.kill({os.getpid()},signal.SIGINT)
            if {outcome!r} in ('timeout','cancel','interrupt'):
                time.sleep(600)
            sys.exit(7 if {outcome!r} == 'failure' else 0)
            """
        )
    )
    args.binary_sha256 = hashlib.sha256(args.binary.read_bytes()).hexdigest()
    args.max_seconds = 2 if outcome == "timeout" else 10
    monkeypatch.setattr(causal_cost_capture, "_execute_owned", _execute_owned)
    previous_handler = signal.getsignal(signal.SIGTERM)
    previous_interrupt_handler = signal.getsignal(signal.SIGINT)
    if outcome == "interrupt":
        with pytest.raises(KeyboardInterrupt):
            capture(args)
        result = json.loads((args.out_root / "execution.json").read_text())
    else:
        result = capture(args)
    assert signal.getsignal(signal.SIGTERM) == previous_handler
    assert signal.getsignal(signal.SIGINT) == previous_interrupt_handler
    assert pid_file.exists(), "the background daemon must have started"
    pid = int(pid_file.read_text())
    if sys.platform.startswith("linux"):
        try:
            raw_stat = Path(f"/proc/{pid}/stat").read_text()
        except FileNotFoundError:
            state = ""
        else:
            state = raw_stat.rsplit(")", 1)[1].split()[0]
    else:
        state = subprocess.run(
            ["ps", "-o", "stat=", "-p", str(pid)],
            capture_output=True,
            text=True,
            check=False,
            timeout=5,
        ).stdout.strip()
    # An orphan may briefly remain a zombie until its system reaper runs.
    assert not state or state.startswith(("Z", "X", "x"))
    assert result["status"] == "FAILED"
    assert result["timed_out"] is (outcome == "timeout")
    expected_exit = {"failure": 7, "exit_zero": 0}.get(outcome)
    assert result["exit_code"] == expected_exit
    if outcome in ("timeout", "cancel", "interrupt"):
        assert result["producer_error"]
        assert result["reason"] == result["producer_error"]
    else:
        assert result["producer_error"] is None
    assert b"harness stdout" in (args.out_root / "stdout").read_bytes()
    assert b"harness stderr" in (args.out_root / "stderr").read_bytes()
    assert json.loads((args.out_root / "execution.json").read_text()) == result
    timeline_status = json.loads((args.out_root / "timeline-status.json").read_text())
    assert timeline_status["execution"]["producer_error"] == result["producer_error"]
    assert not (args.out_root / "causal-profile.json").exists()
    assert capture_fixture.state["replays"] == 0


@pytest.mark.parametrize("cause_kind", ["none", "timeout", "interrupt"])
def test_capture_never_qualifies_output_after_incomplete_owned_cleanup(
    capture_fixture: SimpleNamespace, monkeypatch: pytest.MonkeyPatch, cause_kind: str
) -> None:
    original = causal_cost_capture._execute_owned
    reason = "producer stopped; incomplete cleanup: pipe drain deadline exceeded"

    def incomplete(command, **kwargs):
        original(command, **kwargs)
        cause = (
            subprocess.TimeoutExpired(command, 10)
            if cause_kind == "timeout"
            else KeyboardInterrupt()
            if cause_kind == "interrupt"
            else None
        )
        raise causal_cost_capture.ProducerExecutionError(reason) from cause

    monkeypatch.setattr(causal_cost_capture, "_execute_owned", incomplete)
    if cause_kind == "interrupt":
        with pytest.raises(KeyboardInterrupt):
            capture(capture_fixture.args)
        result = json.loads((capture_fixture.args.out_root / "execution.json").read_text())
    else:
        result = capture(capture_fixture.args)
    assert result["status"] == "FAILED"
    assert result["product_status"] == "UNVERIFIED"
    assert result["exit_code"] is None
    assert result["timed_out"] is (cause_kind == "timeout")
    expected_reason = (
        f"benchmark producer interrupted by SIGINT; {reason}"
        if cause_kind == "interrupt"
        else reason
    )
    assert result["reason"] == result["producer_error"] == expected_reason
    assert capture_fixture.state["product_validations"] == 0
    assert capture_fixture.state["replays"] == 0
    assert (capture_fixture.args.out_root / "stdout").read_bytes() == b"fixed stdout\n"
    assert not (capture_fixture.args.out_root / "causal-profile.json").exists()


def test_recorder_start_failure_keeps_explicit_unavailable_artifacts(
    capture_fixture: SimpleNamespace, monkeypatch: pytest.MonkeyPatch
) -> None:
    def failed_without_timeline(_command: list[str], **kwargs: object) -> dict:
        kwargs["sinks"][1].write(b"recorder start failed\n")
        return {"exit_code": 7}

    monkeypatch.setattr(causal_cost_capture, "_execute_owned", failed_without_timeline)
    result = capture(capture_fixture.args)
    output = capture_fixture.args.out_root
    assert result["status"] == "FAILED"
    assert (output / "causal-timeline.jsonl").read_bytes() == b""
    status = json.loads((output / "timeline-status.json").read_bytes())
    summary = json.loads((output / "timeline-summary.json").read_bytes())
    assert status["capture_status"] == "unavailable"
    assert status["coverage"] is None
    assert status["raw_placeholder"] is True
    assert status["input_sha256"] is None
    assert status["viewer_available"] is False
    assert summary["status"] == "unavailable"
    assert summary["spans"] == []
    assert summary["bm25_census_aggregates"] == []
    assert summary["bm25_boundary_aggregates"] == []
    assert summary["bm25_live_build_aggregates"] == []
    assert summary["unavailable_reason"] == status["reason"]
    assert (output / "execution.json").is_file()
    assert not (output / "causal-waterfall.json").exists()


@pytest.mark.parametrize(
    "mutation",
    [
        "binary",
        "same-bytes-replacement",
        "summary",
        "manifest",
        "trace",
        "timeline",
        "head",
        "dirty",
    ],
)
def test_capture_refuses_input_drift_during_replay(
    mutation: str, capture_fixture: SimpleNamespace, monkeypatch: pytest.MonkeyPatch
) -> None:
    args = capture_fixture.args

    def replay_then_mutate(*values: bytes, **kwargs: object) -> dict:
        profile = capture_fixture.replay(*values, **kwargs)
        if mutation == "binary":
            args.binary.write_bytes(b"changed executable\n")
        elif mutation == "same-bytes-replacement":
            replacement = args.binary.with_suffix(".replacement")
            replacement.write_bytes(args.binary.read_bytes())
            replacement.chmod(0o700)
            replacement.replace(args.binary)
        elif mutation in {"summary", "manifest"}:
            name = "summary.json" if mutation == "summary" else "tier_manifest.json"
            (args.out_root / "artifact" / name).write_bytes(b"changed artifact\n")
        elif mutation == "trace":
            (args.out_root / "stderr").write_bytes(b"changed trace\n")
        elif mutation == "timeline":
            (args.out_root / "causal-timeline.jsonl").write_bytes(b"changed timeline\n")
        elif mutation == "head":
            capture_fixture.state["head"] = "b" * 40
        else:
            capture_fixture.state["dirty"] = " M source.rs"
        return profile

    monkeypatch.setattr(causal_cost_capture, "replay", replay_then_mutate)
    result = capture(args)
    assert result["status"] == "FAILED"
    assert not (args.out_root / "causal-profile.json").exists()
    assert (args.out_root / "execution.json").is_file()


def test_phase_only_parity_never_publishes_qualified_profile(
    capture_fixture: SimpleNamespace,
) -> None:
    result = capture(capture_fixture.args)
    assert result["status"] == "PARTIAL_DIAGNOSTIC"
    assert "profile_sha256" not in result
    assert not (capture_fixture.args.out_root / "causal-profile.json").exists()


def test_missing_legacy_markers_leave_a_bound_canonical_partial_capture(
    capture_fixture: SimpleNamespace, monkeypatch: pytest.MonkeyPatch
) -> None:
    def no_legacy(*_values: bytes, **_kwargs: object) -> dict:
        raise ValueError("missing QI_CAUSAL_V1 phase markers")

    monkeypatch.setattr(causal_cost_capture, "replay", no_legacy)
    result = capture(capture_fixture.args)
    assert result["status"] == "PARTIAL_DIAGNOSTIC"
    assert result["legacy_replay_error"] == "missing QI_CAUSAL_V1 phase markers"
    assert result["product_status"] == "PRODUCT_ARTIFACT_VERIFIED"
    assert capture_fixture.state["product_validations"] == 1
    assert result["build_binding"] == "UNPROVEN"
    assert (
        result["timeline_sha256"]
        == hashlib.sha256(
            (capture_fixture.args.out_root / "causal-timeline.jsonl").read_bytes()
        ).hexdigest()
    )
    summary = json.loads((capture_fixture.args.out_root / "timeline-summary.json").read_bytes())
    assert summary["status"] == "complete"
    assert "legacy_reconciliation" not in summary
    assert not (capture_fixture.args.out_root / "causal-profile.json").exists()


def test_invalid_product_artifact_is_a_failed_capture(
    capture_fixture: SimpleNamespace, monkeypatch: pytest.MonkeyPatch
) -> None:
    def reject_product(*_values: object, **_options: object) -> dict:
        raise ValueError("scale artifact is failed")

    monkeypatch.setattr(causal_cost_capture, "validate_product_artifacts", reject_product)
    result = capture(capture_fixture.args)
    assert result["status"] == "FAILED"
    assert result["product_status"] == "EXIT_ZERO_UNQUALIFIED"
    assert result["diagnostic_status"] == "UNAVAILABLE"
    assert result["reason"] == "scale artifact is failed"


@pytest.mark.parametrize(
    ("timeline", "capture_status"),
    [
        (b"not JSON\n", "invalid"),
        (_timeline().rsplit(b'\n{"record_type":"footer"', 1)[0] + b"\n", "partial"),
    ],
)
def test_invalid_or_partial_timeline_preserves_verified_product_artifacts(
    capture_fixture: SimpleNamespace, timeline: bytes, capture_status: str
) -> None:
    capture_fixture.state["timeline"] = timeline
    result = capture(capture_fixture.args)
    assert result["status"] == "PARTIAL_DIAGNOSTIC"
    assert result["product_status"] == "PRODUCT_ARTIFACT_VERIFIED"
    assert result["diagnostic_status"] == "PARTIAL"
    assert result["timeline_sha256"] == hashlib.sha256(timeline).hexdigest()
    assert capture_fixture.state["product_validations"] == 1
    assert capture_fixture.state["replays"] == 0
    timeline_status = json.loads(
        (capture_fixture.args.out_root / "timeline-status.json").read_bytes()
    )
    assert timeline_status["capture_status"] == capture_status
    assert timeline_status["execution"]["product_status"] == "PRODUCT_ARTIFACT_VERIFIED"
    assert not (capture_fixture.args.out_root / "causal-profile.json").exists()


def test_viewer_is_opt_in_but_raw_and_status_are_always_recorded(
    capture_fixture: SimpleNamespace,
) -> None:
    capture_fixture.args.timeline = False
    result = capture(capture_fixture.args)
    assert result["status"] == "PARTIAL_DIAGNOSTIC"
    assert (capture_fixture.args.out_root / "causal-timeline.jsonl").is_file()
    assert (capture_fixture.args.out_root / "timeline-status.json").is_file()
    assert (capture_fixture.args.out_root / "timeline-summary.json").is_file()
    assert not (capture_fixture.args.out_root / "causal-waterfall.json").exists()


def test_offline_partial_export_preserves_raw_and_status(tmp_path: Path) -> None:
    raw = tmp_path / "raw.jsonl"
    raw.write_bytes(_timeline().rsplit(b'\n{"record_type":"footer"', 1)[0] + b"\n")
    original = raw.read_bytes()
    output = tmp_path / "offline"
    status = offline_export(raw, output)
    assert status["capture_status"] == "partial"
    assert status["coverage"] is None
    assert status["source_binding"] == "offline_unverified"
    assert status["viewer_available"] is True
    assert raw.read_bytes() == original
    preserved = output / "causal-timeline.jsonl"
    assert preserved.read_bytes() == original
    raw.write_bytes(b"source changed after offline export\n")
    assert preserved.read_bytes() == original
    execution = json.loads((output / "execution.json").read_bytes())
    assert execution["status"] == "OFFLINE_EXPORT_UNVERIFIED"
    assert execution["product_status"] == "UNVERIFIED"
    assert execution["diagnostic_status"] == "UNVERIFIED"
    assert execution["capture_status"] == "partial"
    assert execution["producer_executed"] is False
    assert execution["timeline_sha256"] == hashlib.sha256(original).hexdigest()
    assert (output / "timeline-summary.json").is_file()
    assert (output / "causal-waterfall.json").is_file()


def test_offline_explicit_capture_cap_is_recorded(tmp_path: Path) -> None:
    source = tmp_path / "raw.jsonl"
    source.write_bytes(_timeline())
    output = tmp_path / "offline"
    status = offline_export(source, output, viewer=False, max_capture_bytes=1_073_741_824)
    assert status["capture_status"] == "complete"
    execution = json.loads((output / "execution.json").read_bytes())
    assert execution["trace_max_output_bytes"] == 1_073_741_824


def test_offline_invalid_input_retains_required_artifacts(tmp_path: Path) -> None:
    source = tmp_path / "raw.jsonl"
    source.write_bytes(b"not JSON\n")
    output = tmp_path / "offline"
    status = offline_export(source, output)
    summary = json.loads((output / "timeline-summary.json").read_bytes())
    execution = json.loads((output / "execution.json").read_bytes())
    assert (output / "causal-timeline.jsonl").stat().st_size == source.stat().st_size
    assert (output / "causal-timeline.jsonl").read_bytes() == b"not JSON\n"
    assert status["capture_status"] == "invalid"
    assert status["coverage"] is None
    assert status["viewer_available"] is False
    assert status["raw_placeholder"] is False
    assert summary["status"] == "invalid"
    assert summary["spans"] == []
    assert summary["unavailable_reason"] == status["reason"]
    assert execution["capture_status"] == "invalid"
    assert not (output / "causal-waterfall.json").exists()


@pytest.mark.parametrize("source_kind", ["oversized", "device", "symlink"])
def test_offline_refuses_unbounded_or_nonregular_sources_before_output(
    tmp_path: Path, source_kind: str
) -> None:
    source = tmp_path / "raw.jsonl"
    if source_kind == "oversized":
        with source.open("wb") as stream:
            stream.truncate(128 * 1024 * 1024 + 1)
    elif source_kind == "device":
        source = Path("/dev/zero")
    else:
        target = tmp_path / "target.jsonl"
        target.write_bytes(_timeline())
        source.symlink_to(target)
    output = tmp_path / "offline"
    with pytest.raises(ValueError, match="regular file|replay byte bound"):
        offline_export(source, output)
    assert not output.exists()


def test_offline_growth_after_size_probe_is_still_bounded(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    source = tmp_path / "growing.jsonl"
    source.write_bytes(b"small")
    monkeypatch.setattr(causal_cost_capture, "MAX_CAPTURE_BYTES", 64)

    def grow_on_first_read(path: Path, consume):
        with path.open("rb") as stream:

            class GrowingStream:
                def __init__(self):
                    self.grew = False

                def fileno(self):
                    return stream.fileno()

                def read(self, size):
                    if not self.grew:
                        self.grew = True
                        with path.open("r+b") as writer:
                            writer.truncate(65)
                    return stream.read(size)

            return consume(GrowingStream())

    monkeypatch.setattr(causal_cost_capture, "_consume_regular_file", grow_on_first_read)
    output = tmp_path / "offline"
    with pytest.raises(ValueError, match="replay byte bound"):
        offline_export(source, output)
    assert not output.exists()


def test_offline_missing_source_records_unavailable_placeholder(tmp_path: Path) -> None:
    output = tmp_path / "offline"
    status = offline_export(tmp_path / "missing.jsonl", output)
    assert status["capture_status"] == "unavailable"
    assert status["raw_placeholder"] is True
    assert (output / "causal-timeline.jsonl").read_bytes() == b""
    assert (output / "timeline-summary.json").is_file()
    assert (output / "execution.json").is_file()


def test_scale_command_forwards_pair_and_total_as_distinct_flags() -> None:
    args = argparse.Namespace(
        backend="legacy",
        tier="xlarge",
        seed=7,
        client_timeout_ms=None,
        history_max_bytes=300_000_000,
        history_max_total_bytes=600_000_000,
        disk_observation="periodic",
    )
    assert _history_inputs(args.history_max_bytes, args.history_max_total_bytes) == {
        "history_policy_id": "explicit-pair-total-diagnostic-v1",
        "requested_history_max_bytes": 300_000_000,
        "history_max_bytes": 300_000_000,
        "requested_history_max_total_bytes": 600_000_000,
        "history_max_total_bytes": 600_000_000,
    }
    assert _scale_command(args, Path("/tmp/scale_matrix"), Path("/tmp/new/artifact")) == [
        "/tmp/scale_matrix",
        "--tier",
        "xlarge",
        "--seed",
        "7",
        "--out-dir",
        "/tmp/new/artifact",
        "--trace-mode",
        "record",
        "--trace-out",
        "/tmp/new/causal-timeline.jsonl",
        "--disk-observation",
        "periodic",
        "--history-max-bytes",
        "300000000",
        "--history-max-total-bytes",
        "600000000",
    ]
    args.history_max_bytes = None
    args.history_max_total_bytes = None
    args.disk_observation = None
    assert "--disk-observation" in _scale_command(
        args, Path("/tmp/scale_matrix"), Path("/tmp/new/artifact")
    )


def test_native_explicit_trace_cap_is_validated_and_forwarded(
    capture_fixture: SimpleNamespace,
) -> None:
    args = capture_fixture.args
    args.backend = "index-native"
    args.trace_max_output_bytes = 1_073_741_824
    assert causal_cost_capture._native_inputs(args)["mode"] == "fixed"
    command = _scale_command(
        args,
        Path("/tmp/scale_matrix"),
        Path("/tmp/new/artifact"),
        native_daemon=Path("/tmp/searchd"),
    )
    assert command[command.index("--trace-max-output-bytes") + 1] == "1073741824"
    args.native_trace_mode = "off"
    args.timeline = False
    with pytest.raises(ValueError, match="requires Native Record"):
        causal_cost_capture._native_inputs(args)
    args.native_trace_mode = "record"
    args.trace_max_output_bytes = 1_073_741_825
    with pytest.raises(ValueError, match="6144..=1073741824"):
        causal_cost_capture._native_inputs(args)


def test_capture_refuses_replayed_disk_mode_mismatch(
    capture_fixture: SimpleNamespace, monkeypatch: pytest.MonkeyPatch
) -> None:
    args = capture_fixture.args
    args.disk_observation = "periodic"

    def wrong_mode(*values: bytes, **kwargs: object) -> dict:
        profile = capture_fixture.replay(*values, **kwargs)
        profile["runtime_config"]["disk_observation_mode"] = "boundary-only"
        return profile

    monkeypatch.setattr(causal_cost_capture, "replay", wrong_mode)
    result = capture(args)
    assert result["status"] == "PARTIAL_DIAGNOSTIC"
    assert result["product_status"] == "PRODUCT_ARTIFACT_VERIFIED"
    assert result["diagnostic_status"] == "PARTIAL"
    assert "disk observation mode" in result["reason"]
    assert result["command"][-2:] == ["--disk-observation", "periodic"]
    assert not (args.out_root / "causal-profile.json").exists()


@pytest.mark.parametrize(
    ("pair", "total"),
    [
        (None, 600_000_000),
        (600_000_001, 600_000_000),
        (2_147_483_649, None),
        (0, None),
        (True, None),
        (300_000_000, 0),
        (300_000_000, 600_000_000.0),
        (300_000_000, 1 << 64),
    ],
)
def test_invalid_history_inputs_refuse_without_output(
    pair: object, total: object, tmp_path: Path
) -> None:
    output = tmp_path / "must-remain-absent"
    with pytest.raises(ValueError):
        capture(
            argparse.Namespace(
                history_max_bytes=pair, history_max_total_bytes=total, out_root=output
            )
        )
    assert not output.exists()
