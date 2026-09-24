"""Linux /proc sampler with process-group ownership for benchmark children.

The root is a new session leader and is not reaped until all observed members
have exited or cleanup finishes. This keeps its PID/PGID reserved. A process
that leaves the group while still visible as a descendant is detected, pinned
with a pidfd, and killed during cleanup. A child that daemonizes and is
reparented entirely between scans cannot be attributed by /proc. Callers must
reject observed escapes and must not treat this fallback as cgroup containment
or native-host qualification.
"""

from __future__ import annotations

import math
import os
import signal
import subprocess
import sys
import time
from dataclasses import dataclass
from pathlib import Path


class ProcessError(RuntimeError):
    """Ownership, sampling, or cleanup evidence was incomplete."""


@dataclass(frozen=True, order=True)
class ProcessIdentity:
    pid: int
    start_ticks: int


@dataclass(frozen=True)
class ProcessStat:
    identity: ProcessIdentity
    ppid: int
    pgid: int
    state: str
    user_ticks: int
    kernel_ticks: int
    rss_pages: int

    @property
    def live(self) -> bool:
        return self.state not in ("Z", "X", "x")


@dataclass(frozen=True)
class ProcessEvidence:
    identity: ProcessIdentity
    peak_rss_bytes: int
    user_cpu_ns: int
    kernel_cpu_ns: int


@dataclass(frozen=True)
class ProcessResult:
    """Sampled CPU totals are lower bounds for processes missed between scans."""

    root: ProcessIdentity
    root_exit_code: int | None
    timed_out: bool
    elapsed_ms: float
    sample_interval_ms: int
    samples: int
    peak_tree_rss_bytes: int
    total_user_cpu_ns: int
    total_kernel_cpu_ns: int
    processes: tuple[ProcessEvidence, ...]
    escaped: tuple[ProcessIdentity, ...]
    sampling_complete: bool
    cleanup_complete: bool
    stdout_path: str | None
    stderr_path: str | None

    @property
    def total_cpu_ns(self) -> int:
        return self.total_user_cpu_ns + self.total_kernel_cpu_ns

    @property
    def ownership_complete(self) -> bool:
        """Whether all *observed* members were sampled and cleaned without escape."""
        return self.sampling_complete and self.cleanup_complete and not self.escaped


def parse_proc_stat(raw: str, *, expected_pid: int | None = None) -> ProcessStat:
    """Parse /proc/PID/stat without splitting the parenthesized command."""
    try:
        left, tail = raw.rstrip().split(" (", 1)
        command_end = tail.rfind(") ")
        if command_end < 0:
            raise ValueError("missing command terminator")
        pid = int(left)
        fields = tail[command_end + 2 :].split()
        if len(fields) < 22 or len(fields[0]) != 1:
            raise ValueError("truncated stat")
        if expected_pid is not None and pid != expected_pid:
            raise ValueError("PID mismatch")
        ppid, pgid = int(fields[1]), int(fields[2])
        user, kernel = int(fields[11]), int(fields[12])
        start, rss = int(fields[19]), int(fields[21])
        if pid <= 0 or ppid < 0 or pgid <= 0 or start <= 0 or min(user, kernel, rss) < 0:
            raise ValueError("negative or invalid stat field")
    except (IndexError, TypeError, ValueError) as exc:
        raise ProcessError(f"invalid /proc stat for PID {expected_pid}: {exc}") from exc
    return ProcessStat(ProcessIdentity(pid, start), ppid, pgid, fields[0], user, kernel, rss)


def _read_snapshot(proc_root: Path) -> dict[int, ProcessStat]:
    rows: dict[int, ProcessStat] = {}
    try:
        entries = list(proc_root.iterdir())
    except OSError as exc:
        raise ProcessError(f"cannot enumerate {proc_root}: {exc}") from exc
    for entry in entries:
        if not entry.name.isdecimal():
            continue
        pid = int(entry.name)
        try:
            row = parse_proc_stat((entry / "stat").read_text(), expected_pid=pid)
        except FileNotFoundError:
            # A process may exit while /proc is being enumerated.
            continue
        except OSError as exc:
            raise ProcessError(f"cannot read {entry / 'stat'}: {exc}") from exc
        rows[pid] = row
    return rows


