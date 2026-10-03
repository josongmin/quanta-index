"""Exhaustive, source-derived judgments for narrow lexical benchmark contracts.

These labels describe observable source matches, not human relevance review.
The caller must bind every file to a frozen source snapshot before indexing.
"""

from __future__ import annotations

import hashlib
import importlib.metadata
import json
import re
from collections import defaultdict
from functools import lru_cache
from pathlib import Path
from typing import Any

IDENTIFIER = re.compile(r"[A-Za-z_][A-Za-z0-9_]*\Z")
# Identifier fragments (prefix/infix/typo) may start with a digit; components are
# lowercase ASCII words joined by single spaces.
FRAGMENT = re.compile(r"[A-Za-z0-9_]+\Z")
COMPONENT_QUERY = re.compile(r"[a-z0-9]+( [a-z0-9]+)+\Z")
WORDS = re.compile(rb"(?<![A-Za-z0-9_])[A-Za-z_][A-Za-z0-9_]*")
# Conservative byte-token superset for proving fuzzy content absence. Splitting
# around non-ASCII bytes can only reject an otherwise valid negative label.
ASCII_TOKEN_SUPERSET = re.compile(rb"[A-Za-z0-9_]+")
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
ASCII_CODE_SEARCH_ABSENT_CASEFOLD = "ascii_code_search_absent_casefold_v1"
ASCII_CODE_SEARCH_DEFAULT_ABSENT_CASEFOLD = "ascii_code_search_default_absent_casefold_v1"
ASCII_IDENTIFIER_OSA1_ABSENT_CASEFOLD = "ascii_identifier_osa1_absent_casefold_v1"
# Variants over the same indexed Go declaration set as v3. Prefix, infix and
# osa1 compare exact (case-sensitive) name text; components compare lowercased
# camel-snake-v1 components on both sides.
GO_NAME_PREFIX = "go_declaration_name_prefix_v1"
GO_NAME_INFIX = "go_declaration_name_infix_v1"
GO_NAME_COMPONENTS = "go_declaration_name_components_v1"
GO_NAME_OSA1 = "go_declaration_name_osa1_v1"
GO_NAME_OSA1_CASEFOLD = "go_declaration_name_osa1_casefold_v1"
# Declaration-name contracts by language. Go keeps its v3 producer-mirroring
# census; the other languages use declaration_census_v1 kinds below. A name is
# the declared token as written (`r#match`, `#private`), never a normalized form.
NAME_VARIANTS = ("exact", "prefix", "infix", "components", "osa1", "osa1_casefold")
ALL_DECLARATION_LANGUAGES = "all_supported"
NAME_CONTRACTS: dict[str, tuple[str, str]] = {
    GO_EXACT_LOCAL_NAME: ("go", "exact"),
    GO_NAME_PREFIX: ("go", "prefix"),
    GO_NAME_INFIX: ("go", "infix"),
    GO_NAME_COMPONENTS: ("go", "components"),
    GO_NAME_OSA1: ("go", "osa1"),
    GO_NAME_OSA1_CASEFOLD: ("go", "osa1_casefold"),
}
for _language in ("rust", "python", "typescript", "javascript"):
    NAME_CONTRACTS[f"{_language}_exact_local_name_v1"] = (_language, "exact")
    for _variant in NAME_VARIANTS[1:]:
        NAME_CONTRACTS[f"{_language}_declaration_name_{_variant}_v1"] = (_language, _variant)
for _variant in NAME_VARIANTS:
    NAME_CONTRACTS[f"declaration_name_{_variant}"] = (ALL_DECLARATION_LANGUAGES, _variant)
