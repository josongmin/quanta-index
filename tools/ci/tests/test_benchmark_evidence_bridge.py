"""Native-artifact to typed-evidence bridge: no fabricated rows, no silent drops."""

from __future__ import annotations

import importlib.util
import json
import sys
from pathlib import Path

import pytest

REPO_ROOT = Path(__file__).resolve().parents[3]
BENCHMARK_DIR = REPO_ROOT / "tools" / "benchmark"
if str(BENCHMARK_DIR) not in sys.path:
    sys.path.insert(0, str(BENCHMARK_DIR))


def _bridge():
    for name, path in (
        ("evidence", BENCHMARK_DIR / "evidence.py"),
        ("evidence_bridge", BENCHMARK_DIR / "evidence_bridge.py"),
    ):
        spec = importlib.util.spec_from_file_location(name, path)
        assert spec and spec.loader
        module = importlib.util.module_from_spec(spec)
        sys.modules[name] = module
        spec.loader.exec_module(module)
    return sys.modules["evidence_bridge"]


def _artifact(rows: list[dict], schema_version: int = 2) -> dict:
    return {
        "schema_version": schema_version,
        "dimension": "dsl-warm",
        "mode": "warm",
        "concurrency": 1,
        "provenance": {"git_head": "a" * 40},
        "host": {"os": "macos", "arch": "aarch64", "cpu_count": 10, "mem_bytes": 1, "hostname_hash": "sha256:" + "0" * 64},
        "resources": {"peak_rss_bytes": 1},
        "phases": {"build_ms": None, "update_ms": None, "gc_ms": None},
        "disk_amplification": None,
        "rows": rows,
        "detail": {},
    }


def _row(scenario: str, **overrides) -> dict:
    row = {
        "scenario_id": scenario,
        "route_family": "lexical",
        "syntax": "native",
        "result_shape": "candidates",
        "latency": {"p50_ms": 0.42, "p95_ms": 0.55, "p99_ms": 0.61, "samples": 200},
        "qps": None,
        "error_count": 0,
        "timeout_count": 0,
        "result_count": 3,
        "typed_error_code": None,
        "engine_touched": ["lexical"],
        "early_stop_reason": None,
    }
    row.update(overrides)
    return row


def test_latency_payload_preserves_rows_and_sums_failures() -> None:
    bridge = _bridge()
    payload = bridge.latency_payload_from_artifact(
        _artifact(
            [
                _row("a", error_count=2, timeout_count=1),
                _row("b", latency={"p50_ms": 1.0, "p95_ms": 1.0, "p99_ms": 1.0, "samples": 20}),
            ]
        )
    )
    assert payload["kind"] == "latency"
    assert [row["case_id"] for row in payload["rows"]] == ["a", "b"]
    assert payload["errors"] == 2
    assert payload["timeouts"] == 1
    assert payload["drops"] == 0
    assert payload["rows"][0]["unit"] == "ms"


def test_unmeasured_rows_keep_their_reason_and_no_percentile() -> None:
    bridge = _bridge()
    payload = bridge.latency_payload_from_artifact(
        _artifact([_row("a", latency=None, early_stop_reason="fixture-gap")])
    )
    row = payload["rows"][0]
    assert row["early_stop_reason"] == "fixture-gap"
    assert row["samples"] == 0
    assert row["p50"] is None and row["p95"] is None and row["p99"] is None


def test_artifact_without_rows_is_refused_not_fabricated() -> None:
    bridge = _bridge()
    with pytest.raises(bridge.EvidenceError, match="no measured rows"):
        bridge.latency_payload_from_artifact(_artifact([]))


def test_wrong_artifact_schema_is_refused() -> None:
    bridge = _bridge()
    with pytest.raises(bridge.EvidenceError, match="schema_version"):
        bridge.latency_payload_from_artifact(_artifact([_row("a")], schema_version=1))


def test_micro_payload_never_converts_instructions_to_latency() -> None:
    bridge = _bridge()
    wall = bridge.micro_payload_from_criterion(
        bench_id="pipeline", statistic="mean", value_ns=100.0, iterations=10, samples=10
    )
    assert (wall["unit"], wall["instrumentation"]) == ("ns", "wall")
    counted = bridge.micro_payload_from_criterion(
        bench_id="pipeline",
        statistic="mean",
        value_ns=100.0,
        iterations=10,
        samples=10,
        instrumentation="instructions",
    )
    assert (counted["unit"], counted["instrumentation"]) == ("instructions", "instructions")


