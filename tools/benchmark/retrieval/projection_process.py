"""Bounded POSIX child I/O for candidate grammar projection helpers."""

from __future__ import annotations

import os
import selectors
import signal
import subprocess
import time
from collections.abc import Callable


class ProjectionProcessError(ValueError):
    def __init__(self, kind: str) -> None:
        self.kind = kind
        super().__init__(kind)


def _terminate(child: subprocess.Popen[bytes]) -> None:
    try:
        os.killpg(child.pid, signal.SIGKILL)
    except ProcessLookupError:
        pass
    child.wait()
    for stream in (child.stdin, child.stdout, child.stderr):
        if stream is not None:
            stream.close()


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
                        except BrokenPipeError:
                            selector.unregister(stream)
                            stream.close()
                            continue
                        offset += wrote
                    else:
                        buffer, limit = (
                            (output, max_stdout) if kind == "stdout" else (errors, max_stderr)
                        )
                        chunk = os.read(stream.fileno(), min(65536, limit - len(buffer) + 1))
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
