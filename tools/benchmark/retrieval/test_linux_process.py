"""Focused Linux process ownership tests; native cases require Linux."""

from __future__ import annotations

import os
import signal
import subprocess
import sys
import time
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


def test_exiting_stat_may_lose_process_group_but_live_stat_may_not():
    for state in ("Z", "X", "x"):
        exited = row(41, pgid=-1, state=state)
        assert not exited.live
        assert exited.pgid == -1
    with pytest.raises(linux_process.ProcessError, match="invalid /proc stat"):
        row(41, pgid=-1, state="S")


def test_kernel_task_zero_group_is_unrelated_to_owned_processes():
    kernel_task = row(2, ppid=0, pgid=0, rss=0)
    kernel_child = row(3, ppid=2, pgid=0, rss=0)
    root = row(41)
    selected = linux_process._select_owned(
        {
            kernel_task.identity.pid: kernel_task,
            kernel_child.identity.pid: kernel_child,
            root.identity.pid: root,
        },
        root.identity,
        set(),
    )
    assert set(selected) == {root.identity}
    zero_group_descendant = row(42, ppid=41, pgid=0)
    selected = linux_process._select_owned(
        {root.identity.pid: root, zero_group_descendant.identity.pid: zero_group_descendant},
        root.identity,
        set(),
    )
    assert set(selected) == {root.identity, zero_group_descendant.identity}


def test_exiting_root_with_missing_group_retains_known_descendant():
    root = row(41, pgid=-1, state="X")
    child = row(42, ppid=41, pgid=41)
    selected = linux_process._select_owned(
        {41: root, 42: child}, root.identity, {root.identity, child.identity}
    )
    assert set(selected) == {root.identity, child.identity}
    tracker = linux_process._Tracker(root.identity, clock_ticks=100, page_bytes=4096)
    tracker.known.update(selected)
    tracker.observe({41: root, 42: child})
    assert not tracker.escaped


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


def test_cgroup_counters_reject_missing_duplicate_and_negative():
    assert linux_process._keyed_counters("user_usec 1\nsystem_usec 2", {"user_usec"}) == {
        "user_usec": 1,
        "system_usec": 2,
    }
    for raw in ("user_usec 1\nuser_usec 2", "user_usec -1", "system_usec 2"):
        with pytest.raises(linux_process.ProcessError):
            linux_process._keyed_counters(raw, {"user_usec"})


def test_cgroup_owner_only_kills_its_unique_child_and_rejects_replacement(tmp_path):
    parent = tmp_path / "delegated"
    parent.mkdir()
    child = parent / "quanta-retrieval-test"
    child.mkdir()
    (parent / "cgroup.kill").write_text("parent untouched")
    (child / "cgroup.kill").write_text("")
    owner = linux_process._CgroupOwner(parent, child)
    owner.kill()
    assert (child / "cgroup.kill").read_text() == "1"
    assert (parent / "cgroup.kill").read_text() == "parent untouched"
    replacement = parent / "old-child"
    child.rename(replacement)
    child.mkdir()
    (child / "cgroup.kill").write_text("replacement untouched")
    with pytest.raises(linux_process.ProcessError, match="identity changed"):
        owner.kill()
    assert (child / "cgroup.kill").read_text() == "replacement untouched"


def test_cgroup_owner_reads_accounting_and_refuses_populated_removal(tmp_path):
    parent = tmp_path / "delegated"
    parent.mkdir()
    child = parent / "quanta-retrieval-test"
    child.mkdir()
    (child / "cgroup.events").write_text("populated 1\nfrozen 0\n")
    (child / "cgroup.procs").write_text("41\n")
    (child / "memory.peak").write_text("8192\n")
    (child / "cpu.stat").write_text("usage_usec 12\nuser_usec 8\nsystem_usec 4\n")
    owner = linux_process._CgroupOwner(parent, child)
    assert owner.populated()
    assert owner.members() == {41}
    assert owner.accounting() == (8192, 8000, 4000, 12000)
    with pytest.raises(linux_process.ProcessError, match="populated"):
        owner.remove()
    assert child.is_dir()


def test_qualified_missing_delegation_fails_before_launch(tmp_path, monkeypatch):
    parent = tmp_path / "ordinary-directory"
    parent.mkdir()
    monkeypatch.setattr(
        linux_process.subprocess,
        "Popen",
        lambda *args, **kwargs: pytest.fail("workload launched without cgroup delegation"),
    )
    real_read_text = Path.read_text
    monkeypatch.setattr(
        Path,
        "read_text",
        lambda self: (
            "1 1 0:1 / /sys/fs/cgroup rw - cgroup2 cgroup rw"
            if self == Path("/proc/self/mountinfo")
            else real_read_text(self)
        ),
    )
    with pytest.raises(linux_process.ProcessError, match="explicit delegated cgroup parent"):
        linux_process._CgroupOwner.create(None)
    with pytest.raises(linux_process.ProcessError, match="not on a cgroup v2 mount"):
        linux_process._CgroupOwner.create(str(parent))


