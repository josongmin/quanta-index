"""Exhaustive, source-derived judgments for narrow lexical benchmark contracts.

These labels describe observable source matches, not human relevance review.
The caller must bind every file to a frozen source snapshot before indexing.
"""

from __future__ import annotations

import re
from collections import defaultdict
from typing import Any

IDENTIFIER = re.compile(r"[A-Za-z_][A-Za-z0-9_]*\Z")
WORDS = re.compile(rb"[A-Za-z_][A-Za-z0-9_]*")
GO_DECLARATIONS = frozenset({"function_declaration", "method_declaration", "type_spec"})
GO_EXACT_LOCAL_NAME = "go_exact_local_name_v1"
ASCII_IDENTIFIER_WORD = "ascii_identifier_word_v1"
MAX_FILES = 4096
MAX_SOURCE_BYTES = 512 * 1024 * 1024
MAX_QUERIES = 2000


class SourceOracleError(ValueError):
    """The declared oracle cannot be derived exhaustively."""


class SourceOracleIndex:
    def __init__(self, files: dict[str, tuple[bytes, str]], query_names: set[str]) -> None:
        if not 0 < len(files) <= MAX_FILES:
            raise SourceOracleError("source oracle file limit exceeded")
        if sum(len(raw) for raw, _digest in files.values()) > MAX_SOURCE_BYTES:
            raise SourceOracleError("source oracle byte limit exceeded")
        if not 0 < len(query_names) <= MAX_QUERIES or any(
            IDENTIFIER.fullmatch(name) is None for name in query_names
        ):
            raise SourceOracleError("source oracle requires bounded ASCII bare identifiers")
        self.files = files
        self.query_tokens = {name.encode("ascii") for name in query_names}
        self._words: dict[bytes, set[str]] | None = None
        self._go_declarations: dict[bytes, list[tuple[str, int, int]]] | None = None

    def _index_words(self) -> dict[bytes, set[str]]:
        if self._words is None:
            words: dict[bytes, set[str]] = defaultdict(set)
            for path, (raw, _digest) in self.files.items():
                for match in WORDS.finditer(raw):
                    if match.group() in self.query_tokens:
                        words[match.group()].add(path)
            self._words = words
        return self._words

    def _index_go_declarations(self) -> dict[bytes, list[tuple[str, int, int]]]:
        if self._go_declarations is None:
            try:
                from tree_sitter_language_pack import get_parser

                parser = get_parser("go")
            except (ImportError, LookupError, ValueError) as exc:
                raise SourceOracleError("Go source oracle parser unavailable") from exc
            declarations: dict[bytes, list[tuple[str, int, int]]] = defaultdict(list)
            for path, (raw, _digest) in self.files.items():
                if not path.endswith(".go"):
                    continue
                root = parser.parse(raw).root_node
                if root.has_error:
                    raise SourceOracleError(f"Go source oracle parse error: {path}")
                nodes = [root]
                while nodes:
                    node = nodes.pop()
                    if node.type in GO_DECLARATIONS:
                        name = node.child_by_field_name("name")
                        if name is None:
                            raise SourceOracleError(f"Go declaration lacks name: {path}")
                        token = raw[name.start_byte : name.end_byte]
                        if token in self.query_tokens:
                            declarations[token].append((path, name.start_byte, name.end_byte))
                    nodes.extend(reversed(node.children))
            self._go_declarations = declarations
        return self._go_declarations

    def expected_rows(self, contract: str, query: str, unit: str) -> list[dict[str, Any]]:
        if not isinstance(query, str) or IDENTIFIER.fullmatch(query) is None:
            raise SourceOracleError("source oracle requires an ASCII bare identifier")
        token = query.encode("ascii")
        if contract == GO_EXACT_LOCAL_NAME and unit in ("symbol", "distinct_file"):
            matches = self._index_go_declarations().get(token, [])
            if unit == "symbol":
                return [
                    {
                        "path": path,
                        "file_sha256": self.files[path][1],
                        "start_byte": start,
                        "end_byte": end,
                        "grade": 3,
                    }
                    for path, start, end in sorted(matches)
                ]
            paths = {path for path, _start, _end in matches}
        elif contract == ASCII_IDENTIFIER_WORD and unit == "distinct_file":
            paths = self._index_words().get(token, set())
        else:
            raise SourceOracleError("unsupported source oracle contract/unit combination")
        return [
            {"path": path, "file_sha256": self.files[path][1], "grade": 3} for path in sorted(paths)
        ]
