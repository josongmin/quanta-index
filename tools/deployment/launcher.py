"""Durable, cooperative selection of one searchd executable and state root.

The deployment directory and the selected root must already exist. This tool
does not create, migrate, or repair a state root. Every invocation that changes
or serves a selection must use the same deployment directory and lock file.
Install and verify-bootstrap use searchd verify-state only for a pristine
bootstrap artifact. Normal serving and restart use the durable selection and
delegate mutable-root validation and recovery to searchd's boot owners;
bootstrap verification is not a restart gate.
External same-UID mutation of an executable between hash and exec is outside
this cooperative contract; path execution does not prove inode-atomic exec.
"""

from __future__ import annotations

import argparse
import contextlib
import fcntl
import hashlib
import json
import os
import re
import signal
import stat
import subprocess
import sys
import tempfile
import threading
import time
from collections.abc import Iterator, Mapping
from dataclasses import dataclass
from pathlib import Path
from typing import Callable

FORMAT_VERSION = 1
MAX_SELECTION_BYTES = 64 * 1024
# searchd's hard drain deadline is 125 seconds. Allow five seconds for exit.
DEFAULT_SHUTDOWN_TIMEOUT_SECONDS = 130.0
DEPLOYMENT_LOCK_NAME = ".quanta-index-deployment.lock"
_DIGEST = re.compile(r"[0-9a-f]{64}\Z")
_FIELDS = {"schema_version", "binary", "binary_sha256", "state_root", "environment"}
# SearchdConfig's current env lookup surface plus explicit process necessities.
# Unknown keys are refused so a stale or misspelled policy cannot be silently
# carried into selected searchd commands. Nothing is inherited from os.environ.
_CONFIG_ENV = frozenset(
    """QUANTA_INDEX_CACHE_ROOT QUANTA_INDEX_CONTROL_SOCKET_ACCESS
    QUANTA_INDEX_EMBEDDER QUANTA_INDEX_EMBED_BATCH QUANTA_INDEX_EMBED_CACHE
    QUANTA_INDEX_EMBED_CACHE_MAX_AGE_SECS QUANTA_INDEX_EMBED_CACHE_MAX_BYTES
    QUANTA_INDEX_EMBED_CACHE_MAX_ENTRIES QUANTA_INDEX_EMBED_CACHE_MAX_NAMESPACES
    QUANTA_INDEX_EMBED_CACHE_MAX_TOTAL_BYTES QUANTA_INDEX_EMBED_CONCURRENCY
    QUANTA_INDEX_EMBED_DIM QUANTA_INDEX_EMBED_MAX_EST_TOKENS
    QUANTA_INDEX_EMBED_MAX_RETRIES QUANTA_INDEX_EMBED_MODEL
    QUANTA_INDEX_EMBED_MODEL_DIR QUANTA_INDEX_EMBED_MODEL_REVISION
    QUANTA_INDEX_EMBED_TIMEOUT_SECS QUANTA_INDEX_EXPERIMENTAL_HYBRID_FETCH_FLOOR
    QUANTA_INDEX_INGEST_MAX_RECORDS QUANTA_INDEX_INGEST_MAX_TEXT_BYTES
    QUANTA_INDEX_INGEST_MAX_VECTOR_BYTES QUANTA_INDEX_INGEST_SOCKET_ACCESS
    QUANTA_INDEX_INTEGRITY_SCRUB_INTERVAL_MS QUANTA_INDEX_INTEGRITY_SCRUB_MAX_BYTES_PER_STEP
    QUANTA_INDEX_LEXICAL_MAX_EXAMINED_CANDIDATES QUANTA_INDEX_LEXICAL_WRITER_ENVELOPE_BYTES
    QUANTA_INDEX_LEXICAL_WRITER_HEAP_BYTES QUANTA_INDEX_LEXICAL_WRITER_IDLE_SECS
    QUANTA_INDEX_MAINTENANCE_TICK_MS QUANTA_INDEX_PROCESS_MEMORY_CEILING_BYTES
    QUANTA_INDEX_PROCESS_RSS_CEILING_BYTES QUANTA_INDEX_PROVIDER_COST_CEILING_MICROS
    QUANTA_INDEX_PROVIDER_ENDPOINT QUANTA_INDEX_PROVIDER_MAX_INFLIGHT_BYTES
    QUANTA_INDEX_PROVIDER_MAX_INFLIGHT_REQUESTS QUANTA_INDEX_PROVIDER_MAX_RETRY_ATTEMPTS
    QUANTA_INDEX_PROVIDER_PROFILE QUANTA_INDEX_PROVIDER_REGION QUANTA_INDEX_PROVIDER_RETENTION
    QUANTA_INDEX_PROVIDER_SOURCE_CONTENT_CONSENT QUANTA_INDEX_PROVIDER_TENANT
    QUANTA_INDEX_QUERY_DISPATCH_BUDGET_MS QUANTA_INDEX_QUERY_DISPATCH_SLOTS
    QUANTA_INDEX_QUERY_MAX_CONNECTIONS QUANTA_INDEX_QUERY_MAX_IN_FLIGHT_PER_REPO
    QUANTA_INDEX_QUERY_QUEUE_WAIT_MS QUANTA_INDEX_QUERY_RESPONSE_MAX_BYTES
    QUANTA_INDEX_QUERY_SOCKET_ACCESS QUANTA_INDEX_QUERY_STAGE_OBSERVATION
    QUANTA_INDEX_REGEX_CACHE_MAX_ENTRIES QUANTA_INDEX_REGEX_CACHE_MAX_MATCHES_PER_ENTRY
    QUANTA_INDEX_REGEX_CACHE_MAX_RESIDENT_BYTES QUANTA_INDEX_SEARCH_CORPUS_HISTORY_MAX_BYTES
    QUANTA_INDEX_SEARCH_CORPUS_HISTORY_MAX_GENERATIONS
    QUANTA_INDEX_SEARCH_CORPUS_HISTORY_MAX_REVISION_PAIRS
    QUANTA_INDEX_SEARCH_CORPUS_HISTORY_MAX_TOTAL_BYTES
    QUANTA_INDEX_SEMANTIC_STREAM_WINDOW_SCOPES QUANTA_INDEX_SEMANTIC_STREAM_WINDOW_VECTOR_BYTES
    QUANTA_INDEX_SNAPSHOT_MAX_ENTRIES QUANTA_INDEX_SNAPSHOT_MAX_RESIDENT_BYTES
    QUANTA_INDEX_SOURCE_PUBLICATION_MAX_BYTES OPENAI_API_KEY""".split()
)
_PROCESS_ENV = frozenset({"HOME", "PATH", "TMPDIR", "XDG_CACHE_HOME", "LANG", "LC_ALL"})
_ALLOWED_ENV = _CONFIG_ENV | _PROCESS_ENV
FaultHook = Callable[[str], None]


