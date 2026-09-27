"""Independent raw Criterion samples govern typed evidence, not PASS flags."""

import copy
import json
import sys
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[3] / "tools/benchmark"))
import criterion_capture as capture


def raw():
    estimate = {
        "point_estimate": 100.0,
        "standard_error": 0.0,
        "confidence_interval": {
            "confidence_level": 0.95,
            "lower_bound": 100.0,
            "upper_bound": 100.0,
        },
    }
    return {
        "listing.txt": b"parse/1024: benchmark\n",
        "benchmark.json": json.dumps(
            {
                "full_id": "parse/1024",
                "group_id": "parse",
                "function_id": None,
                "value_str": "1024",
                "throughput": None,
                "directory_name": "parse/1024",
                "title": "parse/1024",
            }
        ).encode(),
        "sample.json": json.dumps(
            {
                "sampling_mode": "Linear",
                "iters": list(range(1, 11)),
                "times": [100 * n for n in range(1, 11)],
            }
        ).encode(),
        "estimates.json": json.dumps(
            {name: estimate for name in ("mean", "median", "median_abs_dev", "slope", "std_dev")}
        ).encode(),
    }


def test_raw_mean_and_iterations_are_preserved():
    result = capture.payload(raw(), "parse/1024")
    assert result == {
        "kind": "micro",
        "bench_id": "parse/1024",
        "metric": "mean",
        "unit": "ns",
        "instrumentation": "wall",
        "statistic": "mean",
        "value": 100.0,
        "iterations": 55,
        "samples": 10,
    }


@pytest.mark.parametrize(
    "filename,field,value",
    [
        ("benchmark.json", "full_id", "wrong"),
        ("benchmark.json", "group_id", []),
        ("benchmark.json", "throughput", {"Bytes": True}),
        ("sample.json", "iters", [1] * 9),
        ("sample.json", "iters", [True] * 10),
        ("sample.json", "iters", [1.5] * 10),
        ("sample.json", "times", [None] * 10),
        ("sample.json", "times", [float("nan")] * 10),
        ("sample.json", "sampling_mode", "unknown"),
        ("estimates.json", "mean", {"point_estimate": 99.0}),
        ("estimates.json", "mean", {}),
        ("estimates.json", "slope", None),
    ],
)
def test_bad_native_facts_are_refused(filename, field, value):
    data = raw()
    document = json.loads(data[filename])
    document[field] = value
    data[filename] = json.dumps(document).encode()
    with pytest.raises(ValueError):
        capture.payload(data, "parse/1024")


@pytest.mark.parametrize("listing", [b"", b"a: benchmark\na: benchmark\n", b"a\n"])
def test_listing_is_exact_and_nonempty(listing):
    with pytest.raises(ValueError):
        capture.listed_cases(listing)


def test_duplicate_json_keys_are_not_last_writer_wins():
    data = raw()
    data["benchmark.json"] = b'{"full_id":"wrong","full_id":"parse/1024"}'
    with pytest.raises(ValueError, match="duplicate"):
        capture.payload(data, "parse/1024")


def test_missing_native_sample_refused():
    data = raw()
    del data["sample.json"]
    with pytest.raises(ValueError, match="incomplete"):
        capture.payload(data, "parse/1024")


