"""Experimental Linux filesystem isolation for a retrieval benchmark child.

Invoke in a fresh, single-threaded process::

    python3 -m tools.benchmark.retrieval.linux_isolation \
      --policy /absolute/policy.json -- /absolute/runner arg...

Policy JSON has exactly ``readonly``, ``writable``, and ``denied`` arrays of
absolute, existing paths.  The denied paths are checked against every grant;
all paths without a grant are denied by Landlock.  The caller must supply the
runner, loader, libraries, query pack and corpus as narrow readonly grants,
and its output directory as a writable grant.  Keep the mount topology and
granted trees immutable during a run: bind mounts and pre-existing hard links
can alias a denied file.  Landlock does not confine all metadata operations,
UDP, or Unix sockets at ABI 5.  Stdio pipes remain trusted caller channels.
This module does not independently qualify a retrieval run.
"""

from __future__ import annotations

import argparse
import ctypes
import errno
import hashlib
import json
import os
import platform
import stat
import sys
from pathlib import Path

BACKEND = "linux-landlock-fs-prototype-v1"
MIN_ABI = 5
EXIT_UNAVAILABLE = 78
EXIT_REJECTED = 70
_SYS = {"x86_64": (444, 445, 446), "aarch64": (444, 445, 446), "riscv64": (444, 445, 446)}
_READ = (1 << 0) | (1 << 2) | (1 << 3)  # execute, read file, read directory
_WRITE = sum(1 << bit for bit in (1, 2, 4, 5, 6, 7, 8, 9, 10, 11, 12, 14))
_HANDLED_FS = sum(1 << bit for bit in range(16))
_HANDLED_NET = (1 << 0) | (1 << 1)  # deny TCP bind/connect, no port grants
_O_PATH = getattr(os, "O_PATH", 0)


class IsolationError(RuntimeError):
    """The child must not execute without the requested restriction."""


class IsolationUnavailable(IsolationError):
    """This host cannot enforce the minimum policy."""


class _Ruleset(ctypes.Structure):
    _fields_ = [("handled_access_fs", ctypes.c_uint64), ("handled_access_net", ctypes.c_uint64)]


class _PathBeneath(ctypes.Structure):
    _pack_ = 1  # UAPI struct has no trailing padding
    _fields_ = [("allowed_access", ctypes.c_uint64), ("parent_fd", ctypes.c_int32)]


def _syscall(index: int, *args: object) -> int:
    numbers = _SYS.get(platform.machine())
    if sys.platform != "linux" or numbers is None:
        raise IsolationUnavailable("Landlock requires a supported Linux architecture")
    libc = ctypes.CDLL(None, use_errno=True)
    libc.syscall.restype = ctypes.c_long
    result = libc.syscall(numbers[index], *args)
    if result < 0:
        error = ctypes.get_errno()
        raise OSError(error, os.strerror(error))
    return int(result)


def probe() -> dict[str, object]:
    """Return explicit availability; never interpret an unknown ABI as support."""
    try:
        abi = _syscall(0, ctypes.c_void_p(), 0, 1)  # CREATE_RULESET_VERSION
    except IsolationUnavailable as exc:
        return {"backend": BACKEND, "state": "unavailable", "reason": str(exc)}
    except OSError as exc:
        state = (
            "unavailable" if exc.errno in {errno.ENOSYS, errno.EOPNOTSUPP, errno.EPERM} else "error"
        )
        return {"backend": BACKEND, "state": state, "reason": str(exc)}
    if abi < MIN_ABI:
        return {
            "backend": BACKEND,
            "state": "unavailable",
            "abi": abi,
            "reason": f"Landlock ABI {MIN_ABI} or newer required",
        }
    return {"backend": BACKEND, "state": "available", "abi": abi}


def _path(value: object) -> Path:
    if not isinstance(value, str) or not value or not value.startswith("/"):
        raise IsolationError("policy paths must be absolute strings")
    path = Path(value)
    try:
        canonical = path.resolve(strict=True)
    except (OSError, RuntimeError) as exc:
        raise IsolationError(f"policy path is missing or invalid: {value}") from exc
    if path == Path("/") or canonical != path:
        raise IsolationError(f"policy path is root, missing, or noncanonical: {value}")
    return path


