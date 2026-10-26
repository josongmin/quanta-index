"""Linux benchmark process owners: qualified cgroup v2 or diagnostic group.

The root is a new session leader and is not reaped until all observed members
have exited or cleanup finishes. This keeps its PID/PGID reserved. A process
that leaves the group while still visible as a descendant is detected, pinned
with a pidfd, and killed during cleanup. A child that daemonizes and is
reparented entirely between scans cannot be attributed by /proc. Callers must
reject observed escapes and must not treat this fallback as qualified ownership.
Qualified runs require an explicitly delegated cgroup v2 parent and never fall
back to process-group polling.

The cgroup result proves the dedicated subtree is empty after cleanup. A
workload with permission to migrate itself to a cgroup outside that subtree
can escape before observation; deployment must prevent such migration or
exclude the run from an unrestricted orphan=0 claim.
"""

from __future__ import annotations

import errno
import math
import os
import re
import signal
import stat
import subprocess
import sys
import tempfile
import time
from dataclasses import dataclass
from pathlib import Path


class ProcessError(RuntimeError):
    """Ownership, sampling, or cleanup evidence was incomplete."""


_SMAPS_ROLLUP_MAX_BYTES = 32 * 1024
_PROC_STATUS_MAX_BYTES = 64 * 1024
_U64_MAX = (1 << 64) - 1
# include/linux/sched.h: set by exit_signals() before exit_mm() drops task->mm.
_PF_EXITING = 0x00000004
_SMAPS_RSS_LINE = re.compile(rb"Rss:[ \t]+([0-9]+)[ \t]+kB")


def _read_smaps_rollup_rss_bytes(pid: int, proc_root: Path = Path("/proc")) -> int:
    """Read one bounded, unambiguous Linux smaps_rollup RSS value."""
    path = proc_root / str(pid) / "smaps_rollup"
    with path.open("rb") as stream:
        raw = stream.read(_SMAPS_ROLLUP_MAX_BYTES + 1)
    if len(raw) > _SMAPS_ROLLUP_MAX_BYTES:
        raise ProcessError(f"oversized smaps_rollup for PID {pid}")
    rss_lines = [line for line in raw.splitlines() if line.startswith(b"Rss:")]
    if len(rss_lines) != 1:
        raise ProcessError(f"missing or duplicate smaps_rollup Rss for PID {pid}")
    matched = _SMAPS_RSS_LINE.fullmatch(rss_lines[0])
    if matched is None:
        raise ProcessError(f"invalid smaps_rollup Rss for PID {pid}")
    digits = matched.group(1).lstrip(b"0") or b"0"
    if len(digits) > 20:
        raise ProcessError(f"smaps_rollup Rss overflow for PID {pid}")
    rss_kib = int(digits)
    if rss_kib > _U64_MAX // 1024:
        raise ProcessError(f"smaps_rollup Rss overflow for PID {pid}")
    return rss_kib * 1024


def _read_thread_tgid(tid: int, proc_root: Path = Path("/proc")) -> int:
    """Read the kernel thread-group ID without trusting an unbounded status file."""
    with (proc_root / str(tid) / "status").open("rb") as stream:
        raw = stream.read(_PROC_STATUS_MAX_BYTES + 1)
    if len(raw) > _PROC_STATUS_MAX_BYTES:
        raise ProcessError(f"oversized /proc status for TID {tid}")
    values = [line.split() for line in raw.splitlines() if line.startswith(b"Tgid:")]
    if len(values) != 1 or len(values[0]) != 2 or values[0][0] != b"Tgid:":
        raise ProcessError(f"missing or duplicate Tgid for TID {tid}")
    value = values[0][1]
    if re.fullmatch(rb"[0-9]+", value) is None:
        raise ProcessError(f"invalid Tgid for TID {tid}")
    digits = value.lstrip(b"0") or b"0"
    if len(digits) > 20:
        raise ProcessError(f"invalid Tgid for TID {tid}")
    tgid = int(digits)
    if tgid <= 0 or tgid > _U64_MAX:
        raise ProcessError(f"invalid Tgid for TID {tid}")
    return tgid


