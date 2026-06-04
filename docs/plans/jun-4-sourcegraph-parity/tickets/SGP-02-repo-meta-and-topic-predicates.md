# SGP-02 Repo Meta And Topic Predicates

Parent RFC: [../rfc.md](../rfc.md)

Status: `planned`

## Objective

Implement:

- `repo:has.meta(...)`
- `repo:has.topic(...)`

with explicit repo authority and exact SG proof.

## Current Source Truth

- no executable registry family exists
- current `meta.*` surface is runtime catalog document metadata, not repo metadata authority
  - it must not be reused as if it were `repo:has.meta(...)`
- no repo metadata/topic authority is exposed through current lexical runtime
- current analysis doc lists both as unsupported
- `has.meta` and `has.topic` must not be treated as the same authority without proof
- this ticket is authority-blocked until a producer-side repo metadata/topic owner is chosen

## Files To Touch

- `crates/quanta-index-lq-bridge/src/translator.rs`
- `crates/quanta-index-search-plane/src/query_dispatcher.rs` once repo authority exists
- producer-side repo metadata/topic ingestion/materialization owner
- `crates/quanta-index-searchd-runtime/tests/fixtures/lexical_corpus/runtime_rows.toml`
- `crates/quanta-index-searchd-runtime/tests/e2e_filter_execution.rs`
- `crates/quanta-index-searchd-runtime/tests/e2e_dual_syntax_lowering_parity.rs`

## Concrete First Increment

Start with `repo:has.meta(key:value)` only.

Do not mix these shapes into the same first PR:

- key-only `repo:has.meta(key)`
- tag/null-value `repo:has.meta(tag:)`
- `repo:has.topic(...)`

## Implementation Steps

1. choose repo authority shape and persistence contract
2. land `repo:has.meta(key:value)` exact green
3. then widen to key-only and tag/null-value
4. only after `has.meta` authority is stable, decide whether `has.topic` can reuse it or needs a separate owner surface
5. then add `repo:has.topic(...)`

## Red Rail First

- exact runtime row plus parity row for `repo:has.meta(key:value)`

## DoD

- repo metadata/topic no-op behavior cannot pass the oracle

## Not Done If

- topic support is claimed while only generic metadata exists
- the ticket hides a new codehost-topic authority seam behind generic `meta` wording
- runtime `meta.*` document catalog fields are reused as if they were repo metadata
