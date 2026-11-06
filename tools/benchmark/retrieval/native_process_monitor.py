"""Bounded process custody for a prebuilt native product test on macOS.

Only process identity and sampled RSS are observed. RSS is a lower bound, not
a lifetime peak. The caller owns source/binary checks and config semantics.
"""

from __future__ import annotations

import math
import os
import re
import shlex
import subprocess
import time
from collections.abc import Mapping
from dataclasses import dataclass
from pathlib import Path
from typing import Callable


class MonitorError(ValueError):
    """The selected process or config cannot be authenticated."""


@dataclass(frozen=True)
class Process:
    pid: int
    ppid: int
    started: str
    command: str
    rss_kib: int

    def identity(self) -> tuple[int, int, str, str]:
        return self.pid, self.ppid, self.started, self.command


def _ps_rows(pids: tuple[int, ...] | None) -> dict[int, Process]:
    command = ["/bin/ps"]
    if pids is None:
        command.append("-e")
    else:
        if not pids or any(pid < 1 for pid in pids):
            raise MonitorError("invalid targeted PID selection")
        command.extend(("-p", ",".join(str(pid) for pid in sorted(set(pids)))))
    command.extend((
        "-ww", "-o", "pid=", "-o", "ppid=", "-o", "rss=", "-o", "lstart=", "-o", "command=",
    ))
    result = subprocess.run(command, capture_output=True, text=True, timeout=5, check=False)
    if ((pids is None and result.returncode != 0)
            or (pids is not None and result.returncode not in (0, 1))
            or (pids is not None and result.returncode == 1 and result.stdout)
            or result.stderr or len(result.stdout) > 16 * 1024 * 1024):
        raise MonitorError("process snapshot unavailable or oversized")
    rows: dict[int, Process] = {}
    for line in result.stdout.splitlines():
        match = re.fullmatch(r"\s*(\d+)\s+(\d+)\s+(\d+)\s+(.{24})\s+(.+)", line)
        if match is None:
            raise MonitorError("malformed process snapshot")
        pid, ppid, rss, started, argv = match.groups()
        row = Process(int(pid), int(ppid), started.strip(), argv, int(rss))
        if row.pid < 1 or row.pid in rows or row.rss_kib < 0:
            raise MonitorError("duplicate or invalid process row")
        rows[row.pid] = row
    return rows


def ps_all() -> dict[int, Process]:
    return _ps_rows(None)


def ps_pids(pids: tuple[int, ...]) -> dict[int, Process]:
    return _ps_rows(pids)


def _descends_from(rows: Mapping[int, Process], pid: int, root_pid: int) -> bool:
    visited: set[int] = set()
    while pid in rows and pid not in visited:
        if pid == root_pid:
            return True
        visited.add(pid)
        pid = rows[pid].ppid
    return False


def _argv_after_executable(row: Process, executable: Path) -> list[str] | None:
    prefix = str(executable) + " "
    if not row.command.startswith(prefix):
        return None
    try:
        return shlex.split(row.command[len(prefix):])
    except ValueError as error:
        raise MonitorError("malformed selected process argv") from error


