# E2E-04 - History/Structural E2E

Status: `proposed`
Priority: `P1`
Depends on: [E2E-00](E2E-00-live-dsl-matrix-harness.md), [LXE-08](LXE-08-history-live-integration.md), [LXE-09](LXE-09-structural-live-integration.md)

## Purpose

Prove history and structural surfaces are wired and fail-closed when external
producer data is absent.

## Owner files

- new `crates/quanta-index-searchd-runtime/tests/e2e_history_structural.rs`
- `crates/quanta-index-search-plane/src/query_dispatcher.rs`
- `crates/quanta-index-lq-history/src/**`
- `crates/quanta-index-lq-structural/src/**`
- `crates/quanta-index-contract/src/results/**`

## File-level work breakdown

- `crates/quanta-index-searchd-runtime/tests/e2e_history_structural.rs`: add
  fail-closed and positive-fixture rows for commit, diff, and structural
  surfaces.
- `crates/quanta-index-search-plane/src/query_dispatcher.rs`: expose typed
  unavailable and readiness states asserted by the test.
- `crates/quanta-index-lq-history/src/**` and
  `crates/quanta-index-lq-structural/src/**`: provide real fixture-backed
  execution where producer data exists.
- `crates/quanta-index-contract/src/results/**`: preserve candidate kind and
  typed failure shape needed by the assertions.

## Required scenarios

- `type:commit` without history producer data returns
  `HISTORY_PRODUCER_UNAVAILABLE` or the more specific configured typed code.
- `type:diff` without history producer data returns typed unavailable.
- structural query without parse-tree data returns
  `STR_PRODUCER_PARSE_TREE_UNAVAILABLE`.
- content-only data cannot produce structural candidates.
- if in-repo fixtures exist:
  - commit query returns `CommitCandidate`
  - diff query returns `DiffCandidate`
  - structural query returns `StructuralCandidate` only when parse-tree data is
    present and generation-ready

## Test plan

- fail-closed rows run by default.
- positive rows are gated behind actual fixture/shard readiness, not mocked
  result candidates.
- explanation asserts early stop reason and unavailable reason.

## DoD

- absence of producer data is typed and deterministic.
- no history/structural row returns empty success for missing data.
- structural Option B is enforced in E2E.
- positive history/structural claims are absent unless fixture data is truly
  indexed and queried.

## Failure modes

- claiming endpoint wiring as live producer support.
- using content search fallback for structural.
- hiding producer absence behind generic internal errors.
