# LXE-08 - History Live Integration

Status: `completed`
Priority: `P1`
Depends on: [LXE-01](LXE-01-active-contract-and-dead-route-cleanup.md), [LXE-06](LXE-06-symbol-select-type-execution.md)

## Purpose

Wire commit and diff result carriers into the active IPC/search surface while
remaining fail-closed when producer history data is absent.

## Owner files

- `crates/quanta-index-contract/src/results/**`
- `crates/quanta-index-contract/src/query/requests.rs`
- `crates/quanta-index-lq-history/src/**`
- `crates/quanta-index-search-plane/src/query_dispatcher.rs`
- `crates/quanta-index-core/src/domains/lexical/**`
- `crates/quanta-index-searchd-runtime/tests/sdk_frontdoor.rs`
- `crates/quanta-index-searchd-runtime/tests/end_to_end.rs`

## File-level work breakdown

- `crates/quanta-index-contract/src/results/**`: keep `CommitCandidate` and
  `DiffCandidate` as active response carriers.
- `crates/quanta-index-contract/src/query/requests.rs`: ensure history request
  variants are part of the active IPC surface.
- `crates/quanta-index-lq-history/src/**`: expose search primitives over commit
  and diff shards when producer data exists.
- `crates/quanta-index-search-plane/src/query_dispatcher.rs`: return typed
  unavailable/not-ready codes when history data or readiness is missing.
- `crates/quanta-index-searchd-runtime/tests/sdk_frontdoor.rs`: prove the live
  positive commit/diff path against indexed fixture data.
- `crates/quanta-index-searchd-runtime/tests/end_to_end.rs`: own typed
  generation-not-ready / producer-unavailable / shard-unavailable history rows
  against the raw runtime harness.

## Work items

- Add or verify active response carriers:
  - `CommitCandidate`
  - `DiffCandidate`
- Add IPC request/response variants for history surfaces.
- Wire `type:commit` and `type:diff` planner routes to history domain ports.
- Define typed unavailable/not-ready codes:
  - `HISTORY_PRODUCER_UNAVAILABLE`
  - `HISTORY_GENERATION_NOT_READY`
  - `HISTORY_SHARD_UNAVAILABLE`
- Keep the active live path truthful: `type:commit` and `type:diff` must query
  indexed history authority, not lexical fallback.
- Preserve typed fail-closed behavior when history authority is absent or not
  ready; do not silently downgrade into content search.

## Test plan

- contract round-trip tests for commit/diff carriers.
- unit tests for history planner routing.
- typed unavailable tests when no history authority exists or a required shard
  is not materialized.
- runtime positive proof in `sdk_frontdoor.rs` for `type:commit` and
  `type:diff`.
- any absent-authority history rail must be proven in runtime tests, not only
  in lexical/planner unit tests.

## E2E plan

Covered by `E2E-04`:

- active `type:commit` returns a `CommitCandidate` on indexed fixture data.
- active `type:diff` returns a `DiffCandidate` on indexed fixture data.
- history absent-authority/not-ready/shard-unavailable runtime rows are proven
  in `tests/end_to_end.rs` and surfaced through the public SDK front door in
  `tests/sdk_frontdoor.rs`.
- deeper history-authority corruption is still unclaimed; current runtime truth
  proves shard-materialization absence, not an additional integrity taxonomy.

## DoD

- active commit/diff queries return carrier-specific results on indexed fixture
  data.
- commit/diff carriers are part of active `results::*`.
- producer absence, when asserted, is a typed runtime status, not a log-only
  condition.
- public SDK and raw IPC runtime rails both prove:
  - `HISTORY_GENERATION_NOT_READY`
  - `HISTORY_PRODUCER_UNAVAILABLE`
  - `HISTORY_SHARD_UNAVAILABLE`

## Failure modes

- treating history as lexical content search over commit text.
- marking history done because only the active positive path exists while the
  absent-authority runtime matrix is still unproven.
- claiming a deeper corruption-specific history error when the runtime only
  exposes shard materialization absence.
- returning generic internal error instead of typed not-ready/unavailable.
