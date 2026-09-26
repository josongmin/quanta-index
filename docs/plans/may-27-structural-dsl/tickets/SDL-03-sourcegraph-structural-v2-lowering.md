# SDL-03 - Sourcegraph Structural V2 Lowering

> Archive status: `Historical program record`. Current architecture: [JUN-02-001](../../../adr/JUN-02-001-search-dsl-authority-and-runtime-contract.md). Archive map: [Completed Plan Archive](../../ARCHIVE-INDEX.md).


Status: `planned`
Priority: `P1`
Depends on: [SDL-01](SDL-01-structural-boolean-composition.md), relevant [SDL-02](SDL-02-typed-hole-semantics.md) semantics

## Purpose

Expand the Sourcegraph structural frontdoor only after native structural owns
the same semantics.

This ticket keeps the honesty rule from `BRIDGE-02/03`: parser/translator
surface may widen only where the native structural route already executes.

## Owner files

- `crates/quanta-index-lq-bridge/src/syntax.rs`
- `crates/quanta-index-lq-bridge/src/translator.rs`
- `crates/quanta-index-search-plane/src/lowering.rs`
- `crates/quanta-index-search-plane/src/query_dispatcher.rs`
- `crates/quanta-index-searchd-runtime/tests/e2e_dual_syntax_lowering_parity.rs`
- `crates/quanta-index-searchd-runtime/tests/sdk_frontdoor.rs`

## File-level work breakdown

- `syntax.rs`
  - parse the next SG structural subset explicitly instead of collapsing back
    into lexical syntax buckets
- `translator.rs`
  - lower SG structural boolean/typed-hole shapes onto native structural IR
- `lowering.rs`
  - keep route selection structural-only; no lexical fallback path
- runtime tests
  - prove parity and typed rejection rows from the public frontdoor

## Work items

- widen the SG structural subset in lock-step with native semantics:
  - structural boolean composition
  - typed holes where native semantics exist
  - richer body forms only when the native structural IR can represent them
- keep unsupported SG regex-body and unsupported composition rows as typed
  `BRIDGE_TRANSLATE_FAIL` or the owning structural error
- preserve current quoted/keyword SG structural behavior unchanged
- make SG/native parity the shipping gate for every widened structural form

## Test plan

- SG syntax tests for the new structural forms
- translator tests proving SG structural never lowers as lexical text
- lowering tests for route selection and typed rejection
- parity tests on native vs SG structural results

## E2E plan

Owned by [SDL-E2E-01](SDL-E2E-01-structural-proof-and-observability.md):

- SG/native parity rows for structural `AND` / `OR`
- SG typed-hole parity rows where native semantics exist
- SG rejection rows for still-unsupported structural regex bodies

## DoD

- SG structural frontdoor supports only semantics native structural already owns
- parity proof exists for every widened SG structural form
- no SG structural request falls back to lexical execution

## Failure modes

- widening SG syntax ahead of native semantics
- treating SG structural regex-body support as "parse yes, maybe runtime no"
- hiding translator gaps behind generic lexical lowering
