"""Garbage-collect quanta-index Cargo target lanes under the shared cache root.

Layout (owned by scripts/quanta-index-env.sh):

    <cache_root>/target/<sha256(repo_root)[:16]>/<lane>/<profile>/...

Policies, applied in order:

1. Orphan checkout dirs: a hash dir that maps to no live checkout (main repo,
   `git worktree list`, the current repo root) is removed only when every lane
   in it is idle beyond ``--idle-hours``, no Cargo lock in it is held, and no
   running process references its path.
2. Idle lanes: inside live checkouts, lanes idle beyond ``--idle-hours`` are
   removed unless protected (default set, the current lane, ``--protect-lane``)
   or busy (held Cargo lock or a process referencing the lane path).
3. Stale artifacts: in every retained lane, ``<profile>/deps`` test executables
   and ``<profile>/incremental`` crate dirs that are older than
   ``--artifact-hours`` and are not the newest entry of their crate stem are
   removed while the lane's Cargo locks are held by this tool.
4. Optional size budget (``--max-total-gb``): the least recently active,
   non-protected, non-busy lanes are evicted until the target root fits.

Activity is the newest of: a ``.quanta-lane-last-used`` stamp that
scripts/cargow touches on every build-like invocation, Cargo lock files, and
the mtimes of the lane's shallow directory structure (lane, profile dirs and
their direct children such as ``deps``/``.fingerprint``/``incremental``).
Huge directories are never listed for activity, so a scan stays cheap.

Busy detection never deletes while Cargo holds ``.cargo-lock``,
``.cargo-build-lock`` or ``.cargo-artifact-lock`` (non-blocking ``flock``
probe; Cargo uses ``flock(2)`` on Unix). During a destructive step this tool
holds those locks, renames the victim into a ``.gc-trash-*`` sibling, and only
then deletes it, so a concurrent Cargo either waits on the lock or never sees
a half-deleted tree under the lane name.

A failure of this tool must never fail a build: scripts/cargow launches the
``--auto`` mode detached, rate-limited by a stamp file, with output redirected
to a log under the cache root.
"""

from __future__ import annotations

import argparse
import errno
import fcntl
import hashlib
import json
import os
import shutil
import subprocess
import sys
import time
from collections.abc import Callable, Iterable, Iterator
from contextlib import contextmanager
from dataclasses import dataclass, field
from pathlib import Path

SCHEMA = "quanta-index-target-gc-v1"
LANE_STAMP = ".quanta-lane-last-used"
TRASH_PREFIX = ".gc-trash-"
CARGO_LOCK_NAMES = frozenset({".cargo-lock", ".cargo-build-lock", ".cargo-artifact-lock"})
DEFAULT_PROTECTED_LANES = ("shared", "test-daemon-lane", "test-fast-lane", "test-workspace-lane")
# Directories whose entry count can be huge; their own mtime is observed but
# they are never listed during activity or lock discovery.
UNLISTED_DIRS = frozenset({"deps", ".fingerprint", "build", "incremental", "examples", "doc"})
SCAN_DEPTH = 3  # lane/<triple>/<profile>/<child>
HOUR = 3600.0


def default_cache_root() -> Path:
    explicit = os.environ.get("QUANTA_INDEX_CACHE_ROOT")
    if explicit:
        return Path(explicit)
    if sys.platform == "darwin":
        return Path.home() / "Library" / "Caches" / "quanta-index"
    return Path(os.environ.get("XDG_CACHE_HOME") or Path.home() / ".cache") / "quanta-index"


def checkout_id(path: str) -> str:
    """Mirror scripts/quanta-index-env.sh: sha256 of the checkout path, 16 hex."""
    return hashlib.sha256(path.encode()).hexdigest()[:16]


