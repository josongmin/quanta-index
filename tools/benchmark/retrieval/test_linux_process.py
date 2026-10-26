"""Focused Linux process ownership tests; native cases require Linux."""

from __future__ import annotations

import errno
import os
import signal
import subprocess
import sys
import time
from dataclasses import replace
from pathlib import Path
from types import SimpleNamespace

import pytest

from tools.benchmark.retrieval import linux_process

LINUX_PIDFD_AVAILABLE = (
    sys.platform == "linux" and hasattr(os, "pidfd_open") and hasattr(signal, "pidfd_send_signal")
)


def stat(
    pid, *, ppid=1, pgid=None, start=100, user=0, kernel=0, rss=1,
    state="S", flags=0, num_threads=1,
):
    # Fields after comm begin at field 3 (state). Indices 6, 17, 11, 12,
    # 19, 21 correspond to flags, thread count, CPU, starttime, and RSS.
    fields = ["0"] * 22
    fields[0] = state
    fields[1] = str(ppid)
    fields[2] = str(pid if pgid is None else pgid)
    fields[6] = str(flags)
    fields[11] = str(user)
    fields[12] = str(kernel)
    fields[17] = str(num_threads)
    fields[19] = str(start)
    fields[21] = str(rss)
    return f"{pid} (worker ) odd) " + " ".join(fields)


def row(pid, **kwargs):
    return linux_process.parse_proc_stat(stat(pid, **kwargs), expected_pid=pid)


def test_result_rss_method_binds_backend_and_refuses_unknown_method():
    diagnostic = linux_process.ProcessResult(
        root=linux_process.ProcessIdentity(41, 100),
        root_exit_code=0,
        timed_out=False,
        elapsed_ms=1.0,
        sample_interval_ms=10,
        samples=1,
        peak_tree_rss_bytes=1024,
        total_user_cpu_ns=0,
        total_kernel_cpu_ns=0,
        processes=(),
        escaped=(),
        sampling_complete=True,
        cleanup_complete=True,
        stdout_path=None,
        stderr_path=None,
    )
    assert diagnostic.rss_method == "sampled_proc_stat_rss_sum_shared_pages_may_repeat"
    assert (
        replace(diagnostic, backend="cgroup-v2").rss_method
        == "sampled_smaps_rollup_rss_sum_shared_pages_may_repeat"
    )
    with pytest.raises(linux_process.ProcessError, match="unsupported process backend"):
        _method = replace(diagnostic, backend="unknown").rss_method


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
    (child / "cgroup.threads").write_text("41\n42\n")
    (child / "memory.peak").write_text("8192\n")
    (child / "cpu.stat").write_text("usage_usec 12\nuser_usec 8\nsystem_usec 4\n")
    owner = linux_process._CgroupOwner(parent, child)
    assert owner.populated()
    assert owner.members() == {41}
    assert owner.threads() == {41, 42}
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


def test_qualified_run_rejects_unavailable_delegation_before_workload(tmp_path, monkeypatch):
    parent = tmp_path / "ordinary-directory"
    parent.mkdir()
    marker = tmp_path / "workload-ran"
    monkeypatch.setattr(linux_process.sys, "platform", "linux")
    monkeypatch.setattr(linux_process.os, "pidfd_open", lambda *_args: -1, raising=False)
    monkeypatch.setattr(
        linux_process.signal,
        "pidfd_send_signal",
        lambda *_args: None,
        raising=False,
    )
    real_read_text = Path.read_text
    monkeypatch.setattr(
        Path,
        "read_text",
        lambda self, *args, **kwargs: (
            "1 1 0:1 / /sys/fs/cgroup rw - cgroup2 cgroup rw"
            if self == Path("/proc/self/mountinfo")
            else real_read_text(self, *args, **kwargs)
        ),
    )
    with pytest.raises(linux_process.ProcessError, match="not on a cgroup v2 mount"):
        linux_process.run(
            [sys.executable, "-c", f"from pathlib import Path; Path({str(marker)!r}).touch()"],
            timeout_secs=1,
            qualified=True,
            cgroup_parent=str(parent),
        )
    assert not marker.exists()