@pytest.mark.parametrize("large_log", [False, True])
def test_replay_refuses_forged_typed_mean(tmp_path, large_log, monkeypatch):
    import evidence_bridge
    import host_monitor

    data = raw()
    digest = capture.digest_bytes(b"binary")
    build_argv = [
        "/fixture/scripts/cargow",
        "--lane",
        "bench-lane",
        "bench",
        "-p",
        "owner",
        "--bench",
        "pipeline",
        "--all-features",
        "--locked",
        "--no-run",
        "--message-format=json",
    ]
    command = {"argv": ["/fixture/pipeline"]}
    data.update(
        {
            "execution.json": json.dumps(
                {
                    "measure": command,
                    "capture_id": "r1",
                    "binary_digest": digest,
                    "features": [],
                    "build": {"argv": build_argv, "status": "completed", "exit_code": 0},
                }
            ).encode(),
            "rustc.txt": b"rustc pinned",
            "build.jsonl": json.dumps(
                {
                    "reason": "compiler-artifact",
                    "target": {"name": "pipeline", "kind": ["bench"]},
                    "features": [],
                    "executable": "/fixture/pipeline",
                }
            ).encode()
            + b'\n{"reason":"build-finished","success":true}\n',
        }
    )
    run = tmp_path / "runs/r1-0/raw"
    run.mkdir(parents=True)
    for name, content in data.items():
        with (run / name).open("wb") as stream:
            if name == "build.jsonl" and large_log:
                for _ in range(300):
                    stream.write(
                        b'{"reason":"compiler-message","message":"' + b"x" * 65536 + b'"}\n'
                    )
            stream.write(content)
    monkeypatch.setattr(host_monitor, "lock_path", lambda: tmp_path / "host-lock")
    host = {"os": "macos", "arch": "arm64", "cpu_count": 8, "hostname_hash": "sha256:" + "a" * 64}
    facts = {
        "load_average": [0.1, 0.2, 0.3],
        "disk_available_bytes": 100,
        "process_count": 1,
        "process_snapshot_sha256": "sha256:" + "b" * 64,
        "foreign_rust": [],
    }
    monkeypatch.setattr(host_monitor, "observe", lambda: (host, facts))
    host_raw = (
        host_monitor.HostMonitor(run / "host-observations.jsonl", "r1", "micro").start().finish()
    )
    evidence = {
        "run_id": "r1-0",
        "profile": "micro",
        "case_id": "parse/1024",
        "payload": copy.deepcopy(capture.payload(data, "parse/1024")),
        "verdict": {"scope": "diagnostic"},
        "raw": [{"path": f"raw/{name}"} for name in data]
        + [
            {
                "path": "raw/host-observations.jsonl",
                "sha256": host_raw.sha256,
                "bytes": host_raw.size,
            }
        ],
        "inputs": [
            {
                "id": "benchmark-host-observations",
                "availability": "present",
                "digest": host_raw.sha256,
                "reason": None,
            }
        ],
        "host": evidence_bridge.host_from_observations(
            host_raw,
            policy="local-diagnostic",
            capture_id="r1",
            profile="micro",
        ),
        "boundary": {"start_event": "criterion_sample_start"},
        "command": command,
        "build": {
            "binaries": [{"name": "pipeline", "sha256": digest}],
            "toolchain": "rustc pinned",
            "flags": build_argv[3:],
        },
    }
    capture.replay_run(capture.RunStore(tmp_path), evidence)
    evidence["payload"]["value"] = 42.0
    with pytest.raises(ValueError, match="differs"):
        capture.replay_run(capture.RunStore(tmp_path), evidence)
    evidence["payload"]["value"] = 100.0
    execution = json.loads(data["execution.json"])
    execution["capture_id"] = "another-capture"
    (run / "execution.json").write_text(json.dumps(execution))
    with pytest.raises(ValueError, match="does not belong"):
        capture.replay_run(capture.RunStore(tmp_path), evidence)


@pytest.mark.parametrize(
    "messages",
    [
        b'{"reason":"build-finished","success":false}\n',
        b'{"reason":"compiler-artifact","target":[]}',
        b"[1,2,3]",
        b'{"reason":"compiler-artifact","target":{"name":"pipeline","kind":["bench"]},"executable":"/fixture/pipeline","features":[]}',
    ],
)
def test_malformed_or_partial_cargo_inventory_is_refused(tmp_path, messages):
    with pytest.raises(ValueError):
        capture._binary(capture.write_raw_file(tmp_path / "build.jsonl", [messages]), "pipeline")


def cargo_artifact():
    return b'{"reason":"compiler-artifact","target":{"name":"pipeline","kind":["bench"]},"executable":"/fixture/pipeline","features":["feature-a"]}\n'


@pytest.mark.parametrize("suffix", [b"", b"\n", b"\r\n"])
def test_cargo_stream_inventory_accepts_exact_terminal_without_materialization(tmp_path, suffix):
    raw = capture.write_raw_file(
        tmp_path / "build.jsonl",
        [
            b"cargow: routed lane\n",
            cargo_artifact(),
            b'  {"reason":"build-finished","success":true}' + suffix,
        ],
    )
    assert capture._binary(raw, "pipeline") == (Path("/fixture/pipeline"), ["feature-a"])


