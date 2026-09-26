"""POSIX process/thread exclusion for publication versus destructive GC."""

from __future__ import annotations

import contextlib
import functools
import os
import stat
import threading
from pathlib import Path

_guard = threading.Lock()
_locks: dict[str, threading.RLock] = {}
_local = threading.local()


@contextlib.contextmanager
def custody(root: Path):
    from evidence import EvidenceError

    try:
        import fcntl
    except ImportError as exc:
        raise EvidenceError("capture/GC custody requires POSIX file locking") from exc

    if root.is_symlink() or any(parent.is_symlink() for parent in root.absolute().parents):
        raise EvidenceError("custody root or ancestor is a symlink")

    root.mkdir(parents=True, exist_ok=True)
    key = str(root.resolve())
    with _guard:
        lock = _locks.setdefault(key, threading.RLock())
    with lock:
        depths = getattr(_local, "depths", {})
        _local.depths = depths
        if depths.get(key, 0):
            depths[key] += 1
            try:
                yield
            finally:
                depths[key] -= 1
            return
        descriptor = os.open(root / ".custody.lock", os.O_RDWR | os.O_CREAT | os.O_NOFOLLOW, 0o600)
        try:
            if not stat.S_ISREG(os.fstat(descriptor).st_mode):
                raise EvidenceError("custody lock is not a regular file")
            fcntl.flock(descriptor, fcntl.LOCK_EX)
            depths[key] = 1
            try:
                yield
            finally:
                del depths[key]
                fcntl.flock(descriptor, fcntl.LOCK_UN)
        finally:
            os.close(descriptor)


def publication(function):
    @functools.wraps(function)
    def wrapped(root: Path, *args, **kwargs):
        from profile_capture import _directories

        _directories(root)
        with custody(root):
            return function(root, *args, **kwargs)

    return wrapped
