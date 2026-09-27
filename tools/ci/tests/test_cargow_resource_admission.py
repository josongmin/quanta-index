"""Exercise the real Cargo front door with bounded, observable fake leaf commands."""

from __future__ import annotations

import json
import os
import subprocess
import sys
import time
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[3]
CARGOW = ROOT / "scripts" / "cargow"
FAKE_CARGO = r'''
import fcntl
import json
import os
import sys
import time
from pathlib import Path

lock = Path(os.environ["QUANTA_INDEX_CACHE_ROOT"]) / "resource-admission/build-test.lock"
held = False
if lock.exists():
    with lock.open("r+") as handle:
        try:
            fcntl.flock(handle, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            held = True
        else:
            fcntl.flock(handle, fcntl.LOCK_UN)
record = {"argv": sys.argv[1:], "held": held,
          "target": os.environ["CARGO_TARGET_DIR"],
          "pin": os.environ.get("QUANTA_INDEX_SEARCHD_BIN")}
print(json.dumps(record), flush=True)
with Path(os.environ["FAKE_CARGO_RECORDS"]).open("a") as log:
    log.write(json.dumps(record) + "\n")
marker = os.environ.get("FAKE_CARGO_STARTED")
if marker:
    Path(marker).write_text("started")
release = os.environ.get("FAKE_CARGO_RELEASE")
if release:
    deadline = time.monotonic() + 8
    while not Path(release).exists():
        if time.monotonic() >= deadline:
            raise SystemExit(91)
        time.sleep(0.01)
if sys.argv[1:2] == ["build"] and "quanta-index-searchd-runtime" in sys.argv:
    binary = Path(record["target"]) / "debug/quanta-index-searchd"
    binary.parent.mkdir(parents=True, exist_ok=True)
    binary.write_text("#!/bin/sh\nexit 0\n")
    binary.chmod(0o755)
raise SystemExit(int(os.environ.get("FAKE_CARGO_EXIT", "0")))
'''


@pytest.fixture
def cargo_env(tmp_path):
    binary = tmp_path / "bin"
    binary.mkdir()
    cargo = binary / "cargo"
    cargo.write_text(f"#!{sys.executable}\n{FAKE_CARGO}")
    cargo.chmod(0o755)
    env = os.environ.copy()
    for key in (
        "CI", "QUANTA_INDEX_BUILD_LANE", "QUANTA_INDEX_SEARCHD_BIN",
        "QUANTA_INDEX_PRESERVE_CARGO_TARGET_DIR", "CARGO_TARGET_DIR",
        "QUANTA_INDEX_RESOURCE_ADMISSION", "RUSTC_WRAPPER",
    ):
        env.pop(key, None)
    env.update({
        "PATH": f"{binary}:{Path(sys.executable).parent}:{env['PATH']}",
        "QUANTA_INDEX_CACHE_ROOT": str(tmp_path / "cache"),
        "QUANTA_INDEX_BUILD_LOGGING": "0",
        "QUANTA_INDEX_SCCACHE": "0",
        "QUANTA_INDEX_RESOURCE_WAIT_SECONDS": "1",
        "QUANTA_INDEX_RESOURCE_TIMEOUT_SECONDS": "10",
        "FAKE_CARGO_RECORDS": str(tmp_path / "records.jsonl"),
    })
    return env


def run(env, *args):
    return subprocess.run(
        [str(CARGOW), *args], cwd=ROOT, env=env, capture_output=True,
        text=True, timeout=15,
    )


def records(env):
    return [json.loads(line) for line in Path(env["FAKE_CARGO_RECORDS"]).read_text().splitlines()]


@pytest.mark.parametrize("mode,ci,expected", [
    (None, None, True), ("auto", "true", False),
    ("1", "true", True), ("0", None, False), ("auto", "false", True),
])
def test_auto_local_ci_and_explicit_modes_have_observable_lock_ownership(
    cargo_env, mode, ci, expected
):
    if mode is not None:
        cargo_env["QUANTA_INDEX_RESOURCE_ADMISSION"] = mode
    if ci is not None:
        cargo_env["CI"] = ci
    result = run(cargo_env, "check", "--lib")
    assert result.returncode == 0, result.stderr
    assert records(cargo_env)[0]["held"] is expected
    assert ("cooperative resource admission disabled" in result.stderr) is (not expected)


@pytest.mark.parametrize("mode", ["maybe", ""])
def test_invalid_mode_refuses_before_cargo(cargo_env, mode):
    cargo_env["QUANTA_INDEX_RESOURCE_ADMISSION"] = mode
    result = run(cargo_env, "build")
    assert result.returncode == 2
    assert "invalid QUANTA_INDEX_RESOURCE_ADMISSION" in result.stderr
    assert not Path(cargo_env["FAKE_CARGO_RECORDS"]).exists()


def test_leaf_failure_exit_code_survives_admission(cargo_env):
    cargo_env["FAKE_CARGO_EXIT"] = "7"
    result = run(cargo_env, "build")
    assert result.returncode == 7, result.stderr
    assert records(cargo_env)[0]["held"] is True


