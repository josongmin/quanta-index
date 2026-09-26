"""Child terminal status uses notifications without relaxing process custody."""

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
