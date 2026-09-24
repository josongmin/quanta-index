#!/usr/bin/env python3
"""Refuse contended timing rails and optionally emit a bound host preflight receipt."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import platform
import re
import subprocess
import sys
import tempfile
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path

RUST_PROCESS = re.compile(r"(?:^|/)(?:cargo(?:-[A-Za-z0-9_-]+)?|rustc)(?:\s|$)")


@dataclass(frozen=True)
class Process:
    pid: int
    ppid: int
    command: str


def parse_processes(output: str) -> list[Process]:
    processes: list[Process] = []
    seen_pids: set[int] = set()
    for line in output.splitlines():
        if not line.strip():
            continue
        fields = line.strip().split(maxsplit=2)
        if len(fields) != 3:
            raise ValueError("process row has no command")
        try:
            pid = int(fields[0])
            ppid = int(fields[1])
        except ValueError as exc:
            raise ValueError("process row has a non-numeric pid") from exc
        if pid <= 0 or ppid < 0 or pid in seen_pids:
            raise ValueError("process row has an invalid or duplicate pid")
        seen_pids.add(pid)
        processes.append(Process(pid=pid, ppid=ppid, command=fields[2]))
    if not processes:
        raise ValueError("process snapshot is empty")
    return processes


def ancestor_pids(processes: list[Process], pid: int) -> set[int]:
    parents = {process.pid: process.ppid for process in processes}
    ancestors = {pid}
    while pid in parents and parents[pid] > 0 and parents[pid] not in ancestors:
        pid = parents[pid]
        ancestors.add(pid)
    return ancestors


def foreign_rust_processes(processes: list[Process], pid: int) -> list[Process]:
    own_chain = ancestor_pids(processes, pid)
    return [
        process
        for process in processes
        if process.pid not in own_chain and RUST_PROCESS.search(process.command)
    ]


def sha256_text(value: str) -> str:
    return "sha256:" + hashlib.sha256(value.encode("utf-8")).hexdigest()


def host_snapshot() -> dict[str, object]:
    """Portable host facts recorded without pretending they establish isolation."""
    try:
        load_average: list[float] | None = [round(value, 6) for value in os.getloadavg()]
    except (AttributeError, OSError):
        load_average = None
    try:
        disk = os.statvfs(".")
        disk_available_bytes: int | None = disk.f_frsize * disk.f_bavail
    except OSError:
        disk_available_bytes = None
    hostname = platform.node()
    return {
        "os": platform.system().lower() or "unknown",
        "arch": platform.machine().lower() or "unknown",
        "cpu_count": os.cpu_count(),
        "hostname_hash": sha256_text(hostname),
        "load_average": load_average,
        "disk_available_bytes": disk_available_bytes,
    }


def preflight_receipt(
    *,
    run_id: str | None,
    processes: list[Process],
    foreign: list[Process],
    override: bool,
    ps_ok: bool,
    expected_os: str | None = None,
) -> dict[str, object]:
    """Build a stable receipt; the caller decides whether the verdict blocks."""
    host = host_snapshot()
    actual_os = host["os"]
    host_matches = expected_os is None or actual_os == expected_os
    if not ps_ok:
        status = "error"
    elif not host_matches:
        status = "unsupported_host"
    elif foreign and override:
        status = "contended_override"
    elif foreign:
        status = "blocked"
    else:
        status = "clean"
    return {
        "schema_version": 1,
        "kind": "quanta-index-timing-preflight",
        "run_id": run_id,
        "captured_at": datetime.now(timezone.utc).isoformat().replace("+00:00", "Z"),
        "status": status,
        "host": host,
        "expected_os": expected_os,
        "process_snapshot_sha256": sha256_text(
            "\n".join(f"{process.pid} {process.ppid} {process.command}" for process in processes)
        ),
        "foreign_rust_processes": [
            {
                "pid": process.pid,
                "ppid": process.ppid,
                "command_sha256": sha256_text(" ".join(process.command.split())),
            }
            for process in foreign
        ],
    }


def write_receipt(path: Path, receipt: dict[str, object]) -> None:
    """Atomically publish one receipt, never a partial JSON document."""
    path.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.NamedTemporaryFile(
        "w", encoding="utf-8", dir=path.parent, prefix=f".{path.name}.", delete=False
    ) as handle:
        temporary = Path(handle.name)
        json.dump(receipt, handle, sort_keys=True, indent=2)
        handle.write("\n")
    try:
        os.replace(temporary, path)
    except OSError:
        temporary.unlink(missing_ok=True)
        raise


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--allow-contended",
        action="store_true",
        help="record the contention but do not block (also settable by environment)",
    )
    parser.add_argument(
        "--expected-os",
        choices=("linux", "darwin"),
        default=None,
        help="refuse a timing claim unless the measured host has this operating system",
    )
    parser.add_argument(
        "--receipt",
        type=Path,
        help="write an atomic JSON preflight receipt to this exact path",
    )
    parser.add_argument(
        "--run-id",
        default=None,
        help="non-empty caller-supplied identifier binding this preflight to one measurement run",
    )
    args = parser.parse_args(argv)
    if args.run_id is not None and (not args.run_id or args.run_id.strip() != args.run_id):
        print("TIMING_PREFLIGHT_ERROR reason=invalid_run_id", file=sys.stderr)
        return 2
    completed = subprocess.run(
        ["ps", "-axo", "pid=,ppid=,command="],
        check=False,
        capture_output=True,
        text=True,
    )
    if completed.returncode != 0:
        if args.receipt is not None:
            write_receipt(
                args.receipt,
                preflight_receipt(
                    run_id=args.run_id,
                    processes=[],
                    foreign=[],
                    override=False,
                    ps_ok=False,
                    expected_os=args.expected_os,
                ),
            )
        print("TIMING_PREFLIGHT_ERROR reason=ps_failed", file=sys.stderr)
        return 2
    try:
        processes = parse_processes(completed.stdout)
        if os.getpid() not in {process.pid for process in processes}:
            raise ValueError("current process is absent from snapshot")
    except ValueError as exc:
        if args.receipt is not None:
            try:
                write_receipt(
                    args.receipt,
                    preflight_receipt(
                        run_id=args.run_id,
                        processes=[],
                        foreign=[],
                        override=False,
                        ps_ok=False,
                        expected_os=args.expected_os,
                    ),
                )
            except OSError as write_exc:
                print(
                    f"TIMING_PREFLIGHT_ERROR reason=receipt_write_failed detail={write_exc}",
                    file=sys.stderr,
                )
                return 2
        print(
            f"TIMING_PREFLIGHT_ERROR reason=invalid_process_snapshot detail={exc}", file=sys.stderr
        )
        return 2
    foreign = foreign_rust_processes(processes, os.getpid())
    override = args.allow_contended or os.environ.get("QUANTA_INDEX_ALLOW_CONTENDED_TIMINGS") == "1"
    if args.receipt is not None:
        try:
            write_receipt(
                args.receipt,
                preflight_receipt(
                    run_id=args.run_id,
                    processes=processes,
                    foreign=foreign,
                    override=override,
                    ps_ok=True,
                    expected_os=args.expected_os,
                ),
            )
        except OSError as exc:
            print(
                f"TIMING_PREFLIGHT_ERROR reason=receipt_write_failed detail={exc}", file=sys.stderr
            )
            return 2
    actual_os = platform.system().lower() or "unknown"
    if args.expected_os is not None and actual_os != args.expected_os:
        print(
            f"TIMING_PREFLIGHT_BLOCKED reason=unsupported_host expected_os={args.expected_os} actual_os={actual_os}",
            file=sys.stderr,
        )
        return 1
    if not foreign:
        print("TIMING_PREFLIGHT_OK foreign_rust_processes=0 status=clean")
        return 0

    status = "OVERRIDE" if override else "BLOCKED"
    stream = sys.stdout if override else sys.stderr
    print(
        f"TIMING_PREFLIGHT_{status} reason=foreign_rust_processes count={len(foreign)}",
        file=stream,
    )
    for process in foreign[:12]:
        command = " ".join(process.command.split())
        print(f"pid={process.pid} ppid={process.ppid} command={command[:240]}", file=stream)
    if len(foreign) > 12:
        print(f"additional_processes={len(foreign) - 12}", file=stream)
    if not override:
        print(
            "wait for the foreign build or set QUANTA_INDEX_ALLOW_CONTENDED_TIMINGS=1 "
            "to run without a clean timing claim",
            file=sys.stderr,
        )
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
