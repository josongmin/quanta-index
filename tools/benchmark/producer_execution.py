"""Owned POSIX producer execution shared by benchmark adapters."""

from __future__ import annotations

import os
import selectors
import shutil
import signal
import stat
import struct
import subprocess
import sys
import tempfile
import threading
import time
from contextlib import ExitStack, contextmanager
from pathlib import Path
from typing import TYPE_CHECKING, NamedTuple

if TYPE_CHECKING:
    from tools.benchmark.evidence import RawFile

CLEANUP_TIMEOUT_SECONDS = 10


class ProducerExecutionError(ValueError):
    """A producer did not complete under its execution/cleanup contract."""


class ExecutionResult(NamedTuple):
    stdout: RawFile
    stderr: RawFile
    command: dict


@contextmanager
def transient_log_dir(prefix: str):
    """Yield a fresh external log directory retained only for failures.

    Successful executions are fully consumed inside the block, so their raw
    files are removed on exit. Any exception (execution or consumption)
    retains the directory because failure notes cite its path as evidence.
    """
    log_dir = Path(tempfile.mkdtemp(prefix=prefix)).resolve()
    yield log_dir
    shutil.rmtree(log_dir)


@contextmanager
def _child_exit_notifications():
    """Wake the private guard on SIGCHLD, including exits during Popen.

    The pipe is installed before spawning and is nonblocking at both ends.
    A full pipe already represents a pending wakeup; terminal status is always
    obtained from waitpid through Popen.poll, never from the notification byte.
    """
    read_fd, write_fd = os.pipe()
    try:
        os.set_blocking(read_fd, False)
        os.set_blocking(write_fd, False)
        previous_handler = signal.signal(signal.SIGCHLD, lambda *_: None)
        previous_wakeup = None
        previous_mask = None
        try:
            previous_wakeup = signal.set_wakeup_fd(write_fd, warn_on_full_buffer=False)
            # Popen inherits the controlling thread's signal mask. A blocked
            # SIGCHLD must not turn an exited child into a timeout.
            previous_mask = signal.pthread_sigmask(signal.SIG_UNBLOCK, {signal.SIGCHLD})
            yield read_fd
        finally:
            try:
                if previous_mask is not None:
                    signal.pthread_sigmask(signal.SIG_SETMASK, previous_mask)
                if previous_wakeup is not None:
                    signal.set_wakeup_fd(previous_wakeup)
            finally:
                signal.signal(signal.SIGCHLD, previous_handler)
    finally:
        os.close(read_fd)
        os.close(write_fd)


def _owned_child(lifeline: int, terminal: int, argv: list[str]) -> int:
    """Keep nested groups tied to their actual parent's descriptor lifetime."""
    if os.getpid() != os.getpgrp() or os.getpid() != os.getsid(0):
        raise ProducerExecutionError("owned child requires its private session")
    if (
        lifeline < 3
        or terminal < 3
        or lifeline == terminal
        or not stat.S_ISFIFO(os.fstat(lifeline).st_mode)
        or not stat.S_ISFIFO(os.fstat(terminal).st_mode)
        or not argv
    ):
        raise ProducerExecutionError("owned child requires distinct pipes and a command")

    with _child_exit_notifications() as notifications, selectors.DefaultSelector() as watch:
        watch.register(lifeline, selectors.EVENT_READ)
        watch.register(notifications, selectors.EVENT_READ)

        def parent_gone(timeout: float | None = 0) -> bool:
            for key, _ in watch.select(timeout):
                if key.fd == lifeline:
                    if os.read(lifeline, 1):
                        raise ProducerExecutionError("unexpected parent-liveness pipe data")
                    return True
                if key.fd == notifications:
                    # Coalesced or unrelated SIGCHLD events only request another
                    # poll. Drain them without creating a busy-loop or treating
                    # stopped/continued children as completed producers.
                    while True:
                        try:
                            if not os.read(notifications, 4096):
                                raise ProducerExecutionError("child notification pipe closed")
                        except BlockingIOError:
                            break
            return False

        # Never launch work after the controlling descriptor has already closed.
        if parent_gone():
            return 125
        child = subprocess.Popen(argv, close_fds=True)
        reported = False
        try:
            while True:
                if parent_gone():
                    # This live session leader pins the group identity. Descendant
                    # owners observe EOF too, so nested owned sessions cascade.
                    os.killpg(os.getpid(), signal.SIGKILL)
                result = None if reported else child.poll()
                if result is not None:
                    # Publish only the direct child's actual terminal status. The
                    # controller keeps this leader's PID unreaped until it kills
                    # the group. This leader stays alive until that kill or loss
                    # of its controller, so a terminal record is never mistaken
                    # for proof that background descendants have terminated.
                    record = struct.pack("!i", result)
                    if os.write(terminal, record) != len(record):
                        raise ProducerExecutionError("incomplete child terminal record")
                    reported = True
                # Both exit and controller loss wake this wait immediately.
                # The guard remains alive after publishing, pinning the group
                # identity until the controller performs custody cleanup.
                if parent_gone(None):
                    os.killpg(os.getpid(), signal.SIGKILL)
        finally:
            # Unexpected guard failure also terminates its owned group. It has no
            # valid terminal record and therefore cannot establish completion.
            os.killpg(os.getpid(), signal.SIGKILL)