def _validate_pass_fds(pass_fds: tuple[int, ...]) -> tuple[int, ...]:
    """Accept at most one explicit attestation pipe write end."""
    if type(pass_fds) is not tuple or len(pass_fds) > 1:
        raise ProcessError("pass_fds must be a tuple with at most one attestation FD")
    if not pass_fds:
        return pass_fds
    try:
        import fcntl
    except ImportError as exc:
        raise ProcessError("attestation FD forwarding requires POSIX") from exc
    for fd in pass_fds:
        if type(fd) is not int or fd < 3:
            raise ProcessError("attestation FD must be an integer outside stdio")
        try:
            mode = os.fstat(fd).st_mode
            flags = fcntl.fcntl(fd, fcntl.F_GETFL)
        except OSError as exc:
            raise ProcessError(f"attestation FD {fd} is not open: {exc}") from exc
        if not stat.S_ISFIFO(mode) or flags & os.O_ACCMODE != os.O_WRONLY:
            raise ProcessError("attestation FD must be a pipe write end")
    return pass_fds


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
    task_flags: int
    num_threads: int

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
    """Cgroup CPU is kernel-accounted; cgroup memory peak is not RSS.

    Qualified ``peak_tree_rss_bytes`` sums live members' smaps_rollup Rss at
    each sample; the diagnostic process-group path uses stat RSS pages. Both
    can miss short-lived members and count shared resident pages repeatedly.
    ``ownership_complete`` covers the dedicated cgroup subtree, subject to
    the migration boundary described above.
    """

    root: ProcessIdentity
    root_exit_code: int | None
    timed_out: bool
    elapsed_ms: float
    sample_interval_ms: int
    samples: int
    peak_tree_rss_bytes: int | None
    total_user_cpu_ns: int
    total_kernel_cpu_ns: int
    processes: tuple[ProcessEvidence, ...]
    escaped: tuple[ProcessIdentity, ...]
    sampling_complete: bool
    cleanup_complete: bool
    stdout_path: str | None
    stderr_path: str | None
    backend: str = "process-group"
    cgroup_path: str | None = None
    peak_cgroup_memory_bytes: int | None = None
    cgroup_cpu_usage_ns: int | None = None

    @property
    def total_cpu_ns(self) -> int:
        return self.total_user_cpu_ns + self.total_kernel_cpu_ns

    @property
    def rss_method(self) -> str:
        """Bind a reported RSS number to its actual process sampling method."""
        if self.backend == "cgroup-v2":
            return "sampled_smaps_rollup_rss_sum_shared_pages_may_repeat"
        if self.backend == "process-group":
            return "sampled_proc_stat_rss_sum_shared_pages_may_repeat"
        raise ProcessError(f"unsupported process backend for RSS method: {self.backend}")

    @property
    def ownership_complete(self) -> bool:
        """Require cgroup accounting and empty cleanup for observed ownership."""
        return (
            self.backend == "cgroup-v2"
            and self.sampling_complete
            and self.cleanup_complete
            and not self.escaped
            and self.peak_tree_rss_bytes is not None
            and self.peak_cgroup_memory_bytes is not None
            and self.cgroup_cpu_usage_ns is not None
        )


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
        flags, num_threads = int(fields[6]), int(fields[17])
        user, kernel = int(fields[11]), int(fields[12])
        start, rss = int(fields[19]), int(fields[21])
        # Linux can report pgrp=-1 while an exited task is being torn down.
        # It is not a valid group for a live process.
        missing_dead_group = fields[0] in ("Z", "X", "x") and pgid == -1
        # Kernel tasks, including children of kthreadd, report pgrp=0. The
        # launched root and all selected descendants are checked separately.
        if (
            pid <= 0
            or ppid < 0
            or (pgid < 0 and not missing_dead_group)
            or start <= 0
            or min(flags, num_threads, user, kernel, rss) < 0
        ):
            raise ValueError("negative or invalid stat field")
    except (IndexError, TypeError, ValueError) as exc:
        raise ProcessError(f"invalid /proc stat for PID {expected_pid}: {exc}") from exc
    return ProcessStat(
        ProcessIdentity(pid, start), ppid, pgid, fields[0], user, kernel, rss, flags, num_threads
    )


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
    if current_root.pgid != root.pid and (current_root.live or current_root.pgid != -1):
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
            if row.pgid != self.root.pid and (row.live or row.pgid != -1):
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


def _keyed_counters(raw: str, required: set[str]) -> dict[str, int]:
    counters: dict[str, int] = {}
    for line in raw.splitlines():
        fields = line.split()
        if len(fields) != 2 or fields[0] in counters:
            raise ProcessError("malformed or duplicate cgroup counter")
        try:
            value = int(fields[1])
        except ValueError as exc:
            raise ProcessError("non-integer cgroup counter") from exc
        if value < 0:
            raise ProcessError("negative cgroup counter")
        counters[fields[0]] = value
    if not required <= counters.keys():
        raise ProcessError(f"missing cgroup counters: {sorted(required - counters.keys())}")
    return counters


