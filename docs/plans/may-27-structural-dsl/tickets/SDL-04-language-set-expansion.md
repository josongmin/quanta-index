# SDL-04 - Structural Language Set Expansion

> Archive status: `Historical program record`. Current architecture: [JUN-02-001](../../../adr/JUN-02-001-search-dsl-authority-and-runtime-contract.md). Archive map: [Completed Plan Archive](../../ARCHIVE-INDEX.md).


Status: `planned`
Priority: `P1`
Depends on: [SDL-01](SDL-01-structural-boolean-composition.md), [SDL-02](SDL-02-typed-hole-semantics.md)

## Purpose

Expand the structural ship language set only after the semantic core is stable.

This ticket is ordered after the semantic core on purpose. Adding grammars
before the matcher semantics stabilize multiplies the proof matrix without
closing the main correctness gap.

## Owner files

- `crates/quanta-index-lq-structural/src/lib.rs`
- `crates/quanta-index-lq-structural/src/types.rs`
- `crates/quanta-index-lq-structural/src/matcher.rs`
- `crates/quanta-index-search-plane/src/query_dispatcher.rs`
- `crates/quanta-index-searchd-runtime/tests/sdk_frontdoor.rs`
- `crates/quanta-index-searchd-runtime/tests/end_to_end.rs`
- `docs/ssot/producer-handoff.md`
- `docs/plans/may-24-lexical-indexing-sourcegraph/feature-scope.md`

## File-level work breakdown

- `lib.rs`, `types.rs`, `matcher.rs`
  - widen language gating and kind admissibility tables
- `query_dispatcher.rs`
  - keep explicit `STR_LANG_NOT_SUPPORTED` behavior until a language is truly
    shipped
- runtime tests
  - add per-language positive/negative proof
- docs
  - keep producer handoff and feature-scope language claims synchronized

## Work items

- split language expansion into two phases:
  - Java first
  - C / C++ / Ruby only after Java proof is stable
- require producer parse-tree support and matcher proof before a language moves
  from reserved/stretch to shipped
- extend typed-hole admissibility tables for each new language
- keep unknown or unshipped languages typed fail-closed

## Test plan

- per-language structural matcher golden tests
- runtime frontdoor tests for each newly shipped language
- negative rows for still-unshipped languages
- doc consistency review against producer handoff

## E2E plan

Owned by [SDL-E2E-01](SDL-E2E-01-structural-proof-and-observability.md):

- Java positive structural row
- Java typed-hole row
- unshipped-language negative rows after Java lands

## DoD

- ship language list is source/document/proof consistent
- every newly shipped language has positive runtime proof
- unshipped languages still fail with `STR_LANG_NOT_SUPPORTED`

## Failure modes

- upgrading language claims from parser acceptance alone
- adding a language without typed-hole admissibility coverage
- drifting producer-handoff docs away from the actual runtime ship set
