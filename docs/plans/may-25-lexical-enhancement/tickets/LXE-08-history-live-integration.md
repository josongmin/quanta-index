# LXE-08 - History Live Integration

Status: `proposed`
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
- new `crates/quanta-index-searchd-runtime/tests/e2e_history_structural.rs`

## File-level work breakdown

- `crates/quanta-index-contract/src/results/**`: keep `CommitCandidate` and
  `DiffCandidate` as active response carriers.
- `crates/quanta-index-contract/src/query/requests.rs`: ensure history request
  variants are part of the active IPC surface.
- `crates/quanta-index-lq-history/src/**`: expose search primitives over commit
  and diff shards when producer data exists.
- `crates/quanta-index-search-plane/src/query_dispatcher.rs`: return typed
  unavailable/not-ready codes when history data or readiness is missing.
- `crates/quanta-index-searchd-runtime/tests/e2e_history_structural.rs`: prove
  fail-closed behavior and any real positive path backed by indexed fixtures.

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
- If in-repo history fixture ingestion exists, add a minimal live positive path.
- If producer ops are absent, keep positive runtime path blocked and prove typed
  unavailable.

## Test plan

- contract round-trip tests for commit/diff carriers.
- unit tests for history planner routing.
- typed unavailable tests when no history shard exists.
- if fixture ingestion exists: unit test commit/diff search over fixture data.

## E2E plan

Covered by `E2E-04`:

- `type:commit` returns typed unavailable without history producer data.
- `type:diff` returns typed unavailable without history producer data.
- if fixture data is available, commit/diff queries return carrier-specific
  candidates and explanation lists history engine touched.

## DoD

- no history request returns empty success when data is absent.
- commit/diff carriers are part of active `results::*`.
- producer absence is a typed runtime status, not a log-only condition.

## Failure modes

- treating history as lexical content search over commit text.
- marking history done because the contract type exists.
- returning generic internal error instead of typed not-ready/unavailable.