def _is_cgroup2_path(parent: Path, mountinfo: str) -> bool:
    for line in mountinfo.splitlines():
        if " - " not in line:
            continue
        before, after = line.split(" - ", 1)
        fields, fs = before.split(), after.split()
        if len(fields) < 5 or not fs or fs[0] != "cgroup2":
            continue
        mount = Path(fields[4].replace("\\040", " ").replace("\\011", "\t"))
        if parent == mount or mount in parent.parents:
            return True
    return False


class _CgroupOwner:
    """Own exactly one freshly created child of an explicit delegated parent."""

    def __init__(self, parent: Path, path: Path):
        self.parent = parent
        self.path = path
        identity = path.stat()
        self._dev_ino = (identity.st_dev, identity.st_ino)

    @classmethod
    def create(cls, parent_arg: str | None) -> _CgroupOwner:
        if not parent_arg or not isinstance(parent_arg, str):
            raise ProcessError("qualified run requires an explicit delegated cgroup parent")
        parent = Path(parent_arg)
        try:
            canonical = parent.resolve(strict=True)
        except OSError as exc:
            raise ProcessError(f"cgroup parent unavailable: {exc}") from exc
        if not parent.is_absolute() or canonical != parent:
            raise ProcessError("cgroup parent must be an absolute, non-symlink path")
        try:
            mountinfo = Path("/proc/self/mountinfo").read_text()
        except OSError as exc:
            raise ProcessError(f"cannot inspect cgroup v2 mount: {exc}") from exc
        if not _is_cgroup2_path(parent, mountinfo):
            raise ProcessError("delegated parent is not on a cgroup v2 mount")
        try:
            path = Path(tempfile.mkdtemp(prefix="quanta-retrieval-", dir=parent))
        except OSError as exc:
            raise ProcessError(f"cgroup delegation unavailable at {parent}: {exc}") from exc
        try:
            owner = cls(parent, path)
            if (path / "cgroup.type").read_text().strip() != "domain":
                raise ProcessError("dedicated cgroup is not a domain cgroup")
            for name in (
                "cgroup.procs", "cgroup.threads", "cgroup.events", "cgroup.kill",
                "cpu.stat", "memory.peak",
            ):
                if not (path / name).is_file():
                    raise ProcessError(f"required cgroup v2 file unavailable: {name}")
            owner.accounting()
            if owner.populated():
                raise ProcessError("new cgroup unexpectedly populated")
        except BaseException as exc:
            try:
                path.rmdir()
            except OSError as cleanup_exc:
                raise ProcessError(
                    f"cgroup setup failed: {exc}; dedicated subgroup cleanup failed: {cleanup_exc}"
                ) from exc
            raise ProcessError(f"cgroup setup failed: {exc}") from exc
        return owner

    def _verify(self) -> None:
        if self.path.parent != self.parent or not self.path.name.startswith("quanta-retrieval-"):
            raise ProcessError("cgroup path escaped dedicated child")
        current = self.path.lstat()
        if (current.st_dev, current.st_ino) != self._dev_ino or not self.path.is_dir():
            raise ProcessError("dedicated cgroup identity changed")

    def assign(self, pid: int) -> None:
        self._verify()
        (self.path / "cgroup.procs").write_text(str(pid))
        if pid not in self.members():
            raise ProcessError("child not present in dedicated cgroup after assignment")

    def populated(self) -> bool:
        self._verify()
        state = _keyed_counters((self.path / "cgroup.events").read_text(), {"populated"})
        if state["populated"] not in (0, 1):
            raise ProcessError("invalid cgroup populated state")
        return bool(state["populated"])

    def accounting(self) -> tuple[int, int, int, int]:
        self._verify()
        raw_peak = (self.path / "memory.peak").read_text().strip()
        try:
            peak = int(raw_peak)
        except ValueError as exc:
            raise ProcessError("invalid cgroup memory.peak") from exc
        if peak < 0:
            raise ProcessError("negative cgroup memory.peak")
        cpu = _keyed_counters(
            (self.path / "cpu.stat").read_text(), {"usage_usec", "user_usec", "system_usec"}
        )
        return peak, cpu["user_usec"] * 1000, cpu["system_usec"] * 1000, cpu["usage_usec"] * 1000

    def _ids(self, name: str) -> set[int]:
        self._verify()
        ids: set[int] = set()

        def fail_walk(exc: OSError) -> None:
            raise ProcessError(f"cannot enumerate dedicated cgroup: {exc}") from exc

        for root, _dirs, _files in os.walk(self.path, onerror=fail_walk, followlinks=False):
            for line in (Path(root) / name).read_text().splitlines():
                if not line.isdecimal() or int(line) <= 0:
                    raise ProcessError(f"invalid {name} ID")
                ids.add(int(line))
        return ids

    def members(self) -> set[int]:
        return self._ids("cgroup.procs")

    def threads(self) -> set[int]:
        return self._ids("cgroup.threads")

    def kill(self) -> None:
        self._verify()
        (self.path / "cgroup.kill").write_text("1")

    def wait_empty(self, timeout_secs: float) -> bool:
        deadline = time.monotonic() + timeout_secs
        while self.populated():
            now = time.monotonic()
            if now >= deadline:
                return False
            time.sleep(min(0.01, deadline - now))
        return True

    def remove(self) -> None:
        self._verify()
        if self.populated():
            raise ProcessError("refusing to remove populated cgroup")

        def fail_walk(exc: OSError) -> None:
            raise ProcessError(f"cannot enumerate dedicated cgroup: {exc}") from exc

        for root, _dirs, _files in os.walk(
            self.path, topdown=False, onerror=fail_walk, followlinks=False
        ):
            candidate = Path(root)
            if candidate != self.path and self.path not in candidate.parents:
                raise ProcessError("refusing to remove outside dedicated cgroup")
            candidate.rmdir()