def _select_owned(
    snapshot: dict[int, ProcessStat], root: ProcessIdentity, known: set[ProcessIdentity]
) -> dict[ProcessIdentity, ProcessStat]:
    current_root = snapshot.get(root.pid)
    if current_root is None or current_root.identity != root:
        raise ProcessError("root PID/start-time identity disappeared before reap")
    if current_root.pgid != root.pid:
        raise ProcessError("root left its reserved process group")
    selected = {
        row.identity: row
        for row in snapshot.values()
        if row.pgid == root.pid or row.identity in known
    }
    # Find descendants that changed process group but retain a visible parent.
    owned_pids = {item.pid for item in selected}
    changed = True
    while changed:
        changed = False
        for row in snapshot.values():
            if row.identity not in selected and row.ppid in owned_pids:
                selected[row.identity] = row
                owned_pids.add(row.identity.pid)
                changed = True
    return selected


class _Tracker:
    def __init__(self, root: ProcessIdentity, clock_ticks: int, page_bytes: int):
        self.root = root
        self.clock_ticks = clock_ticks
        self.page_bytes = page_bytes
        self.known: set[ProcessIdentity] = set()
        self.pidfds: dict[ProcessIdentity, int] = {}
        self.escaped: set[ProcessIdentity] = set()
        self.peaks: dict[ProcessIdentity, tuple[int, int, int]] = {}
        self.peak_tree_rss_bytes = 0
        self.samples = 0

    def observe(self, snapshot: dict[int, ProcessStat]) -> dict[ProcessIdentity, ProcessStat]:
        selected = _select_owned(snapshot, self.root, self.known)
        for identity, row in selected.items():
            if identity not in self.known:
                fd = None
                try:
                    fd = os.pidfd_open(identity.pid, 0)
                    # pidfd_open and /proc reading are not atomic. Reject PID reuse.
                    actual = parse_proc_stat(
                        Path(f"/proc/{identity.pid}/stat").read_text(), expected_pid=identity.pid
                    )
                    if actual.identity != identity:
                        raise ProcessError(f"PID {identity.pid} changed start time while pinning")
                except (OSError, ProcessError) as exc:
                    if fd is not None:
                        os.close(fd)
                    raise ProcessError(f"cannot pin PID {identity.pid}: {exc}") from exc
                self.pidfds[identity] = fd
                self.known.add(identity)
            if row.pgid != self.root.pid:
                self.escaped.add(identity)
            rss = row.rss_pages * self.page_bytes
            old = self.peaks.get(identity, (0, 0, 0))
            self.peaks[identity] = (
                max(old[0], rss),
                max(old[1], row.user_ticks),
                max(old[2], row.kernel_ticks),
            )
        self.peak_tree_rss_bytes = max(
            self.peak_tree_rss_bytes,
            sum(row.rss_pages * self.page_bytes for row in selected.values() if row.live),
        )
        self.samples += 1
        return selected

    def evidence(self) -> tuple[ProcessEvidence, ...]:
        return tuple(
            ProcessEvidence(
                identity,
                rss,
                user * 1_000_000_000 // self.clock_ticks,
                kernel * 1_000_000_000 // self.clock_ticks,
            )
            for identity, (rss, user, kernel) in sorted(self.peaks.items())
        )

    def close(self) -> None:
        for fd in self.pidfds.values():
            os.close(fd)
        self.pidfds.clear()


def _signal_owned(tracker: _Tracker, sig: signal.Signals) -> None:
    try:
        os.killpg(tracker.root.pid, sig)
    except ProcessLookupError:
        pass
    for identity in tracker.escaped:
        try:
            signal.pidfd_send_signal(tracker.pidfds[identity], sig)
        except ProcessLookupError:
            pass


def _cleanup(tracker: _Tracker, proc_root: Path, timeout_secs: float) -> bool:
    deadline = time.monotonic() + timeout_secs
    _signal_owned(tracker, signal.SIGTERM)
    escalated = False
    while True:
        selected = tracker.observe(_read_snapshot(proc_root))
        if not any(row.live for row in selected.values()):
            return True
        now = time.monotonic()
        if not escalated and now >= deadline - timeout_secs / 2:
            _signal_owned(tracker, signal.SIGKILL)
            escalated = True
        if now >= deadline:
            return False
        time.sleep(min(0.01, deadline - now))


