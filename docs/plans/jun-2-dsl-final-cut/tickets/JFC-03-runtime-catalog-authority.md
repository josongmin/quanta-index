# JFC-03 Runtime Catalog Authority

> Archive status: `Historical program record`. Current architecture: [JUN-02-001](../../../adr/JUN-02-001-search-dsl-authority-and-runtime-contract.md). Archive map: [Completed Plan Archive](../../ARCHIVE-INDEX.md).


Parent packet: [../README.md](../README.md)

Status: `closed`

## Objective

Hold the landed generation-pinned runtime catalog substrate for `dirty:`, `changed:`, `stale:`, `snapshot:`, `meta.*`, `affected:`, and `invalidated_by:` with direct persisted edge authority.

## Current Source Truth

- the current authority snapshot stores `dirty_docs`, `changed_docs`, `affected_docs`, `invalidated_by_docs`, `doc_facets`, `snapshots`, catalog head timestamps, and a materialization bit inside `RuntimeMetadataState`
- `DirectRuntimeMetadataMaterializer` applies both `DirtyIngestBatch` and `RuntimeCatalogIngestBatch`
- runtime execution now allows `dirty:yes` / `dirty:no` plus `changed:`, `stale:`, `snapshot:`, `meta.*`, `affected:`, and `invalidated_by:` with path/lang/content narrowing; `dirty:only` now typed-rejects `RUNTIME_DIRTY_ONLY_UNSUPPORTED` instead of aliasing `dirty:yes`
- active proof for `stale:`, `snapshot:`, and `meta.*` is now non-vacuous:
  `stale:` uses same-doc bound inversion, while `snapshot:` / `meta.*` use
  shared-token decoy rows so a no-op filter cannot pass
- `dirty:no` executes as the clean complement inside the pinned generation, and `dirty:only` is now an explicit typed reject
- repo-map already has generation-pinned snapshot/query machinery, and runtime catalog ingestion now materializes edge-authority entries for `affected:` / `invalidated_by:` directly into the persisted runtime state
- front-door breadth is now sibling-complete by surface family:
  `sdk_frontdoor` covers `dirty:no`, `affected:`, and `invalidated_by:`;
  `end_to_end` covers `snapshot:` and `meta.*`;
  `e2e_restart_replay_determinism` reopens `changed`, `stale`, `snapshot`, `meta.*`,
  `affected:`, `invalidated_by:`, and `dirty:no`;
  `e2e_perf_chaos` carries typed-reject/no-poison proof across the widened runtime-catalog families

## 핵심 로직

- runtime metadata must become a persisted generation-pinned catalog, not a request-time overlay guess
- query execution must read only from that catalog for runtime-only filters
- missing catalog readiness must typed-fail; it must not silently degrade to empty success
- ownership facets should reuse typed repo-map authority instead of inventing an ad-hoc second graph source
- derivative invalidation filters remain valid only while their backing edge authority is persisted and replayed through the owning runtime catalog path

## 건드릴 파일

- `crates/quanta-index-lq-norm/src/ast.rs`
- `crates/quanta-index-lq-norm/src/parser/implementation.rs`
- `crates/quanta-index-lq-norm/src/normalizer/implementation.rs`
- `crates/quanta-index-search-plane/src/readiness.rs`
- `crates/quanta-index-search-plane/src/ingest_dispatcher.rs`
- `crates/quanta-index-search-plane/src/query_dispatcher.rs`
- `crates/quanta-index-searchd/src/app/runtime.rs`
- `crates/quanta-index-contract/src/ipc/ingest.rs`
- `crates/quanta-index-contract/src/repomap.rs`
- `crates/quanta-index-repomap/src/materializer.rs`
- `crates/quanta-index-repomap/src/model.rs`
- `crates/quanta-index-repomap/src/query.rs`
- `crates/quanta-index-repomap/src/store.rs`
- `crates/quanta-index-searchd-runtime/tests/common/e2e_harness.rs`
- `crates/quanta-index-searchd-runtime/tests/e2e_full_corpus.rs`
- `crates/quanta-index-searchd-runtime/tests/end_to_end.rs`
- runtime catalog fixture files under `crates/quanta-index-searchd-runtime/tests/fixtures/lexical_corpus/`
- `crates/quanta-index-searchd-runtime/tests/fixtures/lexical_corpus/runtime_rows.toml`
- `docs/plans/may-25-lexical-enhancement/dsl-proof-ledger.toml`
- `docs/plans/may-25-lexical-enhancement/lexical-capability-matrix.md`

## 건드리지 말 것

- semantic generation authority
- lexical result-surface semantics already closed
- bridge widening ahead of native runtime-catalog execution
- free-form metrics or ad-hoc JSON side stores outside the authority snapshot path

## TODO

- [x] materialize and query persisted edge-authority entries for `affected:` / `invalidated_by:`
- [x] preserve generation pinning and restart restore behavior for the landed catalog (`reopen_preserves_runtime_catalog_changed_query`)
- [x] preserve typed not-ready / unknown-snapshot / unknown-facet failure modes
- [x] prove widened runtime-only execution does not regress into lexical fallback (`e2e_full_corpus`, `e2e_perf_chaos`)
- [x] make runtime-catalog success rows non-vacuous so `stale:` / `snapshot:` /
  `meta.*` cannot pass with a no-op filter
- [x] add public front-door and sibling-complete replay rails for the shipped runtime-catalog families

## NOT TODO

- no request-time git scans
- no empty-success for missing catalog materialization
- no authority source parallel to repo-map for ownership/invalidation data
- no cross-generation read that ignores the active generation pin

## Test Plan

- owner-local readiness / persistence rails for runtime catalog snapshot restore
- `./scripts/cargow test -p quanta-index-searchd-runtime --test e2e_full_corpus -- --nocapture`
- `./scripts/cargow test -p quanta-index-searchd-runtime --test sdk_frontdoor -- --nocapture`
- `./scripts/cargow test -p quanta-index-searchd-runtime --test end_to_end -- --nocapture`
- `./scripts/cargow test -p quanta-index-searchd-runtime --test e2e_restart_replay_determinism -- --nocapture`
- `./scripts/cargow test -p quanta-index-searchd-runtime --test e2e_perf_chaos -- --nocapture`

## DoD

- landed runtime catalog filters exist in parser/executor only because persisted authority now exists
- restart restores the runtime catalog deterministically across `changed`, `stale`, `snapshot`, `meta.*`, `affected:`, `invalidated_by:`, and `dirty:no`
- unknown or not-ready runtime catalog requests typed-fail
- success-path proof for `stale:` / `snapshot:` / `meta.*` is non-vacuous
- `affected:` / `invalidated_by:` are backed by real persisted edge authority with positive/miss/runtime-parity/chaos coverage
- shipped runtime-catalog families also have public front-door proof instead of only runtime-corpus coverage
- docs no longer describe the landed catalog surfaces as parser rejects

## Failure Modes

- runtime filters silently degrade to lexical/content search
- runtime success rows stay green even if `stale:` / `snapshot:` / `meta.*` stop narrowing
- generation pin is ignored and stale catalog data leaks across revisions
- edge-authority rows stop replaying across restart or silently fall back to lexical/content search
- repo-map and runtime catalog drift into two competing ownership authorities
- `dirty` semantics regress while widening the catalog