def test_recorded_experiment_stays_diagnostic_only() -> None:
    bridge = _bridge()
    payload = bridge.recorded_experiment_payload(
        experiment_id="scan-vs-index",
        points=[{"label": "2000", "metric": "scan_ms", "unit": "ms", "value": 1.0}],
        source_digest="sha256:" + "ab" * 32,
    )
    assert payload["diagnostic_only"] is True


def test_host_identity_digest_tracks_the_host() -> None:
    bridge = _bridge()
    first = bridge.host_identity(
        policy="local-diagnostic",
        os_name="macos",
        arch="aarch64",
        cpu_count=10,
        hostname="host-a",
        lease_mode="shared",
        lease_samples=1,
    )
    second = bridge.host_identity(
        policy="local-diagnostic",
        os_name="macos",
        arch="aarch64",
        cpu_count=10,
        hostname="host-b",
        lease_mode="shared",
        lease_samples=1,
    )
    assert first["identity_digest"] != second["identity_digest"]
    assert first["hostname_hash"] != second["hostname_hash"]
    assert "host-a" not in json.dumps(first)


def test_promote_native_run_writes_an_immutable_verifiable_run(tmp_path: Path) -> None:
    bridge = _bridge()
    evidence_module = sys.modules["evidence"]
    artifact = _artifact([_row("lexical.keyword.native")])
    native_bytes = (json.dumps(artifact) + "\n").encode("utf-8")
    native_path = tmp_path / "warm-matrix.json"
    native_path.write_bytes(native_bytes)
    payload = bridge.latency_payload_from_artifact(artifact)
    promotion = bridge.promote_native_run(
        evidence_root=tmp_path / "root",
        run_id="dsl-warm-20260926T120000Z-deadbeef",
        family="dsl-warm",
        profile="dsl-authority",
        created_utc="2026-09-26T12:00:00Z",
        native_path=native_path,
        native_bytes=native_bytes,
        payload=payload,
        source={
            "revision": "a" * 40,
            "dirty": False,
            "dirty_paths_digest": None,
            "closure_profile": "benchmark-control-plane",
            "closure_digest": "sha256:" + "11" * 32,
        },
        build={
            "toolchain": "rustc 1.92.0",
            "target_triple": "aarch64-apple-darwin",
            "lockfile_digest": "sha256:" + "22" * 32,
            "profile": "bench",
            "flags": ["--locked"],
            "binaries": [],
        },
        inputs=[
            {
                "id": "workspace-fixture",
                "availability": "unavailable",
                "digest": None,
                "reason": "in-process deterministic fixture",
            }
        ],
        host=bridge.host_identity(
            policy="local-diagnostic",
            os_name="macos",
            arch="aarch64",
            cpu_count=10,
            hostname="host-a",
            lease_mode="shared",
            lease_samples=1,
        ),
        command={
            "argv": ["benchctl", "run", "dsl-authority"],
            "cwd": ".",
            "status": "completed",
            "exit_code": 0,
            "timeout_seconds": 1800,
            "wall_ms": 10,
        },
        boundary={
            "clock": "monotonic",
            "instrumentation": "none",
            "start_event": "producer_exec",
            "end_event": "artifact_written",
        },
        verdict={
            "scope": "diagnostic",
            "status": "not_run",
            "reason": "capture only; the comparator issues the regression verdict",
            "metrics": [],
        },
    )
    store = evidence_module.RunStore(tmp_path / "root")
    loaded = store.load(promotion["run_id"])
    assert loaded["digest"] == promotion["evidence"]["digest"]
    assert loaded["raw"][0]["sha256"] == evidence_module.digest_bytes(native_bytes)
    # A second identical capture is refused: run ids are immutable.
    with pytest.raises(evidence_module.EvidenceError, match="already exists"):
        bridge.promote_native_run(
            evidence_root=tmp_path / "root",
            run_id=promotion["run_id"],
            family="dsl-warm",
            profile="dsl-authority",
            created_utc="2026-09-26T12:00:00Z",
            native_path=native_path,
            native_bytes=native_bytes,
            payload=payload,
            source=loaded["source"],
            build=loaded["build"],
            inputs=loaded["inputs"],
            host=loaded["host"],
            command=loaded["command"],
            boundary=loaded["boundary"],
            verdict=loaded["verdict"],
        )


