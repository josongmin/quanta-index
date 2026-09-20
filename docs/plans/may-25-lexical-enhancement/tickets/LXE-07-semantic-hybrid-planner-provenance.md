# LXE-07 - Semantic/Hybrid Planner Provenance

Status: `completed`
Priority: `P0`
Depends on: [LXE-02](LXE-02-planner-authority-ir.md), [LXE-03](LXE-03-lexical-filter-execution.md)

## Purpose

Make semantic and hybrid search consume lexical scope as a materialized,
planner-proven candidate universe. The lexical side must not be a best-effort
filter applied after semantic retrieval.

## Current live truth (2026-05-27)

Landed on the current tree:

- semantic lexical scope is carried through `TextQueryRequest`
- semantic scope is materialized before semantic ranking
- hybrid constructs the lexical universe first and fuses only within that
  planned universe
- semantic/hybrid explanations now carry planner trace, engines touched,
  strategy, summary, and bounded early-stop truth when present
- runtime owner proof lives across:
  - `crates/quanta-index-searchd-runtime/tests/end_to_end.rs`
  - `crates/quanta-index-searchd-runtime/tests/dsl_scenarios.rs`
  - `crates/quanta-index-searchd-runtime/tests/sdk_frontdoor.rs`

## Owner files

- `crates/quanta-index-search-plane/src/query_dispatcher.rs`
- `crates/quanta-index-search-plane/src/lowering.rs`
- `crates/quanta-index-core/src/domains/semantic/**`
- `crates/quanta-index-core/src/domains/hybrid/**`
- `crates/quanta-index-core/src/domains/lexical/**`
- `crates/quanta-index-contract/src/query/requests.rs`
- `crates/quanta-index-contract/src/results/**`
- `crates/quanta-index-searchd-runtime/tests/end_to_end.rs`
- `crates/quanta-index-searchd-runtime/tests/dsl_scenarios.rs`
- `crates/quanta-index-searchd-runtime/tests/sdk_frontdoor.rs`

## File-level work breakdown

- `crates/quanta-index-contract/src/query/requests.rs`: ensure semantic and
  hybrid requests reference the shared `TextQueryRequest` scope struct.
- `crates/quanta-index-search-plane/src/lowering.rs`: lower semantic/hybrid
  lexical scope through the same lexical front door as standalone lexical
  search.
- `crates/quanta-index-search-plane/src/query_dispatcher.rs`: materialize the
  lexical universe before invoking semantic or hybrid execution.
- `crates/quanta-index-core/src/domains/{semantic,hybrid}/**`: accept planned
  lexical candidate universes and record provenance carried in
  `SearchExplanation`.
- `crates/quanta-index-searchd-runtime/tests/end_to_end.rs`: scoped versus
  unscoped behavior with exact candidate IDs, repeated tied hybrid ordering,
  and fail-closed scope behavior.
- `crates/quanta-index-searchd-runtime/tests/dsl_scenarios.rs`: explanation and
  planner-trace provenance for scoped semantic/hybrid execution.
- `crates/quanta-index-searchd-runtime/tests/sdk_frontdoor.rs`: public SDK
  semantic/hybrid happy-path proof on the same execution stack.

## Work items

- Semantic:
  - parse lexical scope through `TextQueryRequest`
  - materialize lexical candidate IDs first
  - pass candidate universe to semantic executor
  - fail typed if lexical scope cannot be planned
- Hybrid:
  - execute lexical planner first
  - define lexical universe before semantic fusion
  - fuse only candidates in the planned universe unless the request explicitly
    asks for unscoped hybrid
  - preserve deterministic tie ordering
- Add provenance fields to explanation:
  - lexical scope plan hash
  - lexical candidate count
  - semantic candidate count
  - fusion strategy
  - early stop reason

## Test plan

- unit tests for scoped semantic request lowering.
- unit tests for hybrid lexical-first ordering.
- deterministic merge tests with equal scores.
- typed error tests for invalid lexical scope in semantic/hybrid requests.
- live proof:
  - `./scripts/cargow --lane test-daemon-lane nextest run -p quanta-index-searchd-runtime --test runtime_fast_suite --all-features --locked -E 'test(/end_to_end::(default_indexed_queries_share_one_fixture|semantic_query_with_lexical_scope_returns_intersection_only|semantic_scoped_query_ignores_out_of_scope_global_nearest_hit|hybrid_query_admits_a_semantic_only_relevant_hit_beside_the_lexical_hits|hybrid_query_repeated_tied_scope_query_keeps_stable_order)/)'`
  - `./scripts/cargow --lane test-daemon-lane nextest run -p quanta-index-searchd-runtime --test runtime_risk_suite --all-features --locked -E 'test(/dsl_scenarios::(semantic_scoped_query_with_complex_scope_excludes_outsiders_and_explains_scope|hybrid_query_reports_complex_scope_explanation_accounting)/)'`

## E2E plan

Covered by `E2E-03`:

- same semantic query returns different results when scoped by repo/path.
- invalid lexical scope prevents semantic execution.
- hybrid returns only lexical-universe candidates when scoped.
- `SearchExplanation` includes lexical and semantic provenance.

## DoD

- semantic/hybrid call sites cannot bypass lexical scope planning.
- scoped semantic/hybrid tests fail if lexical scope is applied post hoc.
- explanation proves which lexical plan constrained the semantic path.
- repeated tied hybrid runtime queries keep deterministic ordering.

## Failure modes

- semantic retrieves globally and filters after ranking.
- hybrid fusion runs before lexical universe construction.
- invalid lexical scope degrades into unscoped semantic search.