DECLARATION_NAME_CONTRACTS = frozenset(NAME_CONTRACTS)
# Suffix -> Tree-sitter grammar. TSX needs its own grammar; JSX parses as JavaScript.
DECLARATION_GRAMMARS = {
    "go": {".go": "go"},
    "rust": {".rs": "rust"},
    "python": {".py": "python"},
    "typescript": {".ts": "typescript", ".tsx": "tsx"},
    "javascript": {
        ".js": "javascript",
        ".jsx": "javascript",
        ".mjs": "javascript",
        ".cjs": "javascript",
    },
}
DECLARATION_CENSUS = {
    "go": "go_indexed_definition_v3",
    "rust": "rust_declaration_census_v1",
    "python": "python_declaration_census_v1",
    "typescript": "typescript_declaration_census_v1",
    "javascript": "javascript_declaration_census_v1",
}
# Named item/declaration nodes only: no variables, fields, parameters, enum
# variants, namespaces, closures or anonymous class/function expressions. Rust
# `const _` is anonymous by language rule; `_` is an ordinary name elsewhere.
DECLARATION_KINDS = {
    "rust": frozenset(
        {
            "function_item",
            "function_signature_item",
            "struct_item",
            "enum_item",
            "union_item",
            "trait_item",
            "type_item",
            "associated_type",
            "const_item",
            "static_item",
            "mod_item",
            "macro_definition",
        }
    ),
    "python": frozenset({"function_definition", "class_definition"}),
    "typescript": frozenset(
        {
            "function_declaration",
            "generator_function_declaration",
            "function_signature",
            "class_declaration",
            "abstract_class_declaration",
            "method_definition",
            "method_signature",
            "abstract_method_signature",
            "interface_declaration",
            "type_alias_declaration",
            "enum_declaration",
        }
    ),
    "javascript": frozenset(
        {
            "function_declaration",
            "generator_function_declaration",
            "class_declaration",
            "method_definition",
        }
    ),
}
# String, numeric and computed member names are not identifier declarations.
NAME_NODE_TYPES = frozenset(
    {"identifier", "type_identifier", "property_identifier", "private_property_identifier"}
)
MAX_FILES = 4096
MAX_SOURCE_BYTES = 512 * 1024 * 1024
MAX_QUERIES = 2000


class SourceOracleError(ValueError):
    """The declared oracle cannot be derived exhaustively."""


def _excludable_census_refusal(error: SourceOracleError) -> bool:
    return "parse error:" in str(error) or "declaration lacks name:" in str(error)


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


def _variant_matches(variant: str, query: str, name: str) -> bool:
    if variant == "prefix":
        return name.startswith(query)
    if variant == "infix":
        return query in name
    if variant == "components":
        wanted, have = tuple(query.split(" ")), name_components(name)
        return any(have[i : i + len(wanted)] == wanted for i in range(len(have) - len(wanted) + 1))
    if variant == "osa1":
        return name != query and osa_distance_at_most_one(query, name)
    if variant == "osa1_casefold":
        return (
            IDENTIFIER.fullmatch(name) is not None
            and name.casefold() != query.casefold()
            and osa_distance_at_most_one(query.casefold(), name.casefold())
        )
    raise SourceOracleError("unsupported declaration-name variant contract")


@lru_cache(maxsize=128)
def _osa1_text_pattern(query: str) -> re.Pattern[str]:
    """Overapproximate every one-edit name as an unanchored source substring."""
    alternatives = set()
    for index in range(len(query)):
        left, right = re.escape(query[:index]), re.escape(query[index + 1 :])
        alternatives.add(left + right)
        alternatives.add(left + r"[^\n]" + right)
        if index + 1 < len(query):
            swapped = query[:index] + query[index + 1] + query[index] + query[index + 2 :]
            alternatives.add(re.escape(swapped))
    for index in range(len(query) + 1):
        alternatives.add(re.escape(query[:index]) + r"[^\n]" + re.escape(query[index:]))
    return re.compile("(?:" + "|".join(sorted(alternatives)) + ")")


