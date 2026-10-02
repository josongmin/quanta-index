"""Independent Python re-derivation of the bench query input policies.

RBR-02 replay oracle: for a current v5 runner record, the evaluator re-derives the
per-task lexical request and identity digests from the frozen query-pack
query plus the recorded policy and compares them with the recorded
evidence. This module intentionally re-implements — rather than imports —
the Rust planner in ``benchmarks/retrieval/src/query_plan.rs`` so agreement
between the two is independent evidence, not shared code. Any divergence
fails closed (rejection), never silently passes.

Canonical spellings (policy config JSON, escaping, tokenization) are part
of the frozen profile contract; changing either side requires changing
both in one commit and re-freezing evidence.
"""

from __future__ import annotations

import hashlib
import re

import regex
import unicodedata2

NL_PLAN_PROFILE = "nl-token-or-v2"
PLANNING_COST_IN_LATENCY = False
TEXT_NORMALIZER_VERSION = "2.0"
MAX_TOKEN_BYTES = 256
MAX_INPUT_BYTES = 16 * 1024

#: Pinned natural-language plan profile (Rust ``NlPlanConfig::default()``).
DEFAULT_NL_CONFIG = {"max_token_chars": 96, "max_tokens": 32, "min_token_chars": 1}

#: Current policies and the immutable v4 policy inventory (RBR-02).
V4_SUPPORTED_POLICIES = ("native", "literal", "natural_language")
SUPPORTED_POLICIES = (
    *V4_SUPPORTED_POLICIES,
    "exact_symbol_name",
    "literal_file",
    "keyword_file",
    "substring_file",
    "code_search_file",
)
#: File-projection policies and how each orders its distinct files. A phrase or
#: raw substring is a match-only restriction (constant score, path order); a
#: bare keyword is scored.
ORDERING_SCORE_DESC = "score_desc_path_tiebreak"
ORDERING_PATH_ORDER = "path_order_constant_score"
FILE_PROJECTION_ORDERING = {
    "literal_file": ORDERING_PATH_ORDER,
    "keyword_file": ORDERING_SCORE_DESC,
    "substring_file": ORDERING_PATH_ORDER,
    "code_search_file": ORDERING_SCORE_DESC,
}
MAX_KEYWORD_FILE_BYTES = 256
MIN_SUBSTRING_FILE_BYTES = 3
MAX_SUBSTRING_FILE_BYTES = 256
#: Bare words the LQ tokenizer reads as boolean operators (case-sensitive).
_LQ_OPERATOR_WORDS = frozenset({"AND", "OR", "NOT"})
PROFILE_IDS = {
    "native": "quanta-native-v1",
    "literal": "quanta-literal-v1",
    "natural_language": "quanta-natural-language-ucd17-v2",
    "exact_symbol_name": "quanta-exact-symbol-name-v1",
    "literal_file": "quanta-literal-file-v1",
    "keyword_file": "quanta-keyword-file-v1",
    "substring_file": "quanta-substring-file-v1",
    "code_search_file": "quanta-code-search-file-v1",
}
_BARE_SYMBOL_NAME = re.compile(r"[A-Za-z_][A-Za-z_0-9]*\Z")
_BARE_CODE_SEARCH_ATOM = re.compile(r"[A-Za-z_0-9]+\Z")
_ASCII_WHITESPACE = re.compile(r"[ \t\n\v\f\r]+")
_NATIVE_NON_CONTENT_PROJECTION = re.compile(
    r"(?<![A-Za-z_0-9.])(?:select:(?:repo|file|file\.owners|path|symbol)|type:(?:repo|path))(?=$|[\s()])"
)
MAX_EXACT_SYMBOL_NAME_BYTES = 4096
# Independently mirror the public product contract, not the Rust bench planner.
MAX_CODE_SEARCH_TERMS = 32
MAX_CODE_SEARCH_TERM_BYTES = 256

_JOINING = "-_./"
_ALPHANUMERIC = regex.compile(r"\A(?:\p{Alphabetic}|\p{Number})\Z")
_MARK = regex.compile(r"\A\p{Mark}\Z")


class QueryPlanError(ValueError):
    """Typed refusal while re-deriving a query plan."""


