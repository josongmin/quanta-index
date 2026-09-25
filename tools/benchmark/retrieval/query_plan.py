"""Independent Python re-derivation of the bench query input policies.

RBR-02 replay oracle: for a v4 runner record, the evaluator re-derives the
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

NL_PLAN_PROFILE = "nl-token-or-v1"
PLANNING_COST_IN_LATENCY = False

#: Pinned natural-language plan profile (Rust ``NlPlanConfig::default()``).
DEFAULT_NL_CONFIG = {"max_token_chars": 96, "max_tokens": 32, "min_token_chars": 1}

#: The three canonical query input policies (RBR-02).
SUPPORTED_POLICIES = ("native", "literal", "natural_language")

_JOINING = "-_./"


class QueryPlanError(ValueError):
    """Typed refusal while re-deriving a query plan."""


def _sha256_hex(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def policy_config_canonical(policy: str, config: dict[str, int] | None = None) -> str:
    """Canonical policy-config JSON (byte-identical to the Rust planner)."""
    if policy == "native":
        return '{"policy":"native"}'
    if policy == "literal":
        return '{"escaping":"lq-norm-phrase-v1","policy":"literal"}'
    if policy == "natural_language":
        resolved = dict(DEFAULT_NL_CONFIG) if config is None else config
        return (
            '{"escaping":"lq-norm-phrase-v1","policy":"natural_language",'
            f'"max_token_chars":{resolved["max_token_chars"]},'
            f'"max_tokens":{resolved["max_tokens"]},'
            f'"min_token_chars":{resolved["min_token_chars"]},'
            f'"profile":"{NL_PLAN_PROFILE}",'
            '"tokenization":"unicode-alnum-joined-punct"}'
        )
    raise QueryPlanError(f"unsupported query input policy: {policy}")


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


def tokenize_nl(raw: str) -> list[str]:
    """Deterministic tokenization: Unicode alphanumeric runs joined by
    ``-``, ``_``, ``.``, ``/``; everything else separates. Case preserved."""

    def joins(ch: str) -> bool:
        return ch in _JOINING

    tokens: list[str] = []
    current: list[str] = []
    for ch in raw:
        if ch.isalnum() or joins(ch):
            current.append(ch)
        elif current:
            tokens.append("".join(current))
            current = []
    if current:
        tokens.append("".join(current))
    return tokens


def plan_lexical_request(
    policy: str, raw: str, config: dict[str, int] | None = None
) -> str:
    """Re-derive the effective lexical request bytes for one query.

    Raises ``QueryPlanError`` for an unsupported policy or an empty /
    over-limit natural-language plan — mirroring the Rust typed refusals.
    """
    if policy == "native":
        return raw
    if policy == "literal":
        return literalize(raw)
    if policy == "natural_language":
        resolved = dict(DEFAULT_NL_CONFIG) if config is None else config
        distinct: list[str] = []
        for token in tokenize_nl(raw):
            if len(token) > resolved["max_token_chars"]:
                raise QueryPlanError(
                    f"token of {len(token)} chars exceeds max "
                    f"{resolved['max_token_chars']}"
                )
            if len(token) < resolved["min_token_chars"]:
                continue
            if token not in distinct:
                distinct.append(token)
        if not distinct:
            raise QueryPlanError("natural-language plan produced no tokens")
        if len(distinct) > resolved["max_tokens"]:
            raise QueryPlanError(
                f"natural-language plan has {len(distinct)} tokens "
                f"(max {resolved['max_tokens']})"
            )
        return " OR ".join(literalize(token) for token in distinct)
    raise QueryPlanError(f"unsupported query input policy: {policy}")


def derive_query_identity(
    policy: str, raw: str, config: dict[str, int] | None = None
) -> dict[str, str]:
    """Re-derive the three per-task identity digests of a v4 result."""
    if policy not in SUPPORTED_POLICIES:
        raise QueryPlanError(f"unsupported query input policy: {policy}")
    lexical_request = plan_lexical_request(policy, raw, config)
    return {
        "original_query_sha256": _sha256_hex(raw.encode()),
        "effective_lexical_request_sha256": _sha256_hex(lexical_request.encode()),
        "semantic_text_sha256": _sha256_hex(raw.encode()),
    }