def _osa1_text_witness(text: str, query: str) -> bool:
    """Search only positions that can start a one-edit substring."""
    pattern = _osa1_text_pattern(query)
    size = len(query)
    if size <= 2:
        return pattern.search(text) is not None

    # One edit (including an adjacent swap) cannot change both ends of a
    # query of this length. A surviving prefix starts at offset zero or one;
    # a surviving suffix ends after size-1, size, or size+1 characters.
    anchor_size = 3 if size >= 7 else 2 if size >= 5 else 1
    anchors = (
        (query[:anchor_size], (0, 1)),
        (
            query[-anchor_size:],
            (size - 1 - anchor_size, size - anchor_size, size + 1 - anchor_size),
        ),
    )
    for anchor, offsets in anchors:
        position = text.find(anchor)
        while position >= 0:
            for offset in offsets:
                start = position - offset
                if start >= 0 and pattern.match(text, start) is not None:
                    return True
            position = text.find(anchor, position + 1)
    return False


def declaration_query_textually_excluded(raw: bytes, query: str, variant: str) -> bool:
    """Conservatively prove that an uncensused file cannot contain a matching name.

    A text hit leaves the file ineligible even when the hit is a comment or a
    non-declaration. This never turns a parser refusal into an empty census.
    """
    if variant in ("exact", "prefix", "infix"):
        return query.encode("utf-8") not in raw
    if variant == "components":
        folded = raw.decode("utf-8", "replace").casefold()
        return any(part not in folded for part in query.split(" "))
    if (
        variant == "osa1_casefold"
        and IDENTIFIER.fullmatch(query) is not None
        and 3 <= len(query) <= 64
    ):
        # This variant only matches ASCII identifier names. An arbitrary
        # one-edit substring inside a longer token cannot be a matching
        # declaration. Include exact folded collisions, which also make a
        # submitted typo ambiguous even though _variant_matches excludes them.
        query_folded = query.casefold()
        for match in ASCII_TOKEN_SUPERSET.finditer(raw):
            token = match.group()
            if not token or token[0] in b"0123456789" or abs(len(token) - len(query)) > 1:
                continue
            if osa_distance_at_most_one(query_folded, token.decode("ascii").casefold()):
                return False
        return True
    if variant in ("osa1", "osa1_casefold"):
        if not 1 < len(query) <= 64:
            return False
        text = raw.decode("utf-8", "replace")
        if variant == "osa1_casefold":
            text, query = text.casefold(), query.casefold()
        # Any one edit of a name this long preserves either its leading or
        # trailing anchor. Check those substrings before searching the full
        # one-edit regex over a potentially large parser-refused source file.
        anchor = 3 if len(query) >= 7 else 2 if len(query) >= 5 else 0
        if anchor and query[:anchor] not in text and query[-anchor:] not in text:
            return True
        # One insertion, deletion or substitution can disturb at most three
        # distinct query trigrams; one adjacent swap can disturb at most four.
        # A regex witness therefore retains all but at most four query grams.
        # This is only a rejection filter: the conservative regex remains the
        # authority whenever enough grams occur anywhere in the source.
        grams = {query[index : index + 3] for index in range(len(query) - 2)}
        if len(grams) > 4:
            missing = 0
            for gram in sorted(grams):
                if gram not in text:
                    missing += 1
                    if missing > 4:
                        return True
        return not _osa1_text_witness(text, query)
    raise SourceOracleError("unsupported declaration-name variant contract")


def _name_text(token: bytes) -> str:
    """Source is UTF-8; an undecodable declaration name refuses the oracle."""
    try:
        return token.decode("utf-8")
    except UnicodeDecodeError as exc:
        raise SourceOracleError("declaration name is not valid UTF-8") from exc


