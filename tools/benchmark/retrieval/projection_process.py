"""Bounded POSIX child I/O for candidate grammar projection helpers."""

from __future__ import annotations

import json
import os
import selectors
import signal
import stat
import subprocess
import time
from collections.abc import Callable
from pathlib import Path

HELPER_ENV = {"PATH": "/usr/bin:/bin"}


class ProjectionProcessError(ValueError):
    def __init__(self, kind: str) -> None:
        self.kind = kind
        super().__init__(kind)


def read_bounded_regular(path: Path, maximum: int) -> bytes:
    """Check and read the same opened file without blocking on a swapped FIFO."""
    fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    with os.fdopen(fd, "rb") as source:
        metadata = os.fstat(source.fileno())
        if not stat.S_ISREG(metadata.st_mode) or metadata.st_size > maximum:
            raise OSError("projection input must be a bounded regular file")
        data = source.read(maximum + 1)
    if len(data) > maximum:
        raise OSError("projection input exceeds its size limit")
    return data


def parse_helper_json(output: bytes) -> object:
    """Reject ambiguous object fields before validating the helper schema."""

    def unique_pairs(pairs: list[tuple[str, object]]) -> dict[str, object]:
        result: dict[str, object] = {}
        for key, value in pairs:
            if key in result:
                raise ValueError("duplicate JSON field")
            result[key] = value
        return result

    return json.loads(output, object_pairs_hook=unique_pairs)


def _terminate(child: subprocess.Popen[bytes]) -> None:
    try:
        os.killpg(child.pid, signal.SIGKILL)
    except ProcessLookupError:
        pass
    child.wait()
    for stream in (child.stdin, child.stdout, child.stderr):
        if stream is not None:
            stream.close()


def run_helper(
    command: list[str], source: bytes, *, timeout_seconds: float, max_stdout: int
) -> tuple[bytes, bytes, int]:
    """Own the group of an installed helper and any workers it starts."""
    deadline = time.monotonic() + timeout_seconds
    child = subprocess.Popen(
        command,
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        env=HELPER_ENV,
        close_fds=True,
        start_new_session=True,
    )
    return run_bounded(child, source, deadline=deadline, cancelled=None, max_stdout=max_stdout)


def run_bounded(
    child: subprocess.Popen[bytes],
    source: bytes,
    *,
    deadline: float,
    cancelled: Callable[[], bool] | None,
    max_stdout: int,
    max_stderr: int = 1 << 16,
) -> tuple[bytes, bytes, int]:
    """Drain both pipes concurrently; kill before either exceeds its cap."""
    assert child.stdin is not None and child.stdout is not None and child.stderr is not None
    output = bytearray()
    errors = bytearray()
    offset = 0
    with selectors.DefaultSelector() as selector:
        try:
            for stream, event, kind in (
                (child.stdin, selectors.EVENT_WRITE, "stdin"),
                (child.stdout, selectors.EVENT_READ, "stdout"),
                (child.stderr, selectors.EVENT_READ, "stderr"),
            ):
                os.set_blocking(stream.fileno(), False)
                selector.register(stream, event, kind)
            while selector.get_map() or child.poll() is None:
                if cancelled is not None and cancelled():
                    raise ProjectionProcessError("cancelled")
                remaining = deadline - time.monotonic()
                if remaining <= 0:
                    raise ProjectionProcessError("timeout")
                for key, _events in selector.select(timeout=min(remaining, 0.05)):
                    stream = key.fileobj
                    kind = key.data
                    if kind == "stdin":
                        if offset == len(source):
                            selector.unregister(stream)
                            stream.close()
                            continue
                        try:
                            wrote = os.write(stream.fileno(), source[offset : offset + 65536])
                        except BlockingIOError:
                            continue
                        except BrokenPipeError:
                            selector.unregister(stream)
                            stream.close()
                            continue
                        offset += wrote
                    else:
                        buffer, limit = (
                            (output, max_stdout) if kind == "stdout" else (errors, max_stderr)
                        )
                        try:
                            chunk = os.read(stream.fileno(), min(65536, limit - len(buffer) + 1))
                        except BlockingIOError:
                            continue
                        if not chunk:
                            selector.unregister(stream)
                            stream.close()
                            continue
                        buffer.extend(chunk)
                        if len(buffer) > limit:
                            raise ProjectionProcessError("resource_limit")
            return bytes(output), bytes(errors), child.wait()
        except BaseException:
            _terminate(child)
            raise
