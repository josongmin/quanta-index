"""Real searchd selection, state-root custody, and restart handover.

Run with QUANTA_INDEX_SEARCHD_BIN naming a freshly built daemon. An unset
binary skips this integration scope; the ordinary fake-child launcher tests
remain in test_deployment_launcher.py.
"""

from __future__ import annotations

import fcntl
import hashlib
import json
import os
import shutil
import signal
import socket
import stat
import subprocess
import sys
import tempfile
import time
from collections.abc import Iterator
from pathlib import Path

import pytest

LAUNCHER = Path(__file__).resolve().parents[2] / "deployment/launcher.py"
SOCKETS = ("query.sock", "control.sock", "ingest.sock")
STATE_LOCK = ".searchd-state-root.lock"
MANIFEST = "state-root-manifest-v1.txt"
START_TIMEOUT = 90.0
# The daemon's hard drain is 125 seconds; the launcher allows five more.
STOP_TIMEOUT = 135.0
COMMAND_TIMEOUT = 180.0


def _log_tail(path: Path) -> str:
    if not path.exists():
        return "<no process log>"
    return path.read_bytes()[-16_384:].decode("utf-8", errors="replace")


def _process_children(parent_pid: int) -> set[int]:
    """Find the launcher's direct daemon child for owned-group cleanup."""
    result = subprocess.run(
        ["ps", "-axo", "pid=,ppid="],
        capture_output=True,
        text=True,
        check=True,
        timeout=5,
    )
    children: set[int] = set()
    for line in result.stdout.splitlines():
        fields = line.split()
        if len(fields) == 2 and fields[1] == str(parent_pid):
            children.add(int(fields[0]))
    return children


