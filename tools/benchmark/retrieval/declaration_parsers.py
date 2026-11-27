"""Load released Tree-sitter grammar wheels for the source declaration oracle.

Each parser is bound to the installed distribution's recorded extension bytes.
The independent source oracle may validate original syntax with pinned Go or
TypeScript helpers before asking these released grammars for declaration spans.
"""

from __future__ import annotations

import base64
import hashlib
import importlib
import importlib.metadata
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
RELEASED_VERSIONS = {
    "go": "0.25.0",
    "javascript": "0.25.0",
    "python": "0.25.0",
    "rust": "0.24.2",
    "typescript": "0.23.2",
}
RELEASED_GRAMMARS = frozenset((*RELEASED_VERSIONS, "tsx"))
LANGUAGE_EXPORTS = {
    "go": ("go", "language"),
    "javascript": ("javascript", "language"),
    "python": ("python", "language"),
    "rust": ("rust", "language"),
    "typescript": ("typescript", "language_typescript"),
    "tsx": ("typescript", "language_tsx"),
}
_LOADED_SOURCE_DIGESTS: dict[str, str] | None = None


def _released_extension(grammar: str) -> Path:
    package = "typescript" if grammar == "tsx" else grammar
    if package not in RELEASED_VERSIONS:
        raise ValueError("unsupported released grammar")
    distribution = importlib.metadata.distribution("tree-sitter-" + package)
    if distribution.version != RELEASED_VERSIONS[package]:
        raise ValueError("released grammar version differs from parser lock")
    module_dir = "tree_sitter_" + package
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
    """Bind the actual installed grammar extensions into source/gold identity."""
    digests = {}
    for grammar in sorted(RELEASED_VERSIONS):
        extension = _released_extension(grammar)
        digests[f"tree-sitter-{grammar}/{extension.name}"] = hashlib.sha256(
            extension.read_bytes()
        ).hexdigest()
    if _LOADED_SOURCE_DIGESTS is not None and digests != _LOADED_SOURCE_DIGESTS:
        raise ValueError("loaded parser source identity changed; restart the producer")
    return digests


def get_parser(grammar: str):
    if grammar in RELEASED_GRAMMARS:
        parser, _digest = get_released_syntax_candidate_parser(grammar)
        return parser
    from tree_sitter_language_pack import get_parser as language_pack_parser

    return language_pack_parser(grammar)


def get_released_go_candidate_parser():
    return get_released_syntax_candidate_parser("go")


def get_released_syntax_candidate_parser(grammar: str):
    """Load one official grammar and return its exact extension SHA-256."""
    from tree_sitter import Language, Parser

    module, entry = LANGUAGE_EXPORTS.get(grammar, (None, None))
    if module is None:
        raise ValueError("unsupported released candidate grammar")
    before = component_source_digests()
    extension = _released_extension(module)
    binding = importlib.import_module("tree_sitter_" + module + "._binding")
    if Path(binding.__file__) != extension:
        raise ValueError("loaded grammar extension differs from installed distribution")
    parser = Parser(Language(getattr(binding, entry)()))
    if component_source_digests() != before:
        raise ValueError("released grammar extension changed during loading")
    global _LOADED_SOURCE_DIGESTS
    _LOADED_SOURCE_DIGESTS = before
    return parser, before[f"tree-sitter-{module}/{extension.name}"]