def checkout_path_variants(path: str) -> set[str]:
    # The env script hashes `cd ...; pwd` (a logical path), so a checkout may be
    # hashed through a symlinked spelling such as /tmp vs /private/tmp.
    variants = {path.rstrip("/") or "/"}
    try:
        variants.add(os.path.realpath(path))
    except OSError:
        pass
    for item in list(variants):
        if item.startswith("/private/"):
            variants.add(item[len("/private") :])
    return variants


def git_worktrees(repo_root: Path) -> tuple[list[str], str | None]:
    try:
        result = subprocess.run(
            ["git", "-C", str(repo_root), "worktree", "list", "--porcelain"],
            capture_output=True,
            text=True,
            timeout=30,
            check=False,
        )
    except (OSError, subprocess.SubprocessError) as exc:
        return [], f"git worktree list failed: {exc}"
    if result.returncode != 0:
        return [], f"git worktree list failed: {result.stderr.strip()}"
    paths = [
        line[len("worktree ") :]
        for line in result.stdout.splitlines()
        if line.startswith("worktree ")
    ]
    return paths, None


def known_checkout_ids(checkouts: Iterable[str]) -> dict[str, str]:
    mapping: dict[str, str] = {}
    for checkout in checkouts:
        if not os.path.isdir(checkout):
            continue
        for variant in checkout_path_variants(checkout):
            mapping.setdefault(checkout_id(variant), checkout)
    return mapping


@dataclass
class ProcessCensus:
    commands: list[str] | None  # None when the census could not be taken

    @classmethod
    def capture(cls) -> ProcessCensus:
        try:
            result = subprocess.run(
                ["ps", "-axww", "-o", "pid=,command="],
                capture_output=True,
                text=True,
                timeout=30,
                check=False,
            )
        except (OSError, subprocess.SubprocessError):
            return cls(None)
        if result.returncode != 0:
            return cls(None)
        own = os.getpid()
        commands = []
        for line in result.stdout.splitlines():
            pid, _, command = line.strip().partition(" ")
            if pid.isdigit() and int(pid) == own:
                continue
            commands.append(command)
        return cls(commands)

    def references(self, path: Path) -> bool | None:
        if self.commands is None:
            return None
        needle = str(path)
        resolved = os.path.realpath(path)
        return any(needle in command or resolved in command for command in self.commands)


def _lstat(path: Path) -> os.stat_result | None:
    try:
        return path.lstat()
    except OSError:
        return None


def _scandir(path: Path) -> list[os.DirEntry[str]]:
    try:
        with os.scandir(path) as entries:
            return list(entries)
    except OSError:
        return []


def _shallow_walk(
    root: Path, depth: int = SCAN_DEPTH
) -> Iterator[tuple[Path, os.stat_result, bool]]:
    """Yield (path, lstat, is_dir) for the lane's shallow structure."""
    for entry in _scandir(root):
        try:
            st = entry.stat(follow_symlinks=False)
        except OSError:
            continue
        is_dir = entry.is_dir(follow_symlinks=False)
        path = Path(entry.path)
        yield path, st, is_dir
        if is_dir and depth > 1 and entry.name not in UNLISTED_DIRS:
            yield from _shallow_walk(path, depth - 1)


def lane_activity(lane: Path) -> float:
    st = _lstat(lane)
    newest = st.st_mtime if st else 0.0
    for _path, child, _is_dir in _shallow_walk(lane):
        newest = max(newest, child.st_mtime)
    return newest


def dir_activity(path: Path) -> float:
    """Newest mtime of a directory and its direct children."""
    st = _lstat(path)
    newest = st.st_mtime if st else 0.0
    for entry in _scandir(path):
        try:
            newest = max(newest, entry.stat(follow_symlinks=False).st_mtime)
        except OSError:
            continue
    return newest


def lane_lock_files(lane: Path) -> list[Path]:
    return sorted(
        path
        for path, _st, is_dir in _shallow_walk(lane)
        if not is_dir and path.name in CARGO_LOCK_NAMES
    )


class LockBusy(Exception):
    pass


