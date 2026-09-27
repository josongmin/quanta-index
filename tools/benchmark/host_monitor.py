"""Capture-bound cooperative host observations, never quiet-host attestation."""

from __future__ import annotations

import fcntl
import math
import os
import platform
import subprocess
import tempfile
import threading
import time
import uuid
from pathlib import Path

from evidence import (
    EvidenceError,
    RawFile,
    RawWriter,
    _run_id,
    canonical_json,
    digest_bytes,
    parse_json,
)

RAW_NAME = "host-observations.jsonl"
INPUT_ID = "benchmark-host-observations"
INTERVAL_NS = 1_000_000_000
MAX_GAP_NS = 10_000_000_000
CLOCK_TOLERANCE_NS = 1_000_000_000
MAX_BYTES = 64 * 1024 * 1024
MAX_SAMPLES = 200_000


def lock_path() -> Path:
    # Distinct from leaf build admission: a capture may invoke admitted builds.
    # All checkouts of the same user on this host share this reservation.
    return Path(tempfile.gettempdir()).resolve() / f"quanta-index-benchmark-host-{os.getuid()}.lock"


def observe() -> tuple[dict, dict]:
    """Collect bounded, non-secret facts; command arguments never enter output."""
    from tools.ci.timing.check_host_contention import RUST_PROCESS, ancestor_pids, parse_processes

    cpus, hostname, arch = os.cpu_count(), platform.node(), platform.machine()
    if type(cpus) is not int or cpus < 1 or not hostname or not arch:
        raise EvidenceError("host observation lacks CPU/hostname/architecture identity")
    # Avoid an unbounded communicate() buffer for a large process inventory.
    with tempfile.TemporaryFile() as output:
        result = subprocess.run(
            ["/bin/ps", "-axo", "pid=,ppid=,comm="], stdout=output,
            stderr=subprocess.DEVNULL, stdin=subprocess.DEVNULL, check=False, timeout=3,
        )
        if result.returncode:
            raise EvidenceError("host process observation failed")
        output.seek(0)
        raw = output.read(16 * 1024 * 1024 + 1)
        if len(raw) > 16 * 1024 * 1024:
            raise EvidenceError("host process inventory exceeds control limit")
    processes = parse_processes(raw.decode("utf-8", errors="strict"))
    excluded = ancestor_pids(processes, os.getpid())
    descendants = {os.getpid()}
    while True:
        expanded = descendants | {p.pid for p in processes if p.ppid in descendants}
        if expanded == descendants:
            break
        descendants = expanded
    excluded |= descendants
    disk = os.statvfs(tempfile.gettempdir())
    host = {
        "os": "macos" if platform.system() == "Darwin" else platform.system().lower(),
        "arch": arch, "cpu_count": cpus, "hostname_hash": digest_bytes(hostname.encode()),
    }
    facts = {
        "load_average": list(os.getloadavg()),
        "disk_available_bytes": disk.f_frsize * disk.f_bavail,
        "process_count": len(processes),
        "process_snapshot_sha256": digest_bytes(raw),
        "foreign_rust": [
            {"pid": p.pid, "ppid": p.ppid, "command_sha256": digest_bytes(p.command.encode())}
            for p in processes if p.pid not in excluded and RUST_PROCESS.search(p.command)
        ],
    }
    _facts(facts)
    return host, facts


def _uint(value, name, *, positive=False):
    if type(value) is not int or value < int(positive):
        raise EvidenceError(f"host observation invalid {name}")


def _digest(value):
    from evidence import require_digest

    require_digest("host observation digest", value)