def _proc_cgroup_path(pid: int) -> str:
    lines = Path(f"/proc/{pid}/cgroup").read_text().splitlines()
    if len(lines) != 1 or not lines[0].startswith("0::"):
        raise ProcessError(f"invalid unified cgroup membership for PID {pid}")
    return lines[0][3:]


def _child_shim(read_fd: int, command: list[str]) -> None:
    """Wait for parent cgroup assignment before running the workload."""
    try:
        if os.read(read_fd, 1) != b"1":
            os._exit(126)
        os.close(read_fd)
        os.execvp(command[0], command)
    except OSError as exc:
        os.write(2, f"cgroup child exec failed: {exc}\n".encode())
        os._exit(127)


def _spawn_cgroup_shim(
    command: list[str],
    gate_read_fd: int,
    pass_fds: tuple[int, ...],
    *,
    cwd: str | None,
    env: dict[str, str] | None,
    stdout: object,
    stderr: object,
) -> subprocess.Popen:
    """Pass only the private gate and the validated caller attestation FD."""
    return subprocess.Popen(
        [
            sys.executable,
            "-I",
            str(Path(__file__).resolve()),
            "--cgroup-child",
            str(gate_read_fd),
            *command,
        ],
        cwd=cwd,
        env=env,
        start_new_session=True,
        pass_fds=(gate_read_fd, *pass_fds),
        close_fds=True,
        stdin=subprocess.DEVNULL,
        stdout=stdout,
        stderr=stderr,
    )


def _release_cgroup_workload(write_fd: int, absolute_deadline: float | None) -> float:
    """Do not release the pre-exec gate after the caller's original deadline."""
    if absolute_deadline is not None and time.monotonic() >= absolute_deadline:
        raise ProcessError("producer deadline expired before cgroup workload launch")
    os.write(write_fd, b"1")
    return time.monotonic()