@contextmanager
def hold_locks(lock_files: Iterable[Path]) -> Iterator[None]:
    """Non-blocking exclusive flock on every Cargo lock; raise LockBusy if held."""
    held: list[int] = []
    try:
        for lock in lock_files:
            try:
                fd = os.open(lock, os.O_RDONLY | os.O_NOFOLLOW | os.O_CLOEXEC)
            except FileNotFoundError:
                continue
            held.append(fd)
            try:
                fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
            except OSError as exc:
                if exc.errno in (errno.EWOULDBLOCK, errno.EAGAIN, errno.EACCES):
                    raise LockBusy(str(lock)) from exc
                raise
        yield
    finally:
        for fd in held:
            os.close(fd)


def tree_usage(path: Path) -> tuple[int, int]:
    """Return (allocated bytes, file count) without following symlinks."""
    st = _lstat(path)
    if st is None:
        return 0, 0
    if not os.path.isdir(path) or os.path.islink(path):
        return st.st_blocks * 512, 1
    total, files = 0, 0
    stack = [str(path)]
    while stack:
        current = stack.pop()
        for entry in _scandir(Path(current)):
            try:
                child = entry.stat(follow_symlinks=False)
            except OSError:
                continue
            if entry.is_dir(follow_symlinks=False):
                stack.append(entry.path)
            else:
                total += child.st_blocks * 512
                files += 1
    return total, files


def remove_via_trash(path: Path) -> None:
    trash = path.with_name(f"{TRASH_PREFIX}{path.name}-{os.getpid()}-{time.time_ns()}")
    os.rename(path, trash)
    if trash.is_dir() and not trash.is_symlink():
        shutil.rmtree(trash, ignore_errors=True)
    else:
        trash.unlink(missing_ok=True)


@dataclass
class Report:
    dry_run: bool
    cache_root: str
    target_root: str
    actions: list[dict[str, object]] = field(default_factory=list)
    kept: list[dict[str, object]] = field(default_factory=list)
    errors: list[str] = field(default_factory=list)
    known_checkouts: dict[str, str] = field(default_factory=dict)
    total_before_bytes: int | None = None
    skipped: str | None = None

    def action(self, kind: str, path: Path, size: tuple[int, int], reason: str) -> None:
        self.actions.append(
            {"kind": kind, "path": str(path), "bytes": size[0], "files": size[1], "reason": reason}
        )

    def keep(self, kind: str, path: Path, reason: str) -> None:
        self.kept.append({"kind": kind, "path": str(path), "reason": reason})

    def to_json(self) -> dict[str, object]:
        by_kind: dict[str, dict[str, int]] = {}
        for action in self.actions:
            bucket = by_kind.setdefault(str(action["kind"]), {"count": 0, "bytes": 0, "files": 0})
            bucket["count"] += 1
            bucket["bytes"] += int(action["bytes"])  # type: ignore[arg-type]
            bucket["files"] += int(action["files"])  # type: ignore[arg-type]
        return {
            "schema": SCHEMA,
            "dry_run": self.dry_run,
            "skipped": self.skipped,
            "cache_root": self.cache_root,
            "target_root": self.target_root,
            "known_checkouts": self.known_checkouts,
            "total_before_bytes": self.total_before_bytes,
            "summary": {
                "by_kind": by_kind,
                "reclaim_bytes": sum(int(a["bytes"]) for a in self.actions),  # type: ignore[arg-type]
                "kept": len(self.kept),
                "errors": len(self.errors),
            },
            "actions": self.actions,
            "kept": self.kept,
            "errors": self.errors,
        }


@dataclass
class Policy:
    idle_seconds: float
    artifact_seconds: float
    protected_lanes: frozenset[str]
    prune_orphans: bool = True
    prune_idle_lanes: bool = True
    prune_artifacts: bool = True
    prune_incremental: bool = True
    max_total_bytes: int | None = None
    budget_min_idle_seconds: float = HOUR


