"""Load released grammars where compatible; retain three patched grammars.

Go, JavaScript and TypeScript still require local grammar changes. Rust and
Python use locked upstream extension binaries. All loaded parser bytes are
bound to the census identity for the process lifetime.
"""

from __future__ import annotations

import base64
import ctypes
import hashlib
import importlib
import importlib.metadata
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
JAVASCRIPT_VENDOR = ROOT / "vendor/tree-sitter-javascript"
RELEASED_GRAMMARS = frozenset({"rust", "python"})
RELEASED_VERSIONS = {
    "rust": "0.24.2",
    "python": "0.25.0",
    "go": "0.25.0",
    "javascript": "0.25.0",
    "typescript": "0.23.2",
}
# Loaded grammar pointers are process-local. A later source commitment must
# bind those same bytes; a new grammar generation needs a fresh process.
_LOADED_SOURCE_DIGESTS: dict[str, str] | None = None


def _grammar_sources() -> dict[str, Path]:
    return {
        "go": GO_VENDOR / "src",
        "javascript": JAVASCRIPT_VENDOR / "src",
        "typescript": VENDOR / "typescript/src",
        "tsx": VENDOR / "tsx/src",
    }


def _released_extension(grammar: str) -> Path:
    if grammar not in RELEASED_VERSIONS:
        raise ValueError("unsupported released grammar")
    package = "tree-sitter-" + grammar
    distribution = importlib.metadata.distribution(package)
    if distribution.version != RELEASED_VERSIONS[grammar]:
        raise ValueError("released grammar version differs from parser lock")
    module_dir = "tree_sitter_" + grammar
    candidates = [
        entry
        for entry in distribution.files or ()
        if len(entry.parts) == 2
        and entry.parts[0] == module_dir
        and entry.name.startswith("_binding.")
        and entry.suffix in {".so", ".pyd", ".dylib"}
    ]
    if len(candidates) != 1:
        raise ValueError("released grammar distribution lacks a unique extension")
    entry = candidates[0]
    path = Path(distribution.locate_file(entry))
    if not path.is_file() or path.is_symlink() or path.suffix not in {".so", ".pyd", ".dylib"}:
        raise ValueError("released grammar extension missing or linked")
    digest = entry.hash
    if digest is None or digest.mode != "sha256" or entry.size != path.stat().st_size:
        raise ValueError("released grammar extension has no recorded integrity")
    encoded = base64.urlsafe_b64encode(hashlib.sha256(path.read_bytes()).digest()).rstrip(b"=")
    if encoded.decode("ascii") != digest.value:
        raise ValueError("released grammar extension differs from installed distribution")
    return path


def component_source_digests() -> dict[str, str]:
    paths = []
    for vendor in (VENDOR, GO_VENDOR, JAVASCRIPT_VENDOR):
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
    for grammar in sorted(RELEASED_GRAMMARS):
        extension = _released_extension(grammar)
        digests[f"tree-sitter-{grammar}/{extension.name}"] = hashlib.sha256(
            extension.read_bytes()
        ).hexdigest()
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
    if grammar in RELEASED_GRAMMARS:
        from tree_sitter import Language, Parser

        # Snapshot the actual binary before and after import/parser creation.
        before = component_source_digests()
        module = importlib.import_module("tree_sitter_" + grammar)
        binding = importlib.import_module("tree_sitter_" + grammar + "._binding")
        if Path(binding.__file__) != _released_extension(grammar):
            raise ValueError("loaded grammar extension differs from installed distribution")
        parser = Parser(Language(module.language()))
        if component_source_digests() != before:
            raise ValueError("released grammar extension changed during loading")
        global _LOADED_SOURCE_DIGESTS
        _LOADED_SOURCE_DIGESTS = before
        return parser
    if grammar not in _grammar_sources():
        from tree_sitter_language_pack import get_parser as language_pack_parser

        return language_pack_parser(grammar)
    from tree_sitter import Parser

    language, _library_handle = _language(grammar)
    return Parser(language)


def get_released_go_candidate_parser():
    """Load the pinned Go wheel for opt-in projection comparisons only.

    The ordinary ``get_parser('go')`` and source census continue to use the
    patched grammar until all external corpus and install gates pass.
    """
    return get_released_syntax_candidate_parser("go")


def get_released_syntax_candidate_parser(grammar: str):
    """Load a released grammar without changing the default parser policy."""
    from tree_sitter import Language, Parser

    module, entry = {
        "go": ("go", "language"),
        "javascript": ("javascript", "language"),
        "typescript": ("typescript", "language_typescript"),
        "tsx": ("typescript", "language_tsx"),
    }.get(grammar, (None, None))
    if module is None:
        raise ValueError("unsupported released candidate grammar")
    before = _released_extension(module)
    expected_digest = hashlib.sha256(before.read_bytes()).hexdigest()
    binding = importlib.import_module("tree_sitter_" + module + "._binding")
    if Path(binding.__file__) != before:
        raise ValueError("loaded candidate grammar extension differs from installed distribution")
    parser = Parser(Language(getattr(binding, entry)()))
    after = _released_extension(module)
    if after != before or hashlib.sha256(after.read_bytes()).hexdigest() != expected_digest:
        raise ValueError("released candidate grammar extension changed during loading")
    return parser, expected_digest