class SelectionError(Exception):
    """The selection, custody, or selected binary was refused."""


class DeploymentBusy(SelectionError):
    """Another cooperating launcher or publisher holds this deployment."""


class PersistenceUncertain(SelectionError):
    """The rename may have happened; inspect the durable selection before retry."""


@dataclass(frozen=True)
class Selection:
    binary: Path
    binary_sha256: str
    state_root: Path
    environment: tuple[tuple[str, str], ...]

    def to_wire(self) -> dict[str, object]:
        return {
            "schema_version": FORMAT_VERSION,
            "binary": str(self.binary),
            "binary_sha256": self.binary_sha256,
            "state_root": str(self.state_root),
            "environment": dict(self.environment),
        }

    def child_environment(self) -> dict[str, str]:
        # subprocess receives this exact map; the launcher's ambient env is
        # never copied into verification or serving.
        return dict(self.environment)


def _canonical_existing(value: str | Path, *, directory: bool) -> Path:
    path = Path(value)
    if "\x00" in str(path):
        raise SelectionError("path contains NUL")
    if not path.is_absolute():
        raise SelectionError(f"path must be absolute: {path}")
    try:
        canonical = path.resolve(strict=True)
    except (OSError, RuntimeError) as error:
        raise SelectionError(f"cannot resolve {path}: {error}") from error
    if directory and not canonical.is_dir():
        raise SelectionError(f"not a directory: {canonical}")
    if not directory and not canonical.is_file():
        raise SelectionError(f"not a regular file: {canonical}")
    return canonical


