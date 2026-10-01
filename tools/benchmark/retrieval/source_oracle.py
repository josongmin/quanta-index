"""Exhaustive, source-derived judgments for narrow lexical benchmark contracts.

These labels describe observable source matches, not human relevance review.
The caller must bind every file to a frozen source snapshot before indexing.
"""

from __future__ import annotations

import re
from collections import defaultdict
from typing import Any

IDENTIFIER = re.compile(r"[A-Za-z_][A-Za-z0-9_]*\Z")
# Identifier fragments (prefix/infix/typo) may start with a digit; components are
# lowercase ASCII words joined by single spaces.
FRAGMENT = re.compile(r"[A-Za-z0-9_]+\Z")
COMPONENT_QUERY = re.compile(r"[a-z0-9]+( [a-z0-9]+)+\Z")
WORDS = re.compile(rb"(?<![A-Za-z0-9_])[A-Za-z_][A-Za-z0-9_]*")
# camel-snake-v1: split on "_", then on lower->upper, acronym->Word and letter/digit edges.
COMPONENT_TOKENIZER = "camel-snake-v1"
_COMPONENTS = re.compile(r"[A-Z]+(?=[A-Z][a-z])|[A-Z]?[a-z]+|[A-Z]+|[0-9]+")
_INFERRED_ACRONYM = re.compile(r"[A-Z]{2}[a-z]")
GO_DECLARATIONS = frozenset(
    {"function_declaration", "method_declaration", "type_spec", "type_alias"}
)
GO_EXACT_LOCAL_NAME = "go_exact_local_name_v3"
ASCII_IDENTIFIER_WORD = "ascii_identifier_word_v1"
ASCII_CONTENT_ABSENT_CASEFOLD = "ascii_content_absent_casefold_v1"
# Variants over the same indexed Go declaration set as v3. Prefix, infix and
# osa1 compare exact (case-sensitive) name text; components compare lowercased
# camel-snake-v1 components on both sides.
GO_NAME_PREFIX = "go_declaration_name_prefix_v1"
GO_NAME_INFIX = "go_declaration_name_infix_v1"
GO_NAME_COMPONENTS = "go_declaration_name_components_v1"
GO_NAME_OSA1 = "go_declaration_name_osa1_v1"
GO_NAME_VARIANTS = frozenset({GO_NAME_PREFIX, GO_NAME_INFIX, GO_NAME_COMPONENTS, GO_NAME_OSA1})
GO_NAME_CONTRACTS = GO_NAME_VARIANTS | {GO_EXACT_LOCAL_NAME}
MAX_FILES = 4096
MAX_SOURCE_BYTES = 512 * 1024 * 1024
MAX_QUERIES = 2000


class SourceOracleError(ValueError):
    """The declared oracle cannot be derived exhaustively."""


def name_components(name: str) -> tuple[str, ...]:
    """Lowercase camel/snake components of an ASCII identifier (camel-snake-v1).

    A name with any non-ASCII character has no components: the tokenizer cannot
    place a boundary inside it, so it never matches the components contract.
    """
    if not name.isascii():
        return ()
    return tuple(part.lower() for chunk in name.split("_") for part in _COMPONENTS.findall(chunk))


def has_inferred_acronym_boundary(name: str) -> bool:
    """True when splitting needs the acronym->Word guess (e.g. `CIDRs`, `HTMLFSTest`).

    Only `_`, lower->Upper and letter/digit edges are written in the name; an
    uppercase run followed by lowercase letters may end one character earlier or
    not at all, so such names are not used to *generate* components queries.
    """
    return _INFERRED_ACRONYM.search(name) is not None


def osa_distance_at_most_one(first: str, second: str) -> bool:
    """Optimal-string-alignment distance <= 1: one insert, delete, substitute or swap."""
    if first == second:
        return True
    if abs(len(first) - len(second)) > 1:
        return False
    if len(first) == len(second):
        diffs = [i for i, (a, b) in enumerate(zip(first, second)) if a != b]
        return len(diffs) == 1 or (
            len(diffs) == 2
            and diffs[1] == diffs[0] + 1
            and first[diffs[0]] == second[diffs[1]]
            and first[diffs[1]] == second[diffs[0]]
        )
    shorter, longer = sorted((first, second), key=len)
    index = next((i for i, (a, b) in enumerate(zip(shorter, longer)) if a != b), len(shorter))
    return shorter[index:] == longer[index + 1 :]


