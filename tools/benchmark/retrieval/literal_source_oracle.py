"""Exact UTF-8 content-file oracle for source-bound diagnostic suites.

This contract is byte-exact and case-sensitive. It deliberately excludes paths
and ranking: a file is relevant if its frozen bytes contain the query bytes.
"""

from __future__ import annotations

import re
from typing import Any

import unicodedata2

from tools.benchmark.retrieval import source_oracle

CONTENT_LITERAL_UTF8_EXACT = "content_literal_utf8_exact_v1"
MAX_LITERAL_BYTES = 256


class LiteralOracleError(ValueError):
    """The exact-content source contract cannot be evaluated."""


def require_literal(query: str) -> bytes:
    if not isinstance(query, str) or unicodedata2.normalize("NFC", query) != query:
        raise LiteralOracleError("literal query must be NFC UTF-8")
    if any(unicodedata2.category(char) == "Cc" for char in query):
        raise LiteralOracleError("literal query contains a control character")
    try:
        raw = query.encode("utf-8")
    except UnicodeEncodeError as exc:
        raise LiteralOracleError("literal query is not UTF-8") from exc
    if not 1 <= len(raw) <= MAX_LITERAL_BYTES:
        raise LiteralOracleError("literal query must be 1..=256 UTF-8 bytes")
    return raw


class LiteralSourceOracleIndex:
    """Enumerate overlapping raw-byte occurrences in the complete frozen view."""

    def __init__(self, files: dict[str, tuple[bytes, str]], queries: set[str]) -> None:
        if not 0 < len(files) <= source_oracle.MAX_FILES:
            raise LiteralOracleError("literal oracle file limit exceeded")
        if sum(len(raw) for raw, _digest in files.values()) > source_oracle.MAX_SOURCE_BYTES:
            raise LiteralOracleError("literal oracle byte limit exceeded")
        if not 0 < len(queries) <= source_oracle.MAX_QUERIES:
            raise LiteralOracleError("literal oracle query limit exceeded")
        for query in queries:
            require_literal(query)
        self.files = files
        self.queries = queries

    def spans(self, query: str) -> list[tuple[str, int, int]]:
        needle = require_literal(query)
        if query not in self.queries:
            raise LiteralOracleError("literal query was not admitted to the oracle")
        # Positive lookahead independently enumerates overlapping occurrences;
        # the capsule producer uses a repeated byte find loop.
        pattern = re.compile(b"(?=(" + re.escape(needle) + b"))")
        matches = []
        for path, (raw, _digest) in sorted(self.files.items()):
            matches.extend((path, match.start(1), match.end(1)) for match in pattern.finditer(raw))
        return matches

    def expected_rows(self, contract: str, query: str, unit: str) -> list[dict[str, Any]]:
        if contract != CONTENT_LITERAL_UTF8_EXACT or unit != "distinct_file":
            raise LiteralOracleError("unsupported literal oracle contract/unit")
        return [
            {"path": path, "file_sha256": self.files[path][1], "grade": 3}
            for path in sorted({path for path, _start, _end in self.spans(query)})
        ]

    def first_match(self, contract: str, query: str) -> tuple[str, int, int] | None:
        if contract != CONTENT_LITERAL_UTF8_EXACT:
            raise LiteralOracleError("unsupported literal oracle contract")
        return next(iter(self.spans(query)), None)

    def indexed_nfc_membership_matches(self, query: str) -> bool:
        """Require the product's indexed NFC text to select the same files.

        Raw source offsets remain the gold evidence. A normalization-induced
        file-set difference makes this task ineligible for a byte-exact product
        comparison rather than silently changing its gold contract.
        """
        require_literal(query)
        actual = {path for path, _start, _end in self.spans(query)}
        normalized = set()
        for path, (raw, _digest) in self.files.items():
            try:
                text = raw.decode("utf-8")
            except UnicodeDecodeError:
                return False
            if query in unicodedata2.normalize("NFC", text):
                normalized.add(path)
        return actual == normalized
