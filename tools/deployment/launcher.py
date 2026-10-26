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

FORMAT_VERSION = 2
MAX_SELECTION_BYTES = 64 * 1024
MAX_NATIVE_CONFIG_BYTES = 64 * 1024
MAX_NATIVE_RECEIPT_BYTES = 8 * 1024
# searchd's hard drain deadline is 125 seconds. Allow five seconds for exit.
DEFAULT_SHUTDOWN_TIMEOUT_SECONDS = 130.0
DEPLOYMENT_LOCK_NAME = ".quanta-index-deployment.lock"
_DIGEST = re.compile(r"[0-9a-f]{64}\Z")
_FIELDS = {"schema_version", "mode", "binary", "binary_sha256", "state_root", "environment"}
_NATIVE_FIELDS = _FIELDS | {"native_config", "native_config_sha256", "native_boot_receipt"}
_NATIVE_RECEIPT_FIELDS = {
    "root_nonce_hex",
    "manifest_operation",
    "manifest_object_hex",
    "source_cut_digest_hex",
    "install_catalog_digest_hex",
}
_OPERATION_FIELDS = {"workspace", "epoch", "producer", "stream", "stream_epoch", "sequence"}
_WORKSPACE_FIELDS = {"tenant", "collection", "worktree", "slot"}
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
    QUANTA_INDEX_LEGACY_PHYSICAL_MAX_BYTES QUANTA_INDEX_LEGACY_PHYSICAL_MAX_FILES
    QUANTA_INDEX_LEGACY_PHYSICAL_MAX_PENDING QUANTA_INDEX_LEGACY_PHYSICAL_MAX_ENTRIES
    QUANTA_INDEX_LEGACY_PHYSICAL_MAX_DEPTH QUANTA_INDEX_LEGACY_PHYSICAL_MAX_PENDING_FRAMES
    QUANTA_INDEX_LEGACY_PHYSICAL_MAX_READER_HANDLES QUANTA_INDEX_LEGACY_PHYSICAL_MAX_TRACKED_ENTRIES
    QUANTA_INDEX_LEGACY_PHYSICAL_MAX_PROCESS_ROOTS QUANTA_INDEX_LEGACY_PHYSICAL_GLOBAL_MAX_SQLITE_HANDLES
    QUANTA_INDEX_LEGACY_PHYSICAL_MAINTENANCE_RESERVED_BYTES QUANTA_INDEX_LEGACY_PHYSICAL_MAINTENANCE_RESERVED_FILES
    QUANTA_INDEX_LEGACY_PHYSICAL_CONTROL_RESERVED_BYTES QUANTA_INDEX_LEGACY_PHYSICAL_CONTROL_RESERVED_FILES
    QUANTA_INDEX_LEGACY_PHYSICAL_WORK_RESERVED_BYTES QUANTA_INDEX_LEGACY_PHYSICAL_WORK_RESERVED_FILES
    QUANTA_INDEX_LEGACY_PHYSICAL_HISTORY_RESERVED_BYTES QUANTA_INDEX_LEGACY_PHYSICAL_HISTORY_RESERVED_FILES
    QUANTA_INDEX_LEGACY_PHYSICAL_DURABLE_CONTROL_FILE_MAX_BYTES
    QUANTA_INDEX_LEGACY_PHYSICAL_REPOMAP_CONTROL_FILE_MAX_BYTES
    QUANTA_INDEX_LEGACY_PHYSICAL_REPOMAP_CANDIDATE_FILE_MAX_BYTES
    QUANTA_INDEX_LEGACY_PHYSICAL_CATALOG_PARENT_RESERVED_BYTES QUANTA_INDEX_LEGACY_PHYSICAL_CATALOG_PARENT_RESERVED_FILES
    QUANTA_INDEX_LEXICAL_MAX_EXAMINED_CANDIDATES QUANTA_INDEX_LEXICAL_WRITER_ENVELOPE_BYTES
    QUANTA_INDEX_LEXICAL_MERGE_SOFT_RUNS QUANTA_INDEX_LEXICAL_MERGE_HARD_RUNS
    QUANTA_INDEX_LEXICAL_MERGE_SOFT_INDEX_BYTES QUANTA_INDEX_LEXICAL_MERGE_HARD_INDEX_BYTES
    QUANTA_INDEX_LEXICAL_MERGE_MAX_FAN_IN QUANTA_INDEX_LEXICAL_MERGE_MAX_INPUT_INDEX_BYTES
    QUANTA_INDEX_LEXICAL_STAGING_MAX_BYTES QUANTA_INDEX_LEXICAL_STAGING_MAX_AGE_MS
    QUANTA_INDEX_LEXICAL_POOL_MAX_ENCODED_BYTES QUANTA_INDEX_LEXICAL_POOL_MAX_FILES
    QUANTA_INDEX_RETENTION_MEASUREMENT_SCRATCH_BYTES QUANTA_INDEX_RETENTION_MEASUREMENT_TIMEOUT_MS
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


