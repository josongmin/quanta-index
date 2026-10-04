"""Use the producer's canonical vendored grammars in benchmark oracles.

Other languages retain their existing pinned language-pack parser. Vendored
C sources are compiled once outside the checkout; no downloaded grammar or
fallback is allowed to silently change declaration eligibility.
"""

from __future__ import annotations

import ctypes
import hashlib
import json
import os
import platform
import shutil
import subprocess
import tempfile
from functools import lru_cache
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
VENDOR = ROOT / "vendor/tree-sitter-typescript"
GO_VENDOR = ROOT / "vendor/tree-sitter-go"
RUST_VENDOR = ROOT / "vendor/tree-sitter-rust"
PYTHON_VENDOR = ROOT / "vendor/tree-sitter-python"
JAVASCRIPT_VENDOR = ROOT / "vendor/tree-sitter-javascript"
# Loaded grammar pointers are process-local. A later source commitment must
# bind those same bytes; a new grammar generation needs a fresh process.
_LOADED_SOURCE_DIGESTS: dict[str, str] | None = None


def _grammar_sources() -> dict[str, Path]:
    return {
        "go": GO_VENDOR / "src",
        "rust": RUST_VENDOR / "src",
        "python": PYTHON_VENDOR / "src",
        "javascript": JAVASCRIPT_VENDOR / "src",
        "typescript": VENDOR / "typescript/src",
        "tsx": VENDOR / "tsx/src",
    }


def component_source_digests() -> dict[str, str]:
    paths = []
    for vendor in (VENDOR, GO_VENDOR, RUST_VENDOR, PYTHON_VENDOR, JAVASCRIPT_VENDOR):
        if vendor.is_symlink() or vendor.parent.is_symlink() or not vendor.is_dir():
            raise ValueError("vendored parser sources missing or linked")
        entries = sorted(vendor.rglob("*"))
        if any(path.is_symlink() or not (path.is_file() or path.is_dir()) for path in entries):
            raise ValueError("vendored parser sources must be regular or directories")
        components = [path for path in entries if path.is_file() and path.suffix in (".c", ".h")]
        if not components:
            raise ValueError("vendored parser sources missing or linked")
        paths.extend(components)
    digests = {
        path.relative_to(ROOT).as_posix(): hashlib.sha256(path.read_bytes()).hexdigest()
        for path in paths
    }
    if _LOADED_SOURCE_DIGESTS is not None and digests != _LOADED_SOURCE_DIGESTS:
        raise ValueError("loaded parser source identity changed; restart the producer")
    return digests


def _checked_library(directory: Path, expected: dict) -> Path:
    marker = directory / "ready.json"
    library = directory / "parser.so"
    if directory.is_symlink() or marker.is_symlink() or library.is_symlink():
        raise ValueError("vendored parser cache must not be linked")
    try:
        metadata = json.loads(marker.read_text())
    except (OSError, ValueError) as error:
        raise ValueError("vendored parser cache marker malformed or missing") from error
    if (
        not isinstance(metadata, dict)
        or set(metadata) != {"identity", "binary_sha256"}
        or metadata["identity"] != expected
    ):
        raise ValueError("vendored parser cache identity differs")
    if (
        not library.is_file()
        or metadata["binary_sha256"] != hashlib.sha256(library.read_bytes()).hexdigest()
    ):
        raise ValueError("vendored parser cache binary differs")
    return library


def _library(grammar: str) -> Path:
    if grammar not in _grammar_sources():
        raise ValueError("unsupported vendored grammar")
    before = component_source_digests()
    system = platform.system()
    if system not in ("Darwin", "Linux"):
        raise ValueError("vendored benchmark parser compilation requires Darwin or Linux")
    identity = {
        "sources": before,
        "grammar": grammar,
        "system": system,
        "machine": platform.machine(),
    }
    key = hashlib.sha256(json.dumps(identity, sort_keys=True).encode()).hexdigest()
    cache = Path(
        os.environ.get("QUANTA_CENSUS_PARSER_CACHE", Path.home() / ".cache/quanta-census-parsers")
    )
    cache = cache.resolve()
    if cache == ROOT or ROOT in cache.parents:
        raise ValueError("vendored parser cache must be outside the checkout")
    directory = cache / key
    if directory.exists() or directory.is_symlink():
        return _checked_library(directory, identity)
    compiler = shutil.which("cc")
    if compiler is None:
        raise ValueError("C compiler unavailable for vendored benchmark parser")
    cache.mkdir(parents=True, exist_ok=True)
    stage = Path(tempfile.mkdtemp(prefix=".parser-", dir=cache))
    try:
        source = _grammar_sources()[grammar]
        command = [
            compiler,
            "-dynamiclib" if system == "Darwin" else "-shared",
            "-fPIC",
            "-std=c11",
            "-O0",
            "-I",
            str(source),
            str(source / "parser.c"),
        ]
        if grammar != "go":
            command.append(str(source / "scanner.c"))
        command += ["-o", str(stage / "parser.so")]
        completed = subprocess.run(command, capture_output=True, timeout=120, check=False)
        if completed.returncode:
            raise ValueError(
                "vendored parser compile failed: "
                + completed.stderr.decode("utf-8", "replace")[-2000:]
            )
        if component_source_digests() != before:
            raise ValueError("vendored parser sources changed during compilation")
        marker = {
            "identity": identity,
            "binary_sha256": hashlib.sha256((stage / "parser.so").read_bytes()).hexdigest(),
        }
        (stage / "ready.json").write_text(json.dumps(marker, sort_keys=True))
        try:
            stage.rename(directory)
        except OSError:
            if not directory.exists():
                raise
        return _checked_library(directory, identity)
    finally:
        if stage.exists():
            shutil.rmtree(stage)


@lru_cache(maxsize=6)
def _language(grammar: str):
    global _LOADED_SOURCE_DIGESTS

    from tree_sitter import Language

    library_path = _library(grammar)
    sources = json.loads((library_path.parent / "ready.json").read_text())["identity"]["sources"]
    if component_source_digests() != sources:
        raise ValueError("loaded parser source identity changed before loading")
    library = ctypes.CDLL(str(library_path))
    entry = getattr(library, "tree_sitter_" + grammar)
    entry.restype = ctypes.c_void_p
    capsule = ctypes.pythonapi.PyCapsule_New
    capsule.argtypes = [ctypes.c_void_p, ctypes.c_char_p, ctypes.c_void_p]
    capsule.restype = ctypes.py_object
    language = Language(capsule(entry(), b"tree_sitter.Language", None))
    if component_source_digests() != sources:
        raise ValueError("loaded parser source identity changed during loading")
    _LOADED_SOURCE_DIGESTS = sources
    # Retain the dynamic library for the lifetime of its grammar pointers.
    return language, library


def get_parser(grammar: str):
    if grammar not in _grammar_sources():
        from tree_sitter_language_pack import get_parser as language_pack_parser

        return language_pack_parser(grammar)
    from tree_sitter import Parser

    language, _library_handle = _language(grammar)
    return Parser(language)