def validate_policy(raw: object) -> dict[str, tuple[Path, ...]]:
    if not isinstance(raw, dict) or set(raw) != {"readonly", "writable", "denied"}:
        raise IsolationError("policy requires exactly readonly, writable, denied")
    paths: dict[str, tuple[Path, ...]] = {}
    for key in ("readonly", "writable", "denied"):
        entries = raw[key]
        if not isinstance(entries, list) or not entries:
            raise IsolationError(f"policy.{key} must be a nonempty array")
        resolved = tuple(_path(item) for item in entries)
        if len(set(resolved)) != len(resolved):
            raise IsolationError(f"policy.{key} contains duplicate paths")
        paths[key] = resolved
    for root in paths["readonly"] + paths["writable"]:
        if root == Path("/proc") or Path("/proc") in root.parents:
            raise IsolationError("/proc cannot be granted")
        if any(
            denied == root or root in denied.parents or denied in root.parents
            for denied in paths["denied"]
        ):
            raise IsolationError(f"grant overlaps a denied path: {root}")
    for root in paths["writable"]:
        if not root.is_dir() and root != Path("/dev/null"):
            raise IsolationError(f"writable grant must be a directory or /dev/null: {root}")
    return paths


def _audit_fds(policy: dict[str, tuple[Path, ...]], attest_fd: int | None = None) -> None:
    """Pre-open descriptors bypass Landlock; accept only controlled stdio."""
    try:
        fds = sorted(int(name) for name in os.listdir("/proc/self/fd"))
        tasks = os.listdir("/proc/self/task")
    except OSError as exc:
        raise IsolationUnavailable("/proc fd/task inspection is required") from exc
    if len(tasks) != 1:
        raise IsolationError("child must have exactly one thread")
    for fd in fds:
        try:
            mode = os.fstat(fd).st_mode
        except OSError:  # listdir's own directory descriptor has closed
            continue
        if fd == attest_fd and fd > 2 and stat.S_ISFIFO(mode):
            continue
        if fd > 2:
            raise IsolationError(f"unexpected inherited descriptor: {fd}")
        if stat.S_ISFIFO(mode):
            continue
        if stat.S_ISCHR(mode) and os.readlink(f"/proc/self/fd/{fd}") == "/dev/null":
            continue
        if fd in (1, 2) and stat.S_ISREG(mode):
            target = Path(os.readlink(f"/proc/self/fd/{fd}"))
            if target.is_absolute() and any(
                target == root or root in target.parents for root in policy["writable"]
            ):
                continue
        raise IsolationError(f"stdio descriptor {fd} is not a pipe or /dev/null")


def _add_grant(ruleset_fd: int, path: Path, writable: bool) -> None:
    access = _READ | (_WRITE if writable else 0)
    if path == Path("/dev/null"):
        access = (1 << 2) | (1 << 1)  # read and write, never execute a device
    elif not path.is_dir():
        access &= (1 << 0) | (1 << 1) | (1 << 2) | (1 << 14)
    fd = os.open(path, _O_PATH | os.O_CLOEXEC)
    try:
        rule = _PathBeneath(access, fd)
        _syscall(1, ruleset_fd, 1, ctypes.byref(rule), 0)  # PATH_BENEATH
    finally:
        os.close(fd)


def enforce(policy: dict[str, tuple[Path, ...]], attest_fd: int | None = None) -> int:
    """Restrict this child before exec; return the probed ABI on success."""
    state = probe()
    if state["state"] == "error":
        raise IsolationError(str(state.get("reason")))
    if state["state"] != "available":
        raise IsolationUnavailable(str(state.get("reason")))
    _audit_fds(policy, attest_fd)
    rules = _Ruleset(_HANDLED_FS, _HANDLED_NET)
    ruleset_fd = _syscall(0, ctypes.byref(rules), ctypes.sizeof(rules), 0)
    try:
        for path in policy["readonly"]:
            _add_grant(ruleset_fd, path, False)
        for path in policy["writable"]:
            _add_grant(ruleset_fd, path, True)
        # PR_SET_NO_NEW_PRIVS must succeed before unprivileged restriction.
        libc = ctypes.CDLL(None, use_errno=True)
        libc.prctl.restype = ctypes.c_int
        if libc.prctl(38, 1, 0, 0, 0) != 0:
            error = ctypes.get_errno()
            raise OSError(error, os.strerror(error))
        _syscall(2, ruleset_fd, 0)
    finally:
        os.close(ruleset_fd)
    return int(state["abi"])