def test_cgroup_setup_failure_removes_only_new_subgroup(tmp_path, monkeypatch):
    parent = tmp_path / "delegated"
    parent.mkdir()
    sibling = parent / "existing-sibling"
    sibling.mkdir()
    real_read_text = Path.read_text
    monkeypatch.setattr(
        Path,
        "read_text",
        lambda self: (
            f"1 1 0:1 / {parent} rw - cgroup2 cgroup rw"
            if self == Path("/proc/self/mountinfo")
            else real_read_text(self)
        ),
    )
    with pytest.raises(linux_process.ProcessError, match="cgroup setup failed"):
        linux_process._CgroupOwner.create(str(parent))
    assert list(parent.iterdir()) == [sibling]


@pytest.mark.skipif(sys.platform != "linux", reason="Linux qualification gate")
def test_qualified_run_rejects_unavailable_delegation_before_workload(tmp_path):
    parent = tmp_path / "ordinary-directory"
    parent.mkdir()
    marker = tmp_path / "workload-ran"
    with pytest.raises(linux_process.ProcessError, match="not on a cgroup v2 mount"):
        linux_process.run(
            [sys.executable, "-c", f"from pathlib import Path; Path({str(marker)!r}).touch()"],
            timeout_secs=1,
            qualified=True,
            cgroup_parent=str(parent),
        )
    assert not marker.exists()


def test_child_shim_cannot_run_workload_before_parent_ack(tmp_path):
    marker = tmp_path / "ran"
    reader, writer = os.pipe()
    try:
        child = subprocess.Popen(
            [
                sys.executable,
                "-I",
                str(Path(linux_process.__file__).resolve()),
                "--cgroup-child",
                str(reader),
                sys.executable,
                "-c",
                "from pathlib import Path; import sys; Path(sys.argv[1]).write_text('ran')",
                str(marker),
            ],
            pass_fds=(reader,),
        )
        os.close(reader)
        reader = None
        time.sleep(0.1)
        assert not marker.exists()
        os.write(writer, b"1")
        assert child.wait(timeout=5) == 0
        assert marker.read_text() == "ran"
    finally:
        if reader is not None:
            os.close(reader)
        os.close(writer)


def test_cgroup_shim_forwards_only_explicit_attestation_fd_after_gate(tmp_path):
    gate_read, gate_write = os.pipe()
    attest_read, attest_write = os.pipe()
    unrelated_read, unrelated_write = os.pipe()
    os.set_inheritable(unrelated_write, True)
    marker = tmp_path / "wrapper-ran"
    script = """
import os
import sys
from pathlib import Path

def opened(fd):
    try:
        os.fstat(fd)
        return True
    except OSError:
        return False

attestation_fd = int(sys.argv[1])
assert [fd for fd in range(3, 64) if opened(fd)] == [attestation_fd]
Path(sys.argv[2]).write_text("ran")
os.write(attestation_fd, b"nonce")
"""
    child = None
    try:
        child = linux_process._spawn_cgroup_shim(
            [sys.executable, "-c", script, str(attest_write), str(marker)],
            gate_read,
            (attest_write,),
            cwd=None,
            env=None,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )
        os.close(gate_read)
        gate_read = None
        time.sleep(0.1)
        assert child.poll() is None
        assert not marker.exists()
        os.write(gate_write, b"1")
        stdout, stderr = child.communicate(timeout=5)
        assert child.returncode == 0, stderr.decode()
        assert stdout == b""
        assert marker.read_text() == "ran"
        os.close(attest_write)
        attest_write = None
        assert os.read(attest_read, 5) == b"nonce"
    finally:
        if child is not None and child.poll() is None:
            child.kill()
            child.wait(timeout=5)
        if gate_read is not None:
            os.close(gate_read)
        for fd in (gate_write, attest_read, attest_write, unrelated_read, unrelated_write):
            if fd is not None:
                os.close(fd)


def test_pass_fds_validation_rejects_unlisted_or_non_pipe_fds(tmp_path, monkeypatch):
    monkeypatch.setattr(
        linux_process.subprocess,
        "Popen",
        lambda *args, **kwargs: pytest.fail("invalid attestation FD reached spawn"),
    )
    read_fd, write_fd = os.pipe()
    try:
        with (tmp_path / "regular").open("wb") as regular:
            invalid = (None, [], (1,), (read_fd,), (regular.fileno(),), (write_fd, write_fd))
            for candidate in invalid:
                with pytest.raises(linux_process.ProcessError, match="pass_fds|attestation FD"):
                    linux_process.run(["worker"], timeout_secs=1, pass_fds=candidate)
        os.close(write_fd)
        with pytest.raises(linux_process.ProcessError, match="not open"):
            linux_process.run(["worker"], timeout_secs=1, pass_fds=(write_fd,))
    finally:
        os.close(read_fd)