def _variant_matches(contract: str, query: str, name: str) -> bool:
    if contract == GO_NAME_PREFIX:
        return name.startswith(query)
    if contract == GO_NAME_INFIX:
        return query in name
    if contract == GO_NAME_COMPONENTS:
        wanted, have = tuple(query.split(" ")), name_components(name)
        return any(have[i : i + len(wanted)] == wanted for i in range(len(have) - len(wanted) + 1))
    if contract == GO_NAME_OSA1:
        return name != query and osa_distance_at_most_one(query, name)
    raise SourceOracleError("unsupported Go declaration-name variant contract")


def _name_text(token: bytes) -> str:
    """Go source is UTF-8; an undecodable declaration name refuses the oracle."""
    try:
        return token.decode("utf-8")
    except UnicodeDecodeError as exc:
        raise SourceOracleError("Go declaration name is not valid UTF-8") from exc


def _require_query(contract: str, query: Any) -> None:
    pattern = (
        COMPONENT_QUERY
        if contract == GO_NAME_COMPONENTS
        else FRAGMENT
        if contract in GO_NAME_VARIANTS
        else IDENTIFIER
    )
    if not isinstance(query, str) or pattern.fullmatch(query) is None:
        raise SourceOracleError("source oracle query does not fit its contract's query form")
    if contract in (GO_NAME_PREFIX, GO_NAME_INFIX) and len(query) < 3:
        raise SourceOracleError("prefix/infix oracle requires at least three characters")


def _go_indexed_definition(node: Any) -> bool:
    """Mirror the Go symbol producer's definition query, including interface methods."""
    if node.type in GO_DECLARATIONS:
        return True
    if node.type != "method_elem":
        return False
    interface = node.parent
    owner = interface.parent if interface is not None else None
    return (
        interface is not None
        and interface.type == "interface_type"
        and owner is not None
        and owner.type in ("type_spec", "type_alias")
        and (owner_type := owner.child_by_field_name("type")) is not None
        and owner_type.id == interface.id
    )


def has_identifier_word_in_span(raw: bytes, token: bytes, start: int, end: int) -> bool:
    """Match a whole source word contained in a span, including its outside boundaries."""
    window_start = max(0, start - 1)
    window_end = min(len(raw), end + 1)
    for match in WORDS.finditer(raw[window_start:window_end]):
        word_start = window_start + match.start()
        word_end = window_start + match.end()
        if word_start >= start and word_end <= end and match.group() == token:
            return True
    return False


