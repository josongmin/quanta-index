"""Child terminal status uses notifications without relaxing process custody."""

import hashlib
import json
import os
import selectors
import signal
import struct
import subprocess
import sys
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[3] / "tools/benchmark"))
import producer_execution as execution


@pytest.mark.parametrize(
    "command,expected",
    [
        ("import os; os._exit(0)", 0),
        ("import os; os._exit(7)", 7),
        ("import time; time.sleep(0.02)", 0),
        ("import os,signal; os.kill(os.getpid(),signal.SIGTERM)", -signal.SIGTERM),
    ],
)
@pytest.mark.parametrize("blocked_sigchld", [False, True])
def test_guard_uses_unbounded_event_wait_and_preserves_terminal_status(
    tmp_path, command, expected, blocked_sigchld
):
    """Reject periodic polling deterministically, independently of host speed."""
    lifeline_read, lifeline_write = os.pipe()
    terminal_read, terminal_write = os.pipe()
    trace_read, trace_write = os.pipe()
    script = f"""
import json, os, selectors, signal, subprocess, sys
sys.path.insert(0, {str(Path(execution.__file__).parent)!r})
import producer_execution as execution
OriginalSelector = selectors.DefaultSelector
class EventOnlySelector(OriginalSelector):
    def select(self, timeout=None):
        os.write({trace_write}, (json.dumps(timeout) + '\\n').encode())
        if timeout not in (None, 0):
            raise RuntimeError('periodic polling is forbidden')
        return super().select(timeout)
execution.selectors.DefaultSelector = EventOnlySelector
original_popen = subprocess.Popen
def checked_popen(*args, **kwargs):
    if signal.getsignal(signal.SIGCHLD) == signal.SIG_DFL:
        raise RuntimeError('SIGCHLD handler must precede spawn')
    wakeup = signal.set_wakeup_fd(-1)
    signal.set_wakeup_fd(wakeup)
    if wakeup < 0:
        raise RuntimeError('wakeup pipe must precede spawn')
    return original_popen(*args, **kwargs)
execution.subprocess.Popen = checked_popen
if {blocked_sigchld!r}:
    signal.pthread_sigmask(signal.SIG_BLOCK, {{signal.SIGCHLD}})
execution._owned_child({lifeline_read}, {terminal_write}, [sys.executable, '-c', {command!r}])
"""
    process = None
    try:
        process = subprocess.Popen(
            [sys.executable, "-c", script],
            start_new_session=True,
            pass_fds=(lifeline_read, terminal_write, trace_write),
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )
        for fd in (lifeline_read, terminal_write, trace_write):
            os.close(fd)
        lifeline_read = terminal_write = trace_write = None
        with selectors.DefaultSelector() as watch:
            watch.register(terminal_read, selectors.EVENT_READ)
            assert watch.select(10), "guard failed to publish terminal status"
            terminal = os.read(terminal_read, 5)
        assert terminal == struct.pack("!i", expected)
        # The guard must reach a blocking event wait, not merely publish a
        # terminal record and die. Its live PID still owns the group identity.
        with selectors.DefaultSelector() as watch:
            watch.register(trace_read, selectors.EVENT_READ)
            trace = b""
            while b"null\n" not in trace:
                assert watch.select(10), "guard never entered an event wait"
                block = os.read(trace_read, 4096)
                assert block, "guard died before event wait"
                trace += block
        timeouts = [json.loads(line) for line in trace.splitlines()]
        assert all(timeout is None or timeout == 0 for timeout in timeouts)
    finally:
        if process is not None:
            # Kill before reaping even on a failed assertion: preserve the same
            # group identity custody that the production controller requires.
            os.killpg(process.pid, signal.SIGKILL)
            _, stderr = process.communicate(timeout=10)
        for fd in (
            lifeline_read,
            lifeline_write,
            terminal_read,
            terminal_write,
            trace_read,
            trace_write,
        ):
            if fd is not None:
                os.close(fd)
    assert process.returncode == -signal.SIGKILL, stderr.decode()


@pytest.mark.parametrize("raise_inside", [False, True])
def test_notification_context_restores_handler_wakeup_and_descriptors(raise_inside):
    previous_handler = signal.getsignal(signal.SIGCHLD)
    outside_read, outside_write = os.pipe()
    os.set_blocking(outside_write, False)
    original_wakeup = signal.set_wakeup_fd(outside_write)
    try:
        try:
            with execution._child_exit_notifications() as notifications:
                assert not os.get_blocking(notifications)
                os.kill(os.getpid(), signal.SIGCHLD)
                assert os.read(notifications, 1) == bytes([signal.SIGCHLD])
                if raise_inside:
                    raise RuntimeError("injected setup/body failure")
        except RuntimeError:
            assert raise_inside
        assert signal.getsignal(signal.SIGCHLD) == previous_handler
        assert signal.set_wakeup_fd(outside_write) == outside_write
        with pytest.raises(OSError):
            os.fstat(notifications)
    finally:
        signal.set_wakeup_fd(original_wakeup)
        signal.signal(signal.SIGCHLD, previous_handler)
        os.close(outside_read)
        os.close(outside_write)


