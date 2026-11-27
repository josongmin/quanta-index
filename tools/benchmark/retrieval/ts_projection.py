"""Opt-in TypeScript compiler projection with byte and child-process custody.

The released grammar still has to parse the result; default benchmark parser
selection and source commitments remain on the patched grammars.
"""

from __future__ import annotations

import hashlib
import re
import shutil
import subprocess
import tempfile
import time
from dataclasses import dataclass
from pathlib import Path
from typing import Callable

from tools.benchmark.retrieval.projection_process import (
    HELPER_ENV,
    ProjectionProcessError,
    parse_helper_json,
    read_bounded_regular,
    run_bounded,
)

SCHEMA = "quanta-ts-grammar-projection-v1"
SOURCE = Path(__file__).with_suffix(".cjs")
MAX_SOURCE_BYTES = 1 << 20
MAX_RESPONSE_BYTES = 8 << 20
MAX_LIBRARY_BYTES = 16 << 20
MAX_NODE_BYTES = 256 << 20
MAX_STAGED_BYTES = 512 << 20
MAX_NODE_LIBRARIES = 8
MAX_EDITS = 1 << 16


class TsProjectionError(ValueError):
    def __init__(self, kind: str) -> None:
        self.kind = kind
        super().__init__(f"TypeScript grammar projection {kind}")


@dataclass(frozen=True)
class TsEdit:
    start: int
    end: int
    replacement: bytes


@dataclass(frozen=True)
class TsProjection:
    source: bytes
    projected: bytes
    source_sha256: str
    projected_sha256: str
    node_sha256: str
    typescript_sha256: str
    script_sha256: str
    edits: tuple[TsEdit, ...]

    def overlaps_name(self, start: int, end: int) -> bool:
        return any(edit.start < end and start < edit.end for edit in self.edits)


