# SDL-E2E-01 - Structural Proof and Observability

> Archive status: `Historical program record`. Current architecture: [JUN-02-001](../../../adr/JUN-02-001-search-dsl-authority-and-runtime-contract.md). Archive map: [Completed Plan Archive](../../ARCHIVE-INDEX.md).


Status: `planned`
Priority: `P0`
Depends on: [SDL-01](SDL-01-structural-boolean-composition.md), [SDL-02](SDL-02-typed-hole-semantics.md), [SDL-03](SDL-03-sourcegraph-structural-v2-lowering.md), [SDL-04](SDL-04-language-set-expansion.md), [SDL-05](SDL-05-structural-codeql-bridge.md)

## Purpose

Close the structural proof debt across happy path, typed negatives, parity,
perf/chaos, and bounded-label metrics.

This ticket absorbs the structural residue from historical `E2E-04`,
`E2E-07`, and the structural slice of `LXE-10`.

## Owner files

- `crates/quanta-index-searchd-runtime/tests/sdk_frontdoor.rs`
- `crates/quanta-index-searchd-runtime/tests/end_to_end.rs`
- `crates/quanta-index-searchd-runtime/tests/e2e_dual_syntax_lowering_parity.rs`
- `crates/quanta-index-searchd-runtime/tests/e2e_perf_chaos.rs`
- `crates/quanta-index-search-plane/src/query_dispatcher.rs`
- `crates/quanta-index-searchd/src/app/runtime.rs`
- `crates/quanta-index-lq-obs/src/**`
- structural runtime composition files that emit metrics/events

## File-level work breakdown

- runtime tests
  - own the positive/negative/parity/perf matrix
- `query_dispatcher.rs`, `runtime.rs`
  - emit the typed structural outcomes asserted by the tests
- `lq-obs`
  - define the bounded-label structural metrics/events surface
- composition root
  - wire the structural metrics sink onto the live route only

## Work items

- prove the new native structural semantics:
  - boolean composition
  - typed holes
  - SG/native parity
  - language expansion rows
  - structural CodeQL bridge rows
- close structural perf/chaos rows:
  - plan-limit failure
  - cancellation/isolation
  - shard-unavailable and generation-not-ready
  - deterministic ordering on large tied candidate sets
- add bounded-label structural metrics:
  - route
  - syntax family
  - outcome
  - error code
- forbid raw structural pattern text in labels

## Test plan

- `sdk_frontdoor.rs` for public positive and typed-negative rows
- `end_to_end.rs` for authority/readiness/corruption rows
- `e2e_dual_syntax_lowering_parity.rs` for SG/native parity
- `e2e_perf_chaos.rs` for structural perf/chaos rows
- owner-local observability tests that assert bounded labels only

## E2E plan

- boolean composition positive rows
- typed-hole positive rows
- SG/native parity rows
- plan-limit negative row
- cancellation followed by next-query isolation row
- large tied candidate deterministic order row
- bounded-label metrics assertion row

## DoD

- every new structural semantic claim has a runtime proof row
- structural perf/chaos residue is closed on the live structural route
- structural metrics are emitted with bounded labels only

## Failure modes

- relying on unit tests while the public runtime route still rejects the shape
- counting parse/translation proof as execution proof
- adding structural metrics that include raw query text or unbounded label space
