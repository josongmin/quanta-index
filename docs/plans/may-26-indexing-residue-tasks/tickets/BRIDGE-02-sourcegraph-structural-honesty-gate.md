# BRIDGE-02 - Sourcegraph Structural Honesty Gate

Status: `shipped`
Priority: `P0`
Depends on: none
Blocked by: none

## Purpose

Remove the current Sourcegraph structural ambiguity where
`patterntype:structural` is accepted as an option value but the Sourcegraph
frontdoor cannot actually produce a structural query route.

## Landed truth

- Sourcegraph lexical lowering now rejects `patterntype:structural` early with
  `BRIDGE_TRANSLATE_FAIL`
- the lexical route no longer defers structural shapes to later fail-closed
  runtime behavior
- Sourcegraph lexical subset behavior for non-structural queries remains
  unchanged

## Owner files

- `crates/quanta-index-search-plane/src/lowering.rs`
- `crates/quanta-index-core/src/domains/lexical/service.rs`
- `crates/quanta-index-searchd-runtime/tests/e2e_dual_syntax_lowering_parity.rs`
- `crates/quanta-index-searchd-runtime/tests/e2e_lexical_full_fidelity.rs`
- related bridge/lowering tests only

## File-level work breakdown

- `lowering.rs`
  - stop treating `patterntype:structural` as a harmless accepted slot unless a
    real structural route exists
- `service.rs`
  - remove misleading lexical-surface acceptance paths for structural pattern
    type
- E2E tests
  - assert the new honest behavior explicitly

## Work items

- choose one of two honest outcomes:
  - route SG structural onto a real structural request surface, or
  - reject it early with a dedicated typed boundary before lexical execution
- remove the current accepted-but-not-routed ambiguity
- keep Sourcegraph lexical subset behavior unchanged

## Test plan

- lowering tests for `patterntype:structural`
- typed-error tests for the chosen early-boundary behavior

## E2E plan

- promote one `ExpectedFailing` row or add a new typed-error row in:
  - `crates/quanta-index-searchd-runtime/tests/e2e_dual_syntax_lowering_parity.rs`
  - `crates/quanta-index-searchd-runtime/tests/e2e_lexical_full_fidelity.rs`

## DoD

- `patterntype:structural` no longer pretends to be a valid lexical SG query
  shape
- runtime behavior is either real structural routing or early typed rejection
- no lexical path returns `STR_PRODUCER_PARSE_TREE_UNAVAILABLE` for a shape that
  should have been rejected earlier at the bridge/frontdoor boundary

Status: satisfied.

## Failure modes

- option value still parses but disappears into lexical fail-closed later
- bridge rejects too broadly and breaks existing SG lexical subset
- behavior changes without parity tests documenting the new truth
