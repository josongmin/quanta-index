# LXE-07 - Semantic/Hybrid Planner Provenance

Status: `proposed`
Priority: `P0`
Depends on: [LXE-02](LXE-02-planner-authority-ir.md), [LXE-03](LXE-03-lexical-filter-execution.md)

## Purpose

Make semantic and hybrid search consume lexical scope as a materialized,
planner-proven candidate universe. The lexical side must not be a best-effort
filter applied after semantic retrieval.

## Owner files

- `crates/quanta-index-search-plane/src/query_dispatcher.rs`
- `crates/quanta-index-search-plane/src/lowering.rs`
- `crates/quanta-index-core/src/domains/semantic/**`
- `crates/quanta-index-core/src/domains/hybrid/**`
- `crates/quanta-index-core/src/domains/lexical/**`
- `crates/quanta-index-contract/src/query/requests.rs`
- `crates/quanta-index-contract/src/results/**`
- new `crates/quanta-index-searchd-runtime/tests/e2e_semantic_hybrid.rs`

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
- `crates/quanta-index-searchd-runtime/tests/e2e_semantic_hybrid.rs`: prove
  scoped versus unscoped behavior with exact candidate IDs.

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

## Failure modes

- semantic retrieves globally and filters after ranking.
- hybrid fusion runs before lexical universe construction.
- invalid lexical scope degrades into unscoped semantic search.
