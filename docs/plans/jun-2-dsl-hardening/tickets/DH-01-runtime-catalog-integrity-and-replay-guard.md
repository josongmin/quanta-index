# DH-01 Runtime Catalog Integrity and Replay Guard

Parent packet: [../README.md](../README.md)

Status: `landed`

## Objective

Turn runtime catalog ingest into an authoritative, replay-safe, referentially
validated snapshot path.

## Current Source Truth

- `apply_runtime_catalog_batch` now persists `overlay_epoch_ms` and
  `batch_digest` inside `RuntimeMetadataState`
- runtime catalog apply now replaces the incoming catalog-owned keyspaces
  instead of merging them additively
- runtime catalog ingest now rejects malformed unknown `doc_id` values before
  state mutation, and stale/conflicting replays typed-fail

## Current Code Pointers

- contract:
  `crates/quanta-index-contract/src/ipc/ingest.rs`
  `RuntimeCatalogIngestBatch`
- state/apply path:
  `crates/quanta-index-search-plane/src/readiness.rs`
  `apply_runtime_catalog_batch`, `RuntimeMetadataState`
- publish path:
  `crates/quanta-index-search-plane/src/ingest_dispatcher.rs`
  `publish_catalog_batch`
- restore rails:
  `crates/quanta-index-search-plane/src/readiness.rs`
  owner-local restore tests
  `crates/quanta-index-searchd-runtime/tests/e2e_restart_replay_determinism.rs`

## 핵심 로직

- one runtime catalog batch is an authoritative snapshot for one
  `{repo, revision, generation}`
- acceptance requires three checks before state mutation:
  monotonic epoch ordering, digest compatibility, and referential validity
- apply semantics must be replace-not-merge for the catalog-owned keyspaces
- malformed or stale batches must typed-fail as a whole; partial acceptance is
  not allowed

## 건드릴 파일

- `crates/quanta-index-search-plane/src/readiness.rs`
- `crates/quanta-index-search-plane/src/ingest_dispatcher.rs`
- `crates/quanta-index-contract/src/ipc/ingest.rs`
- `crates/quanta-index-searchd-runtime/tests/e2e_restart_replay_determinism.rs`
- runtime catalog fixture helpers if needed:
  `crates/quanta-index-searchd-runtime/tests/common/e2e_harness.rs`
- proof/docs after code lands:
  `docs/plans/may-25-lexical-enhancement/dsl-proof-ledger.toml`
  `docs/plans/may-25-lexical-enhancement/lexical-capability-matrix.md`

## 건드리지 말 것

- DSL syntax
- lexical predicate execution
- bridge or structural-route semantics
- request-time repair paths for malformed producer input

## TODO

- [x] persist `overlay_epoch_ms` and `batch_digest` in `RuntimeMetadataState`
- [x] reject older epoch batches
- [x] make same-epoch replay idempotent only when the digest matches
- [x] reject same-epoch conflicting digest replays
- [x] replace `changed_docs`, `doc_facets`, `snapshots`, `affected_docs`,
  `invalidated_by_docs` atomically per batch
- [x] validate every catalog `doc_id` against the pinned generation chunk
  universe before commit
- [x] add owner-local rails for removal, stale replay rejection, conflicting
  replay rejection, and malformed unknown-`doc_id` rejection
- [x] keep restart restore deterministic with the new persisted ordering fields

## Concrete First Increment

The first PR for this ticket should do only this:

1. add the state fields and owner-local red tests for replay/replacement
2. land authoritative replacement semantics in `apply_runtime_catalog_batch`
3. persist the new state through restore/reopen before touching runtime corpus

Do not start with e2e fixture churn. Fix the state machine first.

## Implementation Steps

1. red: add owner-local unit rails for:
   newer batch removes old keys, older batch rejects, conflicting same-epoch
   digest rejects, malformed unknown-`doc_id` rejects
2. repair: extend `RuntimeMetadataState` with persisted ordering fields and
   introduce a pre-commit validation step
3. repair: make apply semantics replace catalog-owned keyspaces atomically
4. proof: extend restart/replay rails so replay ordering survives persistence
5. docs: update proof inventory wording once the state machine is real

## Dependency / Import Constraints

- no new side-store or JSON cache for catalog state
- no producer fallback path that repairs malformed batches locally
- no per-filter ad-hoc validation; referential validation belongs to catalog
  ingest

## Red Rails First

- owner-local readiness tests:
  add the targeted replay/replacement rail first, then run
  `./scripts/cargow test -p quanta-index-search-plane --lib -- --nocapture`
- restart/replay companion rail:
  `./scripts/cargow test -p quanta-index-searchd-runtime --test e2e_restart_replay_determinism -- --nocapture`

## NOT TODO

- no additive merge semantics with “best effort” cleanup
- no silent omission for unknown `doc_id`
- no empty digest placeholder receipts

## Test Plan

- owner-local unit rails in `readiness.rs`
- restart/replay integration rail in `e2e_restart_replay_determinism`
- targeted runtime fixture that proves old snapshot/facet/edge keys disappear
  after replacement

## DoD

- runtime catalog state persists and restores epoch/digest ordering evidence
- older or conflicting replays typed-fail
- authoritative replacement removes keys that disappear from the new batch
- malformed unknown-`doc_id` batches typed-fail before state mutation
- downstream runtime queries can rely on catalog integrity rather than
  compensating for malformed state

## Failure Modes

- old snapshot/facet/edge keys survive replacement
- stale replay mutates live state
- malformed producer input is accepted and silently disappears at query time
- restart loses ordering evidence and reintroduces replay ambiguity