def _hex_digest(value: object, label: str) -> str:
    if not isinstance(value, str) or _DIGEST.fullmatch(value) is None:
        raise SelectionError(f"{label} must be lowercase SHA-256")
    return value


def _id16(value: object, label: str) -> list[int]:
    if (
        type(value) is not list
        or len(value) != 16
        or any(type(byte) is not int or byte < 0 or byte > 255 for byte in value)
    ):
        raise SelectionError(f"{label} must be exactly 16 bytes")
    return value


def _uint64(value: object, label: str) -> int:
    if type(value) is not int or value < 0 or value > (1 << 64) - 1:
        raise SelectionError(f"{label} must be a u64")
    return value


def _canonical_operation(value: object) -> str:
    if not isinstance(value, dict) or set(value) != _OPERATION_FIELDS:
        raise SelectionError("manifest_operation fields differ from OperationKey")
    workspace = value["workspace"]
    if not isinstance(workspace, dict) or set(workspace) != _WORKSPACE_FIELDS:
        raise SelectionError("manifest_operation workspace fields differ from WorkspaceKey")
    for name in sorted(_WORKSPACE_FIELDS):
        _id16(workspace[name], f"manifest_operation.workspace.{name}")
    for name in ("producer", "stream"):
        _id16(value[name], f"manifest_operation.{name}")
    for name in ("epoch", "stream_epoch", "sequence"):
        _uint64(value[name], f"manifest_operation.{name}")
    return json.dumps(value, sort_keys=True, separators=(",", ":"))


def _published_handoff_operation(raw: str, label: str) -> str:
    try:
        value = json.loads(raw, object_pairs_hook=_reject_duplicate_fields)
    except (TypeError, ValueError) as error:
        raise SelectionError(f"invalid {label} JSON: {error}") from error
    return _canonical_operation(value)


@dataclass(frozen=True)
class NativeBootReceipt:
    root_nonce_hex: str
    manifest_operation_json: str
    manifest_object_hex: str
    source_cut_digest_hex: str
    install_catalog_digest_hex: str

    @classmethod
    def from_wire(cls, wire: object) -> NativeBootReceipt:
        if not isinstance(wire, dict) or set(wire) != _NATIVE_RECEIPT_FIELDS:
            raise SelectionError("native boot receipt fields differ from format version")
        nonce = _hex_digest(wire["root_nonce_hex"], "root_nonce_hex")
        source = _hex_digest(wire["source_cut_digest_hex"], "source_cut_digest_hex")
        if nonce == "0" * 64 or source == "0" * 64:
            raise SelectionError("native boot receipt has a zero identity digest")
        return cls(
            nonce,
            _canonical_operation(wire["manifest_operation"]),
            _hex_digest(wire["manifest_object_hex"], "manifest_object_hex"),
            source,
            _hex_digest(wire["install_catalog_digest_hex"], "install_catalog_digest_hex"),
        )

    def to_wire(self) -> dict[str, object]:
        return {
            "root_nonce_hex": self.root_nonce_hex,
            "manifest_operation": json.loads(self.manifest_operation_json),
            "manifest_object_hex": self.manifest_object_hex,
            "source_cut_digest_hex": self.source_cut_digest_hex,
            "install_catalog_digest_hex": self.install_catalog_digest_hex,
        }


