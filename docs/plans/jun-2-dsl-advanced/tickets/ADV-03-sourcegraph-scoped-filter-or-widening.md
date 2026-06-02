# ADV-03 Sourcegraph Scoped-Filter OR Widening

Parent packet: [../README.md](../README.md)

Status: `planned`

## Objective

Decide and, if viable, widen the currently rejected SG structural shape where
scoped filters sit under mixed `OR` alongside structural bodies.

## Current Source Truth

- current packet truth says repo-scoped filters under mixed `OR` remain
  `BRIDGE_TRANSLATE_FAIL`
- native execution already has the candidate-universe algebra to evaluate these
  shapes once legality is defined
- SG route has no frozen legality semantics here yet

## Current Code Pointers

- current reject authority:
  `crates/quanta-index-search-plane/src/lowering.rs`
  `rewrite_sourcegraph_structural_expr`,
  `sourcegraph_structural_route_rejects_repo_scoped_filter_under_mixed_or`,
  `scoped_filters_under_or_and_not_fail_closed_with_typed_translate_errors`
- bridge mirror:
  `crates/quanta-index-lq-bridge/src/translator.rs`
  and `crates/quanta-index-lq-bridge/tests/golden_bridge.rs`
- runtime/parity truth:
  `crates/quanta-index-searchd-runtime/tests/e2e_dual_syntax_lowering_parity.rs`
  plus `crates/quanta-index-searchd-runtime/tests/fixtures/lexical_corpus/runtime_rows.toml`
- current packet truth for exclusion:
  `docs/plans/jun-2-dsl-final-cut/tickets/JFC-05-sourcegraph-bridge-and-carrier-parity.md`
- proof/doc truth to update if any family is legalized:
  `docs/plans/may-25-lexical-enhancement/dsl-proof-ledger.toml`,
  `docs/plans/may-25-lexical-enhancement/lexical-capability-matrix.md`

## 핵심 로직

- define whether scoped filters under mixed `OR` are:
  - representable and widened
  - representable only under restrictions
  - permanently non-representable
- legality must be explicit and proof-backed; implicit acceptance is forbidden

## 건드릴 파일

- `crates/quanta-index-search-plane/src/lowering.rs`
- `crates/quanta-index-lq-bridge/src/translator.rs`
- `crates/quanta-index-lq-bridge/tests/golden_bridge.rs`
- `crates/quanta-index-searchd-runtime/tests/e2e_dual_syntax_lowering_parity.rs`
- `docs/plans/may-25-lexical-enhancement/dsl-proof-ledger.toml`
- `docs/plans/may-25-lexical-enhancement/lexical-capability-matrix.md`
- `docs/plans/jun-2-dsl-final-cut/tickets/JFC-05-sourcegraph-bridge-and-carrier-parity.md`

## 생성 가능 파일

- default: add rows to existing tests and `runtime_rows.toml`
- if the legality decision table no longer fits cleanly in `lowering.rs`, extract
  one helper under the same owner crate, for example
  `crates/quanta-index-search-plane/src/lowering/structural_matrix.rs`
- if scenario count becomes too dense, add one focused fixture file under
  `crates/quanta-index-searchd-runtime/tests/fixtures/lexical_corpus/docs-structural-scoped-or.toml`

## 건드리지 말 것

- bridge directives
- native runtime semantics outside the already-executable mixed algebra
- broad planner rewrites unrelated to SG lowering legality

## TODO

- [ ] freeze legality for scoped filters under mixed `OR`
- [ ] either widen the representable subset or document permanent typed-fail
- [ ] add direct lowering tests and parity proof for each accepted scoped-filter family
- [ ] keep all rejected shapes on explicit typed-fail rails with stable diagnostics

## Concrete Decision Table To Produce

At minimum, the ticket must produce a table for:

- filter family: `repo`, `file`, `lang`, predicate-backed repo gate
- boolean placement: left child of `OR`, right child of `OR`, nested under `AND NOT`
- structural sibling kind: structural body, widened raw sibling, widened predicate sibling
- verdict: `accept`, `accept-with-rewrite`, or `BRIDGE_TRANSLATE_FAIL`

No code change should land before this table exists in the ticket or successor docs.

## Concrete First Increment

The first PR for this ticket should do only this:

1. write the legality table for one scoped-filter family at a time
2. prove whether that family is `accept`, `accept-with-rewrite`, or permanent
   `BRIDGE_TRANSLATE_FAIL`
3. if no family is clearly legalizable, land the explicit permanent reject truth
   and stop

Do not start with a multi-family widening PR.

## Implementation Steps

1. red: pin the current rejected family with the existing owner-local fail-closed rails
2. refactor: enumerate the scoped-filter families that matter on the SG route
   and express their legality against the native mixed algebra explicitly
3. widen: choose one of two outcomes per family:
   - canonical widening with parity proof
   - permanent typed-fail with stable diagnostics
4. widen: add one parity row and one typed-fail row per accepted/rejected family
5. proof/doc sync: update packet truth, the ledger, and the matrix so future
   work does not treat unresolved cells as ambiguous

## Dependency / Import Constraints

- depends on `ADV-02` legality-matrix freeze
- must not widen any filter family whose native semantics are still route-specific
- no “accept then narrow later” phased behavior
- no new bridge-only semantics to make a scoped-`OR` family look representable
- keep owner authority in `quanta-index-search-plane`; runtime rows prove, they do not define legality

## Red Rails First

- current reject rail:
  `./scripts/cargow test -p quanta-index-search-plane --lib sourcegraph_structural_route_rejects_repo_scoped_filter_under_mixed_or -- --nocapture`
- cross-route typed-fail rail:
  `./scripts/cargow test -p quanta-index-search-plane --lib scoped_filters_under_or_and_not_fail_closed_with_typed_translate_errors -- --nocapture`
- bridge mirror rail:
  `./scripts/cargow test -p quanta-index-lq-bridge --test golden_bridge -- --nocapture`
- parity rail for any legalized family:
  `./scripts/cargow test -p quanta-index-searchd-runtime --test e2e_dual_syntax_lowering_parity -- --nocapture`

## NOT TODO

- no silent subset acceptance
- no widening that depends on route-local heuristics
- no bridge semantics that diverge from native candidate algebra

## Test Plan

- `./scripts/cargow test -p quanta-index-lq-bridge --test golden_bridge -- --nocapture`
- `./scripts/cargow test -p quanta-index-searchd-runtime --test e2e_dual_syntax_lowering_parity -- --nocapture`

## DoD

- legality is explicit for scoped filters under mixed `OR`
- every accepted scoped-filter family has direct parity proof
- every rejected family has stable `BRIDGE_TRANSLATE_FAIL`

## Concrete Deliverables

1. one scoped-`OR` legality table with family-by-family verdicts
2. one direct owner-local fail-closed rail per permanently rejected family
3. one parity row per legalized family, if any are accepted
4. one synchronized ledger/matrix update so no cell remains implicit

## Failure Modes

- a partially widened subset lands without documentation
- SG accepts scoped `OR` shapes that native parity does not cover
- typed fail surface becomes route-order dependent
- legality differs between translator tests and runtime parity tests