def _require_query(contract: str, query: Any) -> None:
    variant = NAME_CONTRACTS.get(contract, (None, None))[1]
    pattern = (
        COMPONENT_QUERY
        if variant == "components"
        else FRAGMENT
        if variant in ("prefix", "infix", "osa1", "osa1_casefold")
        else IDENTIFIER
    )
    if not isinstance(query, str) or pattern.fullmatch(query) is None:
        raise SourceOracleError("source oracle query does not fit its contract's query form")
    if variant == "osa1_casefold" and (
        IDENTIFIER.fullmatch(query) is None or not 3 <= len(query) <= 64
    ):
        raise SourceOracleError("folded typo oracle requires a product-admissible identifier")
    if variant in ("prefix", "infix") and len(query) < 3:
        raise SourceOracleError("prefix/infix oracle requires at least three characters")


def declaration_language(path: str) -> str | None:
    suffix = "." + path.rsplit(".", 1)[-1] if "." in path.rsplit("/", 1)[-1] else ""
    return next(
        (language for language, grammars in DECLARATION_GRAMMARS.items() if suffix in grammars),
        None,
    )


def declaration_census(
    language: str, path: str, raw: bytes
) -> list[tuple[int, int, int, int, str]]:
    """All declared names in one file as (name start/end, definition start/end, kind).

    A parse error or a declaration whose name cannot be located refuses the
    file: an incomplete census is never an empty answer set.
    """
    if language not in DECLARATION_GRAMMARS:
        raise SourceOracleError("unsupported declaration census language")
    suffix = "." + path.rsplit(".", 1)[-1]
    grammar = DECLARATION_GRAMMARS[language].get(suffix)
    if grammar is None:
        raise SourceOracleError(f"{language} census does not admit file suffix: {path}")
    try:
        from tools.benchmark.retrieval.declaration_parsers import get_parser

        parser = get_parser(grammar)
    except (ImportError, LookupError, ValueError) as exc:
        raise SourceOracleError(f"{language} source oracle parser unavailable") from exc
    root = parser.parse(raw).root_node
    if root.has_error:
        prefix = "Go" if language == "go" else language
        raise SourceOracleError(f"{prefix} source oracle parse error: {path}")
    rows = []
    nodes = [root]
    while nodes:
        node = nodes.pop()
        if (
            _go_indexed_definition(node)
            if language == "go"
            else node.type in DECLARATION_KINDS[language]
        ):
            name = node.child_by_field_name("name")
            if name is None:
                raise SourceOracleError(f"{language} declaration lacks name: {path}")
            token = raw[name.start_byte : name.end_byte]
            if language == "go" or (
                name.type in NAME_NODE_TYPES and (language != "rust" or token != b"_")
            ):
                rows.append(
                    (name.start_byte, name.end_byte, node.start_byte, node.end_byte, node.type)
                )
        nodes.extend(reversed(node.children))
    return rows


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


def census_parser_identity() -> str:
    """Bind shared census rows to the loaded oracle and parser source/grammar."""
    import tree_sitter._binding as tree_sitter_binding
    import tree_sitter_language_pack.bindings.javascript as javascript_binding

    from tools.benchmark.retrieval import declaration_parsers

    payload = {
        "oracle_source_sha256": hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
        "parser_source_sha256": hashlib.sha256(
            Path(declaration_parsers.__file__).read_bytes()
        ).hexdigest(),
        "grammar_sources": declaration_parsers.component_source_digests(),
        "tree_sitter": importlib.metadata.version("tree-sitter"),
        "tree_sitter_language_pack": importlib.metadata.version("tree-sitter-language-pack"),
        "tree_sitter_binary_sha256": hashlib.sha256(
            Path(tree_sitter_binding.__file__).read_bytes()
        ).hexdigest(),
        "javascript_grammar_binary_sha256": hashlib.sha256(
            Path(javascript_binding.__file__).read_bytes()
        ).hexdigest(),
        "census_contracts": DECLARATION_CENSUS,
    }
    return hashlib.sha256(json.dumps(payload, sort_keys=True).encode()).hexdigest()