@dataclass(frozen=True)
class Selection:
    binary: Path
    binary_sha256: str
    state_root: Path
    environment: tuple[tuple[str, str], ...]
    native_config: Path | None = None
    native_config_sha256: str | None = None
    native_boot_receipt: NativeBootReceipt | None = None

    def to_wire(self) -> dict[str, object]:
        wire: dict[str, object] = {
            "schema_version": FORMAT_VERSION,
            "mode": "native" if self.native_config else "legacy",
            "binary": str(self.binary),
            "binary_sha256": self.binary_sha256,
            "state_root": str(self.state_root),
            "environment": dict(self.environment),
        }
        if self.native_config is not None:
            wire.update(
                mode="native",
                native_config=str(self.native_config),
                native_config_sha256=self.native_config_sha256,
                native_boot_receipt=self.native_boot_receipt.to_wire()
                if self.native_boot_receipt is not None
                else None,
            )
        return wire

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
    if not isinstance(wire, dict):
        raise SelectionError("selection must be a JSON object")
    version = wire.get("schema_version")
    if type(version) is not int or version != FORMAT_VERSION:
        raise SelectionError("unsupported selection format")
    mode = wire.get("mode")
    if mode not in ("legacy", "native"):
        raise SelectionError("unsupported selection mode")
    if set(wire) != (_FIELDS if mode == "legacy" else _NATIVE_FIELDS):
        raise SelectionError("selection fields differ from format version")
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
    native_config: Path | None = None
    native_config_sha256: str | None = None
    native_boot_receipt: NativeBootReceipt | None = None
    if mode == "native":
        config_path = wire["native_config"]
        config_digest = wire["native_config_sha256"]
        if not isinstance(config_path, str) or not isinstance(config_digest, str):
            raise SelectionError("native config path and digest must be strings")
        if _DIGEST.fullmatch(config_digest) is None:
            raise SelectionError("native_config_sha256 must be lowercase SHA-256")
        native_config = _canonical_existing(config_path, directory=False)
        if str(native_config) != config_path:
            raise SelectionError("native config path is not canonical")
        native_config_sha256 = config_digest
        native_boot_receipt = NativeBootReceipt.from_wire(wire["native_boot_receipt"])
        _check_native_config(native_config, canonical_root, config_digest)
    return Selection(
        canonical_binary,
        digest,
        canonical_root,
        _environment(wire["environment"]),
        native_config,
        native_config_sha256,
        native_boot_receipt,
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


def proposed_native_selection(
    binary: str | Path,
    state_root: str | Path,
    native_config: str | Path,
    environment: Mapping[str, str],
    receipt: NativeBootReceipt,
) -> Selection:
    canonical_binary = _canonical_existing(binary, directory=False)
    canonical_root = _canonical_existing(state_root, directory=True)
    canonical_config = _canonical_existing(native_config, directory=False)
    config_digest = _check_native_config(canonical_config, canonical_root)
    return Selection(
        canonical_binary,
        _sha256_binary(canonical_binary),
        canonical_root,
        _environment(environment),
        canonical_config,
        config_digest,
        NativeBootReceipt.from_wire(receipt.to_wire()),
    )


def _check_native_config(config: Path, state_root: Path, expected_digest: str | None = None) -> str:
    if _canonical_existing(config, directory=False) != config:
        raise SelectionError("native config path changed")
    flags = os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0) | getattr(os, "O_NONBLOCK", 0)
    try:
        descriptor = os.open(config, flags)
        with os.fdopen(descriptor, "rb") as stream:
            metadata = os.fstat(stream.fileno())
            _check_control_file(metadata, "native config")
            content = stream.read(MAX_NATIVE_CONFIG_BYTES + 1)
    except OSError as error:
        raise SelectionError(f"cannot read native config {config}: {error}") from error
    if len(content) > MAX_NATIVE_CONFIG_BYTES:
        raise SelectionError("native config exceeds byte bound")
    digest = hashlib.sha256(content).hexdigest()
    if expected_digest is not None and digest != expected_digest:
        raise SelectionError("native config SHA-256 changed")
    try:
        config_wire = json.loads(
            content.decode("utf-8"), object_pairs_hook=_reject_duplicate_fields
        )
    except (UnicodeError, ValueError) as error:
        raise SelectionError(f"invalid native config JSON: {error}") from error
    if not isinstance(config_wire, dict) or config_wire.get("state_root") != str(state_root):
        raise SelectionError("native config state_root differs from selected root")
    return digest


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


