# LXE-06 - Symbol, Select, and Type Execution

Status: `proposed`
Priority: `P1`
Depends on: [LXE-02](LXE-02-planner-authority-ir.md), [LXE-03](LXE-03-lexical-filter-execution.md)

## Purpose

Make `symbol`, `select`, and `type` semantics explicit and executable. These
operators define which candidate surface is queried, so they cannot remain as
display-only metadata.

## Owner files

- `crates/quanta-index-lq-symbol/src/**`
- `crates/quanta-index-lexical/src/lib.rs`
- new `crates/quanta-index-lexical/src/symbol.rs`
- `crates/quanta-index-search-plane/src/lowering.rs`
- `crates/quanta-index-contract/src/results/**`
- `crates/quanta-index-searchd-runtime/tests/dsl_scenarios.rs`
- new `crates/quanta-index-searchd-runtime/tests/e2e_lexical_full_fidelity.rs`
- new `crates/quanta-index-searchd-runtime/tests/e2e_sourcegraph_parity.rs`

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
  for content, symbol, commit, diff, and structural responses.

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
- response carrier tests for mixed candidate kinds.
- typed unavailable tests for missing producer surfaces.

## E2E plan

Covered by `E2E-01`, `E2E-02`, and `E2E-04`:

- `select:symbol` returns symbol candidates only.
- `type:file` does not return symbol-only records.
- `type:diff` and `type:commit` return typed unavailable when no producer data
  has been indexed.
- Sourcegraph `select:symbol` matches equivalent LQ behavior.

## DoD

- result kind is not inferred from display fields.
- `select` and `type` rows have live E2E coverage or typed unavailable proof.
- mixed-engine merge preserves total ordering across candidate kinds.

## Failure modes

- implementing `symbol:` as text search over snippets.
- allowing `type:` to filter after retrieval instead of routing the planner.
- returning empty success for unavailable history/structural surfaces.
