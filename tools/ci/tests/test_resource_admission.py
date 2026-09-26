"""Real cooperative admission preserves terminal status and process custody."""

from __future__ import annotations

import fcntl
import json
import os
import signal
import subprocess
import sys
import time
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[3]
LAUNCHER = ROOT / "tools/ci/resource_admission.py"


def command(lock, script, *, wait=5, timeout=10):
    return [sys.executable, str(LAUNCHER), "--lock", str(lock), "--wait-seconds", str(wait),
            "--timeout-seconds", str(timeout), "--", sys.executable, "-c", script]


def wait_file(path):
    deadline = time.monotonic() + 5
    while time.monotonic() < deadline:
        if path.exists():
            return
        time.sleep(0.01)
    pytest.fail(f"command never created {path}")


def finish(process):
    if process.poll() is None:
        process.terminate()
    try:
        return process.communicate(timeout=15)
    except subprocess.TimeoutExpired:
        process.kill()
        process.communicate(timeout=5)
        raise


def test_competing_commands_never_overlap_and_stream_output(tmp_path):
    lock, events, entered = tmp_path / "slot", tmp_path / "events", tmp_path / "entered"
    script = f"""
import os,time
from pathlib import Path
with Path({str(events)!r}).open('a') as stream:
 stream.write('start %s\\n' % os.getpid());stream.flush()
 Path({str(entered)!r}).touch()
 print('visible stdout',flush=True)
 time.sleep(.2)
 stream.write('end %s\\n' % os.getpid());stream.flush()
"""
    processes = []
    try:
        processes.append(subprocess.Popen(command(lock, script), stdout=subprocess.PIPE,
                                          stderr=subprocess.PIPE))
        wait_file(entered)
        processes.append(subprocess.Popen(command(lock, script), stdout=subprocess.PIPE,
                                          stderr=subprocess.PIPE))
        for process in processes:
            stdout, stderr = process.communicate(timeout=10)
            assert process.returncode == 0, stderr.decode()
            assert b"visible stdout" in stdout
            assert all(word in stderr for word in (b"waiting", b"admitted", b"released"))
        rows = [line.split() for line in events.read_text().splitlines()]
        assert [row[0] for row in rows] == ["start", "end", "start", "end"]
        assert rows[0][1] == rows[1][1] and rows[2][1] == rows[3][1]
        assert lock.is_file()
    finally:
        for process in processes:
            finish(process)