def _sha256_hex(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def policy_config_canonical(policy: str, config: dict[str, int] | None = None) -> str:
    """Canonical policy-config JSON (byte-identical to the Rust planner)."""
    if policy == "native":
        return '{"policy":"native"}'
    if policy == "exact_symbol_name":
        return '{"case":"sensitive","field":"symbol.local_name.exact","policy":"exact_symbol_name"}'
    if policy == "literal":
        return '{"escaping":"lq-norm-phrase-v1","policy":"literal"}'
    if policy == "literal_file":
        return '{"escaping":"lq-norm-phrase-v1","policy":"literal_file","projection":"file"}'
    if policy == "keyword_file":
        return (
            '{"case":"sensitive","match":"bare_keyword",'
            f'"max_bytes":{MAX_KEYWORD_FILE_BYTES},"ordering":"{ORDERING_SCORE_DESC}",'
            '"policy":"keyword_file","projection":"file","scope":"content_and_path"}'
        )
    if policy == "substring_file":
        return (
            '{"case":"sensitive","match":"raw_substring",'
            f'"max_bytes":{MAX_SUBSTRING_FILE_BYTES},"min_bytes":{MIN_SUBSTRING_FILE_BYTES},'
            f'"ordering":"{ORDERING_PATH_ORDER}","policy":"substring_file",'
            '"projection":"file","scope":"content"}'
        )
    if policy == "code_search_file":
        return (
            '{"case":"folded","match":"code_search_v1",'
            f'"ordering":"{ORDERING_SCORE_DESC}","policy":"code_search_file",'
            '"projection":"file","scope":"content_and_path","syntax":"code_search"}'
        )
    if policy == "natural_language":
        resolved = dict(DEFAULT_NL_CONFIG) if config is None else config
        return (
            '{"escaping":"lq-norm-phrase-v1","policy":"natural_language",'
            f'"max_token_chars":{resolved["max_token_chars"]},'
            f'"max_tokens":{resolved["max_tokens"]},'
            f'"min_token_chars":{resolved["min_token_chars"]},'
            f'"profile":"{NL_PLAN_PROFILE}",'
            f'"text_normalizer_version":"{TEXT_NORMALIZER_VERSION}",'
            '"tokenization":"lexical-ssot-nfc-with-path-joiners"}'
        )
    raise QueryPlanError(f"unsupported query input policy: {policy}")


def execution_profile(policy: str, config: dict[str, int] | None = None) -> dict:
    if policy not in SUPPORTED_POLICIES:
        raise QueryPlanError(f"unsupported query input policy: {policy}")
    resolved = (
        dict(DEFAULT_NL_CONFIG)
        if policy == "natural_language" and config is None
        else (dict(config) if policy == "natural_language" and config is not None else {})
    )
    return {
        "profile_id": PROFILE_IDS[policy],
        "policy": policy,
        "config": resolved,
        "planning_cost_in_latency": False,
    }


def execution_profile_canonical(policy: str, config: dict[str, int] | None = None) -> str:
    import json

    return json.dumps(execution_profile(policy, config), sort_keys=True, separators=(",", ":"))


def execution_profile_sha256(policy: str, config: dict[str, int] | None = None) -> str:
    return _sha256_hex(execution_profile_canonical(policy, config).encode())


def literalize(raw: str) -> str:
    """Escape one raw string into a single lq-norm phrase literal."""
    out = ['"']
    for ch in raw:
        if ch == "\\":
            out.append("\\\\")
        elif ch == '"':
            out.append('\\"')
        elif ch == "\n":
            out.append("\\n")
        elif ch == "\r":
            out.append("\\r")
        elif ch == "\t":
            out.append("\\t")
        else:
            out.append(ch)
    out.append('"')
    return "".join(out)


def _refuse_native_rank_projection(raw: str) -> None:
    """Reject unquoted native projections whose result unit is not a chunk.

    This intentionally over-refuses ambiguous native syntax. A projected file
    diagnostic must use the named ``literal_file`` profile so its rank unit and
    request bytes are both bound by the runner record.
    """
    visible = list(raw)
    cursor = 0
    while cursor < len(raw):
        marker = raw[cursor]
        if marker not in ('"', "'"):
            cursor += 1
            continue
        start = cursor
        cursor += 1
        while cursor < len(raw):
            if marker == '"' and raw[cursor] == "\\":
                cursor += 2
            elif raw[cursor] == marker:
                cursor += 1
                break
            else:
                cursor += 1
        for offset in range(start, min(cursor, len(raw))):
            visible[offset] = " "
    if _NATIVE_NON_CONTENT_PROJECTION.search("".join(visible)):
        raise QueryPlanError("native non-content projection requires an explicit rank profile")


def tokenize_nl(raw: str) -> list[str]:
    """Deterministic tokenization: Unicode alphanumeric runs joined by
    ``-``, ``_``, ``.``, ``/``; everything else separates. Case preserved."""

    def joins(ch: str) -> bool:
        return ch in _JOINING

    tokens: list[str] = []
    current: list[str] = []
    for ch in unicodedata2.normalize("NFC", raw):
        if _is_token_char(ch) or joins(ch):
            current.append(ch)
        elif current:
            tokens.append("".join(current))
            current = []
    if current:
        tokens.append("".join(current))
    return tokens


def _is_token_char(ch: str) -> bool:
    return ch == "_" or _ALPHANUMERIC.fullmatch(ch) is not None or _MARK.fullmatch(ch) is not None


def _validate_indexable_text(raw: str) -> bool:
    """Validate the lexical SSOT's NFC token existence and byte cap.

    Returns ``False`` only when the text has no lexical token. An oversized
    term is a typed refusal because that term cannot exist in the index.
    """
    normalized = unicodedata2.normalize("NFC", raw)
    current: list[str] = []
    saw_token = False
    for ch in normalized + " ":
        if _is_token_char(ch):
            current.append(ch)
            continue
        if current:
            term = "".join(current)
            size = len(term.encode())
            if size > MAX_TOKEN_BYTES:
                raise QueryPlanError(
                    f"token of {size} bytes exceeds lexical term max {MAX_TOKEN_BYTES}"
                )
            saw_token = True
            current = []
    return saw_token


def plan_lexical_request(policy: str, raw: str, config: dict[str, int] | None = None) -> str:
    """Re-derive the effective lexical request bytes for one query.

    Raises ``QueryPlanError`` for an unsupported policy or an empty /
    over-limit natural-language plan — mirroring the Rust typed refusals.
    """
    if policy == "native":
        request = raw
        if len(request.encode()) > MAX_INPUT_BYTES:
            raise QueryPlanError("lexical request exceeds 16384 bytes")
        _refuse_native_rank_projection(request)
        return request
    if policy == "exact_symbol_name":
        if (
            len(raw.encode()) > MAX_EXACT_SYMBOL_NAME_BYTES
            or _BARE_SYMBOL_NAME.fullmatch(raw) is None
        ):
            raise QueryPlanError(
                "exact-symbol policy requires one bare ASCII symbol name of at most 4096 bytes"
            )
        return f"symbol.local_name.exact({raw}) case:yes"
    if policy in ("literal", "literal_file"):
        if not _validate_indexable_text(raw):
            raise QueryPlanError("natural-language plan produced no tokens")
        request = literalize(raw)
        if policy == "literal_file":
            request = "select:file " + request
        if len(request.encode()) > MAX_INPUT_BYTES:
            raise QueryPlanError("lexical request exceeds 16384 bytes")
        return request
    if policy == "keyword_file":
        if (
            len(raw.encode()) > MAX_KEYWORD_FILE_BYTES
            or _BARE_SYMBOL_NAME.fullmatch(raw) is None
            or raw in _LQ_OPERATOR_WORDS
        ):
            raise QueryPlanError(
                "keyword-file policy requires one bare ASCII identifier of at most 256 bytes"
            )
        return f"select:file case:yes {raw}"
    if policy == "substring_file":
        size = len(raw.encode())
        if size < MIN_SUBSTRING_FILE_BYTES or size > MAX_SUBSTRING_FILE_BYTES:
            raise QueryPlanError("substring-file fragment must be 3 to 256 bytes")
        if "'" in raw:
            raise QueryPlanError("a single quote cannot be carried by a raw string")
        # Rust `char::is_control`: Unicode general category Cc.
        if any(unicodedata2.category(ch) == "Cc" for ch in raw):
            raise QueryPlanError("control characters are not searchable fragment text")
        return f"select:file case:yes '{raw}'"
    if policy == "code_search_file":
        # Match Rust str::split_ascii_whitespace. Python str.split also treats
        # U+001C..U+001F as whitespace, which would admit a request the runner
        # refuses before capture and break independent replay.
        terms = [term for term in _ASCII_WHITESPACE.split(raw) if term]
        if (
            not raw
            or not raw.isascii()
            or len(raw.encode()) > MAX_INPUT_BYTES
            or not terms
            or len(terms) > MAX_CODE_SEARCH_TERMS
            or any(
                len(term.encode()) > MAX_CODE_SEARCH_TERM_BYTES
                or _BARE_CODE_SEARCH_ATOM.fullmatch(term) is None
                for term in terms
            )
        ):
            raise QueryPlanError(
                "code-search-file policy requires 1 to 32 bare ASCII alphanumeric atoms of at most 256 bytes each"
            )
        return raw
    if policy == "natural_language":
        resolved = dict(DEFAULT_NL_CONFIG) if config is None else config
        distinct: list[str] = []
        for token in tokenize_nl(raw):
            if len(token) > resolved["max_token_chars"]:
                raise QueryPlanError(
                    f"token of {len(token)} chars exceeds max {resolved['max_token_chars']}"
                )
            if len(token) < resolved["min_token_chars"]:
                continue
            if not _validate_indexable_text(token):
                continue
            if token not in distinct:
                distinct.append(token)
        if not distinct:
            raise QueryPlanError("natural-language plan produced no tokens")
        if len(distinct) > resolved["max_tokens"]:
            raise QueryPlanError(
                f"natural-language plan has {len(distinct)} tokens (max {resolved['max_tokens']})"
            )
        request = " OR ".join(literalize(token) for token in distinct)
        if len(request.encode()) > MAX_INPUT_BYTES:
            raise QueryPlanError("lexical request exceeds 16384 bytes")
        return request
    raise QueryPlanError(f"unsupported query input policy: {policy}")


def derive_query_identity(
    policy: str, raw: str, config: dict[str, int] | None = None
) -> dict[str, str]:
    """Re-derive the three per-task identity digests of a current v5 result."""
    if policy not in SUPPORTED_POLICIES:
        raise QueryPlanError(f"unsupported query input policy: {policy}")
    lexical_request = plan_lexical_request(policy, raw, config)
    if policy == "code_search_file":
        import json

        effective_bytes = json.dumps(
            {"query_text": lexical_request, "syntax": "code_search"},
            sort_keys=True,
            separators=(",", ":"),
            ensure_ascii=False,
        ).encode()
    else:
        effective_bytes = lexical_request.encode()
    return {
        "original_query_sha256": _sha256_hex(raw.encode()),
        "effective_lexical_request_sha256": _sha256_hex(effective_bytes),
        "semantic_text_sha256": _sha256_hex(raw.encode()),
    }


def policy_config_canonical_v4(policy: str, config: dict[str, int] | None = None) -> str:
    """Immutable runner-v4 profile bytes (nl-token-or-v1)."""
    if policy not in V4_SUPPORTED_POLICIES:
        raise QueryPlanError(f"unsupported v4 query input policy: {policy}")
    if policy != "natural_language":
        return policy_config_canonical(policy, config)
    resolved = dict(DEFAULT_NL_CONFIG) if config is None else config
    return (
        '{"escaping":"lq-norm-phrase-v1","policy":"natural_language",'
        f'"max_token_chars":{resolved["max_token_chars"]},'
        f'"max_tokens":{resolved["max_tokens"]},'
        f'"min_token_chars":{resolved["min_token_chars"]},'
        '"profile":"nl-token-or-v1",'
        '"tokenization":"unicode-alnum-joined-punct"}'
    )


def _plan_lexical_request_v4(policy: str, raw: str, config: dict[str, int] | None = None) -> str:
    if policy in ("native", "literal"):
        return raw if policy == "native" else literalize(raw)
    if policy != "natural_language":
        raise QueryPlanError(f"unsupported query input policy: {policy}")
    resolved = dict(DEFAULT_NL_CONFIG) if config is None else config
    tokens: list[str] = []
    current: list[str] = []
    for ch in raw:
        if ch.isalnum() or ch in _JOINING:
            current.append(ch)
        elif current:
            tokens.append("".join(current))
            current = []
    if current:
        tokens.append("".join(current))
    distinct: list[str] = []
    for token in tokens:
        if len(token) > resolved["max_token_chars"]:
            raise QueryPlanError("v4 token exceeds max_token_chars")
        if len(token) >= resolved["min_token_chars"] and token not in distinct:
            distinct.append(token)
    if not distinct or len(distinct) > resolved["max_tokens"]:
        raise QueryPlanError("v4 natural-language plan is empty or over limit")
    return " OR ".join(literalize(token) for token in distinct)


def derive_query_identity_v4(
    policy: str, raw: str, config: dict[str, int] | None = None
) -> dict[str, str]:
    if policy not in V4_SUPPORTED_POLICIES:
        raise QueryPlanError(f"unsupported v4 query input policy: {policy}")
    lexical_request = _plan_lexical_request_v4(policy, raw, config)
    return {
        "original_query_sha256": _sha256_hex(raw.encode()),
        "effective_lexical_request_sha256": _sha256_hex(lexical_request.encode()),
        "semantic_text_sha256": _sha256_hex(raw.encode()),
    }