@pytest.mark.parametrize(
    "variant", ["duplicate", "after-terminal", "duplicate-terminal", "partial", "utf8"]
)
def test_cargo_stream_refuses_ambiguous_or_partial_terminal_inventory(tmp_path, variant):
    terminal = b'{"reason":"build-finished","success":true}\n'
    messages = {
        "duplicate": cargo_artifact() * 2 + terminal,
        "after-terminal": terminal + cargo_artifact(),
        "duplicate-terminal": cargo_artifact() + terminal * 2,
        "partial": cargo_artifact() + terminal + b'{"reason":',
        "utf8": cargo_artifact() + terminal + b"\xff",
    }[variant]
    raw = capture.write_raw_file(tmp_path / "build.jsonl", [messages])
    with pytest.raises(ValueError):
        capture._binary(raw, "pipeline")


def test_sigterm_kills_owned_producer(tmp_path):
    import os
    import signal
    import subprocess
    import time

    pid_file = tmp_path / "child.pid"
    child_script = (
        f"import os,time; from pathlib import Path; p=Path({str(pid_file)!r}); "
        "staged=p.with_suffix('.tmp'); staged.write_text(str(os.getpid())); staged.replace(p); time.sleep(60)"
    )
    runner = (
        "import os,sys; from pathlib import Path; "
        f"sys.path.insert(0,{str(Path(capture.__file__).parent)!r}); "
        "from criterion_capture import execute; "
        f"execute([sys.executable,'-c',{child_script!r}],cwd=Path({str(tmp_path)!r}),env=dict(os.environ),timeout=60,log_dir=Path({str(tmp_path / 'execution')!r}))"
    )
    process = subprocess.Popen(
        [sys.executable, "-c", runner], stdout=subprocess.PIPE, stderr=subprocess.PIPE
    )
    try:
        deadline = time.monotonic() + 10
        while not pid_file.exists() and time.monotonic() < deadline:
            time.sleep(0.01)
        assert pid_file.exists(), "owned child did not start"
        child_pid = int(pid_file.read_text())
        process.send_signal(signal.SIGTERM)
        _, stderr = process.communicate(timeout=10)
        assert process.returncode != 0
        assert b"interrupted by SIGTERM" in stderr
        with pytest.raises(ProcessLookupError):
            os.kill(child_pid, 0)
    finally:
        if process.poll() is None:
            process.terminate()
            process.communicate(timeout=10)


def test_timeout_kills_owned_process_group(tmp_path):
    import os
    import signal

    previous = signal.getsignal(signal.SIGTERM)
    with pytest.raises(ValueError, match="timed out"):
        capture.execute(
            [sys.executable, "-c", "import time; time.sleep(60)"],
            cwd=tmp_path,
            env=dict(os.environ),
            timeout=1,
            log_dir=tmp_path / "execution",
        )
    assert signal.getsignal(signal.SIGTERM) == previous


def test_failure_cleanup_has_a_finite_pipe_drain_deadline(tmp_path, monkeypatch):
    import subprocess

    import producer_execution as execution

    class Process:
        pid = 123
        returncode = -9

        def __init__(self):
            self.calls = []

        def wait(self, *, timeout=None):
            self.calls.append(timeout)
            assert type(timeout) in (int, float) and 0 < timeout <= 10
            return -9

    process = Process()
    monkeypatch.setattr(execution.subprocess, "Popen", lambda *args, **kwargs: process)
    monkeypatch.setattr(execution.os, "killpg", lambda *args: None)
    monkeypatch.setattr(execution, "_drain_output", lambda p, sinks, timeout: None)

    def terminal_timeout(*args):
        raise subprocess.TimeoutExpired("fixture", args[-1])

    monkeypatch.setattr(execution, "_wait_for_terminal", terminal_timeout)
    with pytest.raises(ValueError, match="timed out"):
        execution.execute(
            ["fixture"], cwd=tmp_path, env={}, timeout=1, log_dir=tmp_path / "execution"
        )
    assert len(process.calls) == 1


