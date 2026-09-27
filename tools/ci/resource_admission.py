"""Cooperative, single-slot admission for leaf build and test commands."""

from __future__ import annotations

import argparse
import fcntl
import os
import signal
import stat
import sys
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[2]))
from tools.benchmark.producer_execution import ProducerExecutionError, _execute_owned


class AdmissionCancelled(Exception):
    def __init__(self, signum: int):
        self.signum = signum


class AdmissionTimeout(Exception):
    pass


class LiveSink:
    def __init__(self, stream):
        self.stream = stream

    def write(self, data: bytes) -> None:
        self.stream.write(data)
        self.stream.flush()


def positive_integer(value: str) -> int:
    parsed = int(value)
    if parsed < 1:
        raise argparse.ArgumentTypeError("must be a positive integer")
    return parsed


def check_lock(fd: int, path: Path) -> tuple:
    opened, named = os.fstat(fd), path.lstat()
    if (
        not stat.S_ISREG(opened.st_mode)
        or not stat.S_ISREG(named.st_mode)
        or opened.st_uid != os.getuid()
        or opened.st_nlink != 1
        or (opened.st_dev, opened.st_ino) != (named.st_dev, named.st_ino)
    ):
        raise ValueError("resource lock must be an owned, singly linked regular file")
    return opened.st_dev, opened.st_ino, opened.st_uid, opened.st_mode, opened.st_nlink


def run(lock: Path, wait_seconds: int, timeout_seconds: int, command: list[str]) -> int:
    """Retain the lease through owned group cleanup, including controller loss."""
    lock = Path(os.path.abspath(lock))
    lock.parent.mkdir(parents=True, exist_ok=True)
    fd = os.open(lock, os.O_RDWR | os.O_CREAT | os.O_NOFOLLOW | os.O_CLOEXEC, 0o600)
    waiting_since = time.monotonic_ns()
    acquired_at = None
    try:
        identity = check_lock(fd, lock)
        deadline = time.monotonic() + wait_seconds
        print(f"resource admission: waiting lock={lock}", file=sys.stderr, flush=True)
        while True:
            if time.monotonic() >= deadline:
                raise AdmissionTimeout("resource admission wait timed out")
            if check_lock(fd, lock) != identity:
                raise ValueError("resource lock identity changed while waiting")
            try:
                fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
                acquired_at = time.monotonic_ns()
                break
            except BlockingIOError:
                remaining = deadline - time.monotonic()
                if remaining <= 0:
                    raise AdmissionTimeout("resource admission wait timed out") from None
                time.sleep(min(0.05, remaining))
        if check_lock(fd, lock) != identity:
            raise ValueError("resource lock identity changed during admission")
        print(
            f"resource admission: admitted lock={lock} wait_ns={acquired_at - waiting_since}",
            file=sys.stderr,
            flush=True,
        )
        result = _execute_owned(
            command,
            cwd=Path.cwd(),
            env=dict(os.environ),
            timeout=timeout_seconds,
            sinks=(LiveSink(sys.stdout.buffer), LiveSink(sys.stderr.buffer)),
            custody_fds=(fd,),
        )
        if check_lock(fd, lock) != identity:
            raise ValueError("resource lock identity changed during execution")
        code = result["exit_code"]
        return code if code >= 0 else 128 - code
    finally:
        # Never unlink: existing waiters must all refer to the same inode.
        # Closing our copy follows guard cleanup; the guard independently keeps
        # its copy until group termination if this controller is killed.
        os.close(fd)
        released_at = time.monotonic_ns()
        if acquired_at is not None:
            # Includes child setup and group cleanup, not only child CPU/runtime.
            print(
                f"resource admission: released lock={lock} held_ns={released_at - acquired_at}",
                file=sys.stderr,
                flush=True,
            )
        else:
            print(
                f"resource admission: not-admitted lock={lock} wait_ns={released_at - waiting_since}",
                file=sys.stderr,
                flush=True,
            )


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--lock", required=True, type=Path)
    parser.add_argument("--wait-seconds", type=positive_integer, default=300)
    parser.add_argument("--timeout-seconds", type=positive_integer, default=7200)
    parser.add_argument("command", nargs=argparse.REMAINDER)
    args = parser.parse_args(argv)
    command = args.command[1:] if args.command[:1] == ["--"] else args.command
    if not command:
        parser.error("a leaf command is required after --")

    def cancel(signum, _frame):
        raise AdmissionCancelled(signum)

    previous = signal.signal(signal.SIGTERM, cancel)
    try:
        return run(args.lock, args.wait_seconds, args.timeout_seconds, command)
    except AdmissionCancelled as error:
        print("resource admission: interrupted while waiting", file=sys.stderr)
        return 128 + error.signum
    except KeyboardInterrupt:
        print("resource admission: interrupted", file=sys.stderr)
        return 130
    except AdmissionTimeout as error:
        print(f"resource admission: {error}", file=sys.stderr)
        return 124
    except ProducerExecutionError as error:
        print(f"resource admission: execution failed: {error}", file=sys.stderr)
        if "interrupted by SIGTERM" in str(error) or "interrupted during spawn" in str(error):
            return 143
        return 124 if "timed out" in str(error) else 126
    except (OSError, ValueError) as error:
        print(f"resource admission: setup or custody failed: {error}", file=sys.stderr)
        return 126
    finally:
        signal.signal(signal.SIGTERM, previous)


if __name__ == "__main__":
    raise SystemExit(main())
