# E2E-03 - Semantic/Hybrid E2E

> Archive status: `Historical execution record`. Current architecture: [JUN-02-001](../../../adr/JUN-02-001-search-dsl-authority-and-runtime-contract.md) and [JUN-06-001](../../../adr/JUN-06-001-sourcegraph-compatibility-boundary.md). Live capability truth: [Lexical Capability Matrix](../lexical-capability-matrix.md). Archive map: [Completed Plan Archive](../../ARCHIVE-INDEX.md).


Status: `completed`
Priority: `P0`
Depends on: [E2E-00](E2E-00-live-dsl-matrix-harness.md), [LXE-07](LXE-07-semantic-hybrid-planner-provenance.md)

## Purpose

Prove semantic and hybrid search respect lexical scope as an execution-time
candidate universe.

## Current live truth (2026-05-27)

No dedicated `e2e_semantic_hybrid.rs` file was required on the current tree.
The live owner proof is split across existing rails:

- `crates/quanta-index-searchd-runtime/tests/end_to_end.rs`
- `crates/quanta-index-searchd-runtime/tests/dsl_scenarios.rs`
- `crates/quanta-index-searchd-runtime/tests/sdk_frontdoor.rs`

Current runtime proof covers:

- unscoped semantic returning the global nearest hit
- scoped semantic exclusion of global nearest outsiders
- scoped semantic intersection-only result sets
- invalid lexical scope failing typed before semantic execution
- hybrid lexical-universe-first fusion
- explanation provenance (`planner_trace`, `engines_touched`, `strategy`,
  `summary`)
- truthful `CountReached` reporting on bounded hybrid execution
- repeated tied hybrid queries keeping stable ordering

## Owner files

- `crates/quanta-index-searchd-runtime/tests/end_to_end.rs`
- `crates/quanta-index-searchd-runtime/tests/dsl_scenarios.rs`
- `crates/quanta-index-searchd-runtime/tests/sdk_frontdoor.rs`
- `crates/quanta-index-search-plane/src/query_dispatcher.rs`
- `crates/quanta-index-core/src/domains/semantic/**`
- `crates/quanta-index-core/src/domains/hybrid/**`
- `crates/quanta-index-contract/src/query/requests.rs`
- `crates/quanta-index-contract/src/results/**`

## File-level work breakdown

- `crates/quanta-index-searchd-runtime/tests/end_to_end.rs`: scoped/unscoped
  semantic and hybrid runtime rows with exact candidate IDs, repeated tied
  hybrid ordering, and fail-closed scope behavior.
- `crates/quanta-index-searchd-runtime/tests/dsl_scenarios.rs`: complex-scope
  explanation accounting and planner-trace assertions.
- `crates/quanta-index-searchd-runtime/tests/sdk_frontdoor.rs`: public SDK
  happy-path proof for semantic/hybrid frontdoors on the same stack.
- `crates/quanta-index-search-plane/src/query_dispatcher.rs`: expose the
  execution boundary that materializes lexical scope before semantic/hybrid.
- `crates/quanta-index-core/src/domains/{semantic,hybrid}/**`: provide hooks or
  fixtures needed to assert lexical-universe-aware execution.
- `crates/quanta-index-contract/src/{query/requests.rs,results/**}`: keep
  request and explanation carriers aligned with the E2E assertions.

## Required scenarios

- unscoped semantic query returns global nearest candidates.
- repo-scoped semantic query excludes semantically similar candidates outside
  the repo.
- file-scoped semantic query excludes semantically similar candidates outside
  the path filter.
- invalid lexical scope fails typed and does not run semantic executor.
- hybrid query builds lexical universe first, then fuses semantic scores.
- hybrid tie ordering is deterministic.
- explanation includes lexical scope count, semantic count, fusion strategy,
  and engines touched.

## Test plan

- exact candidate ID assertions for scoped and unscoped runs.
- negative test proving scope is not post-filtered after semantic ranking.
- restart variant may be delegated to `E2E-05`.
- live rail:
  - `./scripts/cargow --lane test-daemon-lane nextest run -p quanta-index-searchd-runtime --test runtime_fast_suite --all-features --locked -E 'test(/end_to_end::(default_indexed_queries_share_one_fixture|semantic_query_with_lexical_scope_returns_intersection_only|semantic_scoped_query_ignores_out_of_scope_global_nearest_hit|hybrid_query_admits_a_semantic_only_relevant_hit_beside_the_lexical_hits|hybrid_query_repeated_tied_scope_query_keeps_stable_order)/)'`
  - `./scripts/cargow --lane test-daemon-lane nextest run -p quanta-index-searchd-runtime --test runtime_risk_suite --all-features --locked -E 'test(/dsl_scenarios::(semantic_scoped_query_with_complex_scope_excludes_outsiders_and_explains_scope|hybrid_query_reports_complex_scope_explanation_accounting)/)'`

## DoD

Status: satisfied on the current tree.

- semantic/hybrid tests fail if lexical scope is ignored.
- semantic/hybrid tests fail if lexical scope is applied after global semantic
  ranking.
- all requests use `TextQueryRequest` for lexical scope.
- `SearchExplanation` proves both lexical and semantic runtime paths.
- repeated tied hybrid queries keep deterministic ordering.

## Failure modes

- using Tantivy lexical storage as a semantic proof.
- using manually injected semantic candidates.
- accepting invalid lexical scope as unscoped semantic search.
