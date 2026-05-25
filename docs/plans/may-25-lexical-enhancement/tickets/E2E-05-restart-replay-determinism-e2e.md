# E2E-05 - Restart/Replay Determinism E2E

Status: `proposed`
Priority: `P1`
Depends on: [E2E-00](E2E-00-live-dsl-matrix-harness.md), [LXE-10](LXE-10-observability-and-bridge-sink.md)

## Purpose

Prove real indexes and explanations are stable after runtime reopen and replay.

## Owner files

- new `crates/quanta-index-searchd-runtime/tests/e2e_restart_replay_determinism.rs`
- `crates/quanta-index-searchd-runtime/tests/common/e2e_harness.rs`
- `crates/quanta-index-search-plane/src/query_dispatcher.rs`
- `crates/quanta-index-lexical/src/**`
- `crates/quanta-index-contract/src/results/**`

## File-level work breakdown

- `crates/quanta-index-searchd-runtime/tests/e2e_restart_replay_determinism.rs`:
  add reopen, replay, tied-merge, and cancellation-cleanup scenarios.
- `crates/quanta-index-searchd-runtime/tests/common/e2e_harness.rs`: add helper
  support for reopen and replay over persisted storage.
- `crates/quanta-index-search-plane/src/query_dispatcher.rs`: expose stable
  explanation and early-stop information for replay assertions.
- `crates/quanta-index-lexical/src/**`: keep deterministic merge and engine
  state cleanup behavior stable across reopen/replay.

## Required scenarios

- build generation, query, capture result IDs and explanation.
- drop runtime, reopen same storage root, query again.
- assert identical ordered IDs, candidate kinds, and stable explanation fields.
- replay ingest log into a new tempdir and assert equivalent query output.
- run deterministic merge with tied scores across multiple engines.
- cancellation checkpoint test returns typed early stop without corrupting
  subsequent query results.

## Test plan

- exact ID ordering assertions.
- stable planner trace hash assertions.
- replay artifact comparison.
- cancellation state cleanup assertion.

## DoD

- reopen and replay tests use persisted files, not the same process memory.
- deterministic merge is proved across at least two engines.
- `SearchExplanation` remains stable except for explicitly volatile timing
  fields.

## Failure modes

- comparing unordered sets only.
- including wall-clock timing in stable explanation equality.
- replaying through a different path than production ingest.
