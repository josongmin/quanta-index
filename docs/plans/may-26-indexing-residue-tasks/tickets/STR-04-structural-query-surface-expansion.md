# STR-04 - Structural Query Surface Expansion

> Archive status: `Historical program record`. Current architecture: [JUN-02-001](../../../adr/JUN-02-001-search-dsl-authority-and-runtime-contract.md). Archive map: [Completed Plan Archive](../../ARCHIVE-INDEX.md).


Status: `shipped`
Priority: `P1`
Depends on: [STR-02](STR-02-authority-matcher-tree-walk-expansion.md)
Blocked by: none

## Purpose

Expose additional structural query surface only where live runtime authority can
actually execute it.

## Landed truth

- structural dispatcher/runtime now execute `lang:`, `repo:`, and `file:`
  filters on the authority-backed live subset
- unsupported filters still fail typed with `STR_INVALID_REQUEST`
- boolean composition of structural leaves remains an explicit one-top-level-leaf
  fence; this is intentional, not silent narrowing

## Owner files

- `crates/quanta-index-search-plane/src/query_dispatcher.rs`
- `crates/quanta-index-searchd/src/app/runtime.rs`
- `crates/quanta-index-searchd-runtime/tests/sdk_frontdoor.rs`
- `crates/quanta-index-searchd-runtime/tests/end_to_end.rs`
- related structural dispatcher tests only

## File-level work breakdown

- `query_dispatcher.rs`
  - widen accepted structural filters/composition only where runtime support
    exists
- `runtime.rs`
  - execute the widened filter surface against authority-backed structural
    search
- `sdk_frontdoor.rs`
  - add public contract proof for the new surface
- `end_to_end.rs`
  - close typed-error rows for still-unsupported structural filters

## Work items

- evaluate which currently-blocked structural filters are now executable after
  `STR-02`
- open only those filters at the dispatcher boundary
- keep non-executable filters fail-closed with `STR_INVALID_REQUEST`
- consider whether boolean composition of structural leaves is now supportable;
  if not, keep the one-top-level-leaf fence explicit

## Test plan

- dispatcher unit tests for newly-allowed filters
- runtime tests for positive hit/miss behavior
- typed negative tests for filters that remain unsupported

## E2E plan

- extend structural query rows in:
  - `crates/quanta-index-searchd-runtime/tests/sdk_frontdoor.rs`
  - `crates/quanta-index-searchd-runtime/tests/end_to_end.rs`

## DoD

- every newly-accepted structural filter has a real runtime execution path
- no structural filter is accepted and then ignored
- unsupported filters still fail typed before or during execution

Status: satisfied.

## Failure modes

- dispatcher admits a filter that runtime discards
- runtime matches by text heuristics instead of authority-backed metadata
- public surface is widened ahead of matcher support