class _OwnedProcess:
    def __init__(self, command: list[str], environment: dict[str, str], log: Path) -> None:
        self.log = log
        self.child_groups: set[int] = set()
        self.clean_stop = False
        with log.open("wb") as output:
            self.process = subprocess.Popen(
                command,
                env=environment,
                cwd="/",
                stdin=subprocess.DEVNULL,
                stdout=output,
                stderr=subprocess.STDOUT,
                start_new_session=True,
            )

    def remember_launcher_child(self) -> None:
        if self.process.poll() is None:
            self.child_groups = _process_children(self.process.pid)

    def stop(self) -> None:
        assert self.process.poll() is None, (
            f"process exited before SIGTERM: {self.process.returncode}\n{_log_tail(self.log)}"
        )
        os.kill(self.process.pid, signal.SIGTERM)
        try:
            status = self.process.wait(timeout=STOP_TIMEOUT)
        except subprocess.TimeoutExpired as error:
            raise AssertionError(
                f"SIGTERM drain exceeded {STOP_TIMEOUT}s\n{_log_tail(self.log)}"
            ) from error
        assert status == 0, f"SIGTERM drain exited {status}\n{_log_tail(self.log)}"
        self.clean_stop = True

    def cleanup(self) -> None:
        # The launcher puts the real searchd child in its own session. Kill
        # only child groups observed as its descendants, then its own group.
        if not self.clean_stop:
            if self.process.poll() is None:
                self.remember_launcher_child()
            for child_pid in self.child_groups:
                try:
                    os.killpg(child_pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
        if self.process.poll() is None:
            try:
                os.killpg(self.process.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
        try:
            self.process.wait(timeout=5)
        except subprocess.TimeoutExpired as error:
            raise AssertionError(
                f"owned process group did not exit\n{_log_tail(self.log)}"
            ) from error


def _socket_accepts(path: Path) -> bool:
    try:
        if not stat.S_ISSOCK(path.lstat().st_mode):
            return False
        with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as client:
            client.settimeout(0.5)
            client.connect(str(path))
        return True
    except (FileNotFoundError, ConnectionError, OSError):
        return False


def _state_root_lease_locked(root: Path) -> bool:
    path = root / STATE_LOCK
    try:
        with path.open("rb") as lock:
            try:
                fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
            except BlockingIOError:
                return True
            else:
                fcntl.flock(lock, fcntl.LOCK_UN)
                return False
    except FileNotFoundError:
        return False


def _serves(root: Path) -> bool:
    socket_dir = root / "search-plane"
    return _state_root_lease_locked(root) and all(
        _socket_accepts(socket_dir / name) for name in SOCKETS
    )


def _wait_serving(owned: _OwnedProcess, root: Path, *, launcher: bool) -> None:
    deadline = time.monotonic() + START_TIMEOUT
    while time.monotonic() < deadline:
        status = owned.process.poll()
        if status is not None:
            raise AssertionError(
                f"daemon exited before product socket/lease readiness: {status}"
                f"\n{_log_tail(owned.log)}"
            )
        if _serves(root):
            if launcher:
                owned.remember_launcher_child()
                assert owned.child_groups, "launcher has no owned searchd child"
            return
        time.sleep(0.05)
    raise AssertionError(
        f"daemon did not accept on all product sockets with a held root lease"
        f" within {START_TIMEOUT}s\n{_log_tail(owned.log)}"
    )


def _run(command: list[str], environment: dict[str, str]) -> subprocess.CompletedProcess[str]:
    process = subprocess.Popen(
        command,
        env=environment,
        cwd="/",
        stdin=subprocess.DEVNULL,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        start_new_session=True,
    )
    try:
        stdout, stderr = process.communicate(timeout=COMMAND_TIMEOUT)
    except subprocess.TimeoutExpired as error:
        try:
            children = _process_children(process.pid)
        except (OSError, subprocess.SubprocessError):
            children = set()
        for child_pid in children:
            try:
                os.killpg(child_pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
        try:
            os.killpg(process.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        stdout, stderr = process.communicate(timeout=5)
        raise AssertionError(
            f"command timed out: {command!r}\nstdout:\n{stdout}\nstderr:\n{stderr}"
        ) from error
    return subprocess.CompletedProcess(command, process.returncode, stdout, stderr)


def _must_pass(command: list[str], environment: dict[str, str]) -> None:
    result = _run(command, environment)
    assert result.returncode == 0, (
        f"command failed {result.returncode}: {command!r}"
        f"\nstdout:\n{result.stdout}\nstderr:\n{result.stderr}"
    )


def _selection_wire(path: Path, binary: Path, root: Path, environment: dict[str, str]) -> None:
    digest = hashlib.sha256()
    with binary.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    assert json.loads(path.read_text(encoding="utf-8")) == {
        "schema_version": 2,
        "mode": "legacy",
        "binary": str(binary),
        "binary_sha256": digest.hexdigest(),
        "state_root": str(root),
        "environment": environment,
    }


def _boot_fresh_seed(binary: Path, root: Path, environment: dict[str, str], log: Path) -> None:
    root.mkdir(mode=0o700)
    owned = _OwnedProcess([str(binary), "serve", "--state-root", str(root)], environment, log)
    try:
        _wait_serving(owned, root, launcher=False)
        owned.stop()
        assert not _serves(root), "stopped seed still owns its sockets and lease"
    finally:
        owned.cleanup()


def _make_verified_current_root(
    binary: Path, seed: Path, root: Path, environment: dict[str, str]
) -> None:
    backup = root.with_name(root.name + "-backup")
    _must_pass(
        [str(binary), "backup-state", "--source", str(seed), "--destination", str(backup)],
        environment,
    )
    _must_pass(
        [str(binary), "restore-state", "--source", str(backup), "--destination", str(root)],
        environment,
    )
    assert (root / MANIFEST).is_file(), "restore did not publish a current-root manifest"
    _must_pass([str(binary), "verify-state", "--source", str(root)], environment)


def _install(
    path: Path, binary: Path, root: Path, environment: dict[str, str], launcher_env: dict[str, str]
) -> None:
    command = [
        sys.executable,
        str(LAUNCHER),
        "install",
        "--selection",
        str(path),
        "--binary",
        str(binary),
        "--state-root",
        str(root),
    ]
    for name, value in environment.items():
        command.extend(("--env", f"{name}={value}"))
    _must_pass(command, launcher_env)
    _selection_wire(path, binary, root, environment)


def _verify_bootstrap(path: Path, launcher_env: dict[str, str]) -> None:
    _must_pass(
        [sys.executable, str(LAUNCHER), "verify-bootstrap", "--selection", str(path)],
        launcher_env,
    )


def _backup_mutable_root_and_verify(
    binary: Path, root: Path, roundtrip: Path, environment: dict[str, str]
) -> None:
    """Prove the ordinary post-serve root through the product backup owner."""
    backup = roundtrip.with_name(roundtrip.name + "-backup")
    _must_pass(
        [str(binary), "backup-state", "--source", str(root), "--destination", str(backup)],
        environment,
    )
    _must_pass(
        [str(binary), "restore-state", "--source", str(backup), "--destination", str(roundtrip)],
        environment,
    )
    assert (roundtrip / MANIFEST).is_file()
    _must_pass([str(binary), "verify-state", "--source", str(roundtrip)], environment)


def _serve_once(
    path: Path,
    selected: Path,
    unselected: Path,
    launcher_env: dict[str, str],
    log: Path,
) -> None:
    owned = _OwnedProcess(
        [sys.executable, str(LAUNCHER), "serve", "--selection", str(path)],
        launcher_env,
        log,
    )
    try:
        _wait_serving(owned, selected, launcher=True)
        assert not _serves(unselected), "unselected root unexpectedly has live sockets and lease"
        owned.stop()
        still_serving = _serves(selected)
        if still_serving:
            owned.clean_stop = False
        assert not still_serving, "selected root remained live after launcher drain"
    finally:
        owned.cleanup()


@pytest.fixture
def short_state_root_path() -> Iterator[Path]:
    # Product UDS paths must fit sun_path, even when pytest's own tmp path
    # contains a long test-function name or macOS per-user temp prefix.
    with tempfile.TemporaryDirectory(prefix="qdl-", dir="/tmp") as directory:
        yield Path(directory).resolve(strict=True)


@pytest.mark.integration
def test_real_searchd_selection_handover_and_restart(short_state_root_path: Path) -> None:
    selected_binary = os.environ.get("QUANTA_INDEX_SEARCHD_BIN")
    if not selected_binary:
        pytest.skip("QUANTA_INDEX_SEARCHD_BIN is required for real daemon integration")
    binary = Path(selected_binary).resolve(strict=True)
    assert binary.is_file() and os.access(binary, os.X_OK)
    assert shutil.which("ps") is not None, "owned launcher child cleanup requires ps"

    base = short_state_root_path
    home = base / "home"
    temporary = base / "tmp"
    deployment = base / "deployment"
    for directory in (home, temporary, deployment):
        directory.mkdir(mode=0o700)
    environment = {
        "HOME": str(home),
        "TMPDIR": str(temporary),
        "PATH": "/usr/bin:/bin",
        "QUANTA_INDEX_EMBEDDER": "hash-dev",
        "QUANTA_INDEX_SEARCH_CORPUS_HISTORY_MAX_GENERATIONS": "8",
        "QUANTA_INDEX_SEARCH_CORPUS_HISTORY_MAX_BYTES": str(16 * 1024 * 1024),
        "QUANTA_INDEX_SEARCH_CORPUS_HISTORY_MAX_REVISION_PAIRS": "128",
        "QUANTA_INDEX_SEARCH_CORPUS_HISTORY_MAX_TOTAL_BYTES": str(256 * 1024 * 1024),
        # Match TEST_STATE_ROOT_PHYSICAL_POLICY used by real Rust daemon tests.
        # These explicit fixture limits are not deployment defaults.
        "QUANTA_INDEX_LEGACY_PHYSICAL_MAX_BYTES": str(16 << 30),
        "QUANTA_INDEX_LEGACY_PHYSICAL_MAX_FILES": "200000",
        "QUANTA_INDEX_LEGACY_PHYSICAL_MAX_PENDING": "100000",
        "QUANTA_INDEX_LEGACY_PHYSICAL_MAX_ENTRIES": "400000",
        "QUANTA_INDEX_LEGACY_PHYSICAL_MAX_DEPTH": "64",
        "QUANTA_INDEX_LEGACY_PHYSICAL_MAX_PENDING_FRAMES": "4096",
        "QUANTA_INDEX_LEGACY_PHYSICAL_MAX_READER_HANDLES": "4096",
        "QUANTA_INDEX_LEGACY_PHYSICAL_MAX_TRACKED_ENTRIES": "100000",
        "QUANTA_INDEX_LEGACY_PHYSICAL_MAX_PROCESS_ROOTS": "256",
        "QUANTA_INDEX_LEGACY_PHYSICAL_GLOBAL_MAX_SQLITE_HANDLES": "8192",
        "QUANTA_INDEX_LEGACY_PHYSICAL_MAINTENANCE_RESERVED_BYTES": str(1 << 30),
        "QUANTA_INDEX_LEGACY_PHYSICAL_MAINTENANCE_RESERVED_FILES": "1000",
        "QUANTA_INDEX_LEGACY_PHYSICAL_CONTROL_RESERVED_BYTES": str(1 << 30),
        "QUANTA_INDEX_LEGACY_PHYSICAL_CONTROL_RESERVED_FILES": "10000",
        "QUANTA_INDEX_LEGACY_PHYSICAL_WORK_RESERVED_BYTES": str(4 << 30),
        "QUANTA_INDEX_LEGACY_PHYSICAL_WORK_RESERVED_FILES": "30000",
        "QUANTA_INDEX_LEGACY_PHYSICAL_HISTORY_RESERVED_BYTES": str(1 << 30),
        "QUANTA_INDEX_LEGACY_PHYSICAL_HISTORY_RESERVED_FILES": "10000",
        "QUANTA_INDEX_LEGACY_PHYSICAL_DURABLE_CONTROL_FILE_MAX_BYTES": str(1 << 20),
        "QUANTA_INDEX_LEGACY_PHYSICAL_REPOMAP_CONTROL_FILE_MAX_BYTES": str(64 << 20),
        "QUANTA_INDEX_LEGACY_PHYSICAL_REPOMAP_CANDIDATE_FILE_MAX_BYTES": str(512 << 20),
        "QUANTA_INDEX_LEGACY_PHYSICAL_CATALOG_PARENT_RESERVED_BYTES": str(2 << 30),
        "QUANTA_INDEX_LEGACY_PHYSICAL_CATALOG_PARENT_RESERVED_FILES": "10000",
    }
    # Ambient environment is consulted only for binary selection above.
    launcher_env = {key: environment[key] for key in ("HOME", "TMPDIR", "PATH")}
    old_seed, new_seed = base / "old-seed", base / "new-seed"
    old_root, new_root = base / "old-current", base / "new-current"
    _boot_fresh_seed(binary, old_seed, environment, base / "old-seed.log")
    _boot_fresh_seed(binary, new_seed, environment, base / "new-seed.log")
    _make_verified_current_root(binary, old_seed, old_root, environment)
    _make_verified_current_root(binary, new_seed, new_root, environment)

    selection = deployment / "selection.json"
    _install(selection, binary, old_root, environment, launcher_env)
    _verify_bootstrap(selection, launcher_env)

    owned = _OwnedProcess(
        [sys.executable, str(LAUNCHER), "serve", "--selection", str(selection)],
        launcher_env,
        base / "old-selected.log",
    )
    try:
        _wait_serving(owned, old_root, launcher=True)
        assert not _serves(new_root), "new root became live before selection"
        old_wire = selection.read_bytes()
        refused = _run(
            [
                sys.executable,
                str(LAUNCHER),
                "install",
                "--selection",
                str(selection),
                "--binary",
                str(binary),
                "--state-root",
                str(new_root),
                *(
                    part
                    for name, value in environment.items()
                    for part in ("--env", f"{name}={value}")
                ),
            ],
            launcher_env,
        )
        assert refused.returncode == 1, (
            f"live replacement was not refused: {refused.returncode}"
            f"\nstdout:\n{refused.stdout}\nstderr:\n{refused.stderr}"
        )
        assert "deployment is in use" in refused.stderr
        assert selection.read_bytes() == old_wire, "busy install replaced the old selection"
        assert _serves(old_root) and not _serves(new_root)
        owned.stop()
        if _serves(old_root):
            owned.clean_stop = False
            raise AssertionError("old root remained live after launcher drain")
    finally:
        owned.cleanup()

    # The served root is mutable. Its stale bootstrap manifest is not a
    # restart gate; the product backup owner can still freeze it into a new,
    # independently verifiable artifact.
    _backup_mutable_root_and_verify(binary, old_root, base / "old-roundtrip", environment)
    _selection_wire(selection, binary, old_root, environment)
    _serve_once(selection, old_root, new_root, launcher_env, base / "old-restarted.log")
    _selection_wire(selection, binary, old_root, environment)

    _install(selection, binary, new_root, environment, launcher_env)
    _verify_bootstrap(selection, launcher_env)
    _serve_once(selection, new_root, old_root, launcher_env, base / "new-selected.log")
    _backup_mutable_root_and_verify(binary, new_root, base / "new-roundtrip", environment)
    _selection_wire(selection, binary, new_root, environment)
    _serve_once(selection, new_root, old_root, launcher_env, base / "new-restarted.log")
    _selection_wire(selection, binary, new_root, environment)