def test_notification_install_failure_restores_handler_and_closes_pipes(monkeypatch):
    previous_handler = signal.getsignal(signal.SIGCHLD)
    pipe = os.pipe
    descriptors = []

    def tracked_pipe():
        pair = pipe()
        descriptors.extend(pair)
        return pair

    def fail_install(*args, **kwargs):
        raise OSError("injected wakeup installation failure")

    monkeypatch.setattr(execution.os, "pipe", tracked_pipe)
    monkeypatch.setattr(execution.signal, "set_wakeup_fd", fail_install)
    with pytest.raises(OSError, match="injected"):
        with execution._child_exit_notifications():
            pytest.fail("failed installation cannot run producer")
    assert signal.getsignal(signal.SIGCHLD) == previous_handler
    for fd in descriptors:
        with pytest.raises(OSError):
            os.fstat(fd)


@pytest.mark.parametrize("exit_code", [0, 7])
def test_execution_retains_complete_file_logs_and_actual_terminal(tmp_path, exit_code):
    root = tmp_path / "execution"
    command = [
        sys.executable,
        "-c",
        "import os; os.write(1,b'output-oracle'); os.write(2,b'error-oracle'); "
        f"raise SystemExit({exit_code})",
    ]
    if exit_code:
        with pytest.raises(execution.ProducerExecutionError, match="exit 7"):
            execution.execute(command, cwd=tmp_path, env=dict(os.environ), timeout=10, log_dir=root)
    else:
        result = execution.execute(
            command, cwd=tmp_path, env=dict(os.environ), timeout=10, log_dir=root
        )
        assert result.stdout.read_control() == b"output-oracle"
        assert result.stderr.read_control() == b"error-oracle"
        assert result.stdout.size == 13
        assert result.stdout.sha256 == "sha256:" + hashlib.sha256(b"output-oracle").hexdigest()
    record = json.loads((root / "execution.json").read_text())
    assert record["status"] == ("failed" if exit_code else "completed")
    assert record["command"]["exit_code"] == exit_code
    assert record["request"]["argv"] == command
    assert (root / "stdout").read_bytes() == b"output-oracle"
    assert (root / "stderr").read_bytes() == b"error-oracle"
    assert record["raw"][0]["sha256"] == "sha256:" + hashlib.sha256(b"output-oracle").hexdigest()


def test_timeout_retains_prefix_without_inventing_terminal(tmp_path):
    root = tmp_path / "execution"
    with pytest.raises(execution.ProducerExecutionError, match="timed out"):
        execution.execute(
            [sys.executable, "-c", "import os,time; os.write(1,b'before-timeout'); time.sleep(60)"],
            cwd=tmp_path,
            env=dict(os.environ),
            timeout=1,
            log_dir=root,
        )
    record = json.loads((root / "execution.json").read_text())
    assert record["status"] == "failed" and record["command"] is None
    assert (root / "stdout").read_bytes() == b"before-timeout"
    assert record["raw"][0]["bytes"] == 14


def test_execution_does_not_use_whole_output_communicate(tmp_path, monkeypatch):
    def forbidden(*args, **kwargs):
        pytest.fail("execution must not allocate complete output in communicate")

    monkeypatch.setattr(subprocess.Popen, "communicate", forbidden)
    result = execution.execute(
        [sys.executable, "-c", "import os; os.write(1,b'fixed')"],
        cwd=tmp_path,
        env=dict(os.environ),
        timeout=10,
        log_dir=tmp_path / "execution",
    )
    assert result.stdout.read_control() == b"fixed"


def test_public_execution_passes_reservation_to_guard_without_child_inheritance(tmp_path, monkeypatch):
    original = execution._execute_owned
    observed = []

    def checked(argv, **kwargs):
        observed.append(kwargs["custody_fds"])
        return original(argv, **kwargs)

    monkeypatch.setattr(execution, "_execute_owned", checked)
    lock = tmp_path / "reservation"
    fd = os.open(lock, os.O_RDWR | os.O_CREAT | os.O_EXCL, 0o600)
    try:
        script = (
            "import os,sys; "
            "identity=os.stat(sys.argv[1]); "
            "assert not any((lambda s: (s.st_dev,s.st_ino)==(identity.st_dev,identity.st_ino))"
            "(os.fstat(fd)) for fd in range(3,256) if os.path.exists('/dev/fd/'+str(fd)))"
        )
        result = execution.execute(
            [sys.executable, "-c", script, str(lock)], cwd=tmp_path,
            env=dict(os.environ), timeout=10, log_dir=tmp_path / "execution",
            custody_fds=(fd,),
        )
        assert result.command["exit_code"] == 0
        assert observed == [(fd,)]
    finally:
        os.close(fd)