def _read_native_receipt(path: str | Path) -> NativeBootReceipt:
    canonical = _canonical_existing(path, directory=False)
    flags = os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0)
    try:
        descriptor = os.open(canonical, flags)
        with os.fdopen(descriptor, "rb") as stream:
            _check_control_file(os.fstat(stream.fileno()), "native receipt")
            payload = stream.read(MAX_NATIVE_RECEIPT_BYTES + 1)
    except OSError as error:
        raise SelectionError(f"cannot read native receipt {canonical}: {error}") from error
    if len(payload) > MAX_NATIVE_RECEIPT_BYTES:
        raise SelectionError("native receipt exceeds byte bound")
    try:
        wire = json.loads(payload.decode("utf-8"), object_pairs_hook=_reject_duplicate_fields)
    except (UnicodeError, ValueError) as error:
        raise SelectionError(f"invalid native receipt JSON: {error}") from error
    return NativeBootReceipt.from_wire(wire)


def _native_identity_args(receipt: NativeBootReceipt, *, install: bool) -> list[str]:
    args = [
        "--root-nonce",
        receipt.root_nonce_hex,
        "--manifest-operation",
        receipt.manifest_operation_json,
        "--manifest-object",
        receipt.manifest_object_hex,
        "--source-cut-digest",
        receipt.source_cut_digest_hex,
    ]
    if install:
        args.extend(("--catalog-content-digest", receipt.install_catalog_digest_hex))
    return args


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
    if selection.native_config is not None:
        _check_native_config(
            selection.native_config, selection.state_root, selection.native_config_sha256
        )
        if selection.native_config_sha256 is None:
            raise SelectionError("native selection is missing its pinned config digest")
    args = [str(selection.binary), operation]
    if operation == "verify-native-state" and selection.native_config is not None:
        args.extend(("--config", str(selection.native_config)))
        if selection.native_boot_receipt is None:
            raise SelectionError("native selection is missing its immutable boot receipt")
        args.extend(_native_identity_args(selection.native_boot_receipt, install=True))
        args.extend(("--config-digest", selection.native_config_sha256))
    elif operation == "serve-native" and selection.native_config is not None:
        args.extend(("--config", str(selection.native_config)))
        if selection.native_boot_receipt is None:
            raise SelectionError("native selection is missing its immutable boot receipt")
        args.extend(_native_identity_args(selection.native_boot_receipt, install=False))
        args.extend(("--config-digest", selection.native_config_sha256))
    elif operation == "verify-state" and selection.native_config is None:
        args.extend(("--source", str(selection.state_root)))
    elif operation == "serve" and selection.native_config is None:
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
    operation = "verify-native-state" if selection.native_config is not None else "verify-state"
    status = _run_selected_command(selection, operation, lock_fd)
    if status != 0:
        raise SelectionError(f"selected {operation} failed with exit status {status}")
    _check_binary(selection)
    if selection.native_config is not None:
        _check_native_config(
            selection.native_config, selection.state_root, selection.native_config_sha256
        )