def _environment(value: Mapping[str, str]) -> tuple[tuple[str, str], ...]:
    if not isinstance(value, dict):
        raise SelectionError("environment must be a JSON object")
    pairs = []
    for name, setting in value.items():
        if not isinstance(name, str) or name not in _ALLOWED_ENV:
            raise SelectionError(f"unknown environment/config key: {name!r}")
        if not isinstance(setting, str) or "\x00" in setting:
            raise SelectionError(f"invalid environment value for {name}")
        pairs.append((name, setting))
    return tuple(sorted(pairs))


def selection_from_wire(wire: object) -> Selection:
    if not isinstance(wire, dict) or set(wire) != _FIELDS:
        raise SelectionError("selection fields differ from format version 1")
    if type(wire["schema_version"]) is not int or wire["schema_version"] != FORMAT_VERSION:
        raise SelectionError("unsupported selection format")
    binary = wire["binary"]
    state_root = wire["state_root"]
    digest = wire["binary_sha256"]
    if not isinstance(binary, str) or not isinstance(state_root, str):
        raise SelectionError("selection paths must be strings")
    if not isinstance(digest, str) or _DIGEST.fullmatch(digest) is None:
        raise SelectionError("binary_sha256 must be lowercase SHA-256")
    canonical_binary = _canonical_existing(binary, directory=False)
    canonical_root = _canonical_existing(state_root, directory=True)
    if str(canonical_binary) != binary or str(canonical_root) != state_root:
        raise SelectionError("selection paths are not canonical")
    return Selection(
        canonical_binary,
        digest,
        canonical_root,
        _environment(wire["environment"]),
    )


def proposed_selection(
    binary: str | Path, state_root: str | Path, environment: Mapping[str, str]
) -> Selection:
    canonical_binary = _canonical_existing(binary, directory=False)
    canonical_root = _canonical_existing(state_root, directory=True)
    return Selection(
        canonical_binary,
        _sha256_binary(canonical_binary),
        canonical_root,
        _environment(environment),
    )


def _sha256_binary(binary: Path) -> str:
    flags = os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0)
    try:
        descriptor = os.open(binary, flags)
        with os.fdopen(descriptor, "rb") as stream:
            metadata = os.fstat(stream.fileno())
            if not stat.S_ISREG(metadata.st_mode):
                raise SelectionError("selected binary is not regular")
            if not os.access(binary, os.X_OK):
                raise SelectionError("selected binary is not executable")
            digest = hashlib.sha256()
            for chunk in iter(lambda: stream.read(1024 * 1024), b""):
                digest.update(chunk)
            return digest.hexdigest()
    except OSError as error:
        raise SelectionError(f"cannot inspect selected binary {binary}: {error}") from error


def _check_binary(selection: Selection) -> None:
    if _canonical_existing(selection.binary, directory=False) != selection.binary:
        raise SelectionError("selected binary path changed")
    if _sha256_binary(selection.binary) != selection.binary_sha256:
        raise SelectionError("selected binary SHA-256 changed")


def _selection_path(path: str | Path) -> Path:
    path = Path(path)
    if "\x00" in str(path):
        raise SelectionError("selection path contains NUL")
    if not path.is_absolute() or path.name in {"", ".", ".."}:
        raise SelectionError("selection path must be an absolute file path")
    parent = _canonical_existing(path.parent, directory=True)
    if parent != path.parent:
        raise SelectionError("selection parent is not canonical")
    metadata = parent.stat()
    if metadata.st_uid != os.geteuid() or metadata.st_mode & 0o022:
        raise SelectionError(
            "selection parent must be owned by this user and not group/world writable"
        )
    return path


def _check_control_file(metadata: os.stat_result, label: str) -> None:
    if (
        not stat.S_ISREG(metadata.st_mode)
        or metadata.st_uid != os.geteuid()
        or metadata.st_nlink != 1
        or metadata.st_mode & 0o077
    ):
        raise SelectionError(f"{label} must be private, singly linked, and owned by this user")


def _check_existing_selection(path: Path) -> None:
    try:
        metadata = path.lstat()
    except FileNotFoundError:
        return
    _check_control_file(metadata, "existing selection")


