"""Focused Linux process ownership tests; native cases require Linux."""

from __future__ import annotations

import signal
import sys
from pathlib import Path

import pytest

from tools.benchmark.retrieval import linux_process


def stat(pid, *, ppid=1, pgid=None, start=100, user=0, kernel=0, rss=1, state="S"):
    # Fields after comm begin at field 3 (state). Indices 11, 12, 19, 21
    # correspond to utime, stime, starttime, and RSS pages.
    fields = ["0"] * 22
    fields[0] = state
    fields[1] = str(ppid)
    fields[2] = str(pid if pgid is None else pgid)
    fields[11] = str(user)
    fields[12] = str(kernel)
    fields[19] = str(start)
    fields[21] = str(rss)
    return f"{pid} (worker ) odd) " + " ".join(fields)


def row(pid, **kwargs):
    return linux_process.parse_proc_stat(stat(pid, **kwargs), expected_pid=pid)


def test_stat_parser_preserves_pid_start_and_accounting_with_tricky_command():
    item = row(41, ppid=4, pgid=41, start=789, user=50, kernel=25, rss=17)
    assert item.identity == linux_process.ProcessIdentity(41, 789)
    assert (item.ppid, item.pgid, item.user_ticks, item.kernel_ticks, item.rss_pages) == (
        4,
        41,
        50,
        25,
        17,
    )
    with pytest.raises(linux_process.ProcessError, match="PID mismatch"):
        linux_process.parse_proc_stat(stat(41), expected_pid=42)
    with pytest.raises(linux_process.ProcessError, match="truncated"):
        linux_process.parse_proc_stat("41 (worker) S 1")


def test_select_owned_finds_group_descendants_and_detects_escape():
    root = row(41)
    child = row(42, ppid=41, pgid=41)
    escaped = row(43, ppid=42, pgid=43)
    unrelated = row(99, ppid=1, pgid=99)
    snapshot = {r.identity.pid: r for r in (root, child, escaped, unrelated)}
    selected = linux_process._select_owned(snapshot, root.identity, set())
    assert set(selected) == {root.identity, child.identity, escaped.identity}
    assert selected[escaped.identity].pgid != root.identity.pid


def test_pid_reuse_does_not_inherit_prior_ownership():
    root = row(41)
    old_child = row(42, ppid=41, pgid=41, start=12)
    reused = row(42, ppid=1, pgid=42, start=13)
    selected = linux_process._select_owned(
        {41: root, 42: reused}, root.identity, {old_child.identity}
    )
    assert set(selected) == {root.identity}


def test_missing_root_identity_fails_closed():
    root = row(41)
    with pytest.raises(linux_process.ProcessError, match="start-time identity"):
        linux_process._select_owned({41: row(41, start=101)}, root.identity, set())


def test_tracker_keeps_peak_tree_rss_and_exited_cpu(monkeypatch):
    root = row(41, rss=3, user=1)
    child = row(42, ppid=41, pgid=41, rss=5, user=4, kernel=2)
    later_root = row(41, rss=1, user=3, state="Z")
    later_child = row(42, ppid=41, pgid=41, rss=7, user=9, kernel=3)
    raw = {41: stat(41), 42: stat(42, ppid=41, pgid=41)}
    monkeypatch.setattr(linux_process.os, "pidfd_open", lambda pid, flags: pid + 100, raising=False)
    monkeypatch.setattr(Path, "read_text", lambda self: raw[int(self.parent.name)])
    closed = []
    monkeypatch.setattr(linux_process.os, "close", closed.append)
    tracker = linux_process._Tracker(root.identity, clock_ticks=100, page_bytes=4096)
    tracker.observe({41: root, 42: child})
    tracker.observe({41: later_root, 42: later_child})
    tracker.observe({41: later_root})
    assert tracker.peak_tree_rss_bytes == 8 * 4096
    assert tracker.samples == 3
    evidence = tracker.evidence()
    assert evidence[0].user_cpu_ns == 30_000_000
    assert evidence[1].user_cpu_ns == 90_000_000
    assert evidence[1].kernel_cpu_ns == 30_000_000
    assert evidence[1].peak_rss_bytes == 7 * 4096
    tracker.close()
    assert closed == [141, 142]


def test_escape_signal_uses_pidfd_and_group_signal(monkeypatch):
    tracker = linux_process._Tracker(linux_process.ProcessIdentity(41, 100), 100, 4096)
    escape = linux_process.ProcessIdentity(43, 100)
    tracker.escaped.add(escape)
    tracker.pidfds[escape] = 143
    calls = []
    monkeypatch.setattr(
        linux_process.os, "killpg", lambda pgid, sig: calls.append(("group", pgid, sig))
    )
    monkeypatch.setattr(
        linux_process.signal,
        "pidfd_send_signal",
        lambda fd, sig: calls.append(("pidfd", fd, sig)),
        raising=False,
    )
    linux_process._signal_owned(tracker, signal.SIGKILL)
    assert calls == [("group", 41, signal.SIGKILL), ("pidfd", 143, signal.SIGKILL)]


@pytest.mark.skipif(sys.platform != "linux", reason="native Linux process test")
def test_timeout_cleans_live_descendant_on_linux(tmp_path):
    script = "import subprocess,time; subprocess.Popen(['sleep','30']); time.sleep(30)"
    result = linux_process.run(
        [sys.executable, "-c", script],
        timeout_secs=0.3,
        sample_interval_ms=10,
        cleanup_timeout_secs=2,
        stdout_path=str(tmp_path / "out"),
        stderr_path=str(tmp_path / "err"),
    )
    assert result.timed_out
    assert result.cleanup_complete
    assert result.samples > 0
    assert len(result.processes) >= 2
    assert result.root.start_ticks > 0


@pytest.mark.skipif(sys.platform != "linux", reason="native Linux process test")
def test_native_detects_process_group_escape_on_linux(tmp_path):
    script = (
        "import os,subprocess,time; "
        "subprocess.Popen(['sleep','30'], start_new_session=True); "
        "time.sleep(30)"
    )
    result = linux_process.run(
        [sys.executable, "-c", script],
        timeout_secs=0.3,
        sample_interval_ms=10,
        cleanup_timeout_secs=2,
    )
    assert result.timed_out
    assert result.escaped
    assert not result.ownership_complete
    assert result.cleanup_complete


@pytest.mark.skipif(sys.platform != "linux", reason="native Linux process test")
def test_root_exit_does_not_release_descendant_on_linux():
    script = "import subprocess; subprocess.Popen(['sleep','30'])"
    result = linux_process.run(
        [sys.executable, "-c", script], timeout_secs=0.3, sample_interval_ms=10
    )
    assert result.root_exit_code == 0
    assert result.timed_out
    assert result.cleanup_complete
    assert len(result.processes) >= 2


@pytest.mark.skipif(sys.platform != "linux", reason="native Linux process test")
def test_cleanup_escalates_past_ignored_sigterm_on_linux():
    script = "import signal,time; signal.signal(signal.SIGTERM, signal.SIG_IGN); time.sleep(30)"
    result = linux_process.run(
        [sys.executable, "-c", script],
        timeout_secs=0.3,
        sample_interval_ms=10,
        cleanup_timeout_secs=0.5,
    )
    assert result.timed_out
    assert result.root_exit_code == -signal.SIGKILL
    assert result.cleanup_complete
