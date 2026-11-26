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
import ctypes
import errno
import fcntl
import hashlib
import json
import os
import re
import secrets
import signal
import socket as unix_socket
import stat
import struct
import subprocess
import sys
import tempfile
import threading
import time
from collections.abc import Iterator, Mapping
from dataclasses import dataclass, replace
from pathlib import Path
from typing import BinaryIO, Callable, Optional, TypeVar

FORMAT_VERSION = 2
MAX_SELECTION_BYTES = 64 * 1024
MAX_NATIVE_CONFIG_BYTES = 64 * 1024
MAX_NATIVE_RECEIPT_BYTES = 8 * 1024
# searchd's hard drain deadline is 125 seconds. Allow five seconds for exit.
DEFAULT_SHUTDOWN_TIMEOUT_SECONDS = 130.0
DEFAULT_CANDIDATE_PREPARE_TIMEOUT_SECONDS = 600.0
LINUX_UNIX_PATH_BYTES = 108  # sun_path includes the terminating NUL.
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
    QUANTA_INDEX_PROVIDER_OPENAI_MAX_REQUEST_COST_MICROS
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
ControlDocument = TypeVar("ControlDocument")
CandidateSourcePublication = tuple[
    str, str, str, int, Optional[str], Optional[tuple[str, str, int]]
]


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
    protected_source_routes_hex: str | None = None
    retired_policy_object_hex: str | None = None
    retired_policy_sha256_hex: str | None = None
    retired_policy_identity_count: int | None = None

    @classmethod
    def from_wire(cls, wire: object) -> NativeBootReceipt:
        if (
            not isinstance(wire, dict)
            or not _NATIVE_RECEIPT_FIELDS <= set(wire)
            or set(wire)
            - _NATIVE_RECEIPT_FIELDS
            - {
                "protected_source_routes_hex",
                "retired_policy_object_hex",
                "retired_policy_sha256_hex",
                "retired_policy_identity_count",
            }
        ):
            raise SelectionError("native boot receipt fields differ from format version")
        nonce = _hex_digest(wire["root_nonce_hex"], "root_nonce_hex")
        source = _hex_digest(wire["source_cut_digest_hex"], "source_cut_digest_hex")
        if nonce == "0" * 64 or source == "0" * 64:
            raise SelectionError("native boot receipt has a zero identity digest")
        routes = (
            _hex_digest(wire["protected_source_routes_hex"], "protected_source_routes_hex")
            if "protected_source_routes_hex" in wire
            else None
        )
        if routes == "0" * 64:
            raise SelectionError("native boot receipt has a zero protected Source route")
        policy_keys = {
            "retired_policy_object_hex",
            "retired_policy_sha256_hex",
            "retired_policy_identity_count",
        }
        policy_present = policy_keys & set(wire)
        if policy_present and policy_present != policy_keys:
            raise SelectionError("native boot receipt has incomplete retired policy reference")
        policy_object = (
            _hex_digest(wire["retired_policy_object_hex"], "retired policy object")
            if policy_present
            else None
        )
        policy_sha = (
            _hex_digest(wire["retired_policy_sha256_hex"], "retired policy SHA-256")
            if policy_present
            else None
        )
        policy_count = (
            _uint64(wire["retired_policy_identity_count"], "retired policy count")
            if policy_present
            else None
        )
        if (
            policy_object == "0" * 64
            or policy_sha == "0" * 64
            or (policy_count is not None and policy_count > 4096)
        ):
            raise SelectionError("native boot receipt has invalid retired policy reference")
        return cls(
            nonce,
            _canonical_operation(wire["manifest_operation"]),
            _hex_digest(wire["manifest_object_hex"], "manifest_object_hex"),
            source,
            _hex_digest(wire["install_catalog_digest_hex"], "install_catalog_digest_hex"),
            routes,
            policy_object,
            policy_sha,
            policy_count,
        )

    def to_wire(self) -> dict[str, object]:
        wire = {
            "root_nonce_hex": self.root_nonce_hex,
            "manifest_operation": json.loads(self.manifest_operation_json),
            "manifest_object_hex": self.manifest_object_hex,
            "source_cut_digest_hex": self.source_cut_digest_hex,
            "install_catalog_digest_hex": self.install_catalog_digest_hex,
        }
        if self.protected_source_routes_hex is not None:
            wire["protected_source_routes_hex"] = self.protected_source_routes_hex
        if self.retired_policy_object_hex is not None:
            wire["retired_policy_object_hex"] = self.retired_policy_object_hex
            wire["retired_policy_sha256_hex"] = self.retired_policy_sha256_hex
            wire["retired_policy_identity_count"] = self.retired_policy_identity_count
        return wire


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
            "schema_version": (
                3
                if self.native_boot_receipt is not None
                and self.native_boot_receipt.retired_policy_object_hex is not None
                else FORMAT_VERSION
            ),
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
    if type(version) is not int or version not in (FORMAT_VERSION, 3):
        raise SelectionError("unsupported selection format")
    mode = wire.get("mode")
    if mode not in ("legacy", "native"):
        raise SelectionError("unsupported selection mode")
    if version == 3 and mode != "native":
        raise SelectionError("schema-3 requires Native selection")
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
        if native_boot_receipt is not None and (
            (version == 3) != (native_boot_receipt.retired_policy_object_hex is not None)
        ):
            raise SelectionError("selection schema and retired policy reference differ")
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
    digest, _ = _read_native_config(config, state_root, expected_digest)
    return digest


def _read_native_config(
    config: Path, state_root: Path, expected_digest: str | None
) -> tuple[str, dict[str, object]]:
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
    return digest, config_wire


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
    if path.name == DEPLOYMENT_LOCK_NAME:
        raise SelectionError("selection path aliases the deployment lock")
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
    flags = os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0) | getattr(os, "O_NONBLOCK", 0)
    try:
        descriptor = os.open(path, flags)
        with os.fdopen(descriptor, "rb") as stream:
            _check_control_file(os.fstat(stream.fileno()), "selection")
            contents = stream.read(MAX_SELECTION_BYTES + 1)
    except OSError as error:
        raise SelectionError(f"cannot read selection {path}: {error}") from error
    if len(contents) > MAX_SELECTION_BYTES:
        raise SelectionError("selection exceeds byte bound")
    return _decode_selection(contents)


def _decode_selection(contents: bytes) -> Selection:
    try:
        wire = json.loads(contents.decode("utf-8"), object_pairs_hook=_reject_duplicate_fields)
    except (UnicodeError, ValueError) as error:
        raise SelectionError(f"invalid selection JSON: {error}") from error
    return selection_from_wire(wire)


def _read_native_receipt(path: str | Path) -> NativeBootReceipt:
    canonical = _canonical_existing(path, directory=False)
    flags = os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0) | getattr(os, "O_NONBLOCK", 0)
    try:
        descriptor = os.open(canonical, flags)
        with os.fdopen(descriptor, "rb") as stream:
            _check_control_file(os.fstat(stream.fileno()), "native receipt")
            payload = stream.read(MAX_NATIVE_RECEIPT_BYTES + 1)
    except OSError as error:
        raise SelectionError(f"cannot read native receipt {canonical}: {error}") from error
    if len(payload) > MAX_NATIVE_RECEIPT_BYTES:
        raise SelectionError("native receipt exceeds byte bound")
    return _decode_native_receipt(payload)