def verify_bootstrap_selection(path: str | Path) -> Selection:
    """Recheck a pristine bootstrap artifact; unsuitable after ordinary writes."""
    with deployment_lock(path) as lock_fd:
        selection = _read_selection_locked(_selection_path(path))
        if selection.native_config is not None:
            raise SelectionError(
                "native verify-bootstrap is install-only; selected roots may contain valid "
                "incremental updates, so use the native boot identity and recovery gate"
            )
        _verify_bootstrap_locked(selection, lock_fd)
        return selection


def _selection_payload(selection: Selection) -> bytes:
    payload = (
        json.dumps(selection.to_wire(), sort_keys=True, separators=(",", ":")) + "\n"
    ).encode()
    if len(payload) > MAX_SELECTION_BYTES:
        raise SelectionError("selection exceeds byte bound")
    return payload


def _selection_fingerprint(selection: Selection) -> str:
    return hashlib.sha256(_selection_payload(selection)).hexdigest()


def inspect_selection(path: str | Path) -> dict[str, str]:
    """Return a CAS identity without exposing the selected environment secrets."""
    path = _selection_path(path)
    with deployment_lock(path):
        current = _read_selection_locked(path)
        return {
            "selection_sha256": _selection_fingerprint(current),
            "mode": "native" if current.native_config is not None else "legacy",
            "state_root": str(current.state_root),
        }


def _require_current_selection(path: Path, expected: str) -> Selection:
    expected = _hex_digest(expected, "expected_selection_sha256")
    current = _read_selection_locked(path)
    if _selection_fingerprint(current) != expected:
        raise SelectionError("current selection changed; inspect before switching")
    return current


def _previous_legacy_path(path: Path, previous_path: str | Path) -> Path:
    previous = _selection_path(previous_path)
    if previous.parent != path.parent or previous == path or previous.name == DEPLOYMENT_LOCK_NAME:
        raise SelectionError("previous Legacy selection must be a separate file in deployment")
    return previous


def _require_saved_legacy_selection(
    path: Path, previous_path: str | Path, expected_sha256: str
) -> Selection:
    previous = _previous_legacy_path(path, previous_path)
    expected = _hex_digest(expected_sha256, "expected_previous_selection_sha256")
    saved = _read_selection_locked(previous)
    if saved.native_config is not None or _selection_fingerprint(saved) != expected:
        raise SelectionError("previous Legacy selection is absent, changed, or not Legacy")
    return saved


def _require_preserve_destination(previous: Path, current: Selection) -> None:
    try:
        previous.lstat()
    except FileNotFoundError:
        return
    _check_existing_selection(previous)
    saved = _read_selection_locked(previous)
    if saved.native_config is not None or saved != current:
        raise SelectionError("previous Legacy selection differs from current CAS head")


def preserve_legacy_selection(
    path: str | Path,
    previous_path: str | Path,
    expected_current_sha256: str,
    *,
    fault_hook: FaultHook | None = None,
) -> str:
    """Durably preserve the selected Legacy head at a caller-chosen separate path."""
    path = _selection_path(path)
    previous = _previous_legacy_path(path, previous_path)
    with deployment_lock(path):
        current = _require_current_selection(path, expected_current_sha256)
        if current.native_config is not None:
            raise SelectionError("preserve-legacy requires a current Legacy selection")
        _check_binary(current)
        _require_preserve_destination(previous, current)

        def recheck_after_hook(stage: str) -> None:
            if fault_hook is not None:
                fault_hook(stage)
            if stage == "after_file_fsync":
                active = _require_current_selection(path, expected_current_sha256)
                if active.native_config is not None or active != current:
                    raise SelectionError("current Legacy selection changed before preservation")
                _check_binary(active)
                _require_preserve_destination(previous, active)

        # This publisher's expected_current_sha256 checks its destination,
        # not the active source. Recheck both paths in the wrapper instead.
        _publish_selection_locked(previous, current, fault_hook=recheck_after_hook)
        return _selection_fingerprint(current)


