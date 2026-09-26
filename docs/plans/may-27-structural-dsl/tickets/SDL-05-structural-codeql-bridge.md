# SDL-05 - Structural CodeQL Bridge

> Archive status: `Historical program record`. Current architecture: [JUN-02-001](../../../adr/JUN-02-001-search-dsl-authority-and-runtime-contract.md). Archive map: [Completed Plan Archive](../../ARCHIVE-INDEX.md).


Status: `planned`
Priority: `P2`
Depends on: [SDL-01](SDL-01-structural-boolean-composition.md), [SDL-03](SDL-03-sourcegraph-structural-v2-lowering.md)

## Purpose

Close the structural side of `match { ... } + into:codeql` with typed candidate
export and no silent sink fallback.

This is a downstream bridge ticket. It does not alter producer authorship or
structural execution authority.

## Owner files

- `crates/quanta-index-contract/src/query/directives.rs`
- `crates/quanta-index-contract/src/results/query_responses.rs`
- `crates/quanta-index-lq-bridge/src/translator.rs`
- `crates/quanta-index-search-plane/src/lowering.rs`
- `crates/quanta-index-search-plane/src/query_dispatcher.rs`
- bridge execution surface to be created or extended under `crates/quanta-index-lq-bridge/**`
- `crates/quanta-index-searchd-runtime/tests/sdk_frontdoor.rs`

## File-level work breakdown

- contract files
  - pin the structural bridge request/response contract needed for
    `into:codeql`
- bridge/translator/lowering
  - lower structural bridge directives onto a typed bridge invocation
- dispatcher
  - materialize structural candidate packets from the real structural result
    model
- runtime tests
  - prove typed target invocation or typed unavailability

## Work items

- replace `PLAN_DEFERRED` for structural `into:codeql` with a typed bridge path
- preserve structural candidate identity and bindings in the export packet
- keep the bridge one-way: structural result set -> CodeQL sink
- surface typed failure when the CodeQL target is unavailable or rejects the
  packet
- do not route `into:codeql` through lexical candidate export if the query was
  structural

## Test plan

- contract round-trip tests for the bridge packet
- planner/translator tests for `into:codeql` lowering
- target-unavailable typed error test
- provenance and binding-carry tests

## E2E plan

Owned by [SDL-E2E-01](SDL-E2E-01-structural-proof-and-observability.md):

- `match { ... } into:codeql`
- target-unavailable typed error row

## DoD

- structural `into:codeql` no longer dies at plan time
- exported bridge packet preserves structural bindings
- no lexical bridge fallback exists for structural queries

## Failure modes

- dropping bindings during candidate export
- treating structural bridge as a lexical bridge variant
- silently succeeding without invoking the target sink