def run(
    command: list[str],
    *,
    timeout_secs: float,
    sample_interval_ms: int = 50,
    cleanup_timeout_secs: float = 5.0,
    cwd: str | None = None,
    env: dict[str, str] | None = None,
    stdout_path: str | None = None,
    stderr_path: str | None = None,
) -> ProcessResult:
    """Run and sample a Linux process group; terminate observed survivors.

    ``ownership_complete`` is false after any observed process-group escape.
    Polling /proc cannot prove that no unobserved, already-reparented child
    escaped between samples. Callers requiring that guarantee need a delegated
    cgroup or another kernel ownership primitive.
    """
    if sys.platform != "linux":
        raise ProcessError("native Linux host required")
    if not hasattr(os, "pidfd_open") or not hasattr(signal, "pidfd_send_signal"):
        raise ProcessError("Linux pidfd_open and pidfd_send_signal required")
    if (
        not command
        or not isinstance(command[0], str)
        or not command[0]
        or any(not isinstance(arg, str) or "\0" in arg for arg in command)
        or any(
            isinstance(value, bool)
            or not isinstance(value, (int, float))
            or not math.isfinite(value)
            or value <= 0
            for value in (timeout_secs, cleanup_timeout_secs)
        )
        or type(sample_interval_ms) is not int
        or sample_interval_ms <= 0
    ):
        raise ProcessError("invalid command or timeout/sample interval")
    output_files = []
    process = None
    tracker = None
    try:
        for path in (stdout_path, stderr_path):
            output_files.append(open(path, "xb") if path is not None else subprocess.DEVNULL)
        process = subprocess.Popen(
            command,
            cwd=cwd,
            env=env,
            start_new_session=True,
            stdin=subprocess.DEVNULL,
            stdout=output_files[0],
            stderr=output_files[1],
        )
        started = time.monotonic()
        proc_root = Path("/proc")
        root_stat = parse_proc_stat(
            (proc_root / str(process.pid) / "stat").read_text(), expected_pid=process.pid
        )
        if root_stat.pgid != process.pid:
            raise ProcessError("child was not started in a private process group")
        tracker = _Tracker(root_stat.identity, os.sysconf("SC_CLK_TCK"), os.sysconf("SC_PAGE_SIZE"))
        deadline = started + timeout_secs
        timed_out = False
        while True:
            selected = tracker.observe(_read_snapshot(proc_root))
            if not any(row.live for row in selected.values()):
                break
            now = time.monotonic()
            if now >= deadline:
                timed_out = True
                break
            time.sleep(min(sample_interval_ms / 1000, deadline - now))
        stopped = time.monotonic()
        cleanup_complete = _cleanup(tracker, proc_root, cleanup_timeout_secs)
        if not cleanup_complete:
            raise ProcessError("owned process survived cleanup deadline")
        exit_code = process.wait(timeout=cleanup_timeout_secs)
        evidence = tracker.evidence()
        return ProcessResult(
            root=root_stat.identity,
            root_exit_code=exit_code,
            timed_out=timed_out,
            elapsed_ms=(stopped - started) * 1000,
            sample_interval_ms=sample_interval_ms,
            samples=tracker.samples,
            peak_tree_rss_bytes=tracker.peak_tree_rss_bytes,
            total_user_cpu_ns=sum(row.user_cpu_ns for row in evidence),
            total_kernel_cpu_ns=sum(row.kernel_cpu_ns for row in evidence),
            processes=evidence,
            escaped=tuple(sorted(tracker.escaped)),
            sampling_complete=True,
            cleanup_complete=True,
            stdout_path=stdout_path,
            stderr_path=stderr_path,
        )
    except BaseException as exc:
        if process is not None:
            try:
                if tracker is not None:
                    _signal_owned(tracker, signal.SIGKILL)
                    clean = _cleanup(tracker, Path("/proc"), cleanup_timeout_secs)
                    if not clean:
                        raise ProcessError("owned process survived cleanup deadline")
                else:
                    try:
                        os.killpg(process.pid, signal.SIGKILL)
                    except ProcessLookupError:
                        pass
                process.wait(timeout=cleanup_timeout_secs)
            except (OSError, subprocess.TimeoutExpired, ProcessError) as cleanup_exc:
                raise ProcessError(f"sampler failed: {exc}; cleanup failed: {cleanup_exc}") from exc
        raise
    finally:
        if tracker is not None:
            tracker.close()
        for output in output_files:
            if output != subprocess.DEVNULL:
                output.close()