@pytest.mark.parametrize("cancel", [False, True])
def test_wait_timeout_or_cancellation_never_launches_command(tmp_path, cancel):
    lock, marker = tmp_path / "slot", tmp_path / "launched"
    with lock.open("w") as stream:
        fcntl.flock(stream, fcntl.LOCK_EX)
        process = subprocess.Popen(command(lock, f"open({str(marker)!r},'w').close()", wait=1),
                                   stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        try:
            # Read the explicit wait diagnostic before cancellation; no sleep race.
            assert b"waiting" in process.stderr.readline()
            if cancel:
                process.terminate()
            _, stderr = process.communicate(timeout=5)
            assert process.returncode == (143 if cancel else 124), stderr.decode()
            assert not marker.exists()
        finally:
            finish(process)


@pytest.mark.parametrize("exit_code", [0, 7])
def test_real_child_exit_code_is_preserved(tmp_path, exit_code):
    result = subprocess.run(command(tmp_path / "slot", f"raise SystemExit({exit_code})"),
                            capture_output=True, timeout=10)
    assert result.returncode == exit_code, result.stderr.decode()


def test_child_signal_exit_is_preserved(tmp_path):
    result = subprocess.run(command(tmp_path / "slot",
                                    "import os,signal;os.kill(os.getpid(),signal.SIGTERM)"),
                            capture_output=True, timeout=10)
    assert result.returncode == 128 + signal.SIGTERM, result.stderr.decode()


@pytest.mark.parametrize("kind", ["symlink", "directory", "hardlink"])
def test_invalid_lock_path_refuses_before_launch(tmp_path, kind):
    lock, marker = tmp_path / "slot", tmp_path / "launched"
    if kind == "directory":
        lock.mkdir()
    else:
        other = tmp_path / "other"
        other.touch()
        if kind == "symlink":
            lock.symlink_to(other)
        else:
            os.link(other, lock)
    result = subprocess.run(command(lock, f"open({str(marker)!r},'w').close()"),
                            capture_output=True, timeout=5)
    assert result.returncode == 126
    assert not marker.exists()


@pytest.mark.parametrize("controller_signal", [signal.SIGTERM, signal.SIGKILL])
def test_controller_loss_cleans_descendants_and_releases_guard_lease(tmp_path, controller_signal):
    lock, info = tmp_path / "slot", tmp_path / "info"
    grandchild_info, next_marker = tmp_path / "grandchild", tmp_path / "next"
    grandchild = (
        "import os,time;from pathlib import Path;"
        f"pending=Path({str(grandchild_info.with_suffix('.tmp'))!r});"
        "pending.write_text(str(os.getpid()));"
        f"pending.replace({str(grandchild_info)!r});time.sleep(60)"
    )
    script = f"""
import json,os,subprocess,sys,time
from pathlib import Path
lock=Path({str(lock)!r}).stat()
for fd in range(3,256):
 try: row=os.fstat(fd)
 except OSError: continue
 if (row.st_dev,row.st_ino)==(lock.st_dev,lock.st_ino):raise RuntimeError('lease leaked into command')
child=subprocess.Popen([sys.executable,'-c',{grandchild!r}])
pending=Path({str(info.with_suffix('.tmp'))!r})
pending.write_text(json.dumps([os.getpid(),os.getppid(),child.pid]))
pending.replace({str(info)!r})
time.sleep(60)
"""
    holder = subprocess.Popen(command(lock, script), stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    waiter = None
    pids = []
    try:
        wait_file(info)
        wait_file(grandchild_info)
        pids = json.loads(info.read_text())
        waiter = subprocess.Popen(command(lock, f"open({str(next_marker)!r},'w').close()"),
                                  stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        assert b"waiting" in waiter.stderr.readline()
        os.kill(holder.pid, controller_signal)
        _, holder_error = holder.communicate(timeout=15)
        assert holder.returncode == (143 if controller_signal == signal.SIGTERM else -signal.SIGKILL), holder_error.decode()
        _, error = waiter.communicate(timeout=15)
        assert waiter.returncode == 0, error.decode()
        assert next_marker.exists()
        # Orphan zombies cannot execute; platforms may reap them asynchronously.
        for pid in pids:
            result = subprocess.run(["ps", "-o", "stat=", "-p", str(pid)], capture_output=True,
                                    text=True, timeout=5)
            assert not result.stdout.strip() or result.stdout.strip().startswith("Z")
    finally:
        finish(holder)
        if waiter is not None:
            finish(waiter)
        if pids:
            try:
                os.killpg(pids[1], signal.SIGKILL)
            except ProcessLookupError:
                pass


def test_execution_timeout_cleans_then_releases_slot(tmp_path):
    lock = tmp_path / "slot"
    result = subprocess.run(command(lock, "import time;time.sleep(60)", timeout=1),
                            capture_output=True, timeout=15)
    assert result.returncode == 124, result.stderr.decode()
    again = subprocess.run(command(lock, "raise SystemExit(0)"), capture_output=True, timeout=5)
    assert again.returncode == 0, again.stderr.decode()


def test_stopped_guard_keeps_lease_after_controller_is_killed(tmp_path):
    """Make delayed parent-loss cleanup explicit instead of racing its speed."""
    lock, info, marker = tmp_path / "slot", tmp_path / "guard", tmp_path / "launched"
    script = (
        "import os,time;from pathlib import Path;"
        f"pending=Path({str(info.with_suffix('.tmp'))!r});"
        "pending.write_text(str(os.getppid()));"
        f"pending.replace({str(info)!r});time.sleep(60)"
    )
    holder = subprocess.Popen(command(lock, script), stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    guard = None
    try:
        wait_file(info)
        guard = int(info.read_text())
        os.kill(guard, signal.SIGSTOP)
        holder.kill()
        assert holder.wait(timeout=5) == -signal.SIGKILL
        blocked = subprocess.run(command(lock, f"open({str(marker)!r},'w').close()", wait=1),
                                 capture_output=True, timeout=5)
        assert blocked.returncode == 124, blocked.stderr.decode()
        assert not marker.exists()
        os.kill(guard, signal.SIGCONT)
        holder.communicate(timeout=10)
        resumed = subprocess.run(command(lock, "raise SystemExit(0)"), capture_output=True,
                                 timeout=10)
        assert resumed.returncode == 0, resumed.stderr.decode()
    finally:
        if guard is not None:
            try:
                os.killpg(guard, signal.SIGKILL)
            except ProcessLookupError:
                pass
        finish(holder)
