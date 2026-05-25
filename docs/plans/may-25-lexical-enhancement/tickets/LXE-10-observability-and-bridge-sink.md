# LXE-10 - Observability and Bridge Sink

Status: `proposed`
Priority: `P1`
Depends on: [LXE-02](LXE-02-planner-authority-ir.md), [LXE-07](LXE-07-semantic-hybrid-planner-provenance.md)

## Purpose

Make explanation, metrics, and bridge packet export reflect actual execution.
Do not conflate Sourcegraph translation with CodeQL/bridge candidate export.

## Owner files

- `crates/quanta-index-contract/src/results/**`
- `crates/quanta-index-lq-bridge/src/packet.rs`
- `crates/quanta-index-lq-bridge/src/translator.rs`
- `crates/quanta-index-lq-bridge/src/syntax.rs`
- `crates/quanta-index-search-plane/src/query_dispatcher.rs`
- `crates/quanta-index-lexical/src/**`
- new `crates/quanta-index-searchd-runtime/tests/e2e_restart_replay_determinism.rs`
- new `crates/quanta-index-searchd-runtime/tests/e2e_sourcegraph_parity.rs`

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
- `crates/quanta-index-searchd-runtime/tests/*.rs`: assert augmented `SearchExplanation`
  stability, bridge export, and typed reason codes under real queries.

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

## E2E plan

Covered by `E2E-02`, `E2E-05`, and `E2E-06`:

- Sourcegraph query explanation includes translator and planner evidence.
- bridge export test produces packets from actual result candidates.
- restart/replay returns stable explanation shape.
- full corpus CI exports failure artifacts with typed reasons.

## DoD

- no explanation response uses the prior summary-only shape.
- bridge candidate packets are emitted from result candidates, not parser AST.
- metrics have closed label sets and no raw query text labels.

## Failure modes

- explanation describes intended plan rather than executed plan.
- using Sourcegraph translator as the bridge implementation.
- unbounded metric cardinality from raw query strings or file paths.
