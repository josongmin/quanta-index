"""Owned POSIX producer execution shared by benchmark adapters."""

from __future__ import annotations

import os
import signal
import subprocess
import threading
import time
from pathlib import Path

from evidence import EvidenceError


def execute(argv: list[str], *, cwd: Path, env: dict[str, str], timeout: int) -> tuple[bytes, bytes, dict]:
    if threading.current_thread() is not threading.main_thread():
        raise EvidenceError("benchmark execution requires the signal-owning main thread")
    if type(timeout) is not int or timeout < 1:
        raise EvidenceError("producer timeout must be a positive integer")
    started = time.monotonic_ns()
    process, interrupted = None, False

    def on_terminate(_signum, _frame):
        nonlocal interrupted
        interrupted = True
        if process is not None:
            raise EvidenceError("benchmark producer interrupted by SIGTERM")

    previous = signal.signal(signal.SIGTERM, on_terminate)
    try:
        process = subprocess.Popen(argv, cwd=cwd, env=env, stdout=subprocess.PIPE,
                                   stderr=subprocess.PIPE, start_new_session=True)
        try:
            if interrupted:
                raise EvidenceError("benchmark producer interrupted during spawn")
            stdout, stderr = process.communicate(timeout=timeout)
        except BaseException as exc:
            try:
                os.killpg(process.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            process.communicate()
            if isinstance(exc, subprocess.TimeoutExpired):
                raise EvidenceError(f"benchmark producer timed out: {argv!r}") from exc
            raise
    finally:
        signal.signal(signal.SIGTERM, previous)
    if process.returncode != 0:
        raise EvidenceError(f"benchmark producer failed ({process.returncode}): {stderr.decode(errors='replace')[-4000:]}")
    return stdout, stderr, {"argv": argv, "cwd": str(cwd), "status": "completed", "exit_code": 0,
                           "timeout_seconds": timeout, "wall_ms": (time.monotonic_ns() - started) // 1_000_000}
