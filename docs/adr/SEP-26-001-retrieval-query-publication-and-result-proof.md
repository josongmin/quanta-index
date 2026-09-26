# SEP-26-001 — Retrieval Query, Publication and Result-Proof Contracts

Status: `Accepted`

Decided: 2026-09-27

Source campaign: RBR-02, RBR-04, RBR-05, RBR-06 and RBR-08

## Context

The retrieval benchmark previously mixed caller text, product-specific query syntax, published chunks, source symbols
and evaluator spans. That allowed policy changes to look like ranking improvements and allowed a hit to be scored
without proving which published unit produced it.

## Decision

### Query input policy

Every query selects exactly one policy before execution:

- `native`: pass the caller's product-native request through the native parser;
- `literal`: preserve literal text and use the product's declared literal path;
- `natural_language`: build the deterministic lexical token-OR plan and keep the semantic input identity separate.

The runner records the original query digest, the effective lexical request digest and the semantic text digest.
Planning configuration and whether planning time is included in latency are frozen before a run. Evaluator gold,
holdout labels and expected ranks are not query-planner inputs.

Unsupported Phrase, RawString, Regex, regexp-keyword and per-result content-filter combinations on the current symbol
route return `LEX_PLANNER_UNSUPPORTED_FILTER_COMBO`. They cannot become empty exhaustive success or silently fall
back to chunk search. Repository and file predicates remain chunk-owned constraints.

### Publication authority

An admitted source file produces chunks and source-bound symbols in one combined replacement. The scope digest binds
the complete unit set, symbol payload and producer identity. A symbol-only change therefore changes the scope digest.
Duplicate IDs and cross-kind ID collisions are rejected.

The symbol producer identity binds parser and grammar versions, lockfile digest and per-language capability. Supported
parse failure is a coverage failure. Unsupported input is recorded per path with source digest and typed reason; it is
not counted as successful full coverage.

Symbol identity is derived from parser structure, not delimiter scanning or
lexical ancestor guesses. Rust generic impl owners drop arguments only when the
grammar identifies a generic type; methods require direct impl/trait
declaration-list ownership. JavaScript and TypeScript named function
declarations stay functions inside class initializers and static blocks;
explicit method definitions stay methods. Python uses the nearest named scope
through decorators and control-flow blocks. A malformed or unsupported parse
remains a typed failure, never a guessed symbol identity.

### Result proof

The typed published-unit registry is the authority for result kind, path and byte span. A result is valid only when it
binds to an admitted registry entry in the requested generation and the response span satisfies that entry's contract.
Snippets, filenames and search-engine payloads do not create authority. Forged, stale, cross-generation and unanchored
symbol hits are rejected. No-answer and timeout remain distinct from exact exhaustion.

### Span accounting

Indexed bytes, returned SDK bytes and evaluator-scored bytes are separate quantities. Rank metrics use the indexed hit
identity; context cost uses the returned byte/token extent; exact-span recall uses the independently declared source
span. Overlapping byte ranges are unioned before coverage is computed. UTF-8 and CRLF fixtures use byte offsets.

Chunking strategies remain explicit profile values. A development comparison may select among a finite declared
matrix, but it cannot change public defaults or claim semantic quality without the qualification contract in
[SEP-26-003](SEP-26-003-retrieval-evidence-custody-and-qualification.md).

## Consequences

- Query-policy, publication and scoring changes invalidate source-bound retrieval receipts.
- Canonical symbol text authority remains a schema, ingest, lifecycle and cursor migration. Until such an ADR is
  accepted, unsupported symbol text forms stay typed refusals.
- Exact-name ranking or rank boosts require a source-bound misranking case, development ablation and final holdout
  admission. Absence of that evidence preserves the current ranking policy.

## Historical record

The completed RBR packets were removed from the live tree after consolidation.
Their exact pre-deletion bodies are available with
`git show eff53181:<path>`; [the execution SSOT](../plans/sep-27-misc/tickets/INDEX.md)
owns remaining execution.