class SourceOracleIndex:
    def __init__(self, files: dict[str, tuple[bytes, str]], query_names: set[str]) -> None:
        if not 0 < len(files) <= MAX_FILES:
            raise SourceOracleError("source oracle file limit exceeded")
        if sum(len(raw) for raw, _digest in files.values()) > MAX_SOURCE_BYTES:
            raise SourceOracleError("source oracle byte limit exceeded")
        if not 0 < len(query_names) <= MAX_QUERIES or any(
            FRAGMENT.fullmatch(name) is None and COMPONENT_QUERY.fullmatch(name) is None
            for name in query_names
        ):
            raise SourceOracleError("source oracle requires bounded ASCII bare identifiers")
        self.files = files
        self.query_tokens = {name.encode("ascii") for name in query_names}
        self._words: dict[bytes, set[str]] | None = None
        self._first_words: dict[bytes, tuple[str, int, int]] = {}
        # Name bytes identify the match; definition bytes identify the indexed symbol unit.
        self._go_declarations: dict[bytes, list[tuple[str, int, int, int, int]]] | None = None

    def _index_words(self) -> dict[bytes, set[str]]:
        if self._words is None:
            words: dict[bytes, set[str]] = defaultdict(set)
            for path, (raw, _digest) in sorted(self.files.items()):
                for match in WORDS.finditer(raw):
                    token = match.group()
                    if token in self.query_tokens:
                        words[token].add(path)
                        self._first_words.setdefault(token, (path, match.start(), match.end()))
            self._words = words
        return self._words

    def _require_content_absent_casefold(self, query: str) -> None:
        """Reject a negative content label if any frozen file contains the query."""
        _require_query(ASCII_CONTENT_ABSENT_CASEFOLD, query)
        folded = query.casefold()
        for path, (raw, _digest) in sorted(self.files.items()):
            if folded in raw.decode("utf-8", "replace").casefold():
                raise SourceOracleError(f"content absent contract found a match: {path}")

    def _index_go_declarations(self) -> dict[bytes, list[tuple[str, int, int, int, int]]]:
        if self._go_declarations is None:
            try:
                from tree_sitter_language_pack import get_parser

                parser = get_parser("go")
            except (ImportError, LookupError, ValueError) as exc:
                raise SourceOracleError("Go source oracle parser unavailable") from exc
            declarations: dict[bytes, list[tuple[str, int, int, int, int]]] = defaultdict(list)
            for path, (raw, _digest) in self.files.items():
                if not path.endswith(".go"):
                    continue
                root = parser.parse(raw).root_node
                if root.has_error:
                    raise SourceOracleError(f"Go source oracle parse error: {path}")
                nodes = [root]
                while nodes:
                    node = nodes.pop()
                    if _go_indexed_definition(node):
                        name = node.child_by_field_name("name")
                        if name is None:
                            raise SourceOracleError(f"Go declaration lacks name: {path}")
                        # Every declaration is kept: variant contracts match names that
                        # differ from the submitted query bytes.
                        declarations[raw[name.start_byte : name.end_byte]].append(
                            (path, name.start_byte, name.end_byte, node.start_byte, node.end_byte)
                        )
                    nodes.extend(reversed(node.children))
            self._go_declarations = declarations
        return self._go_declarations

    def _go_matches(self, contract: str, query: str) -> list[tuple[str, int, int, int, int]]:
        _require_query(contract, query)
        declarations = self._index_go_declarations()
        if contract == GO_EXACT_LOCAL_NAME:
            return list(declarations.get(query.encode("ascii"), []))
        return [
            match
            for token, rows in declarations.items()
            if _variant_matches(contract, query, _name_text(token))
            for match in rows
        ]

    def expected_rows(self, contract: str, query: str, unit: str) -> list[dict[str, Any]]:
        if contract in GO_NAME_CONTRACTS and unit in ("symbol", "distinct_file"):
            matches = self._go_matches(contract, query)
            if unit == "symbol":
                return [
                    {
                        "path": path,
                        "file_sha256": self.files[path][1],
                        "start_byte": start,
                        "end_byte": end,
                        "grade": 3,
                    }
                    for path, _name_start, _name_end, start, end in sorted(matches)
                ]
            paths = {path for path, *_spans in matches}
        elif contract == ASCII_IDENTIFIER_WORD and unit == "distinct_file":
            _require_query(contract, query)
            paths = self._index_words().get(query.encode("ascii"), set())
        elif contract == ASCII_CONTENT_ABSENT_CASEFOLD and unit == "distinct_file":
            self._require_content_absent_casefold(query)
            paths = set()
        else:
            raise SourceOracleError("unsupported source oracle contract/unit combination")
        return [
            {"path": path, "file_sha256": self.files[path][1], "grade": 3} for path in sorted(paths)
        ]

    def matched_names(self, contract: str, query: str) -> list[str]:
        """Distinct declaration names satisfying a Go name contract, for ambiguity strata."""
        if contract not in GO_NAME_CONTRACTS:
            raise SourceOracleError("unsupported source oracle contract")
        return sorted(
            {
                _name_text(self.files[path][0][start:end])
                for path, start, end, *_definition in self._go_matches(contract, query)
            }
        )

    def go_name_spans(
        self, query: str, contract: str = GO_EXACT_LOCAL_NAME
    ) -> list[tuple[str, int, int]]:
        """Return local-name bytes for validating the separate gold line projection."""
        if contract not in GO_NAME_CONTRACTS:
            raise SourceOracleError("unsupported source oracle contract")
        return [
            (path, name_start, name_end)
            for path, name_start, name_end, *_definition in self._go_matches(contract, query)
        ]

    def first_match(self, contract: str, query: str) -> tuple[str, int, int] | None:
        """Choose the first source match by path and byte offset for a diagnostic gold line."""
        if contract == ASCII_CONTENT_ABSENT_CASEFOLD:
            self._require_content_absent_casefold(query)
            return None
        if contract == ASCII_IDENTIFIER_WORD:
            _require_query(contract, query)
            self._index_words()
            return self._first_words.get(query.encode("ascii"))
        if contract in GO_NAME_CONTRACTS:
            matches = self._go_matches(contract, query)
            if not matches:
                return None
            path, start, end, _definition_start, _definition_end = min(matches)
            return path, start, end
        raise SourceOracleError("unsupported source oracle contract")