@contextlib.contextmanager
def deployment_lock(path: str | Path) -> Iterator[int]:
    selection_path = _selection_path(path)
    lock_path = selection_path.parent / DEPLOYMENT_LOCK_NAME
    flags = os.O_CREAT | os.O_RDWR | getattr(os, "O_NOFOLLOW", 0)
    try:
        descriptor = os.open(lock_path, flags, 0o600)
    except OSError as error:
        raise SelectionError(f"cannot open deployment lock: {error}") from error
    try:
        _check_control_file(os.fstat(descriptor), "deployment lock")
        try:
            fcntl.flock(descriptor, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError as error:
            raise DeploymentBusy(f"deployment is in use: {selection_path}") from error
        yield descriptor
    finally:
        os.close(descriptor)


def _reject_duplicate_fields(pairs: list[tuple[str, object]]) -> dict[str, object]:
    result: dict[str, object] = {}
    for key, value in pairs:
        if key in result:
            raise SelectionError(f"duplicate selection field: {key}")
        result[key] = value
    return result


def _read_selection_locked(path: Path) -> Selection:
    flags = os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0)
    try:
        descriptor = os.open(path, flags)
        with os.fdopen(descriptor, "rb") as stream:
            _check_control_file(os.fstat(stream.fileno()), "selection")
            contents = stream.read(MAX_SELECTION_BYTES + 1)
    except OSError as error:
        raise SelectionError(f"cannot read selection {path}: {error}") from error
    if len(contents) > MAX_SELECTION_BYTES:
        raise SelectionError("selection exceeds byte bound")
    try:
        wire = json.loads(contents.decode("utf-8"), object_pairs_hook=_reject_duplicate_fields)
    except (UnicodeError, ValueError) as error:
        raise SelectionError(f"invalid selection JSON: {error}") from error
    return selection_from_wire(wire)