def _facts(facts):
    if not isinstance(facts, dict) or set(facts) != {
        "load_average", "disk_available_bytes", "process_count", "process_snapshot_sha256", "foreign_rust"
    }:
        raise EvidenceError("host observation facts are incomplete")
    load = facts["load_average"]
    if (not isinstance(load, list) or len(load) != 3 or any(
        type(v) not in {float, int} or not math.isfinite(v) or v < 0 for v in load
    )):
        raise EvidenceError("host load observation is unavailable or invalid")
    _uint(facts["disk_available_bytes"], "available disk")
    _uint(facts["process_count"], "process count", positive=True)
    _digest(facts["process_snapshot_sha256"])
    if not isinstance(facts["foreign_rust"], list):
        raise EvidenceError("host interference inventory is missing")
    seen = set()
    for row in facts["foreign_rust"]:
        if not isinstance(row, dict) or set(row) != {"pid", "ppid", "command_sha256"}:
            raise EvidenceError("host interference row is malformed")
        _uint(row["pid"], "interference PID", positive=True)
        _uint(row["ppid"], "interference parent")
        _digest(row["command_sha256"])
        if row["pid"] in seen:
            raise EvidenceError("host interference repeats a PID")
        seen.add(row["pid"])
    if len(seen) > facts["process_count"]:
        raise EvidenceError("host interference exceeds the observed inventory")


def validate(raw: RawFile, *, capture_id: str, profile: str) -> dict:
    """Rederive the lease count and identity from a complete pinned transcript."""
    if raw.size > MAX_BYTES:
        raise EvidenceError("host observations exceed byte limit")
    header, previous, count, ended = None, None, 0, False

    def consume(lines):
        nonlocal header, previous, count, ended
        for line in lines:
            row = parse_json(line.decode("utf-8", errors="strict"))
            if header is None:
                if not isinstance(row, dict) or set(row) != {
                    "kind", "schema_version", "capture_id", "profile", "reservation_id",
                    "lock_identity", "interval_ns", "max_gap_ns", "clock_tolerance_ns", "host"
                }:
                    raise EvidenceError("host observation header is malformed")
                if (row["kind"] != "cooperative-host-observations" or type(row["schema_version"]) is not int
                        or row["schema_version"] != 1 or row["capture_id"] != capture_id or row["profile"] != profile
                        or type(row["interval_ns"]) is not int or row["interval_ns"] != INTERVAL_NS
                        or type(row["max_gap_ns"]) is not int or row["max_gap_ns"] != MAX_GAP_NS
                        or type(row["clock_tolerance_ns"]) is not int or row["clock_tolerance_ns"] != CLOCK_TOLERANCE_NS):
                    raise EvidenceError("host observation identity or policy differs")
                _run_id(row["reservation_id"])
                identity = row["lock_identity"]
                if not isinstance(identity, list) or len(identity) != 5:
                    raise EvidenceError("host reservation identity is missing")
                for value in identity:
                    _uint(value, "lock identity")
                host = row["host"]
                if not isinstance(host, dict) or set(host) != {"os", "arch", "cpu_count", "hostname_hash"}:
                    raise EvidenceError("host identity is incomplete")
                if any(not isinstance(host[k], str) or not host[k] for k in ("os", "arch")):
                    raise EvidenceError("host platform identity is missing")
                _uint(host["cpu_count"], "CPU count", positive=True)
                _digest(host["hostname_hash"])
                header = row
                continue
            if ended or not isinstance(row, dict) or set(row) != {
                "sequence", "event", "phase", "capture_id", "reservation_id", "monotonic_ns", "wall_ns", "facts", "status"
            }:
                raise EvidenceError("host observation row is malformed or after terminal")
            if (type(row["sequence"]) is not int or row["sequence"] != count
                    or row["capture_id"] != capture_id or row["reservation_id"] != header["reservation_id"]):
                raise EvidenceError("host observations are reordered, missing or mixed")
            event = row["event"]
            if (event not in {"start", "sample", "phase", "end"}
                    or (count == 0) != (event == "start")
                    or row["status"] != ("completed" if event == "end" else "active")):
                raise EvidenceError("host observation has no valid start/terminal")
            _run_id(row["phase"])
            _uint(row["monotonic_ns"], "monotonic time", positive=True)
            _uint(row["wall_ns"], "wall time", positive=True)
            _facts(row["facts"])
            if previous is not None:
                delta = row["monotonic_ns"] - previous["monotonic_ns"]
                wall_delta = row["wall_ns"] - previous["wall_ns"]
                if delta <= 0 or delta > MAX_GAP_NS or abs(delta - wall_delta) > CLOCK_TOLERANCE_NS:
                    raise EvidenceError("host observation gap or clock discontinuity")
            count += 1
            if count > MAX_SAMPLES:
                raise EvidenceError("host observation sample count exceeds limit")
            previous, ended = row, event == "end"

    raw.consume_lines(consume)
    if header is None or not ended or count < 2:
        raise EvidenceError("host observations have no complete interval")
    return {**header["host"], "observed_samples": count,
            "capture_id": capture_id, "profile": profile, "digest": raw.sha256}