def _output_streams(process: subprocess.Popen, sinks: tuple) -> tuple:
    # Keep the Rust front door usable with the system Python 3.9 as well as
    # the project interpreter. There are exactly two streams, never truncation.
    if len(sinks) != 2:
        raise ProducerExecutionError("owned execution requires exactly two output sinks")
    return ((process.stdout, sinks[0]), (process.stderr, sinks[1]))


def _wait_for_terminal(
    process: subprocess.Popen, terminal: int, sinks: tuple, timeout: float
) -> bytes:
    """Drain pipes without reaping the group leader before custody cleanup.

    Even an externally killed leader retains its child PID until the
    controller calls wait. No poll/wait/communicate may precede group kill:
    otherwise a replacement group could reuse its identity.
    """
    deadline = time.monotonic() + timeout
    output = {stream.fileno(): (stream, sink) for stream, sink in _output_streams(process, sinks)}
    with selectors.DefaultSelector() as watch:
        for fd in (terminal, *output):
            watch.register(fd, selectors.EVENT_READ)
        while True:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise subprocess.TimeoutExpired("owned producer terminal", timeout)
            for key, _ in watch.select(remaining):
                fd = key.fd
                block = os.read(fd, 5 if fd == terminal else 65536)
                if fd == terminal:
                    return block
                if block:
                    output[fd][1].write(block)
                else:
                    watch.unregister(fd)
                    output[fd][0].close()
                    del output[fd]


def _drain_output(process: subprocess.Popen, sinks: tuple, timeout: float) -> None:
    """Drain the remaining pipe bytes after group kill without whole-output buffers."""
    deadline = time.monotonic() + timeout
    with selectors.DefaultSelector() as watch:
        for stream, sink in _output_streams(process, sinks):
            if not stream.closed:
                watch.register(stream, selectors.EVENT_READ, sink)
        while watch.get_map():
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise subprocess.TimeoutExpired("owned producer pipe drain", timeout)
            for key, _ in watch.select(remaining):
                block = os.read(key.fd, 65536)
                if block:
                    key.data.write(block)
                else:
                    watch.unregister(key.fileobj)
                    key.fileobj.close()


def _child_result(raw: bytes) -> int:
    if len(raw) != 4:
        raise ProducerExecutionError("owned producer terminated without a valid terminal record")
    result = struct.unpack("!i", raw)[0]
    if result < -(signal.NSIG - 1) or result > 255:
        raise ProducerExecutionError("owned producer supplied an invalid exit code")
    return result


def _terminal_result(returncode: int, raw: bytes) -> int:
    """Interpret a private, bounded record only after group-owner termination."""
    if returncode != -signal.SIGKILL:
        raise ProducerExecutionError("owned producer terminated without a valid terminal record")
    return _child_result(raw)


