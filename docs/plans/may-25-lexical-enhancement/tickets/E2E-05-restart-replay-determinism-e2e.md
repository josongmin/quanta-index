# E2E-05 - Restart/Replay Determinism E2E

Status: `completed`
Priority: `P1`
Depends on: [E2E-00](E2E-00-live-dsl-matrix-harness.md), [LXE-10](LXE-10-observability-and-bridge-sink.md)

## Purpose

Prove real indexes and explanations are stable after runtime reopen and replay.

## Current live truth (2026-05-27)

- `crates/quanta-index-searchd-runtime/tests/e2e_restart_replay_determinism.rs`
  is live and green.
- same-root reopen now bootstraps lexical readiness from persisted lexical
  generation directories during `searchd` runtime assembly, so reopen no
  longer drops back to `NOT_READY` after a successful seal/activate/query cycle.
- same-root reopen also rewinds the semantic channel cursor during runtime
  assembly so the reference in-memory semantic adapter replays its WAL on
  restart instead of resuming from an acked cursor with an empty vector state.
- the live rail proves:
  - identical ordered lexical result IDs across reopen
  - identical `SearchExplanation` equality across reopen
  - identical ordered semantic scoped result IDs and explanation equality
    across reopen
  - equivalent ordered lexical result IDs and explanation equality across a
    fresh replay into a new state root
  - equivalent ordered hybrid result IDs across a fresh dual-track replay into
    a new state root
  - stable hybrid `strategy`, `engines_touched`, and `CountReached` truth
    across that fresh dual-track replay
- multi-engine tie/cancellation behavior is intentionally left to `E2E-07`;
  this ticket now owns persisted lexical/semantic reopen determinism plus fresh
  replay determinism.

## Owner files

- new `crates/quanta-index-searchd-runtime/tests/e2e_restart_replay_determinism.rs`
- `crates/quanta-index-searchd-runtime/tests/common/e2e_harness.rs`
- `crates/quanta-index-search-plane/src/query_dispatcher.rs`
- `crates/quanta-index-searchd-runtime/src/lib.rs`
- `crates/quanta-index-contract/src/results/**`

## File-level work breakdown

- `crates/quanta-index-searchd-runtime/tests/e2e_restart_replay_determinism.rs`:
  add reopen and replay determinism scenarios over persisted state.
- `crates/quanta-index-searchd-runtime/tests/common/e2e_harness.rs`: add helper
  support for reopen and replay over persisted storage.
- `crates/quanta-index-search-plane/src/query_dispatcher.rs`: expose stable
  explanation and early-stop information for replay assertions.
- `crates/quanta-index-searchd-runtime/src/lib.rs`: bootstrap persisted lexical
  reopen state and rewind semantic replay so same-root restart rebuilds the
  in-memory semantic adapter before queries resume.

## Required scenarios

- build generation, query, capture result IDs and explanation.
- drop runtime, reopen same storage root, query again.
- assert identical ordered IDs and stable explanation equality.
- replay the same fixture ingest into a new tempdir and assert equivalent query
  output plus explanation equality.

## Test plan

- exact ID ordering assertions.
- full `SearchExplanation` equality across lexical reopen, semantic scoped
  reopen, and lexical fresh replay.
- hybrid fresh replay keeps ordered IDs plus high-level explanation truth, but
  does not currently claim byte-for-byte explanation equality.
- same-root hybrid reopen is not separately claimed on the current tree;
  dual-track determinism is asserted through fresh replay.
- persisted-state reopen through the real `searchd` runtime path rather than
  same-process memory reuse.

## DoD

- reopen and replay tests use persisted files, not the same process memory.
- `SearchExplanation` remains stable across lexical reopen, semantic scoped
  reopen, and lexical fresh replay.
- owner rail is green on:
  - `cargo test -p quanta-index-searchd-runtime --test e2e_restart_replay_determinism -- --nocapture`
  - `cargo test -p quanta-index-searchd-runtime -- --nocapture`

## Failure modes

- comparing unordered sets only.
- including wall-clock timing in stable explanation equality.
- replaying through a different path than production ingest.