def test_failed_pipe_drain_and_reap_preserve_primary_failure(tmp_path, monkeypatch):
    import io
    import signal
    import subprocess

    import producer_execution as execution

    class Process:
        pid = 123
        stdout = io.BytesIO()
        stderr = io.BytesIO()

        def wait(self, *, timeout):
            assert 0 <= timeout <= 10
            raise subprocess.TimeoutExpired("fixture", timeout)

    process = Process()
    handlers = {sig: signal.getsignal(sig) for sig in (signal.SIGTERM, signal.SIGINT)}
    monkeypatch.setattr(execution.subprocess, "Popen", lambda *args, **kwargs: process)
    monkeypatch.setattr(execution.os, "killpg", lambda *args: None)

    def terminal_timeout(*args):
        raise subprocess.TimeoutExpired("fixture", args[-1])

    monkeypatch.setattr(execution, "_wait_for_terminal", terminal_timeout)
    monkeypatch.setattr(execution, "_drain_output", terminal_timeout)
    with pytest.raises(
        execution.ProducerExecutionError, match="timed out.*incomplete cleanup.*pipe drain.*reap"
    ):
        execution.execute(
            ["fixture"], cwd=tmp_path, env={}, timeout=1, log_dir=tmp_path / "execution"
        )
    assert process.stdout.closed and process.stderr.closed
    assert {sig: signal.getsignal(sig) for sig in handlers} == handlers


@pytest.mark.parametrize("direct_child_exits", [False, True])
def test_escaped_session_held_pipe_returns_bounded_failure(tmp_path, direct_child_exits):
    import os
    import signal
    import subprocess

    pid_file = tmp_path / "escaped.pid"
    escaped = "import time; time.sleep(60)"
    child = (
        "import subprocess,sys,time; from pathlib import Path; "
        f"p=subprocess.Popen([sys.executable,'-c',{escaped!r}],start_new_session=True); "
        f"f=Path({str(pid_file)!r}); staged=f.with_suffix('.tmp'); staged.write_text(str(p.pid)); staged.replace(f); "
        + ("pass" if direct_child_exits else "time.sleep(60)")
    )
    runner = (
        "import os,sys; from pathlib import Path; "
        f"sys.path.insert(0,{str(Path(capture.__file__).parent)!r}); "
        "import producer_execution as execution; execution.CLEANUP_TIMEOUT_SECONDS=1; "
        f"execution.execute([sys.executable,'-c',{child!r}],cwd=Path({str(tmp_path)!r}),env=dict(os.environ),timeout=5,log_dir=Path({str(tmp_path / 'execution')!r}))"
    )
    process = subprocess.Popen(
        [sys.executable, "-c", runner], stdout=subprocess.PIPE, stderr=subprocess.PIPE
    )
    try:
        _, errors = process.communicate(timeout=20)
        assert process.returncode != 0
        assert b"timed out" in errors and b"incomplete cleanup" in errors
        assert b"pipe drain deadline exceeded" in errors
        assert pid_file.is_file(), "escaped child was not exercised"
    finally:
        if process.poll() is None:
            process.kill()
            process.communicate(timeout=5)
        if pid_file.exists():
            # Exact PID created by this fixture; no broad process-name cleanup.
            try:
                os.kill(int(pid_file.read_text()), signal.SIGKILL)
            except ProcessLookupError:
                pass


def test_nested_owned_session_dies_when_outer_owner_is_killed(tmp_path):
    import os
    import signal
    import subprocess
    import time

    pid_file = tmp_path / "nested.pid"
    command = f"import os,time; from pathlib import Path; f=Path({str(pid_file)!r}); staged=f.with_suffix('.tmp'); staged.write_text(str(os.getpid())); staged.replace(f); time.sleep(60)"
    bootstrap = f"import os,sys; from pathlib import Path; sys.path.insert(0,{str(Path(capture.__file__).parent)!r}); from producer_execution import execute; "
    nested = (
        bootstrap
        + f"execute([sys.executable,'-c',{command!r}],cwd=Path({str(tmp_path)!r}),env=dict(os.environ),timeout=60,log_dir=Path({str(tmp_path / 'nested-execution')!r}))"
    )
    outer = (
        bootstrap
        + f"execute([sys.executable,'-c',{nested!r}],cwd=Path({str(tmp_path)!r}),env=dict(os.environ),timeout=60,log_dir=Path({str(tmp_path / 'outer-execution')!r}))"
    )
    process = subprocess.Popen(
        [sys.executable, "-c", outer], stdout=subprocess.PIPE, stderr=subprocess.PIPE
    )
    pid = None
    try:
        deadline = time.monotonic() + 10
        while not pid_file.exists() and time.monotonic() < deadline:
            time.sleep(0.01)
        assert pid_file.exists(), "nested owned session was not exercised"
        pid = int(pid_file.read_text())
        process.kill()  # SIGKILL cannot run Python finally/cleanup handlers.
        process.communicate(timeout=5)
        deadline = time.monotonic() + 10
        while time.monotonic() < deadline:
            state = subprocess.run(
                ["ps", "-o", "stat=", "-p", str(pid)], capture_output=True, text=True, timeout=2
            )
            if not state.stdout.strip() or state.stdout.strip().startswith("Z"):
                break
            time.sleep(0.01)
        else:
            pytest.fail("nested owned command survived controlling owner death")
    finally:
        if process.poll() is None:
            process.kill()
            process.communicate(timeout=5)
        if pid is not None:
            try:
                os.kill(pid, signal.SIGKILL)
            except ProcessLookupError:
                pass