def _decode_native_receipt(payload: bytes) -> NativeBootReceipt:
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
    if receipt.protected_source_routes_hex is not None:
        args.extend(("--protected-source-routes", receipt.protected_source_routes_hex))
    if receipt.retired_policy_object_hex is not None:
        args.extend(
            (
                "--retired-policy-object",
                receipt.retired_policy_object_hex,
                "--retired-policy-sha256",
                receipt.retired_policy_sha256_hex,
                "--retired-policy-identity-count",
                str(receipt.retired_policy_identity_count),
            )
        )
    return args


def _run_selected_command(
    selection: Selection,
    operation: str,
    lock_fd: int,
    *,
    shutdown_timeout_seconds: float = DEFAULT_SHUTDOWN_TIMEOUT_SECONDS,
    candidate_receipt: tuple[Path, str, str, str, str | None, tuple[str, str, int] | None]
    | None = None,
    with_candidate: Callable[[subprocess.Popen[bytes], Callable[[], bool]], None] | None = None,
    fail_on_external_signal: bool = False,
    retirement_approval: tuple[Path, Path] | None = None,
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
    if (
        operation == "select-native-candidate"
        and selection.native_config is not None
        and candidate_receipt is not None
    ):
        path, nonce, published_operation, source_cut, protected_routes, retired_policy = (
            candidate_receipt
        )
        args.extend(
            (
                "--config",
                str(selection.native_config),
                "--config-digest",
                selection.native_config_sha256,
                "--root-nonce",
                nonce,
                "--manifest-operation",
                published_operation,
                "--source-cut-digest",
                source_cut,
                "--receipt",
                str(path),
            )
        )
        if protected_routes is not None:
            args.extend(("--protected-source-routes", protected_routes))
        if retired_policy is not None:
            args.extend(
                (
                    "--retired-policy-object",
                    retired_policy[0],
                    "--retired-policy-sha256",
                    retired_policy[1],
                    "--retired-policy-identity-count",
                    str(retired_policy[2]),
                )
            )
    elif (
        operation in {"verify-native-state", "verify-native-candidate"}
        and selection.native_config is not None
    ):
        args.extend(("--config", str(selection.native_config)))
        if selection.native_boot_receipt is None:
            raise SelectionError("native selection is missing its immutable boot receipt")
        args.extend(_native_identity_args(selection.native_boot_receipt, install=True))
        args.extend(("--config-digest", selection.native_config_sha256))
    elif operation == "serve-native-candidate" and selection.native_config is not None:
        if selection.native_boot_receipt is not None:
            raise SelectionError("candidate process requires an unselected root")
        args.extend(("--config", str(selection.native_config)))
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
    external_interrupted = False

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
        nonlocal external_interrupted
        # Each operator signal has one forward. A first SIGINT must not cause
        # an implicit SIGTERM while searchd is draining.
        external_interrupted = True
        forward(signum)

    def result_after_signal(status: int) -> int:
        if fail_on_external_signal and external_interrupted:
            raise SelectionError(f"{operation} interrupted")
        return status

    old_int = signal.getsignal(signal.SIGINT)
    old_term = signal.getsignal(signal.SIGTERM)
    try:
        signal.signal(signal.SIGINT, handle_signal)
        signal.signal(signal.SIGTERM, handle_signal)
        try:
            child_environment = selection.child_environment()
            if retirement_approval is not None:
                if operation != "serve-native":
                    raise SelectionError(
                        "retirement approval is only valid for selected Native serve"
                    )
                child_environment["QUANTA_NATIVE_RETIREMENT_APPROVAL_PATH_V2"] = str(
                    retirement_approval[0]
                )
                child_environment["QUANTA_NATIVE_RETIREMENT_DECISION_POLICY_PATH_V2"] = str(
                    retirement_approval[1]
                )
                child_environment["QUANTA_NATIVE_DEPLOYMENT_LOCK_FD_V2"] = str(lock_fd)
            child = subprocess.Popen(
                args,
                env=child_environment,
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
        if with_candidate is not None:
            try:
                if first_signal_deadline is not None:
                    raise SelectionError("candidate preparation interrupted")
                with_candidate(child, lambda: first_signal_deadline is not None)
                if first_signal_deadline is not None or child.poll() is not None:
                    raise SelectionError("candidate exited during preparation")
            finally:
                if first_signal_deadline is None and child.poll() is None:
                    forward(signal.SIGTERM)
                remaining = max(0.0, (first_signal_deadline or time.monotonic()) - time.monotonic())
                try:
                    child.wait(timeout=remaining)
                except subprocess.TimeoutExpired as error:
                    with contextlib.suppress(ProcessLookupError):
                        os.killpg(child.pid, signal.SIGKILL)
                    child.wait()
                    raise SelectionError("candidate exceeded its drain deadline") from error
            if external_interrupted:
                raise SelectionError("candidate preparation interrupted")
            return child.returncode
        while True:
            remaining = (
                None if first_signal_deadline is None else first_signal_deadline - time.monotonic()
            )
            if remaining is not None and remaining <= 0:
                status = child.poll()
                if status is not None:
                    return result_after_signal(status)
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
                return result_after_signal(
                    child.wait(timeout=min(0.25, remaining) if remaining else 0.25)
                )
            except subprocess.TimeoutExpired:
                continue
    finally:
        signal.signal(signal.SIGTERM, old_term)
        signal.signal(signal.SIGINT, old_int)


def _verify_bootstrap_locked(
    selection: Selection,
    lock_fd: int,
    *,
    candidate: bool = False,
) -> None:
    if candidate and selection.native_config is None:
        raise SelectionError("candidate verification requires a Native selection")
    operation = (
        "verify-native-candidate"
        if candidate
        else "verify-native-state"
        if selection.native_config is not None
        else "verify-state"
    )
    status = _run_selected_command(selection, operation, lock_fd, fail_on_external_signal=candidate)
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
    if previous.parent != path.parent or previous == path:
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
    with deployment_lock(path) as lock_fd:
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
        _publish_selection_locked(previous, current, lock_fd, fault_hook=recheck_after_hook)
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
            if (
                current.native_config is not None
                and current.native_boot_receipt != selection.native_boot_receipt
            ):
                raise SelectionError(
                    "installed Native baseline cannot be replaced by a reissued BOOT receipt"
                )
        _verify_bootstrap_locked(selection, lock_fd)
        _publish_selection_locked(path, selection, lock_fd, fault_hook=fault_hook)


def prepare_native_candidate(
    active_path: str | Path,
    candidate_path: str | Path,
    selection: Selection,
    expected_current_sha256: str,
    *,
    receipt_path: Path,
    root_nonce: str,
    manifest_operation: str,
    source_cut_digest: str,
) -> str:
    """Verify an offline candidate and persist its document without switching."""
    active, candidate, receipt_path = _candidate_paths(
        active_path, candidate_path, selection.state_root, receipt_path
    )
    with deployment_lock(candidate) as lock_fd:
        return _prepare_native_candidate_locked(
            active,
            candidate,
            selection,
            expected_current_sha256,
            receipt_path,
            root_nonce,
            manifest_operation,
            source_cut_digest,
            lock_fd,
        )


def _candidate_paths(
    active_path: str | Path, candidate_path: str | Path, state_root: Path, receipt_path: Path
) -> tuple[Path, Path, Path]:
    active = _selection_path(active_path)
    candidate = _selection_path(candidate_path)
    receipt_path = _selection_path(receipt_path)
    if (
        candidate == active
        or receipt_path in (active, candidate)
        or candidate.is_relative_to(state_root)
        or receipt_path.is_relative_to(state_root)
    ):
        raise SelectionError(
            "candidate and receipt must be separate control files outside the root"
        )
    if active.parent.samefile(candidate.parent) or receipt_path.parent != candidate.parent:
        raise SelectionError("candidate and receipt require one separate deployment directory")
    return active, candidate, receipt_path


def _require_candidate_source_selection(active: Path, expected: str, state_root: Path) -> None:
    current = _require_current_selection(active, expected)
    if current.native_config is not None and current.native_boot_receipt is None:
        raise SelectionError("active Native selection has no BOOT receipt")
    _require_distinct_switch_roots(current.state_root, state_root)


def _prepare_native_candidate_locked(
    active: Path,
    candidate: Path,
    selection: Selection,
    expected_current_sha256: str,
    receipt_path: Path,
    root_nonce: str,
    manifest_operation: str,
    source_cut_digest: str,
    lock_fd: int,
    protected_source_routes: str | None = None,
    retired_policy: tuple[str, str, int] | None = None,
) -> str:
    _require_candidate_source_selection(active, expected_current_sha256, selection.state_root)
    if not os.path.lexists(receipt_path):
        if os.path.lexists(candidate):
            raise SelectionError("retained candidate has no BOOT receipt")
        status = _run_selected_command(
            selection,
            "select-native-candidate",
            lock_fd,
            candidate_receipt=(
                receipt_path,
                root_nonce,
                manifest_operation,
                source_cut_digest,
                protected_source_routes,
                retired_policy,
            ),
            fail_on_external_signal=True,
        )
        if status != 0 and not os.path.lexists(receipt_path):
            raise SelectionError(f"select-native-candidate failed with exit status {status}")
    if receipt_path.is_symlink():
        raise SelectionError("candidate receipt must be a private file at the requested path")
    receipt = _read_native_receipt(receipt_path)
    if (
        receipt.root_nonce_hex != root_nonce
        or receipt.manifest_operation_json != manifest_operation
        or receipt.source_cut_digest_hex != source_cut_digest
        or receipt.protected_source_routes_hex != protected_source_routes
        or (
            receipt.retired_policy_object_hex,
            receipt.retired_policy_sha256_hex,
            receipt.retired_policy_identity_count,
        )
        != (retired_policy or (None, None, None))
    ):
        raise SelectionError("candidate receipt differs from expected publication identity")
    selection = replace(selection, native_boot_receipt=receipt)
    if selection_from_wire(selection.to_wire()) != selection:
        raise SelectionError("candidate must be a canonical Native selection")
    try:
        retained = _read_selection_locked(candidate)
    except SelectionError as error:
        if not isinstance(error.__cause__, FileNotFoundError):
            raise
        retained = None
    if retained is not None and retained != selection:
        raise SelectionError("retained candidate differs from requested selection")
    _verify_bootstrap_locked(selection, lock_fd, candidate=True)
    _require_current_selection(active, expected_current_sha256)
    _sync_retained_publication(
        receipt_path, lock_fd, receipt, MAX_NATIVE_RECEIPT_BYTES, _decode_native_receipt
    )
    if retained is None:
        _publish_selection_locked(candidate, selection, lock_fd, fault_hook=None, create_only=True)
    else:
        _sync_retained_publication(
            candidate, lock_fd, retained, MAX_SELECTION_BYTES, _decode_selection
        )
    return _selection_fingerprint(selection)


def _candidate_source_handoff(payload: bytes) -> CandidateSourcePublication:
    try:
        wire = json.loads(payload.decode("utf-8"), object_pairs_hook=_reject_duplicate_fields)
    except (UnicodeError, ValueError) as error:
        raise SelectionError("invalid candidate FULL handoff JSON") from error
    required = {
        "root_nonce_hex",
        "manifest_operation",
        "source_cut_digest_hex",
        "original_full_sequence",
    }
    policy_keys = {
        "retired_policy_object_hex",
        "retired_policy_sha256_hex",
        "retired_policy_identity_count",
    }
    if (
        not isinstance(wire, dict)
        or set(wire) - required - {"protected_source_routes_hex"} - policy_keys
        or not required <= set(wire)
        or policy_keys & set(wire) not in (set(), policy_keys)
    ):
        raise SelectionError("candidate FULL handoff has unexpected fields")
    root_nonce = _hex_digest(wire["root_nonce_hex"], "candidate root nonce")
    source_cut = _hex_digest(wire["source_cut_digest_hex"], "candidate SourceCut")
    protected_routes = (
        _hex_digest(wire["protected_source_routes_hex"], "candidate protected Source routes")
        if "protected_source_routes_hex" in wire
        else None
    )
    sequence = _uint64(wire["original_full_sequence"], "candidate Original FULL sequence")
    retired_policy = None
    if policy_keys <= set(wire):
        retired_policy = (
            _hex_digest(wire["retired_policy_object_hex"], "candidate retired policy object"),
            _hex_digest(wire["retired_policy_sha256_hex"], "candidate retired policy SHA-256"),
            _uint64(wire["retired_policy_identity_count"], "candidate retired policy count"),
        )
        if (
            retired_policy[0] == "0" * 64
            or retired_policy[1] == "0" * 64
            or retired_policy[2] > 4096
        ):
            raise SelectionError("candidate retired policy identity is invalid")
    if (
        not sequence
        or root_nonce == "0" * 64
        or source_cut == "0" * 64
        or protected_routes == "0" * 64
    ):
        raise SelectionError("candidate FULL handoff has an empty identity")
    return (
        root_nonce,
        _canonical_operation(wire["manifest_operation"]),
        source_cut,
        sequence,
        protected_routes,
        retired_policy,
    )


def _unreaped_child_exited(process: subprocess.Popen[bytes]) -> bool:
    """Observe S completion without releasing its PID and process-group identity."""
    if hasattr(os, "waitid"):
        return os.waitid(os.P_PID, process.pid, os.WEXITED | os.WNOHANG | os.WNOWAIT) is not None
    if sys.platform != "darwin":
        raise SelectionError("unreaped producer observation is unavailable")
    # macOS has waitid(2), but the system Python 3.9 does not expose os.waitid.
    libc = ctypes.CDLL(None, use_errno=True)
    waitid = libc.waitid
    waitid.argtypes = (ctypes.c_int, ctypes.c_uint, ctypes.c_void_p, ctypes.c_int)
    waitid.restype = ctypes.c_int
    info = (ctypes.c_int * 32)()
    if waitid(1, process.pid, ctypes.byref(info), 0x25):  # P_PID, WEXITED|WNOHANG|WNOWAIT
        code = ctypes.get_errno()
        raise OSError(code, os.strerror(code), str(process.pid))
    if info[3] not in (0, process.pid):  # Darwin siginfo_t.si_pid
        raise SelectionError("producer completion reports a different PID")
    return info[3] == process.pid


def _kill_candidate_producer_group(pid: int) -> None:
    try:
        os.killpg(pid, signal.SIGKILL)
    except ProcessLookupError:
        pass
    except PermissionError:
        if sys.platform != "darwin":
            raise
        # Darwin reports EPERM for a process group containing only an unreaped
        # zombie leader. The trusted one-shot producer has no live same-user
        # descendant in that case.


def _run_candidate_source(
    child: subprocess.Popen[bytes],
    stopping: Callable[[], bool],
    lock_fd: int,
    socket: Path,
    producer: Path,
    arguments: list[str],
    environment: dict[str, str],
    timeout_seconds: float,
) -> bytes:
    deadline = time.monotonic() + timeout_seconds

    _wait_candidate_endpoint(child, stopping, socket, deadline)
    with tempfile.TemporaryFile() as output:
        try:
            process = subprocess.Popen(
                [
                    str(producer),
                    "--prepare-native-candidate-full",
                    *arguments,
                    "--native-candidate-q-pid",
                    str(child.pid),
                ],
                env=environment,
                cwd="/",
                stdin=subprocess.DEVNULL,
                stdout=output,
                # If the launcher and Q exit while S is still settling FULL,
                # S must keep the candidate fenced against another invocation.
                close_fds=True,
                pass_fds=(lock_fd,),
                start_new_session=True,
            )
        except OSError as error:
            raise SelectionError(f"cannot start installed S candidate producer: {error}") from error
        group_terminated = False
        try:
            while not _unreaped_child_exited(process):
                _candidate_checkpoint(child, stopping, deadline)
                if os.fstat(output.fileno()).st_size > MAX_NATIVE_RECEIPT_BYTES:
                    raise SelectionError("candidate FULL handoff exceeds byte bound")
                time.sleep(0.05)
            _kill_candidate_producer_group(process.pid)
            group_terminated = True
            process.wait()
            if process.returncode != 0:
                raise SelectionError(
                    f"candidate FULL refused with exit status {process.returncode}"
                )
            _candidate_checkpoint(child, stopping, deadline)
            output.seek(0)
            payload = output.read(MAX_NATIVE_RECEIPT_BYTES + 1)
            if len(payload) > MAX_NATIVE_RECEIPT_BYTES:
                raise SelectionError("candidate FULL handoff exceeds byte bound")
            return payload
        finally:
            # The one-shot producer owns its entire isolated process group. A
            # successful leader exit must not leave a worker writing after BOOT.
            try:
                if not group_terminated:
                    _kill_candidate_producer_group(process.pid)
            finally:
                process.wait()


def _candidate_checkpoint(
    child: subprocess.Popen[bytes], stopping: Callable[[], bool], deadline: float
) -> None:
    if stopping() or child.poll() is not None:
        raise SelectionError("candidate searchd stopped during FULL preparation")
    if time.monotonic() >= deadline:
        raise SelectionError("candidate FULL preparation exceeded its deadline")


def _wait_candidate_endpoint(
    child: subprocess.Popen[bytes],
    stopping: Callable[[], bool],
    socket: Path,
    deadline: float,
) -> None:
    """Wait for the unselected Q listener before FULL or a drain-only retry."""

    while True:
        _candidate_checkpoint(child, stopping, deadline)
        try:
            _probe_candidate_socket(socket, child.pid)
        except (FileNotFoundError, ConnectionRefusedError, TimeoutError):
            time.sleep(0.05)
        except OSError as error:
            raise SelectionError(f"cannot connect to candidate Native endpoint: {error}") from error
        else:
            _candidate_checkpoint(child, stopping, deadline)
            return


def _probe_candidate_socket(socket: Path, expected_pid: int) -> None:
    """Use the same on-disk endpoint as Q when Linux needs a shorter address."""
    original = socket.lstat()
    _selection_path(socket)
    if not stat.S_ISSOCK(original.st_mode):
        raise SelectionError("candidate Native endpoint is not a socket")
    with contextlib.ExitStack() as custody:
        address = socket
        if sys.platform == "linux" and len(os.fsencode(socket)) >= LINUX_UNIX_PATH_BYTES:
            directory = os.open(socket.parent, os.O_PATH | os.O_DIRECTORY | os.O_NOFOLLOW)
            custody.callback(os.close, directory)
            address = Path(f"/proc/self/fd/{directory}") / socket.name
            if not os.path.samestat(original, address.lstat()):
                raise SelectionError("candidate Native endpoint changed before probe")
        with unix_socket.socket(unix_socket.AF_UNIX, unix_socket.SOCK_STREAM) as probe:
            probe.settimeout(0.1)
            probe.connect(str(address))
            if sys.platform == "linux":
                credential_layout = struct.Struct("@iII")
                pid, _, _ = credential_layout.unpack(
                    probe.getsockopt(
                        unix_socket.SOL_SOCKET, unix_socket.SO_PEERCRED, credential_layout.size
                    )
                )
                if pid != expected_pid:
                    raise SelectionError("candidate Native endpoint belongs to a different process")
        if not os.path.samestat(original, socket.lstat()):
            raise SelectionError("candidate Native endpoint changed during probe")


def build_native_candidate(
    active_path: Path,
    candidate_path: Path,
    selection: Selection,
    expected_current_sha256: str,
    *,
    receipt_path: Path,
    producer: Path,
    producer_sha256: str,
    producer_arguments: list[str],
    producer_environment: dict[str, str],
    timeout_seconds: float = DEFAULT_CANDIDATE_PREPARE_TIMEOUT_SECONDS,
) -> dict[str, object]:
    """Run the digest-pinned FULL producer, retain its handoff, reap Q, and seal BOOT."""
    active, candidate, receipt_path = _candidate_paths(
        active_path, candidate_path, selection.state_root, receipt_path
    )
    handoff = _selection_path(candidate.with_name(f".{candidate.name}.full-handoff.json"))
    drained = _selection_path(candidate.with_name(f".{candidate.name}.clean-drain.json"))
    if receipt_path in (handoff, drained) or not 0 < timeout_seconds < float("inf"):
        raise SelectionError("candidate handoff path and timeout must be valid")
    producer = _canonical_existing(producer, directory=False)
    if _sha256_binary(producer) != _hex_digest(producer_sha256, "S producer SHA-256"):
        raise SelectionError("S producer SHA-256 changed")
    if selection.native_config is None or selection.native_config_sha256 is None:
        raise SelectionError("candidate preparation requires a pinned Native config")
    _, config = _read_native_config(
        selection.native_config, selection.state_root, selection.native_config_sha256
    )
    socket_wire = config.get("socket_path")
    if not isinstance(socket_wire, str):
        raise SelectionError("Native candidate config has no socket path")
    socket = Path(socket_wire)
    if (
        "\x00" in socket_wire
        or not socket.is_absolute()
        or not socket.name
        or ".." in socket.parts
        or socket.name == DEPLOYMENT_LOCK_NAME
    ):
        raise SelectionError("Native candidate socket path must be absolute and canonical")
    # Q owns creation of the socket directory. Check its custody after bind,
    # and check immediately when the directory already exists.
    try:
        parent = socket.parent.resolve()
    except (OSError, RuntimeError) as error:
        raise SelectionError(f"cannot inspect Native candidate socket parent: {error}") from error
    if parent != socket.parent:
        raise SelectionError("Native candidate socket parent is not canonical")
    if parent.exists():
        _selection_path(socket)
    if len(producer_arguments) % 2:
        raise SelectionError("S producer startup arguments must be flag/value pairs")
    history_sources = [
        value
        for flag, value in zip(producer_arguments[::2], producer_arguments[1::2])
        if flag == "--native-candidate-history-source"
    ]
    if len(history_sources) > 1 or (
        history_sources and history_sources[0] not in {"replay", "snapshot", "native"}
    ):
        raise SelectionError(
            "candidate History source must be replay, snapshot or native exactly once"
        )
    snapshot_history = history_sources == ["snapshot"]
    native_history = history_sources == ["native"]
    source_modes = [
        value
        for flag, value in zip(producer_arguments[::2], producer_arguments[1::2])
        if flag == "--native-candidate-source-mode"
    ]
    if len(source_modes) > 1 or (source_modes and source_modes[0] != "source-only"):
        raise SelectionError("candidate Source mode must be source-only exactly once")
    source_only = source_modes == ["source-only"]
    pinned_flags = {
        "--native-source-socket": str(socket),
        "--native-q-executable": str(selection.binary),
        "--native-q-sha256": selection.binary_sha256,
        "--native-q-root": str(selection.state_root),
        "--native-q-config-sha256": selection.native_config_sha256,
    }
    if set(producer_arguments[::2]) & (
        pinned_flags.keys()
        | {
            "--native-candidate-q-pid",
            "--native-candidate-active-selection-path",
            "--native-candidate-active-selection-sha256",
            "--native-candidate-q-config",
            "--native-candidate-q-config-sha256",
            "--native-candidate-old-q-config",
            "--native-candidate-old-q-config-sha256",
            "--native-candidate-source-exporter-executable",
            "--native-candidate-source-exporter-executable-sha256",
        }
    ):
        raise SelectionError("launcher owns the candidate Q endpoint and PID flags")
    if snapshot_history or native_history or source_only:
        pinned_flags.update(
            {
                "--native-candidate-active-selection-path": str(active),
                "--native-candidate-active-selection-sha256": _hex_digest(
                    expected_current_sha256, "active selection SHA-256"
                ),
                "--native-candidate-q-config": str(selection.native_config),
                "--native-candidate-q-config-sha256": selection.native_config_sha256,
            }
        )
    if native_history:
        current_source = _require_current_selection(active, expected_current_sha256)
        if current_source.native_config is None or current_source.native_config_sha256 is None:
            raise SelectionError("Native History export requires an active Native config")
        pinned_flags.update(
            {
                "--native-candidate-old-q-config": str(current_source.native_config),
                "--native-candidate-old-q-config-sha256": current_source.native_config_sha256,
            }
        )
    if source_only:
        current_source = _require_current_selection(active, expected_current_sha256)
        if current_source.native_config is not None and not native_history:
            raise SelectionError("Native Source-only export requires Native History selection")
        pinned_flags.update(
            {
                "--native-candidate-source-exporter-executable": str(selection.binary),
                "--native-candidate-source-exporter-executable-sha256": selection.binary_sha256,
            }
        )
    arguments = list(producer_arguments)
    for flag, value in pinned_flags.items():
        arguments.extend((flag, value))
    binding = hashlib.sha256(
        json.dumps(
            [
                expected_current_sha256,
                _selection_fingerprint(selection),
                str(receipt_path),
                str(producer),
                producer_sha256,
                arguments,
                producer_environment,
            ],
            sort_keys=True,
            separators=(",", ":"),
        ).encode()
    ).hexdigest()
    # The retained binding already covers both values through the producer
    # identity. Preserve existing handoffs and accept only the same explicit pin.
    scan_worker_flags = {
        "--quanta-internal-filesystem-scan-worker-path-v1": str(producer),
        "--quanta-internal-filesystem-scan-worker-sha256-v1": producer_sha256,
    }
    for flag, expected_value in scan_worker_flags.items():
        supplied = [
            value
            for name, value in zip(producer_arguments[::2], producer_arguments[1::2])
            if name == flag
        ]
        if not supplied:
            arguments.extend((flag, expected_value))
        elif supplied != [expected_value]:
            raise SelectionError("S scan worker flags must match the pinned producer exactly once")

    with deployment_lock(candidate) as lock_fd:
        _require_candidate_source_selection(active, expected_current_sha256, selection.state_root)
        if (
            snapshot_history
            and _require_current_selection(active, expected_current_sha256).native_config
            is not None
        ):
            raise SelectionError("candidate History snapshot requires an active Legacy selection")
        if (
            native_history
            and _require_current_selection(active, expected_current_sha256).native_config is None
        ):
            raise SelectionError(
                "candidate Native History source requires an active Native selection"
            )
        if (
            source_only
            and _require_current_selection(active, expected_current_sha256).native_config
            is not None
            and not native_history
        ):
            raise SelectionError("Native Source-only export requires Native History selection")
        completed_drain = False
        if not os.path.lexists(handoff):
            if any(os.path.lexists(path) for path in (drained, receipt_path, candidate)):
                raise SelectionError("candidate has BOOT or drain proof without its FULL handoff")

            def produce(child: subprocess.Popen[bytes], stopping: Callable[[], bool]) -> None:
                if _sha256_binary(producer) != producer_sha256:
                    raise SelectionError("S producer SHA-256 changed before execution")
                payload = _run_candidate_source(
                    child,
                    stopping,
                    lock_fd,
                    socket,
                    producer,
                    arguments,
                    producer_environment,
                    timeout_seconds,
                )
                envelope = _candidate_handoff_envelope(binding, _candidate_source_handoff(payload))
                _publish_control_file_locked(handoff, envelope, lock_fd, create_only=True)

            status = _run_selected_command(
                selection, "serve-native-candidate", lock_fd, with_candidate=produce
            )
            if status != 0:
                raise SelectionError(f"candidate searchd drain failed with exit status {status}")
            completed_drain = True
        publication = _read_candidate_handoff(handoff, binding)
        _sync_retained_publication(
            handoff,
            lock_fd,
            publication,
            MAX_NATIVE_RECEIPT_BYTES,
            lambda payload: _decode_candidate_handoff(payload, binding),
        )
        if not os.path.lexists(drained):
            if os.path.lexists(receipt_path) or os.path.lexists(candidate):
                raise SelectionError("candidate BOOT requires a completed Q drain")
            if not completed_drain:
                # The FULL handoff is durable, but the previous Q drain may have
                # failed or been interrupted. Reopen Q without repeating FULL.
                status = _run_selected_command(
                    selection,
                    "serve-native-candidate",
                    lock_fd,
                    with_candidate=lambda child, stopping: _wait_candidate_endpoint(
                        child, stopping, socket, time.monotonic() + timeout_seconds
                    ),
                )
                if status != 0:
                    raise SelectionError(
                        f"candidate searchd drain failed with exit status {status}"
                    )
            _publish_control_file_locked(
                drained,
                _candidate_handoff_envelope(binding, publication),
                lock_fd,
                create_only=True,
            )
        drained_publication = _read_candidate_handoff(drained, binding)
        if drained_publication != publication:
            raise SelectionError("candidate drain proof differs from FULL handoff")
        _sync_retained_publication(
            drained,
            lock_fd,
            publication,
            MAX_NATIVE_RECEIPT_BYTES,
            lambda payload: _decode_candidate_handoff(payload, binding),
        )
        (
            root_nonce,
            operation,
            source_cut,
            original_full_sequence,
            protected_routes,
            retired_policy,
        ) = publication
        digest = _prepare_native_candidate_locked(
            active,
            candidate,
            selection,
            expected_current_sha256,
            receipt_path,
            root_nonce,
            operation,
            source_cut,
            lock_fd,
            protected_routes,
            retired_policy,
        )
    return {
        "candidate_path": str(candidate),
        "candidate_sha256": digest,
        "original_full_sequence": original_full_sequence,
    }


def _candidate_handoff_envelope(binding: str, publication: CandidateSourcePublication) -> bytes:
    nonce, operation, source_cut, sequence, protected_routes, retired_policy = publication
    published = {
        "root_nonce_hex": nonce,
        "manifest_operation": json.loads(operation),
        "source_cut_digest_hex": source_cut,
        "original_full_sequence": sequence,
    }
    if protected_routes is not None:
        published["protected_source_routes_hex"] = protected_routes
    if retired_policy is not None:
        published["retired_policy_object_hex"] = retired_policy[0]
        published["retired_policy_sha256_hex"] = retired_policy[1]
        published["retired_policy_identity_count"] = retired_policy[2]
    payload = (
        json.dumps(
            {
                "binding": binding,
                "publication": published,
            },
            sort_keys=True,
            separators=(",", ":"),
        ).encode()
        + b"\n"
    )
    if len(payload) > MAX_NATIVE_RECEIPT_BYTES:
        raise SelectionError("candidate FULL handoff exceeds byte bound")
    return payload


def _decode_candidate_handoff(payload: bytes, binding: str) -> CandidateSourcePublication:
    try:
        wire = json.loads(payload.decode("utf-8"), object_pairs_hook=_reject_duplicate_fields)
    except (UnicodeError, ValueError) as error:
        raise SelectionError("invalid retained candidate FULL handoff JSON") from error
    if (
        not isinstance(wire, dict)
        or set(wire) != {"binding", "publication"}
        or wire["binding"] != binding
    ):
        raise SelectionError("candidate FULL handoff differs from preparation inputs")
    return _candidate_source_handoff(json.dumps(wire["publication"]).encode())


def _read_candidate_handoff(path: Path, binding: str) -> CandidateSourcePublication:
    try:
        descriptor = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
        with os.fdopen(descriptor, "rb") as stream:
            _check_control_file(os.fstat(stream.fileno()), "candidate FULL handoff")
            payload = stream.read(MAX_NATIVE_RECEIPT_BYTES + 1)
        if len(payload) > MAX_NATIVE_RECEIPT_BYTES:
            raise SelectionError("candidate FULL handoff exceeds byte bound")
        return _decode_candidate_handoff(payload, binding)
    except (OSError, UnicodeError, ValueError) as error:
        raise SelectionError("cannot read retained candidate FULL handoff") from error


def serve_native_candidate(
    active_path: str | Path,
    expected_current_sha256: str,
    candidate_path: str | Path,
    binary: str | Path,
    binary_sha256: str,
    state_root: str | Path,
    config: str | Path,
    config_sha256: str,
    environment: dict[str, str],
) -> int:
    """Own the unselected searchd process until it exits and has been reaped."""
    active = _selection_path(active_path)
    candidate = _selection_path(candidate_path)
    if active.parent.samefile(candidate.parent):
        raise SelectionError("candidate process requires a separate deployment directory")
    root = _canonical_existing(state_root, directory=True)
    if candidate.is_relative_to(root):
        raise SelectionError("candidate control file must be outside the candidate root")
    config_path = _canonical_existing(config, directory=False)
    config_sha256 = _hex_digest(config_sha256, "config SHA-256")
    selection = Selection(
        _canonical_existing(binary, directory=False),
        _hex_digest(binary_sha256, "binary SHA-256"),
        root,
        _environment(environment),
        config_path,
        config_sha256,
    )
    with deployment_lock(candidate) as lock_fd:
        # The running selected service may hold its deployment lock. The
        # selection file is replaced atomically, so this is one snapshot.
        # Preparation and cutover revalidate it under the active lock.
        _require_candidate_source_selection(active, expected_current_sha256, root)
        if os.path.lexists(candidate):
            raise SelectionError("candidate is already prepared")
        return _run_selected_command(selection, "serve-native-candidate", lock_fd)


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
    Native-to-Native replacement is refused until the offline Catalog and
    fenced producer authorities can prove no source work would be lost.
    Legacy cutover requires a caller-preserved exact private selection in the
    same deployment directory and all three producer handoff values. Callers
    may preserve that head explicitly under its source CAS.
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
        if current.native_boot_receipt is None or any(axis is not None for axis in handoff):
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
            # BOOT does not distinguish a complete restore-forward from an old
            # source cut, and cannot see producer work not yet sent to Catalog.
            raise SelectionError(
                "Native cross-root switching requires offline Catalog publication baseline "
                "and fenced producer outbox verification; these authorities are not available"
            )
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
            path,
            selection,
            lock_fd,
            fault_hook=fault_hook,
            expected_current_sha256=expected_current_sha256,
        )


def rollback_legacy_selection(
    path: str | Path,
    previous_selection_path: str | Path,
    expected_current_sha256: str,
    expected_previous_selection_sha256: str,
    *,
    fault_hook: FaultHook | None = None,
) -> None:
    """Refuse Legacy rollback until both durable authorities can prove it safe.

    BOOT authenticates the installed Native root, not the producer's pending
    work or publications since cutover. Reissuing BOOT cannot authorize a
    backward selection. No caller-supplied success flag substitutes for the
    missing fenced producer and offline Catalog verification.
    """
    path = _selection_path(path)
    with deployment_lock(path):
        current = _require_current_selection(path, expected_current_sha256)
        if current.native_boot_receipt is None:
            raise SelectionError("Legacy rollback requires a current Native selection")
        saved = _require_saved_legacy_selection(
            path, previous_selection_path, expected_previous_selection_sha256
        )
        _require_distinct_switch_roots(current.state_root, saved.state_root)
        _check_binary(saved)
        raise SelectionError(
            "Legacy rollback requires offline Catalog publication baseline and fenced "
            "producer outbox verification; these authorities are not available"
        )


def _publish_selection_locked(
    path: Path,
    selection: Selection,
    lock_fd: int,
    *,
    fault_hook: FaultHook | None,
    expected_current_sha256: str | None = None,
    create_only: bool = False,
) -> None:
    _publish_control_file_locked(
        path,
        _selection_payload(selection),
        lock_fd,
        fault_hook=fault_hook,
        expected_current_sha256=expected_current_sha256,
        create_only=create_only,
    )


def _publish_control_file_locked(
    path: Path,
    payload: bytes,
    lock_fd: int,
    *,
    fault_hook: FaultHook | None = None,
    expected_current_sha256: str | None = None,
    create_only: bool = False,
) -> None:
    # The caller retains the one deployment lock through verification and rename.
    rename_attempted = False
    try:
        directory = os.open(path.parent, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
        try:
            _require_publication_directory(path, directory, lock_fd)
            temporary = f".{path.name}.tmp-{secrets.token_hex(16)}"
            descriptor = os.open(
                temporary,
                os.O_RDWR | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW,
                0o600,
                dir_fd=directory,
            )
            with os.fdopen(descriptor, "w+b") as stream:
                try:
                    os.fchmod(stream.fileno(), 0o600)
                    stream.write(payload)
                    stream.flush()
                    os.fsync(stream.fileno())
                    if fault_hook is not None:
                        fault_hook("after_file_fsync")
                    if create_only:
                        try:
                            os.stat(path.name, dir_fd=directory, follow_symlinks=False)
                        except FileNotFoundError:
                            pass
                        else:
                            raise SelectionError(
                                "candidate already exists; reconcile before retrying"
                            )
                    else:
                        _check_existing_selection(path)
                    if expected_current_sha256 is not None:
                        _require_current_selection(path, expected_current_sha256)
                    _require_publication_directory(path, directory, lock_fd)
                    _require_publication_file(stream, directory, temporary, payload)
                    # A signal can arrive after publication but before its
                    # return. Treat that boundary as uncertain.
                    rename_attempted = True
                    if create_only:
                        try:
                            _rename_create_only(directory, temporary, path.name)
                        except SelectionError:
                            # Unsupported syscall or EEXIST is a known no-publish result.
                            rename_attempted = False
                            raise
                    else:
                        os.replace(temporary, path.name, src_dir_fd=directory, dst_dir_fd=directory)
                    if fault_hook is not None:
                        fault_hook("after_rename")
                    os.fsync(directory)
                    if fault_hook is not None:
                        fault_hook("after_directory_fsync")
                    _require_publication_directory(path, directory, lock_fd)
                    _require_publication_file(stream, directory, path.name, payload)
                finally:
                    with contextlib.suppress(OSError):
                        named = os.stat(temporary, dir_fd=directory, follow_symlinks=False)
                        if os.path.samestat(named, os.fstat(stream.fileno())):
                            os.unlink(temporary, dir_fd=directory)
        finally:
            os.close(directory)
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


def _rename_create_only(directory: int, source: str, destination: str) -> None:
    """Atomically publish one private control file without replacing an existing name."""
    if sys.platform == "linux":
        symbol, no_replace = "renameat2", 1  # RENAME_NOREPLACE
    elif sys.platform == "darwin":
        symbol, no_replace = "renameatx_np", 0x00000004  # RENAME_EXCL
    else:
        raise SelectionError("atomic create-only publication is unsupported on this platform")
    libc = ctypes.CDLL(None, use_errno=True)
    try:
        rename = getattr(libc, symbol)
    except AttributeError as error:
        raise SelectionError("atomic create-only publication is unavailable") from error
    rename.argtypes = (ctypes.c_int, ctypes.c_char_p, ctypes.c_int, ctypes.c_char_p, ctypes.c_uint)
    rename.restype = ctypes.c_int
    if rename(directory, os.fsencode(source), directory, os.fsencode(destination), no_replace):
        code = ctypes.get_errno()
        if code == errno.EEXIST:
            raise SelectionError("candidate already exists; reconcile before retrying")
        raise OSError(code, os.strerror(code), destination)


def _require_publication_directory(path: Path, directory: int, lock_fd: int) -> None:
    _selection_path(path)
    if not os.path.samestat(os.fstat(directory), path.parent.lstat()):
        raise SelectionError("deployment directory changed during publication")
    named = os.stat(DEPLOYMENT_LOCK_NAME, dir_fd=directory, follow_symlinks=False)
    _check_control_file(named, "deployment lock")
    if not os.path.samestat(os.fstat(lock_fd), named):
        raise SelectionError("deployment lock changed during publication")


def _require_publication_file(stream: BinaryIO, directory: int, name: str, payload: bytes) -> None:
    stream.seek(0)
    if stream.read(len(payload) + 1) != payload:
        raise SelectionError("selection bytes changed during publication")
    named = os.stat(name, dir_fd=directory, follow_symlinks=False)
    _check_control_file(named, "selection")
    if not os.path.samestat(os.fstat(stream.fileno()), named):
        raise SelectionError("selection inode changed during publication")


def _sync_retained_publication(
    path: Path,
    lock_fd: int,
    expected: ControlDocument,
    maximum: int,
    decode: Callable[[bytes], ControlDocument],
) -> None:
    """Sync the exact retained object, then recheck its name and contents."""
    try:
        directory = os.open(path.parent, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
        try:
            _require_publication_directory(path, directory, lock_fd)
            descriptor = os.open(
                path.name, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK, dir_fd=directory
            )
            with os.fdopen(descriptor, "rb") as stream:
                _check_control_file(os.fstat(stream.fileno()), "retained control file")
                payload = stream.read(maximum + 1)
                if len(payload) > maximum or decode(payload) != expected:
                    raise SelectionError("retained control file differs from expected identity")
                os.fsync(stream.fileno())
                os.fsync(directory)
                _require_publication_directory(path, directory, lock_fd)
                _require_publication_file(stream, directory, path.name, payload)
        finally:
            os.close(directory)
    except (OSError, SelectionError) as error:
        raise PersistenceUncertain(
            "retained control file durability is uncertain; retry exact candidate"
        ) from error


def _read_signed_approval(path: Path) -> dict[str, object]:
    path = _canonical_existing(path, directory=False)
    descriptor = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK | os.O_CLOEXEC)
    with os.fdopen(descriptor, "rb") as stream:
        _check_control_file(os.fstat(stream.fileno()), "Native release approval")
        data = stream.read(MAX_NATIVE_RECEIPT_BYTES + 1)
    if len(data) > MAX_NATIVE_RECEIPT_BYTES:
        raise SelectionError("Native release approval exceeds byte bound")
    try:
        receipt = json.loads(data, object_pairs_hook=_reject_duplicate_fields)
    except (UnicodeError, ValueError) as error:
        raise SelectionError(f"Native release approval JSON differs: {error}") from error
    if not isinstance(receipt, dict):
        raise SelectionError("Native release approval envelope differs")
    return receipt


def serve_selection(
    path: str | Path,
    *,
    approval_path: Path | None = None,
    decision_policy_path: Path | None = None,
) -> int:
    """Serve the selected root; searchd boot owns live validation and recovery."""
    with deployment_lock(path) as lock_fd:
        selected_path = _selection_path(path)
        selection = _read_selection_locked(selected_path)
        retired = (
            selection.native_boot_receipt is not None
            and selection.native_boot_receipt.retired_policy_object_hex is not None
        )
        if retired:
            if approval_path is None or decision_policy_path is None:
                raise SelectionError("retired Native selection requires signed release approval")
            if __package__:
                from . import native_release_approval
            else:
                import native_release_approval
            approval = _read_signed_approval(approval_path)
            policy_path = _canonical_existing(decision_policy_path, directory=False)
            policy_fd = os.open(
                policy_path,
                os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK | os.O_CLOEXEC,
            )
            with os.fdopen(policy_fd, "rb") as policy_stream:
                if not stat.S_ISREG(os.fstat(policy_stream.fileno()).st_mode):
                    raise SelectionError("Native decision policy must be a regular file")
                policy_bytes = policy_stream.read(1024 * 1024 + 1)
            if len(policy_bytes) > 1024 * 1024:
                raise SelectionError("Native decision policy exceeds byte bound")
            policy_sha256 = hashlib.sha256(policy_bytes).hexdigest()
            try:
                native_release_approval.verify_installed_approval(
                    approval,
                    selection,
                    _selection_fingerprint(selection),
                    policy_sha256,
                    selection_path=selected_path,
                )
            except (OSError, KeyError, TypeError, ValueError) as error:
                raise SelectionError(f"Native release approval differs: {error}") from error
        elif approval_path is not None or decision_policy_path is not None:
            raise SelectionError("Native release approval requires a retired selection")
        return _run_selected_command(
            selection,
            "serve-native" if selection.native_config is not None else "serve",
            lock_fd,
            retirement_approval=(Path(approval_path), policy_path) if retired else None,
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
    serve_candidate = commands.add_parser(
        "serve-native-candidate", help="supervise an unselected Native candidate process"
    )
    prepare_native = commands.add_parser(
        "prepare-native-candidate", help="verify and write a Native candidate without switching"
    )
    build_native = commands.add_parser(
        "build-native-candidate", help="run the pinned FULL producer and seal its Q candidate"
    )
    for candidate_command in (serve_candidate, prepare_native, build_native):
        candidate_command.add_argument("--selection", type=Path, required=True)
        candidate_command.add_argument("--expected-selection-sha256", required=True)
        candidate_command.add_argument("--candidate", type=Path, required=True)
        candidate_command.add_argument("--binary", type=Path, required=True)
        candidate_command.add_argument("--binary-sha256", required=True)
        candidate_command.add_argument("--state-root", type=Path, required=True)
        candidate_command.add_argument("--config", type=Path, required=True)
        candidate_command.add_argument("--config-sha256", required=True)
        candidate_command.add_argument("--env", action="append", default=[], metavar="NAME=VALUE")
    prepare_native.add_argument("--receipt", type=Path, required=True)
    prepare_native.add_argument("--root-nonce", required=True)
    prepare_native.add_argument("--manifest-operation", required=True)
    prepare_native.add_argument("--source-cut-digest", required=True)
    build_native.add_argument("--receipt", type=Path, required=True)
    build_native.add_argument("--producer", type=Path, required=True)
    build_native.add_argument("--producer-sha256", required=True)
    build_native.add_argument("--producer-env", action="append", default=[], metavar="NAME=VALUE")
    build_native.add_argument(
        "--timeout-seconds", type=float, default=DEFAULT_CANDIDATE_PREPARE_TIMEOUT_SECONDS
    )
    build_native.add_argument(
        "producer_arguments",
        nargs=argparse.REMAINDER,
        help="installed S startup and candidate flags after --",
    )
    switch_native = commands.add_parser(
        "switch-native", help="CAS cutover from Legacy to a verified offline Native root"
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
    switch_native.add_argument(
        "--target-operation", help="producer target operation JSON; required for Legacy cutover"
    )
    switch_native.add_argument(
        "--published-operation", help="published operation JSON; required for Legacy cutover"
    )
    switch_native.add_argument(
        "--published-source-cut-digest",
        help="published SourceCut digest; required for Legacy cutover",
    )
    rollback_legacy = commands.add_parser(
        "rollback-legacy", help="Legacy rollback; refuses until offline authorities are connected"
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
    serve.add_argument("--native-release-approval", type=Path)
    serve.add_argument("--decision-policy", type=Path)
    args = parser.parse_args(argv)
    try:
        if args.command == "install":
            selection = proposed_selection(args.binary, args.state_root, _cli_environment(args.env))
            publish_selection(args.selection, selection)
            return 0
        if args.command == "serve-native-candidate":
            return serve_native_candidate(
                args.selection,
                args.expected_selection_sha256,
                args.candidate,
                args.binary,
                args.binary_sha256,
                args.state_root,
                args.config,
                args.config_sha256,
                _cli_environment(args.env),
            )
        if args.command in {"prepare-native-candidate", "build-native-candidate"}:
            selection = Selection(
                _canonical_existing(args.binary, directory=False),
                _hex_digest(args.binary_sha256, "binary SHA-256"),
                _canonical_existing(args.state_root, directory=True),
                _environment(_cli_environment(args.env)),
                _canonical_existing(args.config, directory=False),
                _hex_digest(args.config_sha256, "config SHA-256"),
            )
            if args.command == "build-native-candidate":
                producer_arguments = args.producer_arguments
                if producer_arguments and producer_arguments[0] == "--":
                    producer_arguments = producer_arguments[1:]
                producer_environment = {}
                for item in args.producer_env:
                    name, separator, value = item.partition("=")
                    if (
                        not separator
                        or not re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]*", name)
                        or name in producer_environment
                    ):
                        raise SelectionError("--producer-env requires a unique NAME=VALUE")
                    producer_environment[name] = value
                result = build_native_candidate(
                    args.selection,
                    args.candidate,
                    selection,
                    args.expected_selection_sha256,
                    receipt_path=args.receipt,
                    producer=args.producer,
                    producer_sha256=args.producer_sha256,
                    producer_arguments=producer_arguments,
                    producer_environment=producer_environment,
                    timeout_seconds=args.timeout_seconds,
                )
                print(json.dumps(result))
                return 0
            digest = prepare_native_candidate(
                args.selection,
                args.candidate,
                selection,
                args.expected_selection_sha256,
                receipt_path=args.receipt,
                root_nonce=_hex_digest(args.root_nonce, "candidate root nonce"),
                manifest_operation=_published_handoff_operation(
                    args.manifest_operation, "candidate boot operation"
                ),
                source_cut_digest=_hex_digest(args.source_cut_digest, "candidate SourceCut"),
            )
            print(json.dumps({"candidate_path": str(args.candidate), "candidate_sha256": digest}))
            return 0
        if args.command in ("install-native", "switch-native"):
            receipt = _read_native_receipt(args.receipt)
            selection = proposed_native_selection(
                args.binary,
                args.state_root,
                args.config,
                _cli_environment(args.env),
                receipt,
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
        return serve_selection(
            args.selection,
            approval_path=args.native_release_approval,
            decision_policy_path=args.decision_policy,
        )
    except SelectionError as error:
        print(f"deployment launcher: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
