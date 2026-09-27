# SEP-27-002 — Single Benchmark Orchestrator and Typed Evidence

Status: `Accepted`

Decided: 2026-09-26

Consolidated: 2026-09-27

Source decision: `git show 84c9331f:docs/plans/sep-26-bench-migration/tickets/BM-03-DECISION.md`

Implemented complete-profile publication, capture failure, process/monitor,
bounded file/archive I/O and test-selection decisions are consolidated in
[SEP-27-004](SEP-27-004-benchmark-capture-and-resource-custody.md). Open execution
and measurement acceptance stays in the MISC ledger.

## Context

Benchmark registration, orchestration, evidence custody and domain scoring
needed a common control plane. A Rust replacement for the Python orchestrator
was evaluated as a candidate, not presumed to improve correctness.

## Decision

- `tools/benchmark/registry.toml` is the single benchmark registration
  authority. `registry.py` validates it; `manifest.py` is a read-only
  projection, not another manifest.
- `tools/benchmark/benchctl.py` is the only current orchestration CLI.
  Producers remain the registered Just, Cargo and Python owners. Domain
  comparators retain their own metric mathematics and verdict authority.
- `benchmarks/bench-protocol` owns the typed evidence contract and independent
  Rust validation. `tools/benchmark/evidence.py` writes the same canonical
  representation. Immutable raw bytes and digests are retained for replay.
- Do not add a parallel Rust CLI or duplicate Python source-freeze, host
  preflight, baseline-admission and artifact-refusal guards. A Rust CLI may be
  reconsidered only after exact-source raw/verdict/refusal parity and measured
  execution cost demonstrate a net benefit. The current decision is **NO-GO**.
- A common evidence envelope does not turn a capture-only run into a domain
  benchmark verdict. Missing or ineligible inputs must remain typed
  diagnostic/not-run/blocked outcomes, not passing measurements.

### Workspace and input boundary

Benchmark-only Rust packages stay in the root Cargo workspace and share its
toolchain and `Cargo.lock`; product crates do not acquire normal dependencies
on the evidence protocol. Hot-path benches stay with their owning crates,
while system and retrieval producers retain separate benchmark packages.
Corpora, gold, model assets, raw traces and runs are external to the Git
checkout; the repository contains schemas, recipes and small fixtures.

### Capture and verdict boundary

Each purpose has a typed payload rather than a common fabricated latency row.
A run is staged with its native raw bytes, validated and promoted immutably;
the complete-profile pointer is published only after every declared case is
present. `latest` is advisory, never an admitted baseline. Baselines bind an
explicit compatible run ID and digest. Readers and staged-output writers use
the no-follow custody rule in [SEP-26-003](SEP-26-003-retrieval-evidence-custody-and-qualification.md).

The retrieval contract profile records test proof, not relevance rows. Pair
diagnostics reuse the native pair/verdict owner; lexical diagnostics score
frozen observations and do not perform live searches. Recorded agent imports
are explicitly unauthenticated diagnostics. File, span, context, agent outcome
and speed keep separate denominators and admission rules.

## Rationale and boundary

The vertical slice established cross-language canonical-byte/digest fixtures,
an immutable run store and raw-derived replay for a fixed DSL artifact.
Re-implementing the already guarded Python orchestration path would duplicate
fail-closed policy without demonstrated guarantee gain. The decision did not
include a timed CLI-overhead comparison or canonical-host DSL measurement.
Those exclusions prevent treating this ADR as performance qualification.

The benchmark migration remains **partial**: current execution coverage,
source-bound receipts and missing qualification are tracked in
[the execution SSOT](../plans/sep-27-misc/tickets/INDEX.md). An
accepted orchestration decision does not close BM-03 implementation or any
benchmark family by itself.