def test_execution_peak_rss_is_payload_independent(tmp_path, record_property):
    results = []
    for size in (8 * 1024 * 1024, 128 * 1024 * 1024):
        script = f"""
import hashlib,json,os,resource,sys
from pathlib import Path
sys.path.insert(0,{str(Path(execution.__file__).parent)!r})
from producer_execution import execute
command = "import os; block=b'x'*65536\\nfor _ in range({size // 65536}): os.write(1,block)"
result=execute([sys.executable,'-c',command],cwd=Path({str(tmp_path)!r}),env=dict(os.environ),timeout=30,log_dir=Path({str(tmp_path / str(size))!r}))
expected=hashlib.sha256()
for _ in range({size // 65536}): expected.update(b'x'*65536)
assert result.stdout.sha256=='sha256:'+expected.hexdigest()
assert result.stdout.size=={size}
print(json.dumps({{'rss':resource.getrusage(resource.RUSAGE_SELF).ru_maxrss*(1 if sys.platform=='darwin' else 1024)}}))
"""
        result = subprocess.run([sys.executable, "-c", script], capture_output=True, timeout=40)
        assert result.returncode == 0, result.stderr.decode()
        peak = json.loads(result.stdout)["rss"]
        record_property(f"execution_{size}_peak_bytes", peak)
        results.append(peak)
    assert results[1] - results[0] < 32 * 1024 * 1024


def test_execution_recording_failure_preserves_primary_and_logs(tmp_path, monkeypatch):
    import evidence

    original = evidence.write_raw_file

    def fail_record(path, blocks):
        if path.name == "execution.json":
            raise OSError("record disk full")
        return original(path, blocks)

    monkeypatch.setattr(evidence, "write_raw_file", fail_record)
    root = tmp_path / "execution"
    with pytest.raises(
        execution.ProducerExecutionError, match="exit 7.*record disk full"
    ) as caught:
        execution.execute(
            [sys.executable, "-c", "import os; os.write(2,b'primary'); raise SystemExit(7)"],
            cwd=tmp_path,
            env=dict(os.environ),
            timeout=10,
            log_dir=root,
        )
    assert isinstance(caught.value.__cause__, execution.ProducerExecutionError)
    assert (root / "stderr").read_bytes() == b"primary"
    assert not (root / "execution.json").exists()


def test_controller_death_retains_already_drained_prefix(tmp_path):
    """A pipe-drain acknowledgement must not leave bytes only in Python buffers."""
    read_fd, write_fd = os.pipe()
    root = tmp_path / "execution"
    script = f"""
import os,sys
from pathlib import Path
sys.path.insert(0,{str(Path(execution.__file__).parent)!r})
import evidence
from producer_execution import execute
original=evidence.RawWriter.write
def acknowledged(self, block):
    original(self,block)
    if self.path.name=='stdout' and block==b'retained': os.write({write_fd},b'!')
evidence.RawWriter.write=acknowledged
execute([sys.executable,'-c',"import os,time; os.write(1,b'retained'); time.sleep(60)"],cwd=Path({str(tmp_path)!r}),env=dict(os.environ),timeout=30,log_dir=Path({str(root)!r}))
"""
    process = subprocess.Popen(
        [sys.executable, "-c", script],
        pass_fds=(write_fd,),
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    os.close(write_fd)
    try:
        with selectors.DefaultSelector() as watch:
            watch.register(read_fd, selectors.EVENT_READ)
            assert watch.select(10), "controller did not acknowledge a drained prefix"
            assert os.read(read_fd, 1) == b"!"
        process.kill()
        process.communicate(timeout=10)
        assert (root / "stdout").read_bytes() == b"retained"
        assert not (root / "execution.json").exists(), "controller death has no complete terminal"
    finally:
        os.close(read_fd)
        if process.poll() is None:
            process.kill()
            process.communicate(timeout=10)


@pytest.mark.parametrize("unsafe", ["existing", "linked"])
def test_log_custody_refuses_unsafe_output_before_launch(tmp_path, unsafe):
    root, marker = tmp_path / "execution", tmp_path / "must-not-run"
    root.mkdir()
    if unsafe == "existing":
        (root / "stdout").write_bytes(b"retained")
    else:
        (root / "stdout").symlink_to(marker)
    with pytest.raises(ValueError, match="unsafe"):
        execution.execute(
            [sys.executable, "-c", f"from pathlib import Path; Path({str(marker)!r}).touch()"],
            cwd=tmp_path,
            env=dict(os.environ),
            timeout=10,
            log_dir=root,
        )
    assert not marker.exists()
    if unsafe == "existing":
        assert (root / "stdout").read_bytes() == b"retained"