class NativeProcessMonitor:
    """Discover exact descendants, then sample only known PIDs until exit.

    Discovery is global every two seconds. Stable sampling targets the root,
    selected test and current direct daemon. Sparse compiler inventory is
    diagnostic only and is never used to authenticate a process.
    """

    def __init__(
        self,
        *,
        root_pid: int,
        test_executable: Path,
        daemon_executable: Path,
        selector: str,
        validate_config: Callable[[Path], str],
        validate_daemon_args: Callable[[list[str]], None] | None = None,
        snapshot_all: Callable[[], Mapping[int, Process]] = ps_all,
        snapshot_pids: Callable[[tuple[int, ...]], Mapping[int, Process]] = ps_pids,
        clock: Callable[[], float] = time.monotonic,
        sample_seconds: float = 2.0,
        compiler_seconds: float = 30.0,
        max_daemons: int = 8,
    ) -> None:
        if (root_pid < 1 or not selector or not test_executable.is_absolute()
                or not daemon_executable.is_absolute() or max_daemons < 1
                or not math.isfinite(sample_seconds) or sample_seconds < 1
                or not math.isfinite(compiler_seconds) or compiler_seconds < 30):
            raise ValueError("invalid monitor bounds")
        self.root_pid = root_pid
        self.test_executable = test_executable
        self.daemon_executable = daemon_executable
        self.selector = selector
        self.validate_config = validate_config
        self.validate_daemon_args = validate_daemon_args
        self.snapshot_all = snapshot_all
        self.snapshot_pids = snapshot_pids
        self.clock = clock
        self.sample_seconds = sample_seconds
        self.compiler_seconds = compiler_seconds
        self.max_daemons = max_daemons
        self.root: Process | None = None
        self.test: Process | None = None
        self.active: Process | None = None
        self.daemons: dict[int, dict] = {}
        self.next_sample = float("-inf")
        self.next_compilers = float("-inf")
        self.full_samples = 0
        self.target_samples = 0
        self.compiler_samples = 0
        self.compiler_last: dict[str, int] | None = None
        self.compiler_max: dict[str, int] | None = None
        self.rss_samples = 0
        self.rss_max_kib: int | None = None
        self.ended = False
        self.failure: str | None = None

    def _same(self, old: Process, new: Process | None, label: str) -> None:
        if new is None or new.identity() != old.identity():
            raise MonitorError(f"{label} PID/parent/start/argv identity changed or disappeared")

    def _discover(self, rows: Mapping[int, Process]) -> None:
        root = rows.get(self.root_pid)
        if root is None:
            raise MonitorError("owned admission root unavailable")
        if self.root is None:
            self.root = root
        else:
            self._same(self.root, root, "admission root")
        matches = [row for row in rows.values()
                   if (args := _argv_after_executable(row, self.test_executable)) is not None
                   and args.count(self.selector) == 1 and args.count("--exact") == 1
                   and _descends_from(rows, row.pid, self.root_pid)]
        if len(matches) > 1:
            raise MonitorError("multiple selected owned tests")
        if self.test is None:
            if not matches:
                return
            self.test = matches[0]
        else:
            previous = rows.get(self.test.pid)
            if previous is None:
                self.ended = True
                return
            self._same(self.test, previous, "selected test")
            if not matches:
                raise MonitorError("selected test left the owned process tree")
            self._same(self.test, matches[0], "selected test")
        candidates = [row for row in rows.values()
                      if row.ppid == self.test.pid
                      and (args := _argv_after_executable(row, self.daemon_executable)) is not None
                      and args[:1] == ["serve-native"]]
        if len(candidates) > 1:
            raise MonitorError("multiple direct owned daemons")
        if not candidates:
            return
        daemon = candidates[0]
        if daemon.pid in self.daemons:
            if self.daemons[daemon.pid]["process"] != daemon.identity():
                raise MonitorError("previous daemon PID reused")
            self.active = daemon
            return
        if len(self.daemons) >= self.max_daemons:
            raise MonitorError("owned daemon epoch limit exceeded")
        args = _argv_after_executable(daemon, self.daemon_executable)
        if args is None or args.count("--config") != 1:
            raise MonitorError("daemon config argument missing or duplicated")
        offset = args.index("--config") + 1
        if offset >= len(args) or args[offset].startswith("-"):
            raise MonitorError("daemon config argument has no path")
        config = Path(args[offset])
        if not config.is_absolute():
            raise MonitorError("daemon config path is not absolute")
        if self.validate_daemon_args is not None:
            self.validate_daemon_args(args)
        digest = self.validate_config(config)
        if not isinstance(digest, str) or re.fullmatch(r"[0-9a-f]{64}", digest) is None:
            raise MonitorError("config validator did not return a SHA-256 digest")
        self.active = daemon
        self.daemons[daemon.pid] = {
            "process": daemon.identity(), "config_path": str(config), "config_sha256": digest,
        }

    def _rss(self, rows: Mapping[int, Process]) -> None:
        if self.test is None or self.active is None:
            return
        test, daemon = rows.get(self.test.pid), rows.get(self.active.pid)
        if test is None or daemon is None:
            return
        total = test.rss_kib + daemon.rss_kib
        self.rss_samples += 1
        self.rss_max_kib = max(self.rss_max_kib or 0, total)

    def _host(self, rows: Mapping[int, Process], at: float) -> None:
        if at < self.next_compilers:
            return
        owned_pids = {self.root_pid, *self.daemons}
        if self.test is not None:
            owned_pids.add(self.test.pid)
        counts = {"cargo": 0, "rustc": 0}
        for row in rows.values():
            name = os.path.basename(row.command.split(maxsplit=1)[0])
            if row.pid not in owned_pids and name in counts:
                counts[name] += 1
        self.compiler_samples += 1
        self.compiler_last = counts.copy()
        self.compiler_max = (counts.copy() if self.compiler_max is None else
                             {name: max(self.compiler_max[name], counts[name]) for name in counts})
        self.next_compilers = at + self.compiler_seconds

    def sample(self) -> None:
        """Take one due sample; any custody failure permanently fails closed."""
        if self.failure is not None:
            raise MonitorError(self.failure)
        if self.ended:
            return
        at = self.clock()
        if at < self.next_sample:
            return
        try:
            if self.active is None or at >= self.next_compilers:
                rows = self.snapshot_all()
                self.full_samples += 1
                if self.active is not None:
                    if self.root is None or self.test is None:
                        raise MonitorError("stable sample lacks owned identities")
                    if rows.get(self.test.pid) is None and rows.get(self.active.pid) is None:
                        self.active = None
                        self.ended = True
                        return
                    self._same(self.root, rows.get(self.root_pid), "admission root")
                    self._same(self.test, rows.get(self.test.pid), "selected test")
                    observed = rows.get(self.active.pid)
                    if observed is None:
                        self.active = None
                    else:
                        self._same(self.active, observed, "direct daemon")
                self._discover(rows)
                self._rss(rows)
                self._host(rows, at)
            else:
                if self.test is None or self.root is None:
                    raise MonitorError("stable sample lacks owned identities")
                rows = self.snapshot_pids((self.root_pid, self.test.pid, self.active.pid))
                self.target_samples += 1
                if rows.get(self.test.pid) is None and rows.get(self.active.pid) is None:
                    self.active = None
                    self.ended = True
                    self.next_sample = at + self.sample_seconds
                    return
                self._same(self.root, rows.get(self.root_pid), "admission root")
                self._same(self.test, rows.get(self.test.pid), "selected test")
                observed = rows.get(self.active.pid)
                if observed is not None:
                    self._same(self.active, observed, "direct daemon")
                    self._rss(rows)
                else:
                    self.active = None
                    discovered = self.snapshot_all()
                    self.full_samples += 1
                    self._discover(discovered)
                    self._rss(discovered)
                    self._host(discovered, at)
            self.next_sample = at + self.sample_seconds
        except (OSError, subprocess.SubprocessError, ValueError, RuntimeError, TypeError) as error:
            self.failure = str(error)
            raise MonitorError(self.failure) from error

    def report(self) -> dict:
        return {
            "custody": "FAILED" if self.failure else "OBSERVED" if self.daemons else "NOT_OBSERVED",
            "failure": self.failure,
            "test_identity": None if self.test is None else self.test.identity(),
            "daemon_epochs": list(self.daemons.values()),
            "interval_seconds": self.sample_seconds,
            "compiler_interval_seconds": self.compiler_seconds,
            "full_snapshot_count": self.full_samples,
            "target_snapshot_count": self.target_samples,
            "compiler_sample_count": self.compiler_samples,
            "compiler_last": self.compiler_last,
            "compiler_max": self.compiler_max,
            "rss_sample_count": self.rss_samples,
            "rss_max_observed_aggregate_kib": self.rss_max_kib,
            "rss_qualification": "NOT_RUN",
            "limitation": "sampled owned test plus simultaneous direct daemon RSS lower bound; "
                          "short-lived and between-sample peaks and full host quiet are unobserved",
        }