def _require_distinct_switch_roots(old_root: Path, new_root: Path) -> None:
    if (
        old_root == new_root
        or old_root.is_relative_to(new_root)
        or new_root.is_relative_to(old_root)
    ):
        raise SelectionError("switch roots must be distinct and non-nested")
    try:
        aliased = old_root.samefile(new_root)
    except OSError as error:
        raise SelectionError(f"cannot compare switch roots: {error}") from error
    if aliased:
        raise SelectionError("switch roots alias the same physical directory")


def publish_selection(
    path: str | Path, selection: Selection, *, fault_hook: FaultHook | None = None
) -> None:
    path = _selection_path(path)
    if selection_from_wire(selection.to_wire()) != selection:
        raise SelectionError("selection is not canonical")
    with deployment_lock(path) as lock_fd:
        _check_existing_selection(path)
        if path.exists():
            current = _read_selection_locked(path)
            if (current.native_config is None) != (selection.native_config is None):
                raise SelectionError(
                    "Legacy/Native selection transition requires a fresh-root cutover"
                )
            if current.state_root != selection.state_root and (
                current.native_config is not None or selection.native_config is not None
            ):
                raise SelectionError(
                    "native cross-root installation requires switch-native and selection CAS"
                )
        _verify_bootstrap_locked(selection, lock_fd)
        _publish_selection_locked(path, selection, fault_hook=fault_hook)


def switch_native_selection(
    path: str | Path,
    selection: Selection,
    expected_current_sha256: str,
    *,
    previous_selection_path: str | Path | None = None,
    expected_previous_selection_sha256: str | None = None,
    target_operation_json: str | None = None,
    published_operation_json: str | None = None,
    published_source_cut_digest_hex: str | None = None,
    fault_hook: FaultHook | None = None,
) -> None:
    """CAS switch from an offline Legacy or Native root to a verified Native root.

    The authoritative producer must have rebuilt and published the candidate.
    This consumer proves boot integrity, not parity between different SourceCuts.
    Native-to-Native rollback uses this CAS path with a fresh target receipt.
    Legacy cutover requires a caller-preserved exact private selection in the
    same deployment directory. Callers may preserve that head explicitly under
    its source CAS; this switch never chooses or creates a predecessor.
    """
    path = _selection_path(path)
    if selection_from_wire(selection.to_wire()) != selection:
        raise SelectionError("selection is not canonical")
    with deployment_lock(path) as lock_fd:
        current = _require_current_selection(path, expected_current_sha256)
        if selection.native_boot_receipt is None:
            raise SelectionError("switch-native candidate requires a Native receipt")
        handoff = (
            target_operation_json,
            published_operation_json,
            published_source_cut_digest_hex,
        )
        if any(axis is not None for axis in handoff):
            if not all(axis is not None for axis in handoff):
                raise SelectionError("Native publication handoff requires all three axes")
            target_operation = _published_handoff_operation(
                target_operation_json, "Native target operation"
            )
            published_operation = _published_handoff_operation(
                published_operation_json, "Native published operation"
            )
            published_source_cut_digest = _hex_digest(
                published_source_cut_digest_hex, "Native published SourceCut digest"
            )
            if (
                target_operation != published_operation
                or published_operation != selection.native_boot_receipt.manifest_operation_json
                or published_source_cut_digest
                != selection.native_boot_receipt.source_cut_digest_hex
            ):
                raise SelectionError(
                    "Native publication handoff differs from candidate boot receipt"
                )
        _require_distinct_switch_roots(current.state_root, selection.state_root)
        old_receipt, new_receipt = current.native_boot_receipt, selection.native_boot_receipt
        if old_receipt is not None:
            if (
                previous_selection_path is not None
                or expected_previous_selection_sha256 is not None
            ):
                raise SelectionError("Native switch does not accept a Legacy predecessor")
            if old_receipt.root_nonce_hex == new_receipt.root_nonce_hex:
                raise SelectionError("switch roots must have independent Native incarnations")
            old_operation = json.loads(old_receipt.manifest_operation_json)
            new_operation = json.loads(new_receipt.manifest_operation_json)
            if old_operation["workspace"] != new_operation["workspace"]:
                raise SelectionError("switch candidate belongs to another workspace")
            # A Native old root must still authenticate its actual Catalog bytes.
            # An updated old root must first be reselected with a fresh receipt.
            _verify_bootstrap_locked(current, lock_fd)
        else:
            if previous_selection_path is None or expected_previous_selection_sha256 is None:
                raise SelectionError("Legacy cutover requires the saved exact prior selection")
            saved = _require_saved_legacy_selection(
                path, previous_selection_path, expected_previous_selection_sha256
            )
            if saved != current:
                raise SelectionError("saved Legacy selection differs from current CAS head")
            # The selected Legacy root has no Native receipt and verify-state
            # only accepts a manifest-backed bootstrap/backup. The deployment
            # lock proves it has no cooperating launched server during this CAS.
            _check_binary(current)
        _verify_bootstrap_locked(selection, lock_fd)
        _publish_selection_locked(
            path, selection, fault_hook=fault_hook, expected_current_sha256=expected_current_sha256
        )


