# DH-02 Runtime Metadata Semantics and Pushdown

Parent packet: [../README.md](../README.md)

Status: `landed`

## Objective

Repair runtime metadata semantics so `stale:` consumes real freshness state,
`dirty:` no longer aliases unsupported modes, and runtime-only execution moves
from full-scan membership checks to direct catalog seeds.

## Current Source Truth

- `stale:` now requires `producer_head_applied_at_ms >
  generation_materialized_at_ms` and still applies the user `before=` bound
- `dirty:only` is now an explicit typed reject
  (`RUNTIME_DIRTY_ONLY_UNSUPPORTED`) instead of a silent alias
- runtime-only metadata queries now derive direct catalog seeds for `dirty`,
  `changed`, `stale`, `snapshot`, `meta.*`, `affected`, and `invalidated_by`
- `meta.*` now participates in the same seed-first execution path instead of a
  doc-by-doc full scan

## Current Code Pointers

- route validator and runtime execution:
  `crates/quanta-index-search-plane/src/query_dispatcher.rs`
  `execute_runtime_metadata_query`, `runtime_chunk_matches`,
  `runtime_doc_facet_matches`, `runtime_edge_matches`,
  `parse_runtime_changed_scope_ms`, `parse_runtime_stale_scope_ms`
- runtime state shape:
  `crates/quanta-index-search-plane/src/readiness.rs`
  `RuntimeMetadataState`
- runtime corpus and parity rails:
  `crates/quanta-index-searchd-runtime/tests/fixtures/lexical_corpus/docs-runtime-catalog.toml`
  `crates/quanta-index-searchd-runtime/tests/fixtures/lexical_corpus/runtime_rows.toml`
  `crates/quanta-index-searchd-runtime/tests/e2e_dual_syntax_lowering_parity.rs`
  `crates/quanta-index-searchd-runtime/tests/e2e_perf_chaos.rs`

## 핵심 로직

- semantics first, optimization second; do not optimize a broken contract
- `stale:` must require a real freshness relation:
  `producer_head_applied_at_ms > generation_materialized_at_ms`
- keep the user scope as an additional bound, not as a replacement for freshness
- `dirty:` semantics must be explicit and fail-closed:
  `yes` = dirty-doc seed,
  `no` = clean complement seed,
  `only` = typed reject until a distinct supported contract exists
- runtime-only execution should derive a deterministic seed universe from the
  runtime catalog first, then apply secondary `file:` / `lang:` / `content:`
  narrowing inside that seed

## 건드릴 파일

- `crates/quanta-index-search-plane/src/query_dispatcher.rs`
- `crates/quanta-index-search-plane/src/readiness.rs`
- runtime catalog fixtures:
  `crates/quanta-index-searchd-runtime/tests/fixtures/lexical_corpus/docs-runtime-catalog.toml`
  `crates/quanta-index-searchd-runtime/tests/fixtures/lexical_corpus/runtime_rows.toml`
- proof rails:
  `crates/quanta-index-searchd-runtime/tests/e2e_full_corpus.rs`
  `crates/quanta-index-searchd-runtime/tests/e2e_dual_syntax_lowering_parity.rs`
  `crates/quanta-index-searchd-runtime/tests/e2e_perf_chaos.rs`
- proof/docs after code lands:
  `docs/plans/may-25-lexical-enhancement/dsl-proof-ledger.toml`
  `docs/plans/may-25-lexical-enhancement/lexical-capability-matrix.md`

## 건드리지 말 것

- lexical planner or predicate semantics
- history-route semantics
- bridge-packet carrier handling

## TODO

- [x] freeze canonical `dirty:` semantics in code and docs
- [x] require `producer_head_applied_at_ms > generation_materialized_at_ms`
  before `stale:` can match
- [x] keep the existing user scope as an extra bound on the stale generation
- [x] add fixture siblings for:
  `head == generation`, `head < generation`, `head > generation`,
  dirty docs vs clean docs, facet siblings
- [x] introduce a seed planner for runtime-only queries:
  full generation, dirty set, clean complement, changed set, snapshot set,
  affected set, invalidated-by set
- [x] add a deterministic facet seed path so `meta.*` stops full-scanning
- [x] keep `top_k` application after seed narrowing, not before
- [x] extend chaos/parity rails for the new semantics

## Concrete First Increment

The first PR for this ticket should do only this:

1. add owner-local rails for `stale:` freshness relation and `dirty:` tri-state
2. introduce a seed-planner abstraction without changing every runtime filter at
   once
3. switch one direct-set family first:
   `changed:` / `snapshot:` / `affected:` / `invalidated_by:`

Do not start by optimizing `meta.*`. Land the semantics and the obvious direct
set families first.

## Implementation Steps

1. red: add semantic rails for `stale:` and `dirty:` plus fixture siblings that
   make no-op behavior impossible
2. repair: introduce a `RuntimeSeed` abstraction and move direct-set families to
   seed-first execution
3. repair: implement the real `dirty:` tri-state and the real `stale:`
   freshness relation
4. repair: add a direct facet-seed path for `meta.*`
5. proof: rerun parity and chaos rails with positive/miss sibling cases for the
   changed semantics

## Dependency / Import Constraints

- do not reintroduce lexical fallback on runtime-only queries
- no request-time git or repo-map lookups in the query path
- if a direct seed is already owned by state, do not keep the full scan path as
  the steady-state implementation

## Red Rails First

- query-dispatch owner-local rails:
  add the targeted semantic rail first, then run
  `./scripts/cargow test -p quanta-index-search-plane --lib -- --nocapture`
- runtime corpus:
  `./scripts/cargow test -p quanta-index-searchd-runtime --test e2e_full_corpus -- --nocapture`
- parity and chaos:
  `./scripts/cargow test -p quanta-index-searchd-runtime --test e2e_dual_syntax_lowering_parity --test e2e_perf_chaos -- --nocapture`

## NOT TODO

- no silent aliasing of `dirty:only`
- no “head exists” shortcut for `stale:`
- no scan-first implementation once a deterministic seed already exists

## Test Plan

- owner-local query-dispatch tests for seed semantics
- runtime corpus positive/miss rows for new `dirty:` and `stale:` siblings
- parity rails for runtime metadata filters
- chaos rails proving typed rejects and recovery remain stable after the new
  semantics

## DoD

- `stale:` consumes a real producer-head freshness relation
- `dirty:yes` and `dirty:no` execute with direct proof, and `dirty:only`
  typed-fails explicitly instead of silently aliasing another mode
- direct catalog-set families execute from seed-first planning instead of
  structural full scan
- `meta.*` has a deterministic direct seed path or an explicit scoped residual
  if not yet landed
- parity and chaos rails reflect the repaired semantics

## Failure Modes

- `stale:` still behaves like a renamed time bound
- `dirty:only` regresses from explicit typed reject back into silent aliasing
- runtime-only queries keep scaling with the whole chunk universe despite direct
  catalog seeds being available
- new semantics land without positive/miss sibling proof