@dataclass
class LaneInfo:
    path: Path
    checkout: str
    activity: float
    protected: bool


class Collector:
    def __init__(
        self,
        target_root: Path,
        known: dict[str, str],
        policy: Policy,
        report: Report,
        census: Callable[[], ProcessCensus],
        now: float,
    ) -> None:
        self.target_root = target_root
        self.known = known
        self.policy = policy
        self.report = report
        self.census_provider = census
        self.census = census()
        self.now = now
        self.dry_run = report.dry_run

    # -- helpers ---------------------------------------------------------
    def _busy_reason(self, path: Path, lock_files: list[Path]) -> str | None:
        referenced = self.census.references(path)
        if referenced is None:
            return "process census unavailable"
        if referenced:
            return "referenced by a running process"
        try:
            with hold_locks(lock_files):
                pass
        except LockBusy as exc:
            return f"cargo lock held: {exc}"
        except OSError as exc:
            return f"lock probe failed: {exc}"
        return None

    def _remove_tree(self, kind: str, path: Path, lock_files: list[Path], reason: str) -> bool:
        size = tree_usage(path)
        if self.dry_run:
            self.report.action(kind, path, size, reason)
            return True
        try:
            with hold_locks(lock_files):
                # Take a fresh process census while holding the locks.
                if self.census_provider().references(path) is not False:
                    self.report.keep(kind, path, "busy at removal time")
                    return False
                remove_via_trash(path)
        except LockBusy as exc:
            self.report.keep(kind, path, f"cargo lock held: {exc}")
            return False
        except OSError as exc:
            self.report.errors.append(f"remove {path}: {exc}")
            return False
        self.report.action(kind, path, size, reason)
        return True

    def _idle_hours(self, activity: float) -> float:
        return round((self.now - activity) / HOUR, 1)

    # -- phases ----------------------------------------------------------
    def sweep_trash(self) -> None:
        candidates = [
            Path(e.path) for e in _scandir(self.target_root) if e.name.startswith(TRASH_PREFIX)
        ]
        for hash_dir in _scandir(self.target_root):
            if hash_dir.is_dir(follow_symlinks=False):
                candidates += [
                    Path(e.path)
                    for e in _scandir(Path(hash_dir.path))
                    if e.name.startswith(TRASH_PREFIX)
                ]
        for path in candidates:
            size = tree_usage(path)
            if not self.dry_run:
                if path.is_dir() and not path.is_symlink():
                    shutil.rmtree(path, ignore_errors=True)
                else:
                    path.unlink(missing_ok=True)
            self.report.action("trash", path, size, "leftover from an interrupted gc")

    def lanes(self, hash_dir: Path, checkout: str) -> list[LaneInfo]:
        lanes = []
        for entry in _scandir(hash_dir):
            if entry.name.startswith(".") or not entry.is_dir(follow_symlinks=False):
                continue
            path = Path(entry.path)
            lanes.append(
                LaneInfo(
                    path=path,
                    checkout=checkout,
                    activity=lane_activity(path),
                    protected=entry.name in self.policy.protected_lanes,
                )
            )
        return lanes

    def collect_orphan(self, hash_dir: Path) -> bool:
        lanes = self.lanes(hash_dir, "")
        st = _lstat(hash_dir)
        activity = max([lane.activity for lane in lanes] + [st.st_mtime if st else 0.0])
        if self.now - activity < self.policy.idle_seconds:
            self.report.keep(
                "orphan", hash_dir, f"unknown checkout active {self._idle_hours(activity)}h ago"
            )
            return False
        locks = [lock for lane in lanes for lock in lane_lock_files(lane.path)]
        busy = self._busy_reason(hash_dir, locks)
        if busy:
            self.report.keep("orphan", hash_dir, busy)
            return False
        return self._remove_tree(
            "orphan",
            hash_dir,
            locks,
            f"no live checkout maps to this id; idle {self._idle_hours(activity)}h",
        )

    def collect_known(self, hash_dir: Path, checkout: str) -> list[LaneInfo]:
        retained = []
        for lane in self.lanes(hash_dir, checkout):
            idle = self.now - lane.activity
            if (
                self.policy.prune_idle_lanes
                and not lane.protected
                and idle >= self.policy.idle_seconds
            ):
                locks = lane_lock_files(lane.path)
                busy = self._busy_reason(lane.path, locks)
                if busy is None:
                    if self._remove_tree(
                        "idle-lane", lane.path, locks, f"idle {self._idle_hours(lane.activity)}h"
                    ):
                        continue
                else:
                    self.report.keep("idle-lane", lane.path, busy)
            retained.append(lane)
        return retained

    def prune_artifacts(self, lane: LaneInfo) -> None:
        locks = lane_lock_files(lane.path)
        try:
            with hold_locks(locks):
                for profile in self._profile_dirs(lane.path):
                    self._prune_deps(profile / "deps")
                    if self.policy.prune_incremental:
                        self._prune_incremental(profile / "incremental")
        except LockBusy as exc:
            self.report.keep("stale-artifacts", lane.path, f"cargo lock held: {exc}")
        except OSError as exc:
            self.report.errors.append(f"prune {lane.path}: {exc}")

    def _profile_dirs(self, lane: Path) -> list[Path]:
        return sorted(
            path.parent
            for path, _st, is_dir in _shallow_walk(lane)
            if is_dir and path.name in {"deps", "incremental"}
        )

    @staticmethod
    def _stem(name: str) -> str | None:
        stem, sep, suffix = name.rpartition("-")
        if not sep or len(suffix) != 16 or any(c not in "0123456789abcdef" for c in suffix):
            return None
        return stem

    def _stale_groups(self, entries: list[tuple[str, float]]) -> list[tuple[str, float]]:
        """Entries older than the threshold that are not the newest of their stem."""
        groups: dict[str, list[tuple[str, float]]] = {}
        for name, used in entries:
            stem = self._stem(name)
            if stem is not None:
                groups.setdefault(stem, []).append((name, used))
        stale = []
        for members in groups.values():
            members.sort(key=lambda item: item[1], reverse=True)
            stale += [m for m in members[1:] if self.now - m[1] >= self.policy.artifact_seconds]
        return stale

    def _prune_deps(self, deps: Path) -> None:
        executables = []
        for entry in _scandir(deps):
            if "." in entry.name or not entry.is_file(follow_symlinks=False):
                continue
            try:
                st = entry.stat(follow_symlinks=False)
            except OSError:
                continue
            if st.st_mode & 0o111:
                # atime advances when a test binary runs, so it counts as use.
                executables.append((entry.name, max(st.st_mtime, st.st_atime)))
        for name, used in self._stale_groups(executables):
            for victim in (deps / name, deps / f"{name}.d", deps / f"{name}.dSYM"):
                if _lstat(victim) is None:
                    continue
                size = tree_usage(victim)
                if not self.dry_run:
                    try:
                        if victim.is_dir() and not victim.is_symlink():
                            shutil.rmtree(victim)
                        else:
                            victim.unlink()
                    except OSError as exc:
                        self.report.errors.append(f"remove {victim}: {exc}")
                        continue
                self.report.action(
                    "stale-deps",
                    victim,
                    size,
                    f"superseded test executable; unused {self._idle_hours(used)}h",
                )

    def _prune_incremental(self, incremental: Path) -> None:
        dirs = []
        for entry in _scandir(incremental):
            if not entry.is_dir(follow_symlinks=False):
                continue
            dirs.append((entry.name, dir_activity(Path(entry.path))))
        for name, used in self._stale_groups(dirs):
            victim = incremental / name
            size = tree_usage(victim)
            if not self.dry_run:
                try:
                    remove_via_trash(victim)
                except OSError as exc:
                    self.report.errors.append(f"remove {victim}: {exc}")
                    continue
            self.report.action(
                "stale-incremental",
                victim,
                size,
                f"superseded incremental dir; idle {self._idle_hours(used)}h",
            )

    def enforce_budget(self, lanes: list[LaneInfo]) -> None:
        budget = self.policy.max_total_bytes
        assert budget is not None
        total = tree_usage(self.target_root)[0]
        if self.dry_run:
            total -= sum(int(a["bytes"]) for a in self.report.actions)  # type: ignore[arg-type]
        self.report.total_before_bytes = total
        if total <= budget:
            return
        candidates = sorted(
            (
                lane
                for lane in lanes
                if not lane.protected
                and self.now - lane.activity >= self.policy.budget_min_idle_seconds
            ),
            key=lambda lane: lane.activity,
        )
        for lane in candidates:
            if total <= budget:
                break
            locks = lane_lock_files(lane.path)
            busy = self._busy_reason(lane.path, locks)
            if busy:
                self.report.keep("budget-lane", lane.path, busy)
                continue
            size = tree_usage(lane.path)[0]
            if self._remove_tree(
                "budget-lane",
                lane.path,
                locks,
                f"size budget; idle {self._idle_hours(lane.activity)}h",
            ):
                total -= size
        if total > budget:
            self.report.errors.append(
                f"size budget not met: {total / 1e9:.1f} GB > {budget / 1e9:.1f} GB after eviction"
            )

    def run(self) -> None:
        if not self.target_root.is_dir():
            return
        self.sweep_trash()
        retained: list[LaneInfo] = []
        for entry in sorted(_scandir(self.target_root), key=lambda e: e.name):
            if entry.name.startswith(".") or not entry.is_dir(follow_symlinks=False):
                continue
            hash_dir = Path(entry.path)
            checkout = self.known.get(entry.name)
            if checkout is None:
                if self.policy.prune_orphans:
                    self.collect_orphan(hash_dir)
                continue
            retained += self.collect_known(hash_dir, checkout)
        if self.policy.prune_artifacts:
            for lane in retained:
                self.prune_artifacts(lane)
        if self.policy.max_total_bytes is not None:
            self.enforce_budget(retained)