def rollback_legacy_selection(
    path: str | Path,
    previous_selection_path: str | Path,
    expected_current_sha256: str,
    expected_previous_selection_sha256: str,
    *,
    fault_hook: FaultHook | None = None,
) -> None:
    """CAS restore the caller-preserved Legacy selection after Native cutover.

    This verifies the current Native root and old binary. Legacy verify-state
    cannot verify an arbitrary updated service root. Search/Serve acceptance
    of the retained old root remains a product cutover condition.
    """
    path = _selection_path(path)
    with deployment_lock(path) as lock_fd:
        current = _require_current_selection(path, expected_current_sha256)
        if current.native_boot_receipt is None:
            raise SelectionError("Legacy rollback requires a current Native selection")
        saved = _require_saved_legacy_selection(
            path, previous_selection_path, expected_previous_selection_sha256
        )
        _require_distinct_switch_roots(current.state_root, saved.state_root)
        _check_binary(saved)
        _verify_bootstrap_locked(current, lock_fd)
        _publish_selection_locked(
            path, saved, fault_hook=fault_hook, expected_current_sha256=expected_current_sha256
        )


def _publish_selection_locked(
    path: Path,
    selection: Selection,
    *,
    fault_hook: FaultHook | None,
    expected_current_sha256: str | None = None,
) -> None:
    # The caller retains the one deployment lock through verification and rename.
    payload = _selection_payload(selection)
    temporary: Path | None = None
    rename_attempted = False
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
        if expected_current_sha256 is not None:
            _require_current_selection(path, expected_current_sha256)
        # A signal can arrive after rename succeeds but before Python
        # records its return. From this point, the selected authority is
        # uncertain until the durable file is inspected.
        rename_attempted = True
        os.replace(temporary, path)
        if fault_hook is not None:
            fault_hook("after_rename")
        directory = os.open(path.parent, os.O_RDONLY | getattr(os, "O_DIRECTORY", 0))
        try:
            os.fsync(directory)
        finally:
            os.close(directory)
        if fault_hook is not None:
            fault_hook("after_directory_fsync")
    except BaseException as error:
        if rename_attempted:
            raise PersistenceUncertain(
                "selection rename may have occurred; inspect and verify before another publish"
            ) from error
        if isinstance(error, SelectionError):
            raise
        if not isinstance(error, Exception):
            raise
        raise SelectionError(f"selection was not changed: {error}") from error
    finally:
        # After entering rename, leave any uncertain temporary inode for
        # inspection; cleanup must not mask the authority uncertainty.
        if temporary is not None and not rename_attempted:
            temporary.unlink(missing_ok=True)