class HostMonitor:
    """One cooperative reservation and bounded periodic diagnostic transcript."""

    def __init__(self, path: Path, capture_id: str, profile: str):
        self.path, self.capture_id, self.profile = path, _run_id(capture_id), _run_id(profile)
        self.fd, self.writer, self.thread = None, None, None
        self.stop_event, self.mutex = threading.Event(), threading.Lock()
        self.error, self.sequence, self.bytes = None, 0, 0
        self.phase_name = "preparation"
        self.reservation_id = uuid.uuid4().hex

    def start(self):
        from tools.ci.resource_admission import check_lock

        lock = lock_path()
        self.fd = os.open(lock, os.O_RDWR | os.O_CREAT | os.O_NOFOLLOW | os.O_CLOEXEC, 0o600)
        try:
            self.identity = check_lock(self.fd, lock)
            try:
                fcntl.flock(self.fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
            except BlockingIOError as error:
                raise EvidenceError("benchmark host reservation is already held") from error
            host, facts = observe()
            self.host = host
            self.writer = RawWriter(self.path)
            self.writer.__enter__()
            self._write({"kind": "cooperative-host-observations", "schema_version": 1,
                         "capture_id": self.capture_id, "profile": self.profile,
                         "reservation_id": self.reservation_id, "lock_identity": list(self.identity),
                         "interval_ns": INTERVAL_NS, "max_gap_ns": MAX_GAP_NS,
                         "clock_tolerance_ns": CLOCK_TOLERANCE_NS, "host": host})
            self._sample("start", facts=facts)
            self.thread = threading.Thread(target=self._poll, name="benchmark-host-observer", daemon=True)
            self.thread.start()
            return self
        except BaseException:
            self.close()
            raise

    def _write(self, value):
        encoded = canonical_json(value).encode() + b"\n"
        self.bytes += len(encoded)
        if self.bytes > MAX_BYTES:
            raise EvidenceError("host observations exceed byte limit")
        self.writer.write(encoded)

    def _sample(self, event, *, facts=None, status="active"):
        from tools.ci.resource_admission import check_lock

        if check_lock(self.fd, lock_path()) != self.identity:
            raise EvidenceError("host reservation changed during capture")
        if facts is None:
            host, facts = observe()
            if host != self.host:
                raise EvidenceError("host identity changed during capture")
        if self.sequence >= MAX_SAMPLES:
            raise EvidenceError("host observation sample count exceeds limit")
        self._write({"sequence": self.sequence, "event": event, "phase": self.phase_name,
                     "capture_id": self.capture_id, "reservation_id": self.reservation_id,
                     "monotonic_ns": time.monotonic_ns(), "wall_ns": time.time_ns(),
                     "facts": facts, "status": status})
        self.sequence += 1

    def _poll(self):
        try:
            while not self.stop_event.wait(INTERVAL_NS / 1e9):
                with self.mutex:
                    self._sample("sample")
        except BaseException as error:
            self.error = error
            self.stop_event.set()

    def phase(self, name):
        with self.mutex:
            if self.error is not None:
                raise EvidenceError(f"host monitor failed: {self.error}") from self.error
            self.phase_name = _run_id(name)
            self._sample("phase")

    def finish(self, *, failed=False) -> RawFile:
        self.stop_event.set()
        if self.thread is not None:
            self.thread.join(timeout=5)
            if self.thread.is_alive():
                raise EvidenceError("host monitor did not stop before deadline")
        try:
            if self.error is not None:
                raise EvidenceError(f"host monitor failed: {self.error}") from self.error
            self._sample("end", status="failed" if failed else "completed")
            raw = self.writer.finish()
            if not failed:
                validate(raw, capture_id=self.capture_id, profile=self.profile)
            return raw
        finally:
            self.close()

    def close(self):
        if self.writer is not None:
            self.writer.__exit__(None, None, None)
            self.writer = None
        if self.fd is not None:
            # Never unlink; waiters and producer guards retain this same inode.
            os.close(self.fd)
            self.fd = None