def test_run_rejects_missing_pidfd_apis_before_launch(tmp_path, monkeypatch):
    marker = tmp_path / "workload-ran"
    monkeypatch.setattr(linux_process.sys, "platform", "linux")
    monkeypatch.delattr(linux_process.os, "pidfd_open", raising=False)
    monkeypatch.delattr(linux_process.signal, "pidfd_send_signal", raising=False)
    monkeypatch.setattr(
        linux_process.subprocess,
        "Popen",
        lambda *args, **kwargs: pytest.fail("workload launched without pidfd support"),
    )
    with pytest.raises(
        linux_process.ProcessError,
        match="Linux pidfd_open and pidfd_send_signal required",
    ):
        linux_process.run(
            [sys.executable, "-c", f"from pathlib import Path; Path({str(marker)!r}).touch()"],
            timeout_secs=1,
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


def test_absolute_deadline_is_forwarded_only_to_qualified_owner(monkeypatch):
    captured = {}

    def owner(_command, **kwargs):
        captured.update(kwargs)
        return "owner-selected"

    monkeypatch.setattr(linux_process, "_run_cgroup", owner)
    deadline = linux_process.time.monotonic() + 1
    assert (
        linux_process.run(
            ["worker"],
            timeout_secs=2,
            qualified=True,
            cgroup_parent="/delegated",
            absolute_deadline=deadline,
        )
        == "owner-selected"
    )
    assert captured["absolute_deadline"] == deadline
    with pytest.raises(linux_process.ProcessError, match="finite qualified"):
        linux_process.run(["worker"], timeout_secs=2, absolute_deadline=deadline)


def test_expired_deadline_never_releases_cgroup_preexec_gate():
    reader, writer = os.pipe()
    try:
        with pytest.raises(linux_process.ProcessError, match="before cgroup workload launch"):
            linux_process._release_cgroup_workload(writer, linux_process.time.monotonic() - 1)
        os.set_blocking(reader, False)
        with pytest.raises(BlockingIOError):
            os.read(reader, 1)
    finally:
        os.close(reader)
        os.close(writer)


@pytest.mark.parametrize(
    ("qualified", "timeout_secs", "absolute_deadline"),
    [(False, 1, None), (True, 1, None), (True, 2, 1.0)],
)
@pytest.mark.parametrize(
    ("sample_finished", "observed_finished", "expected_timeout"),
    [
        (0.2, 0.999, False),
        (0.2, 1.0, True),
        (0.2, 1.001, True),
        (1.001, 1.001, True),
    ],
)
def test_final_ownership_observation_obeys_deadline(
    monkeypatch,
    tmp_path,
    qualified,
    timeout_secs,
    absolute_deadline,
    sample_finished,
    observed_finished,
    expected_timeout,
):
    """A slow final sample or empty/dead observation cannot produce a late success."""
    clock = SimpleNamespace(now=0.0)
    monkeypatch.setattr(
        linux_process,
        "time",
        SimpleNamespace(
            monotonic=lambda: clock.now,
            sleep=lambda _seconds: pytest.fail("completed run must not sleep"),
        ),
    )
    monkeypatch.setattr(
        linux_process, "sys", SimpleNamespace(platform="linux", float_info=sys.float_info)
    )
    monkeypatch.setattr(linux_process.os, "pidfd_open", lambda *_args: -1, raising=False)
    monkeypatch.setattr(linux_process.os, "killpg", lambda *_args: None)
    monkeypatch.setattr(
        linux_process.signal, "pidfd_send_signal", lambda *_args: None, raising=False
    )
    monkeypatch.setattr(Path, "read_text", lambda _path: stat(41))
    monkeypatch.setattr(linux_process, "_proc_cgroup_path", lambda _pid: "/delegated/owned")
    process = SimpleNamespace(pid=41, wait=lambda timeout: 0)
    calls = {"kill": 0, "escaped": 0, "remove": 0, "cleanup": 0}

    class Tracker:
        samples = 0
        user_cpu_ns = 0
        kernel_cpu_ns = 0
        cpu_usage_ns = 0
        peak_tree_rss_bytes = 4096
        peak_cgroup_memory_bytes = 4096
        escaped = set()

        def sample(self, _owner, *, include_process_metrics=True):
            if include_process_metrics:
                self.samples += 1
                clock.now = max(clock.now, sample_finished)

        def observe(self, _snapshot):
            self.samples += 1
            clock.now = observed_finished
            return {41: row(41, state="Z")}

        def evidence(self):
            return ()

        def kill_escapes(self):
            calls["escaped"] += 1

        def close(self):
            pass

    tracker = Tracker()
    monkeypatch.setattr(linux_process, "_spawn_cgroup_shim", lambda *_args, **_kwargs: process)
    monkeypatch.setattr(
        linux_process,
        "_release_cgroup_workload",
        lambda _fd, deadline: clock.now if deadline == absolute_deadline else pytest.fail(
            "wrong absolute deadline forwarded to release gate"
        ),
    )
    monkeypatch.setattr(linux_process, "_CgroupTracker", lambda *_args: tracker)
    monkeypatch.setattr(linux_process, "_Tracker", lambda *_args: tracker)

    def populated():
        clock.now = observed_finished
        return False

    def kill():
        calls["kill"] += 1

    def remove():
        calls["remove"] += 1

    owner = SimpleNamespace(
        path=tmp_path / "owned",
        assign=lambda _pid: None,
        populated=populated,
        wait_empty=lambda _timeout: True,
        kill=kill,
        remove=remove,
    )
    monkeypatch.setattr(linux_process._CgroupOwner, "create", lambda _parent: owner)
    monkeypatch.setattr(linux_process.subprocess, "Popen", lambda *_args, **_kwargs: process)

    def snapshot(_proc_root):
        clock.now = sample_finished
        return {}

    def cleanup(_tracker, _proc_root, _timeout):
        calls["cleanup"] += 1
        return True

    monkeypatch.setattr(linux_process, "_read_snapshot", snapshot)
    monkeypatch.setattr(linux_process, "_cleanup", cleanup)
    result = linux_process.run(
        ["worker"],
        timeout_secs=timeout_secs,
        sample_interval_ms=10,
        qualified=qualified,
        cgroup_parent="/delegated" if qualified else None,
        absolute_deadline=absolute_deadline,
    )
    assert result.timed_out is expected_timeout
    assert result.elapsed_ms == pytest.approx(observed_finished * 1000)
    assert result.root_exit_code == 0
    assert result.cleanup_complete
    assert calls["kill"] == 0  # The final ownership observation found an empty cgroup.
    assert calls["escaped"] == (int(expected_timeout) if qualified else 0)
    assert calls["remove"] == (1 if qualified else 0)
    assert calls["cleanup"] == (0 if qualified else 1)


def test_smaps_rollup_rss_parser_uses_kib_with_bounded_exact_field(tmp_path):
    proc_dir = tmp_path / "41"
    proc_dir.mkdir()
    source = proc_dir / "smaps_rollup"
    source.write_bytes(
        b"00400000-00500000 ---p 00000000 00:00 0 [rollup]\nRss:  7 kB\nPss: 2 kB\n"
    )
    assert linux_process._read_smaps_rollup_rss_bytes(41, tmp_path) == 7 * 1024

    invalid = (
        (b"Pss: 7 kB\n", "missing or duplicate"),
        (b"Rss: 7 kB\nRss: 8 kB\n", "missing or duplicate"),
        (b"Rss: 7 MB\n", "invalid"),
        (b"Rss: -7 kB\n", "invalid"),
        (b"Rss: 18446744073709551616 kB\n", "overflow"),
        (b"Rss: " + b"9" * 5000 + b" kB\n", "overflow"),
        (b"Rss: 1 kB\n" + b"x" * (32 * 1024), "oversized"),
    )
    for raw, reason in invalid:
        source.write_bytes(raw)
        with pytest.raises(linux_process.ProcessError, match=reason):
            linux_process._read_smaps_rollup_rss_bytes(41, tmp_path)


def _mock_qualified_members(
    monkeypatch, raw_by_pid, *, membership=None, member_pids=None, thread_pids=None
):
    """Model stable cgroup counters while varying proc identities and RSS."""

    def read_stat(path):
        try:
            return raw_by_pid[int(path.parent.name)]
        except KeyError as exc:
            raise FileNotFoundError(path) from exc

    monkeypatch.setattr(Path, "read_text", read_stat)
    monkeypatch.setattr(linux_process, "_proc_cgroup_path", membership or (lambda pid: "/owned"))
    monkeypatch.setattr(linux_process.os, "pidfd_open", lambda pid, flags: pid + 100, raising=False)
    monkeypatch.setattr(linux_process.os, "close", lambda fd: None)

    class Owner:
        def members(self):
            return set(raw_by_pid if member_pids is None else member_pids)

        def threads(self):
            return set(raw_by_pid if thread_pids is None else thread_pids)

        def accounting(self):
            return (32 * 1024, 1000, 1000, 2000)

    return Owner()


def test_cgroup_rss_sums_smaps_values_without_stat_page_estimate(monkeypatch):
    rows = {41: stat(41, rss=1), 42: stat(42, ppid=41, pgid=41, rss=9999)}
    owner = _mock_qualified_members(monkeypatch, rows)
    monkeypatch.setattr(
        linux_process, "_read_smaps_rollup_rss_bytes", lambda pid: {41: 3, 42: 5}[pid] * 1024
    )
    tracker = linux_process._CgroupTracker(row(41).identity, "/owned", 100, 4096)
    tracker.sample(owner)
    assert tracker.peak_tree_rss_bytes == 8 * 1024
    assert {item.identity.pid: item.peak_rss_bytes for item in tracker.evidence()} == {
        41: 3 * 1024,
        42: 5 * 1024,
    }
    tracker.close()


def test_cgroup_rss_rejects_pid_replacement_after_smaps_read(monkeypatch):
    rows = {41: stat(41, rss=1)}
    owner = _mock_qualified_members(monkeypatch, rows)

    def replace_after_read(pid):
        rows[pid] = stat(pid, start=101, rss=1)
        return 1024

    monkeypatch.setattr(linux_process, "_read_smaps_rollup_rss_bytes", replace_after_read)
    tracker = linux_process._CgroupTracker(row(41).identity, "/owned", 100, 4096)
    with pytest.raises(linux_process.ProcessError, match="changed start time during RSS sample"):
        tracker.sample(owner)
    tracker.close()


def test_cgroup_rss_rejects_migration_after_smaps_read(monkeypatch):
    rows = {41: stat(41, rss=1)}
    membership = ["/owned"]
    owner = _mock_qualified_members(monkeypatch, rows, membership=lambda pid: membership[0])

    def migrate_after_read(pid):
        membership[0] = "/outside"
        return 1024

    monkeypatch.setattr(linux_process, "_read_smaps_rollup_rss_bytes", migrate_after_read)
    tracker = linux_process._CgroupTracker(row(41).identity, "/owned", 100, 4096)
    with pytest.raises(linux_process.ProcessError, match="migrated outside"):
        tracker.sample(owner)
    assert tracker.escaped == {row(41).identity}
    tracker.close()


def test_cgroup_rss_excludes_genuine_exit_during_smaps_read(monkeypatch):
    rows = {41: stat(41, rss=1)}
    owner = _mock_qualified_members(monkeypatch, rows)

    def exit_after_read(pid):
        rows.pop(pid)
        return 1024

    monkeypatch.setattr(linux_process, "_read_smaps_rollup_rss_bytes", exit_after_read)
    tracker = linux_process._CgroupTracker(row(41).identity, "/owned", 100, 4096)
    tracker.sample(owner)
    assert tracker.peak_tree_rss_bytes is None
    assert tracker.samples == 1
    tracker.close()


def test_cgroup_rss_omits_only_esrch_for_same_single_exiting_member(monkeypatch):
    # Linux sets PF_EXITING before exit_mm removes smaps_rollup. The task can
    # still be reported as state-live in /proc/PID/stat during that interval.
    rows = {41: stat(41, flags=linux_process._PF_EXITING, user=7, kernel=3, rss=4096)}
    owner = _mock_qualified_members(monkeypatch, rows)

    def no_mm(pid):
        raise ProcessLookupError(errno.ESRCH, "task mm vanished")

    monkeypatch.setattr(linux_process, "_read_smaps_rollup_rss_bytes", no_mm)
    tracker = linux_process._CgroupTracker(row(41).identity, "/owned", 100, 4096)
    assert row(41).live
    tracker.sample(owner)
    assert tracker.peak_tree_rss_bytes is None
    assert tracker.samples == 1
    assert tracker.evidence()[0].peak_rss_bytes == 0
    assert tracker.evidence()[0].user_cpu_ns > 0
    tracker.close()


@pytest.mark.parametrize(
    ("flags", "num_threads", "failure"),
    [
        (0, 1, ProcessLookupError(errno.ESRCH, "live task")),
        (linux_process._PF_EXITING, 2, ProcessLookupError(errno.ESRCH, "surviving thread")),
        (linux_process._PF_EXITING, 1, PermissionError(errno.EACCES, "denied")),
        (linux_process._PF_EXITING, 1, linux_process.ProcessError("malformed smaps")),
    ],
)
def test_cgroup_rss_refuses_nonterminal_or_non_esrch_loss(
    monkeypatch, flags, num_threads, failure
):
    rows = {41: stat(41, flags=flags, num_threads=num_threads)}
    owner = _mock_qualified_members(monkeypatch, rows)

    def fail_smaps(pid):
        raise failure

    monkeypatch.setattr(linux_process, "_read_smaps_rollup_rss_bytes", fail_smaps)
    tracker = linux_process._CgroupTracker(row(41).identity, "/owned", 100, 4096)
    with pytest.raises(linux_process.ProcessError, match="cannot sample smaps_rollup|no readable live"):
        tracker.sample(owner)
    tracker.close()


@pytest.mark.parametrize("change", ["pid-reuse", "migration"])
def test_cgroup_rss_esrch_refuses_replaced_or_migrated_member(monkeypatch, change):
    rows = {41: stat(41, flags=linux_process._PF_EXITING)}
    membership = ["/owned"]
    owner = _mock_qualified_members(monkeypatch, rows, membership=lambda pid: membership[0])

    def change_after_esrch(pid):
        if change == "pid-reuse":
            rows[pid] = stat(pid, start=101, flags=linux_process._PF_EXITING)
        else:
            membership[0] = "/outside"
        raise ProcessLookupError(errno.ESRCH, "task mm vanished")

    monkeypatch.setattr(linux_process, "_read_smaps_rollup_rss_bytes", change_after_esrch)
    tracker = linux_process._CgroupTracker(row(41).identity, "/owned", 100, 4096)
    with pytest.raises(linux_process.ProcessError, match="changed start time|migrated outside"):
        tracker.sample(owner)
    tracker.close()


def test_cgroup_rss_esrch_allows_genuine_member_disappearance(monkeypatch):
    rows = {41: stat(41, flags=linux_process._PF_EXITING)}
    owner = _mock_qualified_members(monkeypatch, rows)

    def exit_after_open(pid):
        rows.pop(pid)
        raise ProcessLookupError(errno.ESRCH, "task exited")

    monkeypatch.setattr(linux_process, "_read_smaps_rollup_rss_bytes", exit_after_open)
    tracker = linux_process._CgroupTracker(row(41).identity, "/owned", 100, 4096)
    tracker.sample(owner)
    assert tracker.peak_tree_rss_bytes is None
    assert tracker.samples == 1
    tracker.close()


def test_thread_tgid_parser_requires_one_bounded_decimal_value(tmp_path):
    source = tmp_path / "42"
    source.mkdir()
    status = source / "status"
    status.write_bytes(b"Name:\tworker\nTgid:\t41\nPid:\t42\n")
    assert linux_process._read_thread_tgid(42, tmp_path) == 41
    for raw in (
        b"Pid:\t42\n",
        b"Tgid:\t41\nTgid:\t41\n",
        b"Tgid:\twrong\n",
        b"Tgid:\t0\n",
        b"Tgid:\t" + b"9" * 4301 + b"\n",
        b"Tgid:\t41 kB\n",
        b"x" * (linux_process._PROC_STATUS_MAX_BYTES + 1),
    ):
        status.write_bytes(raw)
        with pytest.raises(linux_process.ProcessError):
            linux_process._read_thread_tgid(42, tmp_path)


def test_cgroup_rss_samples_one_worker_after_leader_becomes_zombie(monkeypatch):
    rows = {
        41: stat(41, state="Z", num_threads=1, rss=0),
        42: stat(42, pgid=41, start=200, rss=1),
        43: stat(43, pgid=41, start=300, rss=1),
    }
    owner = _mock_qualified_members(
        monkeypatch, rows, member_pids={41}, thread_pids={41, 42, 43}
    )
    monkeypatch.setattr(linux_process, "_read_thread_tgid", lambda tid: 41)
    reads = []
    monkeypatch.setattr(
        linux_process, "_read_smaps_rollup_rss_bytes",
        lambda pid: reads.append(pid) or 64 * 1024 * 1024,
    )
    tracker = linux_process._CgroupTracker(row(41).identity, "/owned", 100, 4096)
    tracker.sample(owner)
    assert reads == [42]
    assert tracker.peak_tree_rss_bytes == 64 * 1024 * 1024
    assert [(item.identity.pid, item.peak_rss_bytes) for item in tracker.evidence()] == [
        (41, 64 * 1024 * 1024)
    ]
    tracker.close()


@pytest.mark.parametrize("change", ["same-group", "pid-reuse", "migration"])
def test_cgroup_rss_retains_smaps_read_before_leader_becomes_zombie(monkeypatch, change):
    rows = {41: stat(41, state="S", num_threads=2), 42: stat(42, pgid=41, start=200)}
    membership = ["/owned"]
    owner = _mock_qualified_members(
        monkeypatch, rows, membership=lambda pid: membership[0],
        member_pids={41}, thread_pids={41, 42},
    )
    reads = []

    def read_before_zombie(pid):
        reads.append(pid)
        rows[41] = stat(41, state="Z", start=101 if change == "pid-reuse" else 100)
        if change == "migration":
            membership[0] = "/outside"
        return 80 * 1024 * 1024

    monkeypatch.setattr(linux_process, "_read_smaps_rollup_rss_bytes", read_before_zombie)
    tracker = linux_process._CgroupTracker(row(41).identity, "/owned", 100, 4096)
    if change == "same-group":
        tracker.sample(owner)
        assert reads == [41]
        assert tracker.peak_tree_rss_bytes == 80 * 1024 * 1024
        assert [(item.identity.pid, item.peak_rss_bytes) for item in tracker.evidence()] == [
            (41, 80 * 1024 * 1024)
        ]
    else:
        with pytest.raises(linux_process.ProcessError, match="changed start time|migrated outside"):
            tracker.sample(owner)
    tracker.close()


def test_cgroup_rss_esrch_uses_surviving_worker_once(monkeypatch):
    rows = {
        41: stat(41, flags=linux_process._PF_EXITING, num_threads=1),
        42: stat(42, pgid=41, start=200),
    }
    owner = _mock_qualified_members(monkeypatch, rows, member_pids={41}, thread_pids={41, 42})
    monkeypatch.setattr(linux_process, "_read_thread_tgid", lambda tid: 41)
    reads = []

    def read_smaps(pid):
        reads.append(pid)
        if pid == 41:
            raise ProcessLookupError(errno.ESRCH, "leader mm vanished")
        return 8 * 1024 * 1024

    monkeypatch.setattr(linux_process, "_read_smaps_rollup_rss_bytes", read_smaps)
    tracker = linux_process._CgroupTracker(row(41).identity, "/owned", 100, 4096)
    tracker.sample(owner)
    assert reads == [41, 42]
    assert tracker.peak_tree_rss_bytes == 8 * 1024 * 1024
    assert [item.identity.pid for item in tracker.evidence()] == [41]
    tracker.close()


@pytest.mark.parametrize("change", ["tid-reuse", "migration", "wrong-tgid", "tgid-change"])
def test_cgroup_rss_refuses_invalid_thread_representative(monkeypatch, change):
    rows = {
        41: stat(41, state="Z", num_threads=1),
        42: stat(42, pgid=41, start=200),
    }
    membership = ["/owned"]
    owner = _mock_qualified_members(
        monkeypatch, rows, membership=lambda pid: membership[0],
        member_pids={41}, thread_pids={41, 42},
    )
    tgid_reads = [0]

    def read_tgid(tid):
        tgid_reads[0] += 1
        if change == "wrong-tgid" or (change == "tgid-change" and tgid_reads[0] > 1):
            return 99
        return 41

    monkeypatch.setattr(linux_process, "_read_thread_tgid", read_tgid)

    def read_smaps(pid):
        if change == "tid-reuse":
            rows[pid] = stat(pid, pgid=41, start=201)
        elif change == "migration":
            membership[0] = "/outside"
        return 1024

    monkeypatch.setattr(linux_process, "_read_smaps_rollup_rss_bytes", read_smaps)
    tracker = linux_process._CgroupTracker(row(41).identity, "/owned", 100, 4096)
    with pytest.raises(
        linux_process.ProcessError,
        match="changed start time|migrated outside|no readable live RSS representative|changed Tgid",
    ):
        tracker.sample(owner)
    tracker.close()


@pytest.mark.parametrize(
    "failure", [PermissionError(errno.EACCES, "denied"), linux_process.ProcessError("malformed")]
)
def test_cgroup_rss_refuses_unreadable_live_worker(monkeypatch, failure):
    rows = {41: stat(41, state="Z"), 42: stat(42, pgid=41, start=200)}
    owner = _mock_qualified_members(monkeypatch, rows, member_pids={41}, thread_pids={42})
    monkeypatch.setattr(linux_process, "_read_thread_tgid", lambda tid: 41)

    def fail_smaps(pid):
        raise failure

    monkeypatch.setattr(linux_process, "_read_smaps_rollup_rss_bytes", fail_smaps)
    tracker = linux_process._CgroupTracker(row(41).identity, "/owned", 100, 4096)
    with pytest.raises(linux_process.ProcessError, match="cannot sample smaps_rollup for live TID 42"):
        tracker.sample(owner)
    tracker.close()


def test_cgroup_rss_excludes_group_when_all_threads_exit_during_census(monkeypatch):
    rows = {41: stat(41, state="Z", num_threads=0)}
    _mock_qualified_members(monkeypatch, rows)

    class Owner:
        calls = 0

        def members(self):
            self.calls += 1
            return {41} if self.calls == 1 else set()

        def threads(self):
            return set()

        def accounting(self):
            return (32 * 1024, 1000, 1000, 2000)

    tracker = linux_process._CgroupTracker(row(41).identity, "/owned", 100, 4096)
    tracker.sample(Owner())
    assert tracker.peak_tree_rss_bytes is None
    assert tracker.samples == 1
    tracker.close()


@pytest.mark.parametrize(
    "failure", [FileNotFoundError("smaps absent"), linux_process.ProcessError("malformed")]
)
def test_cgroup_rss_refuses_unreadable_live_smaps(monkeypatch, failure):
    rows = {41: stat(41, rss=4096)}
    owner = _mock_qualified_members(monkeypatch, rows)

    def fail_smaps(pid):
        raise failure

    monkeypatch.setattr(linux_process, "_read_smaps_rollup_rss_bytes", fail_smaps)
    tracker = linux_process._CgroupTracker(row(41).identity, "/owned", 100, 4096)
    with pytest.raises(linux_process.ProcessError, match="cannot sample smaps_rollup for live PID 41"):
        tracker.sample(owner)
    tracker.close()


@pytest.mark.skipif(sys.platform != "linux", reason="native Linux smaps_rollup test")
def test_native_smaps_rollup_reads_current_pid():
    assert linux_process._read_smaps_rollup_rss_bytes(os.getpid()) > 0


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
    monkeypatch.setattr(linux_process, "_read_smaps_rollup_rss_bytes", lambda pid: 1024)
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
    sampled = []

    class Owner:
        def members(self):
            return {41}

        def accounting(self):
            return (8192, 1000, 1000, 2000)

    monkeypatch.setattr(Path, "read_text", lambda self: raw[0])
    monkeypatch.setattr(linux_process, "_proc_cgroup_path", lambda pid: "/owned")
    monkeypatch.setattr(
        linux_process,
        "_read_smaps_rollup_rss_bytes",
        lambda pid: sampled.append(pid) or 8 * 4096,
    )
    monkeypatch.setattr(linux_process.os, "pidfd_open", lambda pid, flags: 141, raising=False)
    monkeypatch.setattr(linux_process.os, "close", lambda fd: None)
    tracker = linux_process._CgroupTracker(
        linux_process.ProcessIdentity(41, 100), "/owned", 100, 4096, (10, 4)
    )
    tracker.sample(Owner(), include_process_metrics=False)
    assert tracker.peak_tree_rss_bytes is None
    assert tracker.samples == 0
    assert sampled == []
    raw[0] = stat(41, user=20, kernel=6, rss=8)
    tracker.sample(Owner())
    assert tracker.peak_tree_rss_bytes == 8 * 4096
    assert sampled == [41]
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


@pytest.mark.skipif(not LINUX_PIDFD_AVAILABLE, reason="native Linux pidfd test")
def test_qualified_cgroup_samples_real_root_and_child_until_empty(tmp_path):
    delegated = os.environ.get("QUANTA_INDEX_TEST_CGROUP_PARENT")
    if not delegated:
        pytest.skip("requires QUANTA_INDEX_TEST_CGROUP_PARENT delegated cgroup v2 path")
    parent = Path(delegated)
    parent_identity = (parent.stat().st_dev, parent.stat().st_ino)
    child_pid_path = tmp_path / "child-pid"
    stderr_path = tmp_path / "stderr"
    child_script = """
import os
import sys
import time

resident = bytearray(4 * 1024 * 1024)
for offset in range(0, len(resident), 4096):
    resident[offset] = 1
with open(sys.argv[1], "x", encoding="ascii") as marker:
    marker.write(str(os.getpid()))
time.sleep(1)
"""
    root_script = """
import subprocess
import sys

resident = bytearray(4 * 1024 * 1024)
for offset in range(0, len(resident), 4096):
    resident[offset] = 1
child = subprocess.Popen([sys.executable, "-c", sys.argv[2], sys.argv[1]])
raise SystemExit(child.wait())
"""
    result = linux_process.run(
        [sys.executable, "-c", root_script, str(child_pid_path), child_script],
        timeout_secs=5,
        sample_interval_ms=10,
        cleanup_timeout_secs=5,
        stderr_path=str(stderr_path),
        qualified=True,
        cgroup_parent=str(parent),
    )
    assert result.root_exit_code == 0, stderr_path.read_text()
    assert not result.timed_out
    assert result.backend == "cgroup-v2"
    assert result.rss_method == "sampled_smaps_rollup_rss_sum_shared_pages_may_repeat"
    assert result.ownership_complete
    assert result.cleanup_complete and result.sampling_complete
    assert not result.escaped
    assert result.peak_tree_rss_bytes is not None
    assert result.peak_tree_rss_bytes >= 8 * 1024 * 1024
    child_pid = int(child_pid_path.read_text())
    assert child_pid != result.root.pid
    evidence = {item.identity.pid: item for item in result.processes}
    assert {result.root.pid, child_pid} <= evidence.keys()
    for pid in (result.root.pid, child_pid):
        assert evidence[pid].identity.start_ticks > 0
        assert evidence[pid].peak_rss_bytes >= 4 * 1024 * 1024
    assert result.cgroup_path is not None
    assert Path(result.cgroup_path).parent == parent
    assert not Path(result.cgroup_path).exists()
    assert (parent.stat().st_dev, parent.stat().st_ino) == parent_identity


@pytest.mark.skipif(not LINUX_PIDFD_AVAILABLE, reason="native Linux pidfd test")
def test_qualified_cgroup_samples_worker_after_leader_pthread_exit(tmp_path):
    delegated = os.environ.get("QUANTA_INDEX_TEST_CGROUP_PARENT")
    if not delegated:
        pytest.skip("requires QUANTA_INDEX_TEST_CGROUP_PARENT delegated cgroup v2 path")
    parent = Path(delegated)
    parent_identity = (parent.stat().st_dev, parent.stat().st_ino)
    marker_path = tmp_path / "worker-after-leader-z"
    stderr_path = tmp_path / "stderr"
    script = """
import ctypes
import os
import pathlib
import sys
import threading
import time

marker = pathlib.Path(sys.argv[1])
started = threading.Event()

def worker():
    started.set()
    deadline = time.monotonic() + 3
    while time.monotonic() < deadline:
        raw = pathlib.Path(f"/proc/{os.getpid()}/stat").read_text()
        if raw.rsplit(") ", 1)[1].startswith("Z "):
            break
        time.sleep(0.005)
    else:
        raise RuntimeError("leader never entered Z")
    resident = bytearray(64 * 1024 * 1024)
    for offset in range(0, len(resident), 4096):
        resident[offset] = 1
    marker.write_text("leader-z-and-resident", encoding="ascii")
    time.sleep(1)

threading.Thread(target=worker).start()
if not started.wait(2):
    raise RuntimeError("worker did not start")
libc = ctypes.CDLL(None)
libc.pthread_exit.argtypes = [ctypes.c_void_p]
libc.pthread_exit.restype = None
libc.pthread_exit(None)
"""
    result = linux_process.run(
        [sys.executable, "-c", script, str(marker_path)],
        timeout_secs=5,
        sample_interval_ms=10,
        cleanup_timeout_secs=5,
        stderr_path=str(stderr_path),
        qualified=True,
        cgroup_parent=str(parent),
    )
    assert marker_path.read_text() == "leader-z-and-resident", stderr_path.read_text()
    assert result.root_exit_code == 0 and not result.timed_out, stderr_path.read_text()
    assert result.peak_tree_rss_bytes is not None
    assert result.peak_tree_rss_bytes >= 64 * 1024 * 1024
    evidence = {item.identity.pid: item for item in result.processes}
    assert evidence[result.root.pid].peak_rss_bytes >= 64 * 1024 * 1024
    assert result.ownership_complete and not result.escaped
    assert result.cgroup_path is not None and not Path(result.cgroup_path).exists()
    assert (parent.stat().st_dev, parent.stat().st_ino) == parent_identity


@pytest.mark.parametrize("root_waits", [True, False], ids=["root-waits", "root-exits-early"])
@pytest.mark.skipif(not LINUX_PIDFD_AVAILABLE, reason="native Linux pidfd test")
def test_qualified_cgroup_timeout_cleans_real_child_after_root_lifecycle(tmp_path, root_waits):
    delegated = os.environ.get("QUANTA_INDEX_TEST_CGROUP_PARENT")
    if not delegated:
        pytest.skip("requires QUANTA_INDEX_TEST_CGROUP_PARENT delegated cgroup v2 path")
    parent = Path(delegated)
    parent_identity = (parent.stat().st_dev, parent.stat().st_ino)
    child_pid_path = tmp_path / "held-child-pid"
    stderr_path = tmp_path / "stderr"
    child_script = """
import os
import sys
import time

resident = bytearray(4 * 1024 * 1024)
for offset in range(0, len(resident), 4096):
    resident[offset] = 1
with open(sys.argv[1], "x", encoding="ascii") as marker:
    marker.write(str(os.getpid()))
time.sleep(30)
"""
    root_script = """
import pathlib
import subprocess
import sys
import time

marker = pathlib.Path(sys.argv[1])
child = subprocess.Popen([sys.executable, "-c", sys.argv[2], str(marker)])
while not marker.is_file():
    time.sleep(0.005)
if sys.argv[3] == "wait":
    child.wait()
"""
    result = linux_process.run(
        [
            sys.executable,
            "-c",
            root_script,
            str(child_pid_path),
            child_script,
            "wait" if root_waits else "exit",
        ],
        timeout_secs=2,
        sample_interval_ms=10,
        cleanup_timeout_secs=5,
        stderr_path=str(stderr_path),
        qualified=True,
        cgroup_parent=str(parent),
    )
    assert child_pid_path.is_file(), stderr_path.read_text()
    child_pid = int(child_pid_path.read_text())
    assert child_pid != result.root.pid
    evidence = {item.identity.pid: item for item in result.processes}
    assert child_pid in evidence
    child_identity = evidence[child_pid].identity
    assert child_identity.start_ticks > 0
    assert evidence[child_pid].peak_rss_bytes >= 4 * 1024 * 1024
    assert result.timed_out
    assert result.root_exit_code == (-signal.SIGKILL if root_waits else 0)
    assert result.backend == "cgroup-v2" and result.ownership_complete
    assert result.cleanup_complete and result.sampling_complete and not result.escaped
    assert result.cgroup_path is not None
    assert Path(result.cgroup_path).parent == parent
    assert not Path(result.cgroup_path).exists()
    assert (parent.stat().st_dev, parent.stat().st_ino) == parent_identity
    try:
        survivor = linux_process.parse_proc_stat(
            Path(f"/proc/{child_pid}/stat").read_text(), expected_pid=child_pid
        )
    except FileNotFoundError:
        pass
    else:
        assert survivor.identity != child_identity or not survivor.live


@pytest.mark.skipif(not LINUX_PIDFD_AVAILABLE, reason="native Linux pidfd test")
def test_timeout_cleans_live_descendant_on_linux(tmp_path, monkeypatch):
    ready_path = tmp_path / "descendant-ready"
    observed = wait_for_descendant_fixture(monkeypatch, ready_path)
    script = (
        "import pathlib,subprocess,sys,time; subprocess.Popen(['sleep','30']); "
        "pathlib.Path(sys.argv[1]).touch(); time.sleep(30)"
    )
    result = linux_process.run(
        [sys.executable, "-c", script, str(ready_path)],
        timeout_secs=0.3,
        sample_interval_ms=10,
        cleanup_timeout_secs=2,
        stdout_path=str(tmp_path / "out"),
        stderr_path=str(tmp_path / "err"),
    )
    assert observed["ready"], "descendant did not reach the ownership fixture checkpoint"
    assert result.timed_out
    assert result.cleanup_complete
    assert result.samples > 0
    assert len(result.processes) >= 2
    assert result.root.start_ticks > 0
    assert not result.ownership_complete


@pytest.mark.skipif(not LINUX_PIDFD_AVAILABLE, reason="native Linux pidfd test")
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


def wait_for_descendant_fixture(monkeypatch, ready_path):
    """Test ownership cleanup after fixture setup, not startup throughput."""
    original_popen = linux_process.subprocess.Popen
    observed = {"ready": False}

    def popen_after_fixture_ready(*args, **kwargs):
        process = original_popen(*args, **kwargs)
        deadline = time.monotonic() + 5
        while time.monotonic() < deadline:
            if ready_path.is_file():
                observed["ready"] = True
                break
            time.sleep(0.005)
        return process

    monkeypatch.setattr(linux_process.subprocess, "Popen", popen_after_fixture_ready)
    return observed


@pytest.mark.skipif(not LINUX_PIDFD_AVAILABLE, reason="native Linux pidfd test")
def test_native_detects_process_group_escape_on_linux(tmp_path, monkeypatch):
    ready_path = tmp_path / "descendant-ready"
    observed = wait_for_descendant_fixture(monkeypatch, ready_path)
    script = (
        "import subprocess,sys,time; "
        "subprocess.Popen(['sleep','30'], start_new_session=True); "
        "open(sys.argv[1],'xb').close(); "
        "time.sleep(30)"
    )
    result = linux_process.run(
        [sys.executable, "-c", script, str(ready_path)],
        timeout_secs=0.3,
        sample_interval_ms=10,
        cleanup_timeout_secs=2,
    )
    assert observed["ready"], "descendant did not reach the ownership fixture checkpoint"
    assert result.timed_out
    assert result.escaped
    assert not result.ownership_complete
    assert result.cleanup_complete


@pytest.mark.skipif(not LINUX_PIDFD_AVAILABLE, reason="native Linux pidfd test")
def test_root_exit_does_not_release_descendant_on_linux(tmp_path, monkeypatch):
    ready_path = tmp_path / "descendant-ready"
    observed = wait_for_descendant_fixture(monkeypatch, ready_path)
    script = (
        "import subprocess,sys; subprocess.Popen(['sleep','30']); open(sys.argv[1],'xb').close()"
    )
    result = linux_process.run(
        [sys.executable, "-c", script, str(ready_path)], timeout_secs=0.3, sample_interval_ms=10
    )
    assert observed["ready"], "descendant did not reach the ownership fixture checkpoint"
    assert result.root_exit_code == 0
    assert result.timed_out
    assert result.cleanup_complete
    assert len(result.processes) >= 2


@pytest.mark.skipif(not LINUX_PIDFD_AVAILABLE, reason="native Linux pidfd test")
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