def serve_selection(path: str | Path) -> int:
    """Serve the selected root; searchd boot owns live validation and recovery."""
    with deployment_lock(path) as lock_fd:
        selection = _read_selection_locked(_selection_path(path))
        return _run_selected_command(
            selection, "serve-native" if selection.native_config is not None else "serve", lock_fd
        )


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
    install_native = commands.add_parser(
        "install-native", help="verify a complete native root and select its pinned config"
    )
    install_native.add_argument("--selection", type=Path, required=True)
    install_native.add_argument("--binary", type=Path, required=True)
    install_native.add_argument("--state-root", type=Path, required=True)
    install_native.add_argument("--config", type=Path, required=True)
    install_native.add_argument(
        "--receipt",
        type=Path,
        required=True,
        help="private candidate receipt checked by verify-native-state",
    )
    install_native.add_argument("--env", action="append", default=[], metavar="NAME=VALUE")
    switch_native = commands.add_parser(
        "switch-native", help="CAS switch to a verified offline Native root"
    )
    switch_native.add_argument("--selection", type=Path, required=True)
    switch_native.add_argument("--expected-selection-sha256", required=True)
    switch_native.add_argument("--binary", type=Path, required=True)
    switch_native.add_argument("--state-root", type=Path, required=True)
    switch_native.add_argument("--config", type=Path, required=True)
    switch_native.add_argument("--receipt", type=Path, required=True)
    switch_native.add_argument("--env", action="append", default=[], metavar="NAME=VALUE")
    switch_native.add_argument("--previous-selection", type=Path)
    switch_native.add_argument("--expected-previous-selection-sha256")
    switch_native.add_argument("--target-operation")
    switch_native.add_argument("--published-operation")
    switch_native.add_argument("--published-source-cut-digest")
    rollback_legacy = commands.add_parser(
        "rollback-legacy", help="CAS restore a saved exact Legacy selection"
    )
    rollback_legacy.add_argument("--selection", type=Path, required=True)
    rollback_legacy.add_argument("--expected-selection-sha256", required=True)
    rollback_legacy.add_argument("--previous-selection", type=Path, required=True)
    rollback_legacy.add_argument("--expected-previous-selection-sha256", required=True)
    preserve_legacy = commands.add_parser(
        "preserve-legacy", help="CAS preserve the current Legacy selection at a separate path"
    )
    preserve_legacy.add_argument("--selection", type=Path, required=True)
    preserve_legacy.add_argument("--previous-selection", type=Path, required=True)
    preserve_legacy.add_argument("--expected-selection-sha256", required=True)
    inspect = commands.add_parser(
        "inspect-selection", help="show the selection CAS identity without secrets"
    )
    inspect.add_argument("--selection", type=Path, required=True)
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
        if args.command in ("install-native", "switch-native"):
            selection = proposed_native_selection(
                args.binary,
                args.state_root,
                args.config,
                _cli_environment(args.env),
                _read_native_receipt(args.receipt),
            )
            if args.command == "switch-native":
                switch_native_selection(
                    args.selection,
                    selection,
                    args.expected_selection_sha256,
                    previous_selection_path=args.previous_selection,
                    expected_previous_selection_sha256=args.expected_previous_selection_sha256,
                    target_operation_json=args.target_operation,
                    published_operation_json=args.published_operation,
                    published_source_cut_digest_hex=args.published_source_cut_digest,
                )
            else:
                publish_selection(args.selection, selection)
            return 0
        if args.command == "rollback-legacy":
            rollback_legacy_selection(
                args.selection,
                args.previous_selection,
                args.expected_selection_sha256,
                args.expected_previous_selection_sha256,
            )
            return 0
        if args.command == "preserve-legacy":
            previous_sha256 = preserve_legacy_selection(
                args.selection, args.previous_selection, args.expected_selection_sha256
            )
            print(json.dumps({"previous_selection_sha256": previous_sha256}, sort_keys=True))
            return 0
        if args.command == "inspect-selection":
            print(json.dumps(inspect_selection(args.selection), sort_keys=True))
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