class TsProjector:
    """Execute one exact Node/TypeScript/compiler-helper candidate tuple."""

    def __init__(
        self,
        node: Path,
        typescript_js: Path,
        *,
        node_sha256: str,
        typescript_sha256: str,
        script_sha256: str,
        node_version: str,
        typescript_version: str = "5.9.3",
        node_libraries: tuple[tuple[Path, str], ...] = (),
    ) -> None:
        for digest in (
            node_sha256,
            typescript_sha256,
            script_sha256,
            *(item[1] for item in node_libraries),
        ):
            if re.fullmatch(r"[0-9a-f]{64}", digest) is None:
                raise TsProjectionError("binary_identity")
        self.node = node
        self.typescript_js = typescript_js
        self.node_sha256 = node_sha256
        self.typescript_sha256 = typescript_sha256
        self.script_sha256 = script_sha256
        self.node_version = node_version
        self.typescript_version = typescript_version
        self.node_libraries = node_libraries
        self._directory: Path | None = None
        self._staged_node: Path | None = None
        self._staged_script: Path | None = None
        self._staged_library: Path | None = None

    @staticmethod
    def _read_exact(path: Path, digest: str, maximum: int) -> bytes:
        try:
            blob = read_bounded_regular(path, maximum)
        except OSError as error:
            raise TsProjectionError("binary_identity") from error
        if hashlib.sha256(blob).hexdigest() != digest:
            raise TsProjectionError("binary_identity")
        return blob

    def __enter__(self) -> TsProjector:
        if len(self.node_libraries) > MAX_NODE_LIBRARIES:
            raise TsProjectionError("binary_identity")
        node = self._read_exact(self.node, self.node_sha256, MAX_NODE_BYTES)
        if not node:
            raise TsProjectionError("binary_identity")
        script = self._read_exact(SOURCE, self.script_sha256, MAX_SOURCE_BYTES)
        library = self._read_exact(self.typescript_js, self.typescript_sha256, MAX_LIBRARY_BYTES)
        libraries = {}
        remaining = MAX_STAGED_BYTES - len(node) - len(script) - len(library)
        for path, digest in self.node_libraries:
            if path.name in libraries:
                raise TsProjectionError("binary_identity")
            blob = self._read_exact(path, digest, min(MAX_NODE_BYTES, remaining))
            remaining -= len(blob)
            libraries[path.name] = blob
        try:
            self._directory = Path(tempfile.mkdtemp(prefix="quanta-ts-projection-"))
            bin_dir = self._directory / "bin"
            lib_dir = self._directory / "lib"
            bin_dir.mkdir()
            lib_dir.mkdir()
            self._staged_node = bin_dir / "node"
            self._staged_script = self._directory / "projection.cjs"
            self._staged_library = self._directory / "typescript.js"
            self._staged_node.write_bytes(node)
            self._staged_node.chmod(0o700)
            for name, blob in libraries.items():
                (lib_dir / name).write_bytes(blob)
            self._staged_script.write_bytes(script)
            self._staged_library.write_bytes(library)
        except OSError as error:
            self.__exit__(None, None, None)
            raise TsProjectionError("binary_identity") from error
        return self

    def __exit__(self, _kind: object, _error: object, _traceback: object) -> None:
        if self._directory is not None:
            shutil.rmtree(self._directory)
        self._directory = None
        self._staged_node = None
        self._staged_script = None
        self._staged_library = None

    def project(
        self,
        path: str,
        source: bytes,
        *,
        timeout_seconds: float,
        cancelled: Callable[[], bool] | None = None,
    ) -> TsProjection:
        if self._staged_node is None or self._staged_script is None or self._staged_library is None:
            raise TsProjectionError("binary_identity")
        if not isinstance(source, bytes) or len(source) > MAX_SOURCE_BYTES:
            raise TsProjectionError("resource_limit")
        if not path or path.startswith("-") or "\x00" in path:
            raise TsProjectionError("producer_defect")
        if timeout_seconds <= 0 or not timeout_seconds < float("inf"):
            raise TsProjectionError("timeout")
        if cancelled is not None and cancelled():
            raise TsProjectionError("cancelled")
        deadline = time.monotonic() + timeout_seconds
        try:
            child = subprocess.Popen(
                [str(self._staged_node), str(self._staged_script), path, str(self._staged_library)],
                stdin=subprocess.PIPE,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                env=HELPER_ENV,
                close_fds=True,
                start_new_session=True,
            )
        except OSError as error:
            raise TsProjectionError("producer_defect") from error
        try:
            output, stderr, returncode = run_bounded(
                child,
                source,
                deadline=deadline,
                cancelled=cancelled,
                max_stdout=MAX_RESPONSE_BYTES,
            )
        except ProjectionProcessError as error:
            raise TsProjectionError(error.kind) from error
        except OSError as error:
            raise TsProjectionError("producer_defect") from error
        if returncode != 0 or stderr:
            raise TsProjectionError("producer_defect")
        try:
            response = parse_helper_json(output)
        except (ValueError, UnicodeDecodeError, RecursionError) as error:
            raise TsProjectionError("producer_defect") from error
        if (
            not isinstance(response, dict)
            or set(response)
            != {"schema", "status", "typescript_version", "node_version", "source_sha256", "edits"}
            or response["schema"] != SCHEMA
            or response["typescript_version"] != self.typescript_version
            or response["node_version"] != self.node_version
            or response["source_sha256"] != hashlib.sha256(source).hexdigest()
        ):
            raise TsProjectionError("producer_defect")
        if not isinstance(response["status"], str):
            raise TsProjectionError("producer_defect")
        if response["status"] != "ok":
            if (
                response["status"] in {"parse_failure", "resource_limit"}
                and response["edits"] == []
            ):
                raise TsProjectionError(response["status"])
            raise TsProjectionError("producer_defect")
        edits = response["edits"]
        if not isinstance(edits, list) or len(edits) > MAX_EDITS:
            raise TsProjectionError("producer_defect")
        projected = bytearray(source)
        parsed: list[TsEdit] = []
        last_end = 0
        for edit in edits:
            if not isinstance(edit, dict) or set(edit) != {"start", "end", "replacement"}:
                raise TsProjectionError("producer_defect")
            start, end, replacement = edit["start"], edit["end"], edit["replacement"]
            if type(start) is not int or type(end) is not int or type(replacement) is not str:
                raise TsProjectionError("producer_defect")
            if not replacement.isascii() or start < last_end or not 0 <= start < end <= len(source):
                raise TsProjectionError("producer_defect")
            encoded = replacement.encode("ascii")
            original = source[start:end]
            if len(encoded) != end - start or any(
                before != after and (before in (10, 13) or after in (10, 13))
                for before, after in zip(original, encoded)
            ):
                raise TsProjectionError("producer_defect")
            projected[start:end] = encoded
            parsed.append(TsEdit(start, end, encoded))
            last_end = end
        return TsProjection(
            source,
            bytes(projected),
            hashlib.sha256(source).hexdigest(),
            hashlib.sha256(projected).hexdigest(),
            self.node_sha256,
            self.typescript_sha256,
            self.script_sha256,
            tuple(parsed),
        )