@pytest.mark.parametrize("exit_code", [0, 7, -15])
@pytest.mark.parametrize("kill_guard_after_report", [False, True])
def test_direct_child_exit_terminates_same_group_background_descendant(
    tmp_path, monkeypatch, exit_code, kill_guard_after_report
):
    import os
    import signal
    import subprocess
    import time

    import producer_execution as execution

    if kill_guard_after_report:
        wait_for_terminal = execution._wait_for_terminal

        def kill_reported_guard(process, terminal, sinks, timeout):
            record = wait_for_terminal(process, terminal, sinks, timeout)
            os.kill(process.pid, signal.SIGKILL)
            return record

        monkeypatch.setattr(execution, "_wait_for_terminal", kill_reported_guard)

    pid_file = tmp_path / "background.pid"
    background = (
        "import os,time; from pathlib import Path; "
        f"f=Path({str(pid_file)!r}); staged=f.with_suffix('.tmp'); "
        "staged.write_text(str(os.getpid())); staged.replace(f); time.sleep(60)"
    )
    command = (
        "import os,signal,subprocess,sys,time; from pathlib import Path; "
        f"subprocess.Popen([sys.executable,'-c',{background!r}],"
        "stdin=subprocess.DEVNULL,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL); "
        f"f=Path({str(pid_file)!r}); deadline=time.monotonic()+10\n"
        "while not f.exists() and time.monotonic()<deadline: time.sleep(0.01)\n"
        "assert f.exists(), 'background child was not exercised'\n"
        + (f"sys.exit({exit_code})" if exit_code >= 0 else "os.kill(os.getpid(),signal.SIGTERM)")
    )
    try:
        if exit_code == 0:
            _, _, result = capture.execute(
                [sys.executable, "-c", command],
                cwd=tmp_path,
                env=dict(os.environ),
                timeout=20,
                log_dir=tmp_path / "execution",
            )
            assert result["status"] == "completed" and result["exit_code"] == 0
        else:
            with pytest.raises(ValueError, match=f"exit {exit_code}"):
                capture.execute(
                    [sys.executable, "-c", command],
                    cwd=tmp_path,
                    env=dict(os.environ),
                    timeout=20,
                    log_dir=tmp_path / "execution",
                )
        assert pid_file.is_file(), "background descendant was not exercised"
        pid = int(pid_file.read_text())
        deadline = time.monotonic() + 2
        while time.monotonic() < deadline:
            state = subprocess.run(
                ["ps", "-o", "stat=", "-p", str(pid)], capture_output=True, text=True, timeout=2
            ).stdout.strip()
            if not state or state.startswith("Z"):
                break
            time.sleep(0.01)
        else:
            pytest.fail("same-group descendant survived direct child exit")
    finally:
        if pid_file.exists():
            try:
                os.kill(int(pid_file.read_text()), signal.SIGKILL)
            except ProcessLookupError:
                pass


@pytest.mark.parametrize(
    "returncode,raw",
    [
        (-9, b""),
        (-9, b"\0"),
        (-9, b"\0" * 3),
        (-9, b"\0" * 5),
        (0, b"\0" * 4),
        (1, b"\0" * 4),
        (-15, b"\0" * 4),
        (-9, b"\0\0\x01\0"),
        (-9, b"\xff\xff\xff\0"),
    ],
)
def test_unproven_guard_terminal_record_cannot_establish_completion(returncode, raw):
    import producer_execution as execution

    with pytest.raises(execution.ProducerExecutionError, match="terminal record|exit code"):
        execution._terminal_result(returncode, raw)