def test_cargo_history_duration_uses_monotonic_clock(cargo_env, tmp_path):
    clock_calls = tmp_path / "clock-calls"
    fake_python = tmp_path / "bin" / "python3"
    fake_python.write_text(
        f"#!{sys.executable}\n"
        "import os, sys\n"
        "from pathlib import Path\n"
        f"calls = Path({str(clock_calls)!r})\n"
        "if sys.argv[1:2] == ['-c'] and 'import time; print(' in sys.argv[2]:\n"
        "    index = int(calls.read_text()) if calls.exists() else 0\n"
        "    calls.write_text(str(index + 1))\n"
        "    monotonic = 'clock_gettime_ns(time.CLOCK_MONOTONIC)' in sys.argv[2]\n"
        "    print(([1000, 1500] if monotonic else [5000, 3000])[index])\n"
        "else:\n"
        "    os.execv(sys.executable, [sys.executable, *sys.argv[1:]])\n"
    )
    fake_python.chmod(0o755)
    cargo_env["QUANTA_INDEX_BUILD_LOGGING"] = "1"

    result = run(cargo_env, "metadata", "--no-deps")
    assert result.returncode == 0, result.stderr
    assert clock_calls.read_text() == "2"
    history = tmp_path / "cache" / "state" / "build-profile" / "history.jsonl"
    event = json.loads(history.read_text().splitlines()[0])
    assert event["k"] == "cargo"
    assert event["cmd"] == "metadata"
    assert event["ms"] == 500


@pytest.mark.parametrize("arguments,admitted", [
    ([], True),
    (["--binaries-metadata", "bins.json"], True),
    (["--cargo-metadata", "cargo.json"], True),
    (["--", "--skip", "--binaries-metadata", "--skip", "--cargo-metadata", "foo"], True),
    (["--binaries-metadata", "bins.json", "--cargo-metadata", "cargo.json"], False),
    (["--binaries-metadata=bins.json", "--cargo-metadata=cargo.json"], False),
])
def test_nextest_list_bypasses_only_with_both_reused_metadata_inputs(cargo_env, arguments, admitted):
    result = run(cargo_env, "nextest", "list", *arguments)
    assert result.returncode == 0, result.stderr
    assert records(cargo_env)[0]["held"] is admitted


def test_distinct_lanes_share_one_resource_slot_with_a_live_marker_oracle(cargo_env, tmp_path):
    started, release = tmp_path / "first-started", tmp_path / "release"
    first_env = {**cargo_env, "FAKE_CARGO_STARTED": str(started), "FAKE_CARGO_RELEASE": str(release)}
    first = subprocess.Popen(
        [str(CARGOW), "--lane", "dev-lane", "build"], cwd=ROOT, env=first_env,
        stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True,
    )
    try:
        deadline = time.monotonic() + 5
        while not started.exists():
            assert first.poll() is None, first.communicate()
            assert time.monotonic() < deadline, "first leaf never acquired the slot"
            time.sleep(0.01)
        second_marker = tmp_path / "second-started"
        second_env = {**cargo_env, "FAKE_CARGO_STARTED": str(second_marker)}
        blocked = run(second_env, "--lane", "clippy-lane", "clippy")
        assert blocked.returncode == 124, blocked.stderr
        assert "resource admission wait timed out" in blocked.stderr
        assert not second_marker.exists()
        release.touch()
        _, error = first.communicate(timeout=10)
        assert first.returncode == 0, error
        resumed = run(second_env, "--lane", "clippy-lane", "clippy")
        assert resumed.returncode == 0, resumed.stderr
        assert second_marker.read_text() == "started"
        observed = records(cargo_env)
        assert len(observed) == 2 and all(row["held"] for row in observed)
        assert observed[0]["target"] != observed[1]["target"]
    finally:
        release.touch()
        if first.poll() is None:
            first.communicate(timeout=12)


def test_workspace_daemon_preparation_and_main_each_acquire_without_recursion(cargo_env):
    result = run(cargo_env, "--lane", "test-workspace-lane", "nextest", "run", "--workspace")
    assert result.returncode == 0, result.stderr
    observed = records(cargo_env)
    assert len(observed) == 2 and all(row["held"] for row in observed)
    assert observed[0]["argv"] == [
        "build", "-p", "quanta-index-searchd-runtime", "--bin", "quanta-index-searchd", "--locked"
    ]
    assert observed[1]["argv"] == ["nextest", "run", "--workspace"]
    assert observed[1]["pin"] == str(Path(observed[1]["target"]) / "debug/quanta-index-searchd")
    assert result.stderr.count("resource admission: admitted") == 2
    assert result.stderr.count("resource admission: released") == 2


@pytest.mark.parametrize("arguments", [["+stable", "build"], ["--locked", "build"]])
def test_global_options_and_toolchain_prefixes_cannot_bypass_admission(cargo_env, arguments):
    result = run(cargo_env, *arguments)
    assert result.returncode == 0, result.stderr
    assert records(cargo_env)[0]["held"] is True


@pytest.mark.parametrize("control", ["WAIT", "TIMEOUT"])
def test_explicit_empty_resource_deadline_is_rejected(cargo_env, control):
    cargo_env[f"QUANTA_INDEX_RESOURCE_{control}_SECONDS"] = ""
    result = run(cargo_env, "build")
    assert result.returncode == 2, result.stderr
    assert not Path(cargo_env["FAKE_CARGO_RECORDS"]).exists()