def _cleanup(process: subprocess.Popen, sinks: tuple) -> str | None:
    """Bound pipe drain and direct-child reaping after killing the owned group.

    A process that escapes this group is not contained. A retained pipe must
    still produce an explicit incomplete-cleanup failure, never an endless
    wait or completed execution. Repeated cancellation cannot abort cleanup.
    """
    deadline = time.monotonic() + CLEANUP_TIMEOUT_SECONDS
    previous = {sig: signal.signal(sig, signal.SIG_IGN) for sig in (signal.SIGTERM, signal.SIGINT)}
    errors = []
    try:
        if getattr(process, "returncode", None) is None:
            try:
                os.killpg(process.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            except OSError as error:
                errors.append(f"process-group kill failed: {error}")
        drained = False
        try:
            _drain_output(process, sinks, max(0.0, deadline - time.monotonic()))
            drained = True
        except subprocess.TimeoutExpired:
            errors.append("pipe drain deadline exceeded")
        except (OSError, ValueError) as error:
            errors.append(f"pipe drain failed: {error}")
        if not drained:
            for stream in (process.stdout, process.stderr):
                if stream is not None:
                    try:
                        stream.close()
                    except OSError as error:
                        errors.append(f"pipe close failed: {error}")
        try:
            process.wait(timeout=max(0.0, deadline - time.monotonic()))
        except subprocess.TimeoutExpired:
            errors.append("direct-child reap deadline exceeded")
        except OSError as error:
            errors.append(f"direct-child reap failed: {error}")
        return "; ".join(errors) if errors else None
    finally:
        for sig, handler in previous.items():
            signal.signal(sig, handler)


def _execute_owned(
    argv: list[str],
    *,
    cwd: Path,
    env: dict[str, str],
    timeout: int,
    sinks: tuple,
    custody_fds: tuple[int, ...] = (),
) -> dict:
    if os.name != "posix" or not hasattr(os, "killpg"):
        raise ProducerExecutionError("benchmark execution requires POSIX process-group custody")
    if threading.current_thread() is not threading.main_thread():
        raise ProducerExecutionError("benchmark execution requires the signal-owning main thread")
    if signal.getsignal(signal.SIGCHLD) != signal.SIG_DFL:
        raise ProducerExecutionError("benchmark execution requires default SIGCHLD reaping custody")
    if type(timeout) is not int or timeout < 1:
        raise ProducerExecutionError("producer timeout must be a positive integer")
    if len(sinks) != 2:
        raise ProducerExecutionError("owned execution requires exactly two output sinks")
    if (
        type(custody_fds) is not tuple
        or any(type(fd) is not int or fd < 3 for fd in custody_fds)
        or len(set(custody_fds)) != len(custody_fds)
    ):
        raise ProducerExecutionError("custody descriptors must be distinct open nonstandard FDs")
    for fd in custody_fds:
        os.fstat(fd)
    started = time.monotonic_ns()
    deadline = time.monotonic() + timeout
    process, interrupted, cleaning, communicated = None, False, False, False
    lifeline_read, lifeline_write = os.pipe()
    try:
        terminal_read, terminal_write = os.pipe()
    except BaseException:
        os.close(lifeline_read)
        os.close(lifeline_write)
        raise

    def on_terminate(_signum, _frame):
        nonlocal interrupted
        if interrupted:
            return
        interrupted = True
        if process is not None and not cleaning and not communicated:
            raise ProducerExecutionError("benchmark producer interrupted by SIGTERM")

    previous = signal.signal(signal.SIGTERM, on_terminate)
    try:
        os.set_blocking(terminal_read, False)
        process = subprocess.Popen(
            [
                sys.executable,
                "-I",
                str(Path(__file__).resolve()),
                "--owned-child",
                str(lifeline_read),
                str(terminal_write),
                *argv,
            ],
            cwd=cwd,
            env=env,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            start_new_session=True,
            # The group guard retains leases if this controller dies. Its
            # close_fds=True child launch never leaks leases into the command.
            pass_fds=(lifeline_read, terminal_write, *custody_fds),
        )
        os.close(lifeline_read)
        lifeline_read = None
        os.close(terminal_write)
        terminal_write = None
        if interrupted:
            raise ProducerExecutionError("benchmark producer interrupted during spawn")
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            raise subprocess.TimeoutExpired(argv, timeout)
        terminal_raw = _wait_for_terminal(process, terminal_read, sinks, remaining)
        _child_result(terminal_raw)
        # Never reap/poll the leader before this kill. Its unreaped PID pins
        # group identity even if it was killed between reporting and cleanup.
        os.killpg(process.pid, signal.SIGKILL)
        _drain_output(process, sinks, max(0.0, deadline - time.monotonic()))
        process.wait(timeout=max(0.0, deadline - time.monotonic()))
        communicated = True
    except BaseException as exc:
        cleaning = True
        cleanup_error = (
            _cleanup(process, sinks) if process is not None and not communicated else None
        )
        primary = (
            f"benchmark producer timed out: {argv!r}"
            if isinstance(exc, subprocess.TimeoutExpired)
            else str(exc)
        )
        if cleanup_error is not None:
            raise ProducerExecutionError(f"{primary}; incomplete cleanup: {cleanup_error}") from exc
        if isinstance(exc, subprocess.TimeoutExpired):
            raise ProducerExecutionError(primary) from exc
        raise
    finally:
        signal.signal(signal.SIGTERM, previous)
        if lifeline_read is not None:
            os.close(lifeline_read)
        os.close(lifeline_write)
        if terminal_write is not None:
            os.close(terminal_write)
        os.close(terminal_read)
    result = _terminal_result(process.returncode, terminal_raw)
    if interrupted:
        raise ProducerExecutionError("benchmark producer interrupted by SIGTERM")
    return {
        "argv": argv,
        "cwd": str(cwd),
        "status": "completed",
        "exit_code": result,
        "timeout_seconds": timeout,
        "wall_ms": (time.monotonic_ns() - started) // 1_000_000,
    }


def execute(
    argv: list[str],
    *,
    cwd: Path,
    env: dict[str, str],
    timeout: int,
    log_dir: Path,
    custody_fds: tuple[int, ...] = (),
) -> ExecutionResult:
    """Retain bounded file-backed output for every launched execution epoch.

    The caller owns a fresh external log directory. Failed executions never
    return a success value; their raw files and execution record remain there.
    """
    # Keep the isolated owned-child entrypoint independent of import search paths.
    if __package__:
        from .evidence import RawWriter, canonical_json, write_raw_file
    else:
        from evidence import RawWriter, canonical_json, write_raw_file

    with ExitStack() as stack:
        sinks = tuple(
            stack.enter_context(RawWriter(log_dir / name)) for name in ("stdout", "stderr")
        )
        command, primary = None, None
        try:
            command = _execute_owned(
                argv,
                cwd=cwd,
                env=env,
                timeout=timeout,
                sinks=sinks,
                custody_fds=custody_fds,
            )
        except BaseException as error:
            primary = error
        refs, failures = [], []
        for sink in sinks:
            try:
                refs.append(sink.finish())
            except (OSError, ValueError) as error:
                refs.append(None)
                failures.append(f"{sink.path.name}: {error}")
        if primary is None and command is not None and command["exit_code"] != 0:
            detail = ""
            if refs[1] is not None:
                try:
                    detail = refs[1].tail(4000).decode(errors="replace")
                except (OSError, ValueError) as error:
                    failures.append(f"stderr tail: {error}")
            primary = ProducerExecutionError(
                f"benchmark producer failed with exit {command['exit_code']}: {detail}"
            )
        failed = primary is not None or bool(failures)
        record = {
            "status": "failed" if failed else "completed",
            "command": command,
            "request": {"argv": argv, "cwd": str(cwd), "timeout_seconds": timeout},
            "error_type": type(primary).__name__ if primary is not None else None,
            "output_errors": failures,
            "raw": [
                {
                    "path": str(sink.path),
                    "sha256": ref.sha256 if ref else None,
                    "bytes": ref.size if ref else None,
                }
                for sink, ref in zip(sinks, refs, strict=True)
            ],
        }
        try:
            write_raw_file(log_dir / "execution.json", [canonical_json(record).encode()])
        except (OSError, ValueError) as error:
            failures.append(f"execution record: {error}")
        if failures:
            raise ProducerExecutionError(
                f"{primary or 'producer output could not be sealed'}; retained logs at {log_dir}; "
                + "; ".join(failures)
            ) from primary
        if primary is not None:
            primary.add_note(f"retained execution logs: {log_dir}")
            raise primary
        return ExecutionResult(refs[0], refs[1], command)


if __name__ == "__main__":
    if len(sys.argv) < 5 or sys.argv[1] != "--owned-child":
        raise SystemExit("internal owned-child entrypoint only")
    raise SystemExit(_owned_child(int(sys.argv[2]), int(sys.argv[3]), sys.argv[4:]))