def _run_selected_command(
    selection: Selection,
    operation: str,
    lock_fd: int,
    *,
    shutdown_timeout_seconds: float = DEFAULT_SHUTDOWN_TIMEOUT_SECONDS,
) -> int:
    _check_binary(selection)
    if threading.current_thread() is not threading.main_thread():
        raise SelectionError("deployment command requires the main thread for signal custody")
    if not 0 < shutdown_timeout_seconds < float("inf"):
        raise SelectionError("shutdown timeout must be finite and positive")
    args = [str(selection.binary), operation]
    if operation == "verify-state":
        args.extend(("--source", str(selection.state_root)))
    elif operation == "serve":
        args.extend(("--state-root", str(selection.state_root)))
    else:
        raise SelectionError(f"unsupported operation: {operation}")
    child: subprocess.Popen[bytes] | None = None
    pending_signals: list[int] = []
    first_signal_deadline: float | None = None

    def forward(signum: int) -> None:
        nonlocal first_signal_deadline
        if first_signal_deadline is None:
            first_signal_deadline = time.monotonic() + shutdown_timeout_seconds
        if child is None:
            pending_signals.append(signum)
            return
        try:
            os.killpg(child.pid, signum)
        except ProcessLookupError:
            pass

    def handle_signal(signum: int, _frame: object) -> None:
        # Each operator signal has one forward. A first SIGINT must not cause
        # an implicit SIGTERM while searchd is draining.
        forward(signum)

    old_int = signal.getsignal(signal.SIGINT)
    old_term = signal.getsignal(signal.SIGTERM)
    try:
        signal.signal(signal.SIGINT, handle_signal)
        signal.signal(signal.SIGTERM, handle_signal)
        try:
            child = subprocess.Popen(
                args,
                env=selection.child_environment(),
                cwd="/",
                close_fds=True,
                pass_fds=(lock_fd,),
                start_new_session=True,
            )
        except OSError as error:
            raise SelectionError(f"cannot start {operation}: {error}") from error
        for signum in pending_signals:
            forward(signum)
        pending_signals.clear()
        while True:
            remaining = (
                None if first_signal_deadline is None else first_signal_deadline - time.monotonic()
            )
            if remaining is not None and remaining <= 0:
                status = child.poll()
                if status is not None:
                    return status
                try:
                    os.killpg(child.pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
                child.wait()
                raise SelectionError(
                    f"{operation} exceeded {shutdown_timeout_seconds:g}s signal drain deadline; "
                    "owned process group was killed"
                )
            try:
                return child.wait(timeout=min(0.25, remaining) if remaining else 0.25)
            except subprocess.TimeoutExpired:
                continue
    finally:
        signal.signal(signal.SIGTERM, old_term)
        signal.signal(signal.SIGINT, old_int)


def _verify_bootstrap_locked(selection: Selection, lock_fd: int) -> None:
    status = _run_selected_command(selection, "verify-state", lock_fd)
    if status != 0:
        raise SelectionError(f"selected verify-state failed with exit status {status}")
    _check_binary(selection)


def verify_bootstrap_selection(path: str | Path) -> Selection:
    """Recheck a pristine bootstrap artifact; unsuitable after ordinary writes."""
    with deployment_lock(path) as lock_fd:
        selection = _read_selection_locked(_selection_path(path))
        _verify_bootstrap_locked(selection, lock_fd)
        return selection


def publish_selection(
    path: str | Path, selection: Selection, *, fault_hook: FaultHook | None = None
) -> None:
    path = _selection_path(path)
    # Validate even direct API callers, not only the CLI constructor.
    if selection_from_wire(selection.to_wire()) != selection:
        raise SelectionError("selection is not canonical")
    with deployment_lock(path) as lock_fd:
        _check_existing_selection(path)
        _verify_bootstrap_locked(selection, lock_fd)
        payload = (
            json.dumps(selection.to_wire(), sort_keys=True, separators=(",", ":")) + "\n"
        ).encode()
        if len(payload) > MAX_SELECTION_BYTES:
            raise SelectionError("selection exceeds byte bound")
        temporary: Path | None = None
        replaced = False
        try:
            descriptor, name = tempfile.mkstemp(prefix=f".{path.name}.tmp-", dir=path.parent)
            temporary = Path(name)
            with os.fdopen(descriptor, "wb") as stream:
                os.fchmod(stream.fileno(), 0o600)
                stream.write(payload)
                stream.flush()
                os.fsync(stream.fileno())
            if fault_hook is not None:
                fault_hook("after_file_fsync")
            _check_existing_selection(path)
            os.replace(temporary, path)
            replaced = True
            if fault_hook is not None:
                fault_hook("after_rename")
            directory = os.open(path.parent, os.O_RDONLY | getattr(os, "O_DIRECTORY", 0))
            try:
                os.fsync(directory)
            finally:
                os.close(directory)
            if fault_hook is not None:
                fault_hook("after_directory_fsync")
        except Exception as error:
            if replaced:
                raise PersistenceUncertain(
                    "selection rename occurred; inspect and verify before another publish"
                ) from error
            if isinstance(error, SelectionError):
                raise
            raise SelectionError(f"selection was not changed: {error}") from error
        finally:
            if temporary is not None and not replaced:
                temporary.unlink(missing_ok=True)


def serve_selection(path: str | Path) -> int:
    """Serve the selected root; searchd boot owns live validation and recovery."""
    with deployment_lock(path) as lock_fd:
        selection = _read_selection_locked(_selection_path(path))
        return _run_selected_command(selection, "serve", lock_fd)


def _cli_environment(values: list[str]) -> dict[str, str]:
    environment: dict[str, str] = {}
    for item in values:
        name, separator, value = item.partition("=")
        if not separator or name in environment:
            raise SelectionError(f"--env requires a unique NAME=VALUE: {item!r}")
        environment[name] = value
    _environment(environment)
    return environment


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    install = commands.add_parser(
        "install", help="verify a pristine bootstrap artifact and durably select it"
    )
    install.add_argument("--selection", type=Path, required=True)
    install.add_argument("--binary", type=Path, required=True)
    install.add_argument("--state-root", type=Path, required=True)
    install.add_argument("--env", action="append", default=[], metavar="NAME=VALUE")
    verify_bootstrap = commands.add_parser(
        "verify-bootstrap", help="recheck a pristine bootstrap artifact only"
    )
    verify_bootstrap.add_argument("--selection", type=Path, required=True)
    serve = commands.add_parser("serve", help="restart the selected root under searchd boot")
    serve.add_argument("--selection", type=Path, required=True)
    args = parser.parse_args(argv)
    try:
        if args.command == "install":
            selection = proposed_selection(args.binary, args.state_root, _cli_environment(args.env))
            publish_selection(args.selection, selection)
            return 0
        if args.command == "verify-bootstrap":
            verify_bootstrap_selection(args.selection)
            return 0
        return serve_selection(args.selection)
    except SelectionError as error:
        print(f"deployment launcher: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
