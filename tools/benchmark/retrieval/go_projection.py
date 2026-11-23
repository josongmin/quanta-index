"""Run the pinned Go syntax projector with original-byte and process custody.

The returned source is only a candidate for the released Go grammar. Callers
must still reject a Tree-sitter parse error and derive records from the
original source bytes. This module does not select the benchmark producer.
"""

from __future__ import annotations

import hashlib
import json
import os
import platform
import re
import shutil
import stat
import subprocess
import tempfile
import time
from dataclasses import dataclass
from pathlib import Path
from typing import Callable

from tools.benchmark.retrieval.projection_process import ProjectionProcessError, run_bounded

MAX_SOURCE_BYTES = 1 << 20
MAX_EDITS = 1 << 16
MAX_RESPONSE_BYTES = 8 << 20
MAX_BINARY_BYTES = 32 << 20
SCHEMA = "quanta-go-grammar-projection-v1"


class GoProjectionError(ValueError):
    def __init__(self, kind: str) -> None:
        self.kind = kind
        super().__init__(f"Go grammar projection {kind}")


@dataclass(frozen=True)
class GoEdit:
    start: int
    end: int
    replacement: bytes


@dataclass(frozen=True)
class GoProjection:
    source: bytes
    projected: bytes
    source_sha256: str
    projected_sha256: str
    binary_sha256: str
    edits: tuple[GoEdit, ...]

    def overlaps_name(self, start: int, end: int) -> bool:
        return any(edit.start < end and start < edit.end for edit in self.edits)


class GoProjector:
    """Stage exact verified binary bytes outside the checkout for one run."""

    def __init__(self, binary: Path, *, binary_sha256: str, go_version: str = "go1.25.4") -> None:
        if not re.fullmatch(r"[0-9a-f]{64}", binary_sha256):
            raise GoProjectionError("binary_identity")
        self.binary = binary
        self.binary_sha256 = binary_sha256
        self.go_version = go_version
        self._directory: Path | None = None
        self._staged: Path | None = None

    def __enter__(self) -> GoProjector:
        if platform.system() not in {"Darwin", "Linux"}:
            raise GoProjectionError("unsupported_platform")
        try:
            metadata = self.binary.lstat()
            if not stat.S_ISREG(metadata.st_mode) or metadata.st_size > MAX_BINARY_BYTES:
                raise GoProjectionError("binary_identity")
            # O_NOFOLLOW also refuses a path swapped to a symlink after lstat.
            fd = os.open(self.binary, os.O_RDONLY | os.O_NOFOLLOW)
            with os.fdopen(fd, "rb") as source:
                blob = source.read(MAX_BINARY_BYTES + 1)
            if (
                len(blob) > MAX_BINARY_BYTES
                or hashlib.sha256(blob).hexdigest() != self.binary_sha256
            ):
                raise GoProjectionError("binary_identity")
            self._directory = Path(tempfile.mkdtemp(prefix="quanta-go-projection-"))
            self._staged = self._directory / "projector"
            with self._staged.open("xb") as target:
                target.write(blob)
            self._staged.chmod(0o700)
        except (OSError, ValueError) as error:
            self.__exit__(None, None, None)
            if isinstance(error, GoProjectionError):
                raise
            raise GoProjectionError("binary_identity") from error
        return self

    def __exit__(self, _kind: object, _error: object, _traceback: object) -> None:
        if self._directory is not None:
            shutil.rmtree(self._directory)
        self._directory = None
        self._staged = None

    def project(
        self,
        source: bytes,
        *,
        timeout_seconds: float,
        cancelled: Callable[[], bool] | None = None,
    ) -> GoProjection:
        if self._staged is None:
            raise GoProjectionError("binary_identity")
        if not isinstance(source, bytes) or len(source) > MAX_SOURCE_BYTES:
            raise GoProjectionError("resource_limit")
        if timeout_seconds <= 0 or not timeout_seconds < float("inf"):
            raise GoProjectionError("timeout")
        if cancelled is not None and cancelled():
            raise GoProjectionError("cancelled")
        deadline = time.monotonic() + timeout_seconds
        try:
            child = subprocess.Popen(
                [str(self._staged)],
                stdin=subprocess.PIPE,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                close_fds=True,
                start_new_session=True,
            )
        except OSError as error:
            raise GoProjectionError("producer_defect") from error
        try:
            output, stderr, returncode = run_bounded(
                child,
                source,
                deadline=deadline,
                cancelled=cancelled,
                max_stdout=MAX_RESPONSE_BYTES,
            )
        except ProjectionProcessError as error:
            raise GoProjectionError(error.kind) from error
        except OSError as error:
            raise GoProjectionError("producer_defect") from error
        if returncode != 0 or stderr:
            raise GoProjectionError("producer_defect")
        try:
            response = json.loads(output)
        except (ValueError, UnicodeDecodeError) as error:
            raise GoProjectionError("producer_defect") from error
        if (
            not isinstance(response, dict)
            or set(response) != {"schema", "status", "go_version", "source_sha256", "edits"}
            or response["schema"] != SCHEMA
            or response["go_version"] != self.go_version
        ):
            raise GoProjectionError("producer_defect")
        digest = hashlib.sha256(source).hexdigest()
        if response["source_sha256"] != digest:
            raise GoProjectionError("producer_defect")
        if response["status"] != "ok":
            if (
                response["status"] in {"parse_failure", "resource_limit"}
                and response["edits"] == []
            ):
                raise GoProjectionError(response["status"])
            raise GoProjectionError("producer_defect")
        edits = response["edits"]
        if not isinstance(edits, list) or len(edits) > MAX_EDITS:
            raise GoProjectionError("producer_defect")
        projected = bytearray(source)
        parsed: list[GoEdit] = []
        last_end = 0
        for edit in edits:
            if not isinstance(edit, dict) or set(edit) != {"start", "end", "replacement"}:
                raise GoProjectionError("producer_defect")
            start, end, replacement = edit["start"], edit["end"], edit["replacement"]
            if type(start) is not int or type(end) is not int or type(replacement) is not str:
                raise GoProjectionError("producer_defect")
            replacement_bytes = replacement.encode("ascii") if replacement.isascii() else b""
            if (
                start < last_end
                or not 0 <= start < end <= len(source)
                or (source[start:end], replacement_bytes)
                not in {(b"new", b"foo"), (b"make", b"fooo")}
            ):
                raise GoProjectionError("producer_defect")
            projected[start:end] = replacement_bytes
            parsed.append(GoEdit(start, end, replacement_bytes))
            last_end = end
        return GoProjection(
            source,
            bytes(projected),
            digest,
            hashlib.sha256(projected).hexdigest(),
            self.binary_sha256,
            tuple(parsed),
        )