class _CgroupTracker:
    def __init__(
        self,
        root: ProcessIdentity,
        membership_path: str,
        clock_ticks: int,
        page_bytes: int,
        root_baseline_ticks: tuple[int, int] = (0, 0),
    ):
        self.root = root
        self.membership_path = membership_path
        self.clock_ticks = clock_ticks
        self.page_bytes = page_bytes
        self.root_baseline_ticks = root_baseline_ticks
        self.pidfds: dict[ProcessIdentity, int] = {}
        self.peaks: dict[ProcessIdentity, tuple[int, int, int]] = {}
        self.escaped: set[ProcessIdentity] = set()
        self.peak_tree_rss_bytes: int | None = None
        self.peak_cgroup_memory_bytes = 0
        self.user_cpu_ns = 0
        self.kernel_cpu_ns = 0
        self.cpu_usage_ns = 0
        self.samples = 0

    def _inside(self, path: str) -> bool:
        return path == self.membership_path or path.startswith(self.membership_path + "/")

    def _revalidate_smaps_member(self, identity: ProcessIdentity) -> ProcessStat | None:
        """Validate identity and membership, including an exited group leader."""
        pid = identity.pid
        try:
            row = parse_proc_stat(Path(f"/proc/{pid}/stat").read_text(), expected_pid=pid)
        except FileNotFoundError:
            return None
        except OSError as exc:
            raise ProcessError(f"cannot revalidate /proc stat for PID {pid}: {exc}") from exc
        if row.identity != identity:
            raise ProcessError(f"PID {pid} changed start time during RSS sample")
        try:
            membership = _proc_cgroup_path(pid)
        except FileNotFoundError as exc:
            # /proc can vanish between the stat and cgroup reads. A still-live
            # identity with unreadable membership is not valid evidence.
            try:
                current = parse_proc_stat(Path(f"/proc/{pid}/stat").read_text(), expected_pid=pid)
            except FileNotFoundError:
                return None
            if current.identity != identity:
                raise ProcessError(f"PID {pid} changed start time during RSS sample") from exc
            raise ProcessError(f"cannot revalidate cgroup membership for live PID {pid}") from exc
        except OSError as exc:
            raise ProcessError(f"cannot revalidate cgroup membership for PID {pid}: {exc}") from exc
        if not self._inside(membership):
            self.escaped.add(identity)
            raise ProcessError(f"PID {pid} migrated outside dedicated cgroup")
        return row

    def _validated_thread_row(
        self, tid: int, group: ProcessIdentity, *, permit_other_group: bool = False
    ) -> ProcessStat | None:
        """Validate one cgroup thread against its TGID, identity, and membership."""
        path = Path(f"/proc/{tid}/stat")
        try:
            row = parse_proc_stat(path.read_text(), expected_pid=tid)
        except FileNotFoundError:
            return None
        except OSError as exc:
            raise ProcessError(f"cannot read /proc stat for TID {tid}: {exc}") from exc
        if not row.live:
            return None
        try:
            tgid = _read_thread_tgid(tid)
        except FileNotFoundError as exc:
            try:
                current = parse_proc_stat(path.read_text(), expected_pid=tid)
            except FileNotFoundError:
                return None
            if not current.live:
                return None
            raise ProcessError(f"cannot read Tgid for live TID {tid}") from exc
        except OSError as exc:
            raise ProcessError(f"cannot read Tgid for TID {tid}: {exc}") from exc
        if tgid != group.pid:
            if permit_other_group:
                return None
            raise ProcessError(f"TID {tid} changed Tgid during RSS sample")
        try:
            membership = _proc_cgroup_path(tid)
        except FileNotFoundError as exc:
            try:
                current = parse_proc_stat(path.read_text(), expected_pid=tid)
            except FileNotFoundError:
                return None
            if not current.live:
                return None
            raise ProcessError(f"cannot read cgroup for live TID {tid}") from exc
        except OSError as exc:
            raise ProcessError(f"cannot read cgroup for TID {tid}: {exc}") from exc
        if not self._inside(membership):
            self.escaped.add(group)
            raise ProcessError(f"TID {tid} migrated outside dedicated cgroup")
        return row

    def _surviving_thread_rss(
        self, owner: _CgroupOwner, group: ProcessIdentity, thread_ids: set[int]
    ) -> tuple[int, ProcessStat] | None:
        """Sample one validated live mm when the TGID leader has lost its mm."""
        leader = self._revalidate_smaps_member(group)
        if leader is None:
            return None
        census = thread_ids
        for attempt in range(2):
            for tid in sorted(census - {group.pid}):
                before = self._validated_thread_row(tid, group, permit_other_group=True)
                if before is None:
                    continue
                try:
                    rss = _read_smaps_rollup_rss_bytes(tid)
                except (OSError, ProcessError) as exc:
                    # An exiting representative may lose its mm before the
                    # read; another thread can still represent the group.
                    after = self._validated_thread_row(tid, group)
                    if after is not None and after.identity != before.identity:
                        raise ProcessError(f"TID {tid} changed start time during RSS sample") from exc
                    if (
                        isinstance(exc, OSError)
                        and exc.errno in (errno.ESRCH, errno.ENOENT)
                        and (after is None or after.task_flags & _PF_EXITING)
                    ):
                        continue
                    raise ProcessError(
                        f"cannot sample smaps_rollup for live TID {tid}: {exc}"
                    ) from exc
                after = self._validated_thread_row(tid, group)
                if after is None:
                    continue
                if after.identity != before.identity:
                    raise ProcessError(f"TID {tid} changed start time during RSS sample")
                leader = self._revalidate_smaps_member(group)
                if leader is None:
                    return None
                return rss, leader
            if attempt == 0:
                # A second bounded census can find a newly created worker.
                census = owner.threads()
        leader = self._revalidate_smaps_member(group)
        if leader is None or group.pid not in owner.members():
            return None
        if leader.live and leader.task_flags & _PF_EXITING and leader.num_threads == 1:
            return None
        raise ProcessError(f"no readable live RSS representative for TGID {group.pid}")

    def sample(self, owner: _CgroupOwner, *, include_process_metrics: bool = True) -> None:
        members = owner.members()
        thread_ids: set[int] | None = None
        live_rss = 0
        live_seen = False
        for pid in members:
            try:
                row = parse_proc_stat(Path(f"/proc/{pid}/stat").read_text(), expected_pid=pid)
            except FileNotFoundError:
                continue
            identity = row.identity
            if not self._inside(_proc_cgroup_path(pid)):
                self.escaped.add(identity)
                raise ProcessError(f"PID {pid} migrated outside dedicated cgroup")
            if identity not in self.pidfds:
                fd = None
                try:
                    fd = os.pidfd_open(pid, 0)
                    pinned = parse_proc_stat(
                        Path(f"/proc/{pid}/stat").read_text(), expected_pid=pid
                    )
                    if pinned.identity != identity:
                        raise ProcessError(f"PID {pid} changed start time while pinning")
                except (OSError, ProcessError) as exc:
                    if fd is not None:
                        os.close(fd)
                    raise ProcessError(f"cannot pin cgroup member PID {pid}: {exc}") from exc
                self.pidfds[identity] = fd
            if include_process_metrics:
                rss: int | None = 0
                measured_live = False
                if row.live:
                    try:
                        rss = _read_smaps_rollup_rss_bytes(pid)
                    except (OSError, ProcessError) as exc:
                        post = self._revalidate_smaps_member(identity)
                        if post is None:
                            continue
                        if not (
                            isinstance(exc, OSError)
                            and exc.errno == errno.ESRCH
                            and (not post.live or post.task_flags & _PF_EXITING)
                        ):
                            raise ProcessError(
                                f"cannot sample smaps_rollup for live PID {pid}: {exc}"
                            ) from exc
                        # A leader can have exited its mm while worker TIDs
                        # keep the group's address space resident.
                        if thread_ids is None:
                            thread_ids = owner.threads()
                        representative = self._surviving_thread_rss(owner, identity, thread_ids)
                        if representative is not None:
                            rss, row = representative
                            measured_live = True
                        elif post.live and post.task_flags & _PF_EXITING and post.num_threads == 1:
                            # Single final thread in exit_mm(): no smaps value
                            # exists. This is an omission, not a zero RSS read.
                            rss = None
                            row = post
                        else:
                            continue
                    else:
                        post = self._revalidate_smaps_member(identity)
                        if post is None:
                            continue
                        row = post
                        measured_live = True
                else:
                    if thread_ids is None:
                        thread_ids = owner.threads()
                    representative = self._surviving_thread_rss(owner, identity, thread_ids)
                    if representative is None:
                        continue
                    rss, row = representative
                    measured_live = True
                old = self.peaks.get(identity, (0, 0, 0))
                self.peaks[identity] = (
                    max(old[0], rss or 0),
                    max(old[1], row.user_ticks),
                    max(old[2], row.kernel_ticks),
                )
                if measured_live and rss is not None:
                    if live_rss > _U64_MAX - rss:
                        raise ProcessError("sampled process RSS sum overflow")
                    live_rss += rss
                    live_seen = True
        # Membership is authoritative for current members. Recheck previously
        # pinned identities for a migration out of this dedicated subtree.
        for identity in self.pidfds:
            if identity.pid in members:
                continue
            try:
                row = parse_proc_stat(
                    Path(f"/proc/{identity.pid}/stat").read_text(), expected_pid=identity.pid
                )
            except FileNotFoundError:
                continue
            if (
                row.identity == identity
                and row.live
                and not self._inside(_proc_cgroup_path(identity.pid))
            ):
                self.escaped.add(identity)
                raise ProcessError(f"PID {identity.pid} migrated outside dedicated cgroup")
        if live_seen:
            self.peak_tree_rss_bytes = max(self.peak_tree_rss_bytes or 0, live_rss)
        peak, user, kernel, usage = owner.accounting()
        if (
            peak < self.peak_cgroup_memory_bytes
            or user < self.user_cpu_ns
            or kernel < self.kernel_cpu_ns
            or usage < self.cpu_usage_ns
        ):
            raise ProcessError("cgroup accounting counter regressed")
        self.peak_cgroup_memory_bytes = peak
        self.user_cpu_ns = user
        self.kernel_cpu_ns = kernel
        self.cpu_usage_ns = usage
        if include_process_metrics:
            self.samples += 1

    def kill_escapes(self) -> None:
        for identity in self.escaped:
            fd = self.pidfds.get(identity)
            if fd is None:
                raise ProcessError(f"escaped PID {identity.pid} has no stable pidfd")
            try:
                signal.pidfd_send_signal(fd, signal.SIGKILL)
            except ProcessLookupError:
                pass

    def evidence(self) -> tuple[ProcessEvidence, ...]:
        for identity, (_rss, user, kernel) in self.peaks.items():
            if identity == self.root and (
                user < self.root_baseline_ticks[0] or kernel < self.root_baseline_ticks[1]
            ):
                raise ProcessError("root CPU ticks regressed below pre-exec baseline")
        return tuple(
            ProcessEvidence(
                identity,
                rss,
                (user - (self.root_baseline_ticks[0] if identity == self.root else 0))
                * 1_000_000_000
                // self.clock_ticks,
                (kernel - (self.root_baseline_ticks[1] if identity == self.root else 0))
                * 1_000_000_000
                // self.clock_ticks,
            )
            for identity, (rss, user, kernel) in sorted(self.peaks.items())
        )

    def close(self) -> None:
        for fd in self.pidfds.values():
            os.close(fd)
        self.pidfds.clear()


