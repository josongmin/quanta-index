# SGP-02 Repo Meta And Topic Predicates

Parent RFC: [../rfc.md](../rfc.md)

Status: `landed`

## Objective

Implement:

- `repo:has.meta(...)`
- `repo:has.topic(...)`

with explicit repo authority and exact SG proof.

## Current Source Truth

- current `meta.*` surface is runtime catalog document metadata, not repo metadata authority
  - it must not be reused as if it were `repo:has.meta(...)`
- `repo:has.meta(key:value)` is now executable on the current tree
- current analysis doc distinguishes supported `repo:has.meta(key:value)` / `repo:has.topic(...)` from unsupported key-only cells
- `has.meta` and `has.topic` must not be treated as the same authority without proof
- `repo:has.topic(...)` now has a distinct repo-topic authority batch, runtime gate, and exact proof rail on the current tree

## Files To Touch

- `crates/quanta-index-contract/src/ipc/ingest.rs`
- `crates/quanta-index-core/src/domains/lexical/outbound.rs`
- `crates/quanta-index-lexical/src/predicate_registry.rs`
- `crates/quanta-index-lexical/src/planner.rs`
- `crates/quanta-index-lexical/src/lib.rs`
- `crates/quanta-index-sdk/src/history.rs`
- `crates/quanta-index-search-plane/src/ingest_dispatcher.rs`
- `crates/quanta-index-searchd/src/app/runtime.rs`
- `crates/quanta-index-searchd-runtime/src/lib.rs`
- `crates/quanta-index-searchd-runtime/tests/fixtures/lexical_corpus/runtime_rows.toml`
- `crates/quanta-index-searchd-runtime/tests/e2e_filter_execution.rs`
- `crates/quanta-index-searchd-runtime/tests/e2e_dual_syntax_lowering_parity.rs`
- `crates/quanta-index-searchd-runtime/tests/sdk_frontdoor.rs`
- `crates/quanta-index-searchd-runtime/tests/e2e_full_corpus.rs`

## Concrete First Increment

Started with `repo:has.meta(key:value)` only, then added the distinct repo-topic authority and landed that cell too.

Do not mix these shapes into the same first PR:

- key-only `repo:has.meta(key)`
- tag/null-value `repo:has.meta(tag:)`

## Implemented Verdict

1. `repo:has.meta(key:value)` is supported
2. `repo:has.topic(...)` is supported
3. key-only `repo:has.meta(key)` stays typed-fail

## Proof

- exact runtime row plus parity row for `repo:has.meta(key:value)` are green
- exact runtime row plus parity row for `repo:has.topic(...)` are green
- SDK/front-door exact row is green
- corpus fixture ingests repo metadata and repo topic authorities and exact row is green
- key-only `repo:has.meta(key)` has typed-fail parity and runtime coverage

## DoD

- repo metadata no-op behavior cannot pass the oracle
- repo topic is backed by a distinct authority batch and cannot pass by reusing generic metadata

## Final Residue

- runtime `meta.*` document catalog fields must still not be reused as repo metadata
- key-only `repo:has.meta(key)` remains typed-fail