class DeclarationCensusCache:
    """Per-batch parsed declarations, never a persistent or cross-repository cache."""

    def __init__(self, parser_identity: str) -> None:
        if not re.fullmatch(r"[0-9a-f]{64}", parser_identity):
            raise SourceOracleError("declaration cache parser identity is invalid")
        self.parser_identity = parser_identity
        self._rows: dict[
            tuple[str, str, str],
            tuple[str, tuple[tuple[int, int, int, int, str], ...] | str],
        ] = {}

    def assert_parser_identity(self, observed: str) -> None:
        if observed != self.parser_identity:
            raise SourceOracleError("declaration cache parser identity changed")

    def census(
        self, language: str, path: str, raw: bytes, expected_sha256: str
    ) -> list[tuple[int, int, int, int, str]]:
        actual = hashlib.sha256(raw).hexdigest()
        if actual != expected_sha256:
            raise SourceOracleError(f"declaration cache source digest changed: {path}")
        key = (language, path, actual)
        if key not in self._rows:
            try:
                rows = tuple(declaration_census(language, path, raw))
            except SourceOracleError as exc:
                self._rows[key] = ("error", str(exc))
                raise
            self._rows[key] = ("rows", rows)
        kind, value = self._rows[key]
        if kind == "error":
            raise SourceOracleError(value)
        return list(value)


