# E2E-04 - History/Structural E2E

Status: `partial-implemented`
Priority: `P1`
Depends on: [E2E-00](E2E-00-live-dsl-matrix-harness.md), [LXE-08](LXE-08-history-live-integration.md), [LXE-09](LXE-09-structural-live-integration.md)

## Purpose

Prove history and structural surfaces against the real runtime harness:
history positive paths on indexed authority, and structural positive plus typed
fail-closed paths on materialized parse-tree/chunk authority.

## Owner files

- `crates/quanta-index-searchd-runtime/tests/sdk_frontdoor.rs`
- `crates/quanta-index-searchd-runtime/tests/end_to_end.rs`
- `crates/quanta-index-search-plane/src/query_dispatcher.rs`
- `crates/quanta-index-searchd/src/app/runtime.rs`
- `crates/quanta-index-lq-history/src/**`
- `crates/quanta-index-lq-structural/src/**`
- `crates/quanta-index-contract-base/src/results/**`

## File-level work breakdown

- `crates/quanta-index-searchd-runtime/tests/sdk_frontdoor.rs`: own the public
  active/pinned positive rows for commit, diff, and structural surfaces plus
  structural typed errors visible through the SDK front door.
- `crates/quanta-index-searchd-runtime/tests/end_to_end.rs`: own structural
  typed not-ready/shard-unavailable rows and any future history
  absent-authority runtime proof.
- `crates/quanta-index-search-plane/src/query_dispatcher.rs`: expose typed
  structural invalid-request boundaries and the runtime routes asserted by the
  tests.
- `crates/quanta-index-searchd/src/app/runtime.rs`: execute the structural
  truthful subset against ledger authority and surface typed readiness/shard
  failures.
- `crates/quanta-index-lq-history/src/**` and
  `crates/quanta-index-lq-structural/src/**`: provide real fixture-backed
  execution where producer data exists.
- `crates/quanta-index-contract-base/src/results/**`: preserve candidate kind
  and typed failure shape needed by the assertions.

## Required scenarios

- active `type:commit` query returns `CommitCandidate` on indexed fixture data.
- active `type:diff` query returns `DiffCandidate` on indexed fixture data.
- active and pinned structural queries return `StructuralCandidate` only when
  parse-tree/chunk authority is present and generation-ready.
- structural `repo:` and `file:` filters execute on the same live authority
  path and are proven with positive rows.
- structural `repo:` mismatch returns empty success instead of silently
  dropping the filter.
- structural unsupported-lang query returns `STR_LANG_NOT_SUPPORTED`.
- structural unsupported-filter/composition query returns
  `STR_INVALID_REQUEST`.
- structural query without materialized authority returns
  `STR_GENERATION_NOT_READY`.
- orphaned parse-tree/chunk authority returns `STR_SHARD_UNAVAILABLE`.
- history absent-authority/not-ready runtime rows remain open until a real
  harness assertion lands.

## Test plan

- current green rows run by default in `sdk_frontdoor.rs` and
  `end_to_end.rs`.
- positive rows are backed by actual indexed fixture/shard readiness, not
  mocked result candidates.
- any new history-absence row must use the real runtime harness, not planner or
  lexical-only tests.

## DoD

- positive history and structural claims are backed by indexed runtime data.
- structural typed negative matrix is proven end-to-end.
- no structural row returns empty success for missing parse-tree/chunk
  authority.
- history absent-authority/not-ready behavior stays explicitly open until it is
  proven by the same runtime class of test.

## Failure modes

- claiming endpoint wiring as live producer support.
- using content search fallback for structural.
- accepting structural `repo:` / `file:` filters without proving that the live
  runtime actually applies them before matching.
- treating the current structural truthful subset as proof of full structural
  semantics.
- treating history positive rows as proof that history absent-authority runtime
  behavior is already covered.
- hiding producer absence behind generic internal errors.