def _run_cgroup(
    command: list[str],
    *,
    timeout_secs: float,
    sample_interval_ms: int,
    cleanup_timeout_secs: float,
    cwd: str | None,
    env: dict[str, str] | None,
    stdout_path: str | None,
    stderr_path: str | None,
    cgroup_parent: str | None,
    pass_fds: tuple[int, ...],
    absolute_deadline: float | None = None,
) -> ProcessResult:
    if sys.platform != "linux":
        raise ProcessError("native Linux host required for cgroup v2")
    if not hasattr(os, "pidfd_open") or not hasattr(signal, "pidfd_send_signal"):
        raise ProcessError("Linux pidfd support required for cgroup member identity")
    owner = _CgroupOwner.create(cgroup_parent)
    output_files = []
    process = None
    tracker = None
    read_fd = write_fd = None
    removed = False
    try:
        for path in (stdout_path, stderr_path):
            output_files.append(open(path, "xb") if path is not None else subprocess.DEVNULL)
        read_fd, write_fd = os.pipe()
        process = _spawn_cgroup_shim(
            command,
            read_fd,
            pass_fds,
            cwd=cwd,
            env=env,
            stdout=output_files[0],
            stderr=output_files[1],
        )
        os.close(read_fd)
        read_fd = None
        root_stat = parse_proc_stat(
            Path(f"/proc/{process.pid}/stat").read_text(), expected_pid=process.pid
        )
        if root_stat.pgid != process.pid:
            raise ProcessError("cgroup child was not started in a private process group")
        owner.assign(process.pid)
        membership_path = _proc_cgroup_path(process.pid)
        if membership_path == "/":
            raise ProcessError("dedicated cgroup path is not visible in /proc")
        tracker = _CgroupTracker(
            root_stat.identity,
            membership_path,
            os.sysconf("SC_CLK_TCK"),
            os.sysconf("SC_PAGE_SIZE"),
            (root_stat.user_ticks, root_stat.kernel_ticks),
        )
        tracker.sample(owner, include_process_metrics=False)
        baseline_user = tracker.user_cpu_ns
        baseline_kernel = tracker.kernel_cpu_ns
        baseline_usage = tracker.cpu_usage_ns
        started = _release_cgroup_workload(write_fd, absolute_deadline)
        os.close(write_fd)
        write_fd = None
        deadline = started + timeout_secs
        if absolute_deadline is not None:
            deadline = min(deadline, absolute_deadline)
        timed_out = False
        while True:
            tracker.sample(owner)
            populated = owner.populated()
            now = time.monotonic()
            if now >= deadline:
                timed_out = True
                break
            if not populated:
                break
            time.sleep(min(sample_interval_ms / 1000, deadline - now))
        stopped = now
        if timed_out:
            if populated:
                owner.kill()
            tracker.kill_escapes()
        if not owner.wait_empty(cleanup_timeout_secs):
            raise ProcessError(f"dedicated cgroup remained populated: {owner.path}")
        tracker.sample(owner)
        exit_code = process.wait(timeout=cleanup_timeout_secs)
        evidence = tracker.evidence()
        result = ProcessResult(
            root=root_stat.identity,
            root_exit_code=exit_code,
            timed_out=timed_out,
            elapsed_ms=(stopped - started) * 1000,
            sample_interval_ms=sample_interval_ms,
            samples=tracker.samples,
            peak_tree_rss_bytes=tracker.peak_tree_rss_bytes,
            total_user_cpu_ns=tracker.user_cpu_ns - baseline_user,
            total_kernel_cpu_ns=tracker.kernel_cpu_ns - baseline_kernel,
            processes=evidence,
            escaped=tuple(sorted(tracker.escaped)),
            sampling_complete=True,
            cleanup_complete=True,
            stdout_path=stdout_path,
            stderr_path=stderr_path,
            backend="cgroup-v2",
            cgroup_path=str(owner.path),
            peak_cgroup_memory_bytes=tracker.peak_cgroup_memory_bytes,
            cgroup_cpu_usage_ns=tracker.cpu_usage_ns - baseline_usage,
        )
        owner.remove()
        removed = True
        return result
    except BaseException as exc:
        cleanup_errors = []
        if write_fd is not None:
            os.close(write_fd)
            write_fd = None
        if process is not None:
            try:
                owner.kill()
            except (OSError, ProcessError) as error:
                cleanup_errors.append(f"cgroup.kill: {error}")
            if tracker is not None:
                try:
                    tracker.kill_escapes()
                except (OSError, ProcessError) as error:
                    cleanup_errors.append(f"escaped member: {error}")
            try:
                os.killpg(process.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            except OSError as error:
                cleanup_errors.append(f"shim group: {error}")
            try:
                process.wait(timeout=cleanup_timeout_secs)
            except (OSError, subprocess.TimeoutExpired) as error:
                cleanup_errors.append(f"root wait: {error}")
        try:
            if not owner.wait_empty(cleanup_timeout_secs):
                cleanup_errors.append(f"dedicated cgroup remained populated: {owner.path}")
            elif not removed:
                owner.remove()
                removed = True
        except (OSError, ProcessError) as error:
            cleanup_errors.append(f"dedicated cgroup cleanup: {error}")
        if cleanup_errors:
            raise ProcessError(
                f"cgroup run failed: {exc}; cleanup failed: {'; '.join(cleanup_errors)}"
            ) from exc
        raise
    finally:
        if read_fd is not None:
            os.close(read_fd)
        if write_fd is not None:
            os.close(write_fd)
        if tracker is not None:
            tracker.close()
        for output in output_files:
            if output != subprocess.DEVNULL:
                output.close()


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
    qualified: bool = False,
    cgroup_parent: str | None = None,
    pass_fds: tuple[int, ...] = (),
    absolute_deadline: float | None = None,
) -> ProcessResult:
    """Run a qualified cgroup owner or a diagnostic process-group fallback.

    Qualification never silently falls back when cgroup delegation is absent.
    ``cgroup_parent`` must be an explicit writable cgroup v2 delegated path.
    ``pass_fds`` may contain one caller-owned pipe write FD for attestation;
    the caller retains ownership of that FD. All other non-stdio FDs close.
    """
    if (
        not command
        or not isinstance(command[0], str)
        or not command[0]
        or any(not isinstance(arg, str) or "\0" in arg for arg in command)
        or any(
            type(value) not in (int, float) or not 0 < value <= sys.float_info.max
            for value in (timeout_secs, cleanup_timeout_secs)
        )
        or type(sample_interval_ms) is not int
        or sample_interval_ms <= 0
    ):
        raise ProcessError("invalid command or timeout/sample interval")
    if type(qualified) is not bool:
        raise ProcessError("qualified must be a boolean")
    if absolute_deadline is not None and (
        type(absolute_deadline) not in (int, float)
        or not math.isfinite(absolute_deadline)
        or not qualified
    ):
        raise ProcessError("absolute deadline requires a finite qualified Linux run")
    pass_fds = _validate_pass_fds(pass_fds)
    if qualified:
        options = {"absolute_deadline": absolute_deadline} if absolute_deadline is not None else {}
        return _run_cgroup(
            command,
            timeout_secs=timeout_secs,
            sample_interval_ms=sample_interval_ms,
            cleanup_timeout_secs=cleanup_timeout_secs,
            cwd=cwd,
            env=env,
            stdout_path=stdout_path,
            stderr_path=stderr_path,
            cgroup_parent=cgroup_parent,
            pass_fds=pass_fds,
            **options,
        )
    if cgroup_parent is not None:
        raise ProcessError("cgroup_parent requires qualified=True")
    if sys.platform != "linux":
        raise ProcessError("native Linux host required")
    if not hasattr(os, "pidfd_open") or not hasattr(signal, "pidfd_send_signal"):
        raise ProcessError("Linux pidfd_open and pidfd_send_signal required")
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
            pass_fds=pass_fds,
            close_fds=True,
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
            live = any(row.live for row in selected.values())
            now = time.monotonic()
            if now >= deadline:
                timed_out = True
                break
            if not live:
                break
            time.sleep(min(sample_interval_ms / 1000, deadline - now))
        stopped = now
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


if __name__ == "__main__":
    if len(sys.argv) < 4 or sys.argv[1] != "--cgroup-child":
        raise SystemExit("linux_process.py is an internal child shim")
    _child_shim(int(sys.argv[2]), sys.argv[3:])