def _check_child_access(suite: Path, pack: Path, expected_pack_sha256: str) -> dict[str, bool]:
    def denied(path: Path) -> bool:
        try:
            with path.open("rb"):
                return False
        except PermissionError:
            return True

    suite_denied = denied(suite)
    proc_denied = denied(Path("/proc/self/environ"))
    digest = hashlib.sha256()
    with pack.open("rb") as stream:
        for block in iter(lambda: stream.read(65536), b""):
            digest.update(block)
    result = {
        "suite_read_denied": suite_denied,
        "query_pack_read_allowed": digest.hexdigest() == expected_pack_sha256,
        "proc_read_denied": proc_denied,
    }
    if not all(result.values()):
        raise IsolationError(f"child access checks failed: {result}")
    return result


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--probe", action="store_true")
    parser.add_argument("--policy", type=Path)
    parser.add_argument("--suite", type=Path)
    parser.add_argument("--query-pack", type=Path)
    parser.add_argument("--query-pack-sha256")
    parser.add_argument("--attest-fd", type=int)
    parser.add_argument("--nonce")
    parser.add_argument("command", nargs=argparse.REMAINDER)
    args = parser.parse_args(argv)
    if args.probe:
        state = probe()
        print(json.dumps(state, sort_keys=True))
        if state["state"] == "available":
            return 0
        return EXIT_UNAVAILABLE if state["state"] == "unavailable" else EXIT_REJECTED
    command = args.command[1:] if args.command[:1] == ["--"] else args.command
    if args.policy is None or not command or not Path(command[0]).is_absolute():
        parser.error("--policy and an absolute executable after -- are required")
    attesting = args.attest_fd is not None
    if attesting and (
        args.attest_fd < 3
        or args.suite is None
        or args.query_pack is None
        or not isinstance(args.query_pack_sha256, str)
        or len(args.query_pack_sha256) != 64
        or not isinstance(args.nonce, str)
        or len(args.nonce) != 64
    ):
        parser.error("attestation requires fd, suite, pack, pack digest, and nonce")
    try:
        with args.policy.open("r", encoding="utf-8") as stream:
            policy = validate_policy(json.load(stream))
        executable = Path(command[0])
        resolved_executable = executable.resolve(strict=True)
        if not any(
            resolved_executable == root or root in resolved_executable.parents
            for root in policy["readonly"]
        ):
            raise IsolationError("executable is outside readonly grants")
        abi = enforce(policy, args.attest_fd)
        if attesting:
            checks = _check_child_access(args.suite, args.query_pack, args.query_pack_sha256)
            exec_sha256 = hashlib.sha256(
                json.dumps(
                    command, sort_keys=True, separators=(",", ":"), ensure_ascii=False
                ).encode()
            ).hexdigest()
            message = {"nonce": args.nonce, "abi": abi, "exec_sha256": exec_sha256, **checks}
            os.write(args.attest_fd, (json.dumps(message, sort_keys=True) + "\n").encode())
            os.close(args.attest_fd)
        os.execve(executable, command, os.environ.copy())
    except (IsolationError, OSError, ValueError) as exc:
        unavailable = isinstance(exc, IsolationUnavailable)
        print(f"{BACKEND}: {'unavailable' if unavailable else 'rejected'}: {exc}", file=sys.stderr)
        return EXIT_UNAVAILABLE if unavailable else EXIT_REJECTED
    return EXIT_REJECTED  # execve never returns on success


if __name__ == "__main__":
    raise SystemExit(main())
