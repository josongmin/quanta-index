# LXE-06 - Symbol, Select, and Type Execution

Status: `completed`
Priority: `P1`
Depends on: [LXE-02](LXE-02-planner-authority-ir.md), [LXE-03](LXE-03-lexical-filter-execution.md)

## Purpose

Make `symbol`, `select`, and `type` semantics explicit and executable. These
operators define which candidate surface is queried, so they cannot remain as
display-only metadata.

## Current code-backed status (2026-05-27)

- Landed on the live text/symbol rails:
  - `type:file`
  - `select:file`
  - `select:content`
  - `type:symbol` symbol-doc routing
  - `select:symbol` symbol-doc routing
- Landed on the public symbol contract:
  - `SymbolQueryResponse` is no longer an alias over `TextQueryResponse<LexicalCandidate>`
  - `SymbolCandidate` is the public carrier for symbol results
  - symbol hits carry `symbol_kind` plus optional `symbol_kind_family`
  - lexical symbol docs store and recover `symbol_kind` truth from the index
- Landed public proof:
  - contract round-trip / duplicate-field rejection for `SymbolQueryResponse`
  - SDK/query surface positive proof for native `select:symbol`
  - SDK/query surface positive proof for native `type:symbol`
  - SDK/query surface positive proof for Sourcegraph `select:symbol`
  - SDK/query surface positive proof for Sourcegraph `type:symbol`
  - SG/native parity proof for both `type:symbol` and `select:symbol`
- Still outside this ticket:
  - history/diff surfaces remain owned by `LXE-08`
  - broader structural/result-surface proof remains owned by `LXE-09` and `E2E-04`

## Owner files

- `crates/quanta-index-lq-symbol/src/**`
- `crates/quanta-index-lexical/src/lib.rs`
- new `crates/quanta-index-lexical/src/symbol.rs`
- `crates/quanta-index-search-plane/src/lowering.rs`
- `crates/quanta-index-contract/src/results/**`
- `crates/quanta-index-contract/tests/ipc_query_result_v2_contract.rs`
- `crates/quanta-index-searchd-runtime/tests/dsl_scenarios.rs`
- new `crates/quanta-index-searchd-runtime/tests/e2e_lexical_full_fidelity.rs`
- `crates/quanta-index-searchd-runtime/tests/e2e_dual_syntax_lowering_parity.rs`
- `crates/quanta-index-searchd-runtime/tests/sdk_frontdoor.rs`

## File-level work breakdown

- `crates/quanta-index-lq-symbol/src/**`: expose symbol candidate lookup as a
  first-class engine, not content fallback.
- `crates/quanta-index-lexical/src/symbol.rs`: plan `symbol`, `select`, and
  `type` routes onto symbol/content/path/history/structural engines.
- `crates/quanta-index-lexical/src/lib.rs`: merge candidate kinds
  deterministically and preserve candidate kind in results.
- `crates/quanta-index-search-plane/src/lowering.rs`: lower Sourcegraph and LQ
  `select:` and `type:` into the same planner nodes.
- `crates/quanta-index-contract/src/results/**`: keep distinct result carriers
  for content, symbol, commit, diff, and structural responses; keep
  `SymbolQueryResponse` separate from lexical text carriers.

## Work items

- Define accepted `select` values and result surface:
  - content
  - path
  - symbol
  - repo, if indexed
  - commit/diff/structural only through their owning surfaces
- Define accepted `type` values and planner route:
  - file/content
  - path
  - symbol
  - commit
  - diff
  - structural
- Execute symbol search through symbol index, not content substring search.
- Return typed unavailable for producer-dependent surfaces that lack data.
- Ensure result carriers distinguish content, symbol, commit, diff, and
  structural candidates.
- Ensure Sourcegraph `type:` and `select:` lower into the same planner nodes.

## Test plan

- unit tests for `select` and `type` lowering.
- symbol query tests that avoid content false positives.
- response carrier tests for `SymbolCandidate` round-trip and duplicate-field
  rejection.
- typed unavailable tests for missing producer surfaces.

## E2E plan

Covered by `E2E-01`, `E2E-02`, `sdk_frontdoor.rs`, and contract proof:

- `select:symbol` returns symbol candidates only.
- `select:symbol` exposes `symbol_kind` truth on the public response carrier.
- `type:file` does not return symbol-only records.
- `type:diff` and `type:commit` return typed unavailable when no producer data
  has been indexed.
- Sourcegraph `select:symbol` matches equivalent LQ behavior.
- Sourcegraph `type:symbol` matches equivalent LQ behavior.

## DoD

- result kind is not inferred from display fields.
- `select` and `type` rows have live E2E coverage or typed unavailable proof.
- public symbol results carry authoritative `symbol_kind` truth.
- mixed-engine merge preserves total ordering across candidate kinds.

## Failure modes

- implementing `symbol:` as text search over snippets.
- allowing `type:` to filter after retrieval instead of routing the planner.
- returning empty success for unavailable history/structural surfaces.
