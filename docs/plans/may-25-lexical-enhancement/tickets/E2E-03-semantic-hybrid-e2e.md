# E2E-03 - Semantic/Hybrid E2E

Status: `proposed`
Priority: `P0`
Depends on: [E2E-00](E2E-00-live-dsl-matrix-harness.md), [LXE-07](LXE-07-semantic-hybrid-planner-provenance.md)

## Purpose

Prove semantic and hybrid search respect lexical scope as an execution-time
candidate universe.

## Owner files

- new `crates/quanta-index-searchd-runtime/tests/e2e_semantic_hybrid.rs`
- `crates/quanta-index-search-plane/src/query_dispatcher.rs`
- `crates/quanta-index-core/src/domains/semantic/**`
- `crates/quanta-index-core/src/domains/hybrid/**`
- `crates/quanta-index-contract/src/query/requests.rs`
- `crates/quanta-index-contract/src/results/**`

## File-level work breakdown

- `crates/quanta-index-searchd-runtime/tests/e2e_semantic_hybrid.rs`: add
  scoped/unscoped semantic and hybrid runtime rows with exact candidate IDs.
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

- deterministic fake embedding provider for tests, or a stable local embedding
  fixture if the repo already has one.
- exact candidate ID assertions for scoped and unscoped runs.
- negative test proving scope is not post-filtered after semantic ranking.
- restart variant may be delegated to `E2E-05`.

## DoD

- semantic/hybrid tests fail if lexical scope is ignored.
- semantic/hybrid tests fail if lexical scope is applied after global semantic
  ranking.
- all requests use `TextQueryRequest` for lexical scope.
- `SearchExplanation` proves both lexical and semantic runtime paths.

## Failure modes

- using Tantivy lexical storage as a semantic proof.
- using manually injected semantic candidates.
- accepting invalid lexical scope as unscoped semantic search.
