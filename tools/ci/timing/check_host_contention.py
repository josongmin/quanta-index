#!/usr/bin/env python3
"""Refuse local timing rails while unrelated Rust builds consume the host."""

from __future__ import annotations

import argparse
import os
import re
import subprocess
import sys
from dataclasses import dataclass

RUST_PROCESS = re.compile(r"(?:^|/)(?:cargo|cargo-nextest|rustc)(?:\s|$)")


@dataclass(frozen=True)
class Process:
    pid: int
    ppid: int
    command: str


def parse_processes(output: str) -> list[Process]:
    processes: list[Process] = []
    for line in output.splitlines():
        fields = line.strip().split(maxsplit=2)
        if len(fields) != 3:
            continue
        try:
            pid = int(fields[0])
            ppid = int(fields[1])
        except ValueError:
            continue
        processes.append(Process(pid=pid, ppid=ppid, command=fields[2]))
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


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--allow-contended",
        action="store_true",
        help="record the contention but do not block (also settable by environment)",
    )
    args = parser.parse_args(argv)
    completed = subprocess.run(
        ["ps", "-axo", "pid=,ppid=,command="],
        check=False,
        capture_output=True,
        text=True,
    )
    if completed.returncode != 0:
        print("TIMING_PREFLIGHT_ERROR reason=ps_failed", file=sys.stderr)
        return 2
    foreign = foreign_rust_processes(parse_processes(completed.stdout), os.getpid())
    if not foreign:
        print("TIMING_PREFLIGHT_OK foreign_rust_processes=0")
        return 0

    override = args.allow_contended or os.environ.get("QUANTA_INDEX_ALLOW_CONTENDED_TIMINGS") == "1"
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