class SourceOracleIndex:
    def __init__(
        self,
        files: dict[str, tuple[bytes, str]],
        query_names: set[str],
        declaration_exclusions: dict[tuple[str, str], set[str]] | None = None,
        *,
        census_cache: DeclarationCensusCache | None = None,
    ) -> None:
        if census_cache is not None:
            census_cache.assert_parser_identity(census_parser_identity())
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
        self._census_cache = census_cache
        self.query_tokens = {name.encode("ascii") for name in query_names}
        self._words: dict[bytes, set[str]] | None = None
        self._first_words: dict[bytes, tuple[str, int, int]] = {}
        self._folded_token_paths: dict[str, set[str]] | None = None
        # Name bytes identify the match; definition bytes identify the indexed symbol unit.
        self._declarations: dict[
            tuple[str, frozenset[str]], dict[bytes, list[tuple[str, int, int, int, int]]]
        ] = {}
        self._indexed_names: dict[
            tuple[str, frozenset[str]],
            list[tuple[str, list[tuple[str, int, int, int, int]]]],
        ] = {}
        self._name_lengths: dict[tuple[str, frozenset[str]], dict[int, list[int]]] = {}
        self._folded_declaration_names: dict[tuple[str, frozenset[str]], frozenset[str]] = {}
        self._name_match_cache: dict[
            tuple[str, str], tuple[tuple[str, int, int, int, int], ...]
        ] = {}
        # Explicit per-contract/query qrel eligibility for files whose census refuses.
        # Files stay in self.files and in the caller's full source universe.
        self.declaration_exclusions: dict[tuple[str, str], frozenset[str]] = {}
        self._excluded_declaration_paths: dict[str, set[str]] = defaultdict(set)
        for key, paths in (declaration_exclusions or {}).items():
            if (
                not isinstance(key, tuple)
                or len(key) != 2
                or key[0] not in DECLARATION_NAME_CONTRACTS
                or key[1] not in query_names
                or not isinstance(paths, set)
                or not paths
            ):
                raise SourceOracleError("invalid declaration exclusion contract/query")
            contract, query = key
            _require_query(contract, query)
            language, _variant = NAME_CONTRACTS[contract]
            if any(
                path not in files
                or declaration_language(path) is None
                or (
                    language != ALL_DECLARATION_LANGUAGES and declaration_language(path) != language
                )
                for path in paths
            ):
                raise SourceOracleError("declaration exclusion path is outside its source language")
            self.declaration_exclusions[key] = frozenset(paths)
            self._excluded_declaration_paths[language].update(paths)

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

    def _index_folded_tokens(self) -> dict[str, set[str]]:
        """Build the exact ASCII token collision index once per frozen universe."""
        if self._folded_token_paths is None:
            token_paths: dict[str, set[str]] = defaultdict(set)
            for path, (raw, _digest) in sorted(self.files.items()):
                for match in ASCII_TOKEN_SUPERSET.finditer(raw):
                    token_paths[match.group().decode("ascii").casefold()].add(path)
            self._folded_token_paths = token_paths
        return self._folded_token_paths

    def _require_content_absent_casefold(self, query: str) -> None:
        """Reject a negative content label if any frozen file contains the query."""
        _require_query(ASCII_CONTENT_ABSENT_CASEFOLD, query)
        folded = query.casefold()
        for path, (raw, _digest) in sorted(self.files.items()):
            if folded in raw.decode("utf-8", "replace").casefold():
                raise SourceOracleError(f"content absent contract found a match: {path}")

    def _require_code_search_absent_casefold(self, query: str) -> None:
        """Prove literal content/path absence, excluding the default typo fallback."""
        _require_query(ASCII_CODE_SEARCH_ABSENT_CASEFOLD, query)
        self._require_content_absent_casefold(query)
        folded = query.casefold()
        for path in self.files:
            if folded in path.casefold():
                raise SourceOracleError(f"code search absent contract found a path match: {path}")

    def _require_code_search_default_absent_casefold(self, query: str) -> None:
        """Prove no literal or one-edit fallback match for default file search."""
        _require_query(ASCII_CODE_SEARCH_DEFAULT_ABSENT_CASEFOLD, query)
        self._require_code_search_absent_casefold(query)
        self._require_identifier_osa1_absent_casefold(query)

    def _require_identifier_osa1_absent_casefold(self, query: str) -> None:
        """Prove no ASCII source token is within one OSA edit of the query."""
        _require_query(ASCII_IDENTIFIER_OSA1_ABSENT_CASEFOLD, query)
        if not 3 <= len(query) <= 64:
            raise SourceOracleError("identifier osa1 absent contract requires 3..=64 bytes")
        folded = query.lower()
        for path, (raw, _digest) in sorted(self.files.items()):
            for match in ASCII_TOKEN_SUPERSET.finditer(raw):
                token = match.group().decode("ascii").lower()
                if abs(len(token) - len(folded)) <= 1 and osa_distance_at_most_one(folded, token):
                    raise SourceOracleError(
                        f"identifier osa1 absent contract found a source token: {path}"
                    )

    def _index_declarations(
        self, language: str, excluded: frozenset[str] = frozenset()
    ) -> dict[bytes, list[tuple[str, int, int, int, int]]]:
        key = (language, excluded)
        if key not in self._declarations:
            declarations: dict[bytes, list[tuple[str, int, int, int, int]]] = defaultdict(list)
            for path, (raw, _digest) in self.files.items():
                source_language = declaration_language(path)
                if source_language is None or language not in (
                    source_language,
                    ALL_DECLARATION_LANGUAGES,
                ):
                    continue
                if path in excluded:
                    try:
                        self._declaration_census(source_language, path, raw)
                    except SourceOracleError as exc:
                        if not _excludable_census_refusal(exc):
                            raise
                    else:
                        raise SourceOracleError(
                            f"excluded declaration file has a complete census: {path}"
                        )
                    continue
                # Every declaration is kept: variant contracts match names that
                # differ from the submitted query bytes.
                try:
                    census = self._declaration_census(source_language, path, raw)
                except SourceOracleError as exc:
                    if path in self._excluded_declaration_paths[
                        language
                    ] and _excludable_census_refusal(exc):
                        raise SourceOracleError(
                            f"declaration exclusion lacks explicit query eligibility: {path}"
                        ) from exc
                    raise
                for start, end, definition_start, definition_end, _kind in census:
                    declarations[raw[start:end]].append(
                        (path, start, end, definition_start, definition_end)
                    )
            self._declarations[key] = declarations
        return self._declarations[key]

    def _declaration_census(
        self, language: str, path: str, raw: bytes
    ) -> list[tuple[int, int, int, int, str]]:
        if self._census_cache is None:
            return declaration_census(language, path, raw)
        return self._census_cache.census(language, path, raw, self.files[path][1])

    def declared_names(self, language: str) -> list[str]:
        if self._excluded_declaration_paths[language]:
            raise SourceOracleError("declared names require a complete declaration census")
        return sorted(_name_text(token) for token in self._index_declarations(language))

    def census_failures(self, language: str) -> list[dict[str, str]]:
        """Files whose declaration census refuses, for visible unsupported strata."""
        failures = []
        for path, (raw, _digest) in sorted(self.files.items()):
            source_language = declaration_language(path)
            if source_language is not None and language in (
                source_language,
                ALL_DECLARATION_LANGUAGES,
            ):
                try:
                    declaration_census(source_language, path, raw)
                except SourceOracleError as exc:
                    failures.append({"path": path, "reason": str(exc)})
        return failures

    def _name_matches(self, contract: str, query: str) -> list[tuple[str, int, int, int, int]]:
        _require_query(contract, query)
        cached = self._name_match_cache.get((contract, query))
        if cached is not None:
            return list(cached)
        language, variant = NAME_CONTRACTS[contract]
        excluded = self.declaration_exclusions.get((contract, query), frozenset())
        for path in sorted(excluded):
            if not declaration_query_textually_excluded(self.files[path][0], query, variant):
                raise SourceOracleError(
                    f"excluded declaration file may contain a query match: {path}"
                )
        declarations = self._index_declarations(language, excluded)
        if variant == "exact":
            matches = list(declarations.get(query.encode("ascii"), []))
            self._name_match_cache[(contract, query)] = tuple(matches)
            return matches
        index_key = (language, excluded)
        if index_key not in self._indexed_names:
            indexed_names = []
            by_length: dict[int, list[int]] = defaultdict(list)
            for token, rows in declarations.items():
                name = _name_text(token)
                by_length[len(name)].append(len(indexed_names))
                indexed_names.append((name, rows))
            self._indexed_names[index_key] = indexed_names
            self._name_lengths[index_key] = by_length
        indexed_names = self._indexed_names[index_key]
        if variant in ("osa1", "osa1_casefold"):
            candidate_indices = sorted(
                index
                for length in (len(query) - 1, len(query), len(query) + 1)
                for index in self._name_lengths[index_key].get(length, ())
            )
            candidates = (indexed_names[index] for index in candidate_indices)
        else:
            candidates = iter(indexed_names)
        matches = [
            match
            for name, rows in candidates
            if _variant_matches(variant, query, name)
            for match in rows
        ]
        self._name_match_cache[(contract, query)] = tuple(matches)
        return matches

    def expected_rows(self, contract: str, query: str, unit: str) -> list[dict[str, Any]]:
        if contract in DECLARATION_NAME_CONTRACTS and unit in ("symbol", "distinct_file"):
            matches = self._name_matches(contract, query)
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
        elif contract == ASCII_CODE_SEARCH_ABSENT_CASEFOLD and unit == "distinct_file":
            self._require_code_search_absent_casefold(query)
            paths = set()
        elif contract == ASCII_CODE_SEARCH_DEFAULT_ABSENT_CASEFOLD and unit == "distinct_file":
            self._require_code_search_default_absent_casefold(query)
            paths = set()
        elif contract == ASCII_IDENTIFIER_OSA1_ABSENT_CASEFOLD and unit == "distinct_file":
            self._require_identifier_osa1_absent_casefold(query)
            paths = set()
        else:
            raise SourceOracleError("unsupported source oracle contract/unit combination")
        return [
            {"path": path, "file_sha256": self.files[path][1], "grade": 3} for path in sorted(paths)
        ]

    def matched_names(self, contract: str, query: str) -> list[str]:
        """Distinct declaration names satisfying a name contract, for ambiguity strata."""
        if contract not in DECLARATION_NAME_CONTRACTS:
            raise SourceOracleError("unsupported source oracle contract")
        return sorted(
            {
                _name_text(self.files[path][0][start:end])
                for path, start, end, *_definition in self._name_matches(contract, query)
            }
        )

    def typo_gold_partition(self, language: str, query: str, intended_name: str) -> dict[str, Any]:
        """Separate source facts from the unreviewed intent of a noisy query.

        The near-name file set is exhaustive for the declared OSA1 contract;
        intended-name files are the exact declarations of the generator's base
        name. Neither set is a human relevance judgment.
        """
        contract = next(
            (key for key, value in NAME_CONTRACTS.items() if value == (language, "osa1_casefold")),
            None,
        )
        exact = next(
            (key for key, value in NAME_CONTRACTS.items() if value == (language, "exact")),
            None,
        )
        if contract is None or exact is None:
            raise SourceOracleError("unsupported typo gold language")
        _require_query(contract, query)
        _require_query(exact, intended_name)
        names = self.matched_names(contract, query)
        if intended_name not in names:
            raise SourceOracleError("intended declaration is not within one edit")
        intended = self.expected_rows(exact, intended_name, "distinct_file")
        near = self.expected_rows(contract, query, "distinct_file")
        folded = query.casefold()
        collisions = sorted(self._index_folded_tokens().get(folded, ()))
        excluded = self.declaration_exclusions.get((contract, query), frozenset())
        index_key = (language, excluded)
        if index_key not in self._folded_declaration_names:
            # matched_names() above built and validated the complete name index.
            self._folded_declaration_names[index_key] = frozenset(
                name.casefold() for name, _rows in self._indexed_names[index_key]
            )
        query_is_declaration_name = folded in self._folded_declaration_names[index_key]
        return {
            "intended_base_name": intended_name,
            "intended_base_files": [row["path"] for row in intended],
            "near_declaration_names": names,
            "near_declaration_files": [row["path"] for row in near],
            "other_near_declaration_names": [name for name in names if name != intended_name],
            "exact_content_collision_paths": collisions,
            "query_is_declaration_name": query_is_declaration_name,
            "user_intent_state": "unjudged",
        }

    def declaration_name_spans(self, contract: str, query: str) -> list[tuple[str, int, int]]:
        """Return local-name bytes for validating the separate gold line projection."""
        if contract not in DECLARATION_NAME_CONTRACTS:
            raise SourceOracleError("unsupported source oracle contract")
        return [
            (path, name_start, name_end)
            for path, name_start, name_end, *_definition in self._name_matches(contract, query)
        ]

    def first_match(self, contract: str, query: str) -> tuple[str, int, int] | None:
        """Choose the first source match by path and byte offset for a diagnostic gold line."""
        if contract in (
            ASCII_CONTENT_ABSENT_CASEFOLD,
            ASCII_CODE_SEARCH_ABSENT_CASEFOLD,
            ASCII_CODE_SEARCH_DEFAULT_ABSENT_CASEFOLD,
            ASCII_IDENTIFIER_OSA1_ABSENT_CASEFOLD,
        ):
            if contract == ASCII_CODE_SEARCH_DEFAULT_ABSENT_CASEFOLD:
                self._require_code_search_default_absent_casefold(query)
            elif contract == ASCII_CODE_SEARCH_ABSENT_CASEFOLD:
                self._require_code_search_absent_casefold(query)
            elif contract == ASCII_IDENTIFIER_OSA1_ABSENT_CASEFOLD:
                self._require_identifier_osa1_absent_casefold(query)
            else:
                self._require_content_absent_casefold(query)
            return None
        if contract == ASCII_IDENTIFIER_WORD:
            _require_query(contract, query)
            self._index_words()
            return self._first_words.get(query.encode("ascii"))
        if contract in DECLARATION_NAME_CONTRACTS:
            matches = self._name_matches(contract, query)
            if not matches:
                return None
            path, start, end, _definition_start, _definition_end = min(matches)
            return path, start, end
        raise SourceOracleError("unsupported source oracle contract")