def test_guard_killed_without_terminal_record_is_not_success(tmp_path):
    import os

    with pytest.raises(ValueError, match="without a valid terminal record"):
        capture.execute(
            [sys.executable, "-c", "import os,signal; os.kill(os.getppid(),signal.SIGKILL)"],
            cwd=tmp_path,
            env=dict(os.environ),
            timeout=10,
            log_dir=tmp_path / "execution",
        )


def test_spawn_failure_closes_all_control_pipe_descriptors(tmp_path, monkeypatch):
    import os
    import signal

    import producer_execution as execution

    pipe = os.pipe
    descriptors = []

    def tracked_pipe():
        pair = pipe()
        if len(descriptors) < 4:
            descriptors.extend(pair)
        return pair

    previous = signal.getsignal(signal.SIGTERM)
    monkeypatch.setattr(execution.os, "pipe", tracked_pipe)
    with pytest.raises(ValueError, match="without a valid terminal record"):
        execution.execute(
            [str(tmp_path / "missing")],
            cwd=tmp_path,
            env={},
            timeout=10,
            log_dir=tmp_path / "execution",
        )
    assert signal.getsignal(signal.SIGTERM) == previous
    assert len(descriptors) == 4
    for fd in descriptors:
        with pytest.raises(OSError):
            os.fstat(fd)


def test_terminal_wait_drains_both_outputs_before_child_can_report(tmp_path):
    import os

    stdout, stderr = b"o" * 262144, b"e" * 262144
    actual_out, actual_err, result = capture.execute(
        [
            sys.executable,
            "-c",
            "import sys; sys.stdout.buffer.write(b'o'*262144); sys.stdout.flush(); "
            "sys.stderr.buffer.write(b'e'*262144); sys.stderr.flush()",
        ],
        cwd=tmp_path,
        env=dict(os.environ),
        timeout=20,
        log_dir=tmp_path / "execution",
    )
    assert actual_out.read_control() == stdout and actual_err.read_control() == stderr
    assert result["status"] == "completed"


def test_cleanup_never_signals_an_already_reaped_group_identity(monkeypatch):
    import producer_execution as execution

    class Process:
        pid = 123
        returncode = -9

        def wait(self, *, timeout):
            assert 0 < timeout <= 10
            return -9

    def forbidden_kill(*args):
        pytest.fail("an already reaped group identity cannot be signalled")

    monkeypatch.setattr(execution.os, "killpg", forbidden_kill)
    monkeypatch.setattr(execution, "_drain_output", lambda p, sinks, timeout: None)
    assert execution._cleanup(Process(), ()) is None


@pytest.mark.parametrize("ignored", [False, True])
def test_execution_refuses_external_sigchld_reaping_before_spawning(tmp_path, ignored):
    import os
    import signal

    marker = tmp_path / "must-not-run"
    previous = signal.signal(signal.SIGCHLD, signal.SIG_IGN if ignored else lambda *_: None)
    try:
        with pytest.raises(ValueError, match="SIGCHLD"):
            capture.execute(
                [sys.executable, "-c", f"from pathlib import Path; Path({str(marker)!r}).touch()"],
                cwd=tmp_path,
                env=dict(os.environ),
                timeout=10,
                log_dir=tmp_path / "execution",
            )
        assert not marker.exists(), "an unowned reaping environment cannot launch work"
    finally:
        signal.signal(signal.SIGCHLD, previous)


def test_execution_monitors_high_numbered_pipe_descriptors(tmp_path, monkeypatch):
    import fcntl
    import os
    import resource

    if resource.getrlimit(resource.RLIMIT_NOFILE)[0] <= 2064:
        pytest.skip("host descriptor limit cannot exercise high-numbered pipes")
    pipe = os.pipe

    def high_pipe():
        original = pipe()
        duplicated = []
        try:
            for fd in original:
                duplicated.append(fcntl.fcntl(fd, fcntl.F_DUPFD, 2048))
                os.set_inheritable(duplicated[-1], False)
        except BaseException:
            for fd in duplicated:
                os.close(fd)
            raise
        finally:
            for fd in original:
                os.close(fd)
        return tuple(duplicated)

    monkeypatch.setattr(os, "pipe", high_pipe)
    stdout, _, result = capture.execute(
        [sys.executable, "-c", "print('high-fd')"],
        cwd=tmp_path,
        env=dict(os.environ),
        timeout=20,
        log_dir=tmp_path / "execution",
    )
    assert stdout.read_control() == b"high-fd\n" and result["status"] == "completed"