def test_qualified_run_passes_only_validated_attestation_fd_to_owner(monkeypatch):
    attest_read, attest_write = os.pipe()
    captured = {}

    def owner(command, **kwargs):
        captured.update(kwargs)
        return "owner-selected"

    monkeypatch.setattr(linux_process, "_run_cgroup", owner)
    try:
        assert (
            linux_process.run(
                ["wrapper", "--", "worker"],
                timeout_secs=1,
                qualified=True,
                cgroup_parent="/delegated",
                pass_fds=(attest_write,),
            )
            == "owner-selected"
        )
        assert captured["pass_fds"] == (attest_write,)
        assert captured["cgroup_parent"] == "/delegated"
    finally:
        os.close(attest_read)
        os.close(attest_write)


def test_cgroup_tracker_rejects_observed_migration(monkeypatch):
    current = ["/owned"]
    member_pids = [{41}, set()]
    raw = stat(41)

    class Owner:
        def members(self):
            return member_pids.pop(0)

        def accounting(self):
            return (4096, 1000, 0, 1000)

    monkeypatch.setattr(Path, "read_text", lambda self: raw)
    monkeypatch.setattr(linux_process, "_proc_cgroup_path", lambda pid: current[0])
    monkeypatch.setattr(linux_process.os, "pidfd_open", lambda pid, flags: 141, raising=False)
    monkeypatch.setattr(linux_process.os, "close", lambda fd: None)
    tracker = linux_process._CgroupTracker(
        linux_process.ProcessIdentity(41, 100), "/owned", 100, 4096
    )
    tracker.sample(Owner())
    current[0] = "/outside"
    with pytest.raises(linux_process.ProcessError, match="migrated outside"):
        tracker.sample(Owner())
    assert tracker.escaped == {linux_process.ProcessIdentity(41, 100)}
    tracker.close()


def test_cgroup_tracker_excludes_preexec_rss_and_cpu(monkeypatch):
    raw = [stat(41, user=10, kernel=4, rss=4)]

    class Owner:
        def members(self):
            return {41}

        def accounting(self):
            return (8192, 1000, 1000, 2000)

    monkeypatch.setattr(Path, "read_text", lambda self: raw[0])
    monkeypatch.setattr(linux_process, "_proc_cgroup_path", lambda pid: "/owned")
    monkeypatch.setattr(linux_process.os, "pidfd_open", lambda pid, flags: 141, raising=False)
    monkeypatch.setattr(linux_process.os, "close", lambda fd: None)
    tracker = linux_process._CgroupTracker(
        linux_process.ProcessIdentity(41, 100), "/owned", 100, 4096, (10, 4)
    )
    tracker.sample(Owner(), include_process_metrics=False)
    assert tracker.peak_tree_rss_bytes is None
    assert tracker.samples == 0
    raw[0] = stat(41, user=20, kernel=6, rss=8)
    tracker.sample(Owner())
    assert tracker.peak_tree_rss_bytes == 8 * 4096
    assert tracker.evidence()[0].user_cpu_ns == 100_000_000
    assert tracker.evidence()[0].kernel_cpu_ns == 20_000_000
    tracker.close()


def test_cgroup_remove_stays_under_unique_child(tmp_path, monkeypatch):
    parent = tmp_path / "delegated"
    parent.mkdir()
    sibling = parent / "sibling"
    sibling.mkdir()
    child = parent / "quanta-retrieval-test"
    nested = child / "nested"
    nested.mkdir(parents=True)
    owner = linux_process._CgroupOwner(parent, child)
    monkeypatch.setattr(owner, "populated", lambda: False)
    owner.remove()
    assert not child.exists()
    assert parent.is_dir()
    assert sibling.is_dir()


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
    assert not result.ownership_complete


@pytest.mark.skipif(sys.platform != "linux", reason="native Linux process test")
def test_diagnostic_run_forwards_only_attestation_fd(tmp_path):
    attest_read, attest_write = os.pipe()
    unrelated_read, unrelated_write = os.pipe()
    os.set_inheritable(unrelated_write, True)
    script = """
import os
import sys

def opened(fd):
    try:
        os.fstat(fd)
        return True
    except OSError:
        return False

attestation_fd = int(sys.argv[1])
assert [fd for fd in range(3, 64) if opened(fd)] == [attestation_fd]
os.write(attestation_fd, b"nonce")
"""
    try:
        result = linux_process.run(
            [sys.executable, "-c", script, str(attest_write)],
            timeout_secs=2,
            sample_interval_ms=10,
            pass_fds=(attest_write,),
            stderr_path=str(tmp_path / "stderr"),
        )
        assert result.root_exit_code == 0, (tmp_path / "stderr").read_text()
        os.close(attest_write)
        attest_write = None
        assert os.read(attest_read, 5) == b"nonce"
        assert not result.ownership_complete
    finally:
        for fd in (attest_read, attest_write, unrelated_read, unrelated_write):
            if fd is not None:
                os.close(fd)


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