def _artifact_checker():
    spec = importlib.util.spec_from_file_location(
        "bench_artifacts_fixture",
        REPO_ROOT / "tools" / "ci" / "tests" / "test_check_bench_artifacts.py",
    )
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def test_promoted_real_artifact_fixture_replays_through_the_artifact_oracle(
    tmp_path: Path,
) -> None:
    """Contract-level producer integration: promote a valid rail artifact and
    re-derive its verdict in a fresh `benchctl replay` process."""
    import subprocess

    bridge = _bridge()
    fixture = _artifact_checker()
    artifact = fixture.artifact("dsl-warm")
    native_bytes = (json.dumps(artifact) + "\n").encode("utf-8")
    native_path = tmp_path / "warm-matrix.json"
    native_path.write_bytes(native_bytes)
    payload = bridge.latency_payload_from_artifact(artifact)
    run_id = "dsl-warm-20260926T120000Z-abcdef01"
    promotion = bridge.promote_native_run(
        evidence_root=tmp_path / "root",
        run_id=run_id,
        family="dsl-warm",
        profile="dsl-authority",
        created_utc="2026-09-26T12:00:00Z",
        native_path=native_path,
        native_bytes=native_bytes,
        payload=payload,
        source={
            "revision": fixture.HEAD,
            "dirty": False,
            "dirty_paths_digest": None,
            "closure_profile": "benchmark-control-plane",
            "closure_digest": "sha256:" + "11" * 32,
        },
        build={
            "toolchain": "rustc 1.92.0",
            "target_triple": "aarch64-apple-darwin",
            "lockfile_digest": "sha256:" + "22" * 32,
            "profile": "bench",
            "flags": ["--locked"],
            "binaries": [],
        },
        inputs=[
            {
                "id": "workspace-fixture",
                "availability": "unavailable",
                "digest": None,
                "reason": "in-process deterministic fixture",
            }
        ],
        host=bridge.host_identity(
            policy="local-diagnostic",
            os_name="macos",
            arch="aarch64",
            cpu_count=10,
            hostname="host-a",
            lease_mode="shared",
            lease_samples=1,
        ),
        command={
            "argv": ["benchctl", "run", "dsl-authority"],
            "cwd": ".",
            "status": "completed",
            "exit_code": 0,
            "timeout_seconds": 1800,
            "wall_ms": 10,
        },
        boundary={
            "clock": "monotonic",
            "instrumentation": "none",
            "start_event": "producer_exec",
            "end_event": "artifact_written",
        },
        verdict={
            "scope": "diagnostic",
            "status": "pass",
            "reason": None,
            "metrics": [],
        },
    )
    result = subprocess.run(
        [
            sys.executable,
            str(BENCHMARK_DIR / "benchctl.py"),
            "replay",
            str(promotion["run_dir"]),
        ],
        cwd=REPO_ROOT,
        check=False,
        capture_output=True,
        text=True,
    )
    assert result.returncode == 0, result.stderr
    receipt = json.loads(result.stdout)
    assert receipt["artifact_oracle"] == "pass"
    assert receipt["replay"] == "re_derived"
    assert receipt["run_id"] == run_id

    # Tampering with the captured raw evidence is refused by the same path.
    raw = promotion["run_dir"] / "raw" / "warm-matrix.json"
    raw.write_bytes(raw.read_bytes().replace(b'"samples": 500', b'"samples": 501'))
    tampered = subprocess.run(
        [
            sys.executable,
            str(BENCHMARK_DIR / "benchctl.py"),
            "replay",
            str(promotion["run_dir"]),
        ],
        cwd=REPO_ROOT,
        check=False,
        capture_output=True,
        text=True,
    )
    assert tampered.returncode == 2
    assert "digest mismatch" in tampered.stderr