def run_gc(
    cache_root: Path,
    checkouts: list[str],
    policy: Policy,
    *,
    dry_run: bool,
    census: Callable[[], ProcessCensus] | None = None,
    now: float | None = None,
) -> Report:
    target_root = cache_root / "target"
    report = Report(dry_run=dry_run, cache_root=str(cache_root), target_root=str(target_root))
    known = known_checkout_ids(checkouts)
    report.known_checkouts = dict(sorted(known.items()))
    Collector(
        target_root,
        known,
        policy,
        report,
        census or ProcessCensus.capture,
        time.time() if now is None else now,
    ).run()
    return report


@contextmanager
def gc_singleton(state_dir: Path) -> Iterator[bool]:
    state_dir.mkdir(parents=True, exist_ok=True)
    fd = os.open(state_dir / "gc.lock", os.O_RDWR | os.O_CREAT | os.O_CLOEXEC, 0o600)
    try:
        try:
            fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            yield False
            return
        yield True
    finally:
        os.close(fd)


def parse_args(argv: list[str] | None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--cache-root", type=Path, default=None)
    parser.add_argument("--repo-root", type=Path, default=Path(__file__).resolve().parents[2])
    parser.add_argument("--idle-hours", type=float, default=24.0)
    parser.add_argument("--artifact-hours", type=float, default=24.0)
    parser.add_argument(
        "--protect-lane", action="append", default=[], help="additional protected lane"
    )
    parser.add_argument("--max-total-gb", type=float, default=None)
    parser.add_argument("--budget-min-idle-hours", type=float, default=1.0)
    parser.add_argument("--no-orphans", action="store_true")
    parser.add_argument("--no-idle-lanes", action="store_true")
    parser.add_argument("--no-artifacts", action="store_true")
    parser.add_argument("--no-incremental", action="store_true")
    parser.add_argument("--dry-run", action="store_true")
    parser.add_argument("--json", action="store_true")
    parser.add_argument(
        "--auto",
        action="store_true",
        help="rate-limited background mode used by scripts/cargow; never exits non-zero",
    )
    parser.add_argument("--min-interval-hours", type=float, default=6.0)
    args = parser.parse_args(argv)
    for name in ("idle_hours", "artifact_hours", "min_interval_hours", "budget_min_idle_hours"):
        if getattr(args, name) < 0:
            parser.error(f"--{name.replace('_', '-')} must be non-negative")
    return args


def _print(report: Report, as_json: bool) -> None:
    payload = report.to_json()
    if as_json:
        print(json.dumps(payload, indent=2))
        return
    verb = "would reclaim" if report.dry_run else "reclaimed"
    summary = payload["summary"]
    assert isinstance(summary, dict)
    if report.skipped:
        print(f"target-gc: skipped ({report.skipped})")
        return
    print(f"target-gc: {verb} {summary['reclaim_bytes'] / 1e9:.1f} GB in {report.target_root}")
    for kind, bucket in sorted(summary["by_kind"].items()):
        print(f"  {kind:18} {bucket['count']:6} entries  {bucket['bytes'] / 1e9:8.1f} GB")
    print(f"  kept {summary['kept']} busy/recent candidates; {summary['errors']} errors")
    for error in report.errors:
        print(f"  error: {error}")


def main(argv: list[str] | None = None) -> int:
    args = parse_args(argv)
    cache_root = args.cache_root or default_cache_root()
    state_dir = cache_root / "target-gc"
    protected = set(DEFAULT_PROTECTED_LANES) | set(args.protect_lane)
    current = os.environ.get("QUANTA_INDEX_BUILD_LANE")
    if current:
        protected.add(current)
    policy = Policy(
        idle_seconds=args.idle_hours * HOUR,
        artifact_seconds=args.artifact_hours * HOUR,
        protected_lanes=frozenset(protected),
        prune_orphans=not args.no_orphans,
        prune_idle_lanes=not args.no_idle_lanes,
        prune_artifacts=not args.no_artifacts,
        prune_incremental=not args.no_incremental,
        max_total_bytes=None if args.max_total_gb is None else int(args.max_total_gb * 1e9),
        budget_min_idle_seconds=args.budget_min_idle_hours * HOUR,
    )
    try:
        with gc_singleton(state_dir) as acquired:
            if not acquired:
                report = Report(args.dry_run, str(cache_root), str(cache_root / "target"))
                report.skipped = "another target gc is running"
                _print(report, args.json)
                return 0
            stamp = state_dir / "last-run"
            if args.auto and not args.dry_run:
                st = _lstat(stamp)
                if st and time.time() - st.st_mtime < args.min_interval_hours * HOUR:
                    return 0
                stamp.touch()
                try:
                    os.nice(10)
                except OSError:
                    pass
            worktrees, error = git_worktrees(args.repo_root)
            checkouts = [str(args.repo_root), *worktrees]
            if os.environ.get("QUANTA_INDEX_REPO_ROOT"):
                checkouts.append(os.environ["QUANTA_INDEX_REPO_ROOT"])
            if error:
                # Without the worktree list every sibling checkout would look
                # orphaned; disable orphan removal instead of guessing.
                policy.prune_orphans = False
            report = run_gc(cache_root, checkouts, policy, dry_run=args.dry_run)
            if error:
                report.errors.append(f"{error}; orphan removal disabled")
    except Exception as exc:  # noqa: BLE001 - auto mode must never surface as a build failure
        if args.auto:
            print(f"target-gc: failed: {exc}", file=sys.stderr)
            return 0
        raise
    _print(report, args.json)
    return 0 if args.auto or not report.errors else 1


if __name__ == "__main__":
    raise SystemExit(main())
