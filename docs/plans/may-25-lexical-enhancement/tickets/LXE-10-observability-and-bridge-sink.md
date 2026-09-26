# LXE-10 - Observability and Bridge Sink

> Archive status: `Historical execution record`. Current architecture: [JUN-02-001](../../../adr/JUN-02-001-search-dsl-authority-and-runtime-contract.md) and [JUN-06-001](../../../adr/JUN-06-001-sourcegraph-compatibility-boundary.md). Live capability truth: [Lexical Capability Matrix](../lexical-capability-matrix.md). Archive map: [Completed Plan Archive](../../ARCHIVE-INDEX.md).


Status: `completed`
Priority: `P1`
Depends on: [LXE-02](LXE-02-planner-authority-ir.md), [LXE-07](LXE-07-semantic-hybrid-planner-provenance.md)

## Purpose

Make explanation, metrics, and bridge packet export reflect actual execution.
Do not conflate Sourcegraph translation with CodeQL/bridge candidate export.

## Current live truth (2026-05-27)

Already landed and green on current source:

- `SearchExplanation` stable wire shape carries `planner_trace`,
  `engines_touched`, `early_stop_reason`, and `summary`.
- real runtime queries populate planner trace / engines touched / summary for
  lexical explain, semantic, and hybrid paths.
- `BridgeCandidatePacket` is emitted from executed candidates rather than SG
  parser AST state.
- truthful non-`None` `early_stop_reason` is now proved on bounded hybrid
  execution via `CountReached`.
- bounded-label metrics now exist in `query_dispatcher.rs`; emitted names are
  closed to a fixed taxonomy and runtime dimensions stay cardinality-guarded.
- the closed metrics taxonomy is unit-proved in
  `classify_error_metric_name_uses_closed_taxonomy`, and runtime no-leakage is
  proved on the live `e2e_perf_chaos` rail.
- restart/replay proof lives in `E2E-05` and asserts stable result IDs plus
  stable explanation equality across lexical reopen, semantic scoped reopen,
  and lexical fresh replay, plus stable hybrid ID and high-level explanation
  truth across fresh replay.

## Owner files

- `crates/quanta-index-contract/src/results/**`
- `crates/quanta-index-lq-bridge/src/packet.rs`
- `crates/quanta-index-lq-bridge/src/translator.rs`
- `crates/quanta-index-lq-bridge/src/syntax.rs`
- `crates/quanta-index-search-plane/src/query_dispatcher.rs`
- `crates/quanta-index-searchd-runtime/tests/dsl_scenarios.rs`
- `crates/quanta-index-searchd-runtime/tests/explain.rs`
- new `crates/quanta-index-searchd-runtime/tests/e2e_restart_replay_determinism.rs`
- `crates/quanta-index-searchd-runtime/tests/e2e_perf_chaos.rs`

## File-level work breakdown

- `crates/quanta-index-contract/src/results/**`: make augmented `SearchExplanation` and bridge
  packet carriers part of the stable active result surface.
- `crates/quanta-index-search-plane/src/query_dispatcher.rs`: populate planner
  trace, engines touched, early stop, and typed failure codes from real runtime
  state.
- `crates/quanta-index-lq-bridge/src/{syntax.rs,translator.rs}`: keep SG syntax
  translation separate from bridge export semantics.
- `crates/quanta-index-lq-bridge/src/packet.rs`: export
  `BridgeCandidatePacket` from executed candidates, not parser ASTs.
- `crates/quanta-index-searchd-runtime/tests/{dsl_scenarios,explain}.rs`:
  assert augmented `SearchExplanation` stability, bridge export, and typed
  reason codes under real queries.
- `crates/quanta-index-searchd-runtime/tests/e2e_restart_replay_determinism.rs`:
  prove the explanation surface stays stable across persisted reopen/replay.

## Work items

- Ensure augmented `SearchExplanation` is populated from planner/runtime state:
  - planner trace
  - engines touched
  - early stop reason
  - summary
- Add typed rejection taxonomy for:
  - parse errors
  - normalize errors
  - unsupported syntax
  - unavailable producer data
  - plan limits
  - shard readiness
- Split Sourcegraph translator from bridge packet export:
  - Sourcegraph translator maps SG syntax to canonical active query
  - bridge export emits `BridgeCandidatePacket` from actual candidates
- Add metrics with bounded labels for:
  - intake outcome
  - planner outcome
  - engine fanout
  - merge count
  - early stop reason
  - typed unavailable reason

## Test plan

- unit tests that augmented `SearchExplanation` fields are populated for every successful
  query kind.
- unit tests that typed errors preserve stable codes.
- bridge packet round-trip tests.
- tests proving Sourcegraph translation does not imply bridge export.
- live current-tree proof on:
  - `cargo test -p quanta-index-search-plane query_dispatcher -- --nocapture`
  - `cargo test -p quanta-index-searchd-runtime --test explain -- --nocapture`
  - `cargo test -p quanta-index-searchd-runtime --test dsl_scenarios -- --nocapture`
  - `cargo test -p quanta-index-searchd-runtime --test e2e_restart_replay_determinism -- --nocapture`
  - `cargo test -p quanta-index-searchd-runtime --test e2e_perf_chaos -- --nocapture`

## E2E plan

Covered by `E2E-02`, `E2E-05`, and `E2E-06`:

- Sourcegraph query explanation includes translator and planner evidence.
- bridge export test produces packets from actual result candidates.
- restart/replay returns stable explanation shape.
- full corpus CI exports failure artifacts with typed reasons.

## DoD

Current status: satisfied on the current tree.

- no explanation response uses the prior summary-only shape.
- bridge candidate packets are emitted from result candidates, not parser AST.
- metrics have closed label sets and no raw query text labels.

## Failure modes

- explanation describes intended plan rather than executed plan.
- using Sourcegraph translator as the bridge implementation.
- unbounded metric cardinality from raw query strings or file paths.