@pytest.mark.parametrize("missing", [False, True])
def test_complete_profile_transaction_with_synthetic_native_owner(tmp_path, monkeypatch, missing):
    """Exercise orchestration; synthetic timing is never product evidence."""
    import benchctl

    repo = tmp_path / "repo"
    repo.mkdir()
    (repo / "Cargo.lock").write_bytes(b"locked")
    binary = tmp_path / "pipeline"
    binary.write_bytes(b"synthetic executable identity")
    cases = [
        f"{stage}/{size}"
        for stage in ("tokenize", "parse", "normalize", "hash")
        for size in (1024, 4096, 16384)
    ]
    listing = "".join(f"{case}: benchmark\n" for case in cases).encode()
    source = {
        "revision": "a" * 40,
        "dirty": False,
        "dirty_paths_digest": None,
        "closure_profile": "benchmark-micro",
        "closure_digest": capture.digest_bytes(b"source"),
    }
    monkeypatch.setattr(benchctl, "require_clean_worktree", lambda _repo: None)
    monkeypatch.setattr(benchctl, "require_frozen_source", lambda _repo, _head: None)
    monkeypatch.setattr(benchctl, "resolve_checkout_head", lambda _repo: "a" * 40)
    monkeypatch.setattr(capture, "source_identity", lambda *_args: source)
    registry = {
        "schema_version": 1,
        "closures": {},
        "external_inputs": {},
        "validators": {},
        "scorers": {},
        "profiles": {"micro": {"families": ["micro-lq-norm-pipeline"]}},
        "families": {"micro-lq-norm-pipeline": {"payload": "micro", "producer": "pipeline"}},
        "producers": {
            "pipeline": {"kind": "cargo-bench", "package": "owner", "target": "pipeline"}
        },
    }

    def producer(argv, *, cwd, env, timeout, log_dir, custody_fds):
        assert len(custody_fds) == 1
        command = {
            "argv": argv,
            "cwd": str(cwd),
            "status": "completed",
            "exit_code": 0,
            "timeout_seconds": timeout,
            "wall_ms": 1,
        }
        if "--message-format=json" in argv:
            output = (
                json.dumps(
                    {
                        "reason": "compiler-artifact",
                        "target": {"name": "pipeline", "kind": ["bench"]},
                        "executable": str(binary),
                        "features": [],
                    }
                ).encode()
                + b'\n{"reason":"build-finished","success":true}\n'
            )
        elif argv[0] == "rustc":
            output = b"rustc pinned\nhost: aarch64-apple-darwin\n"
        elif "--list" in argv:
            output = listing
        elif "--test" in argv:
            output = b"Success\n"
        else:
            for index, case in enumerate(cases[:-1] if missing else cases):
                directory = Path(env["CRITERION_HOME"]) / str(index) / "new"
                directory.mkdir(parents=True)
                data = raw()
                meta = json.loads(data["benchmark.json"])
                meta["full_id"] = case
                data["benchmark.json"] = json.dumps(meta).encode()
                for name in ("benchmark.json", "sample.json", "estimates.json"):
                    (directory / name).write_bytes(data[name])
            output = b"measured\n"
        return (
            capture.write_raw_file(log_dir / "stdout", [output]),
            capture.write_raw_file(log_dir / "stderr", [b""]),
            command,
        )

    monkeypatch.setattr(capture, "execute", producer)
    root = tmp_path / "evidence"
    kwargs = dict(samples=10, warmup=0.01, measurement=0.01, resamples=1000, timeout=1)
    if missing:
        with pytest.raises(ValueError, match="missing cases"):
            capture.capture(repo, root, "micro", registry, **kwargs)
        assert not (root / "profiles/micro.json").exists()
        assert not (root / "runs").exists()
    else:
        document = capture.capture(repo, root, "micro", registry, **kwargs)
        assert len(document["runs"]) == 12
        assert capture.validate(repo, root, "micro", registry) == document
