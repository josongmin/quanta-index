# BM-03 — One registry and one benchmark CLI

Status: `PLAN / NOT_RUN`. Priority: P0. Depends on: BM-02. Common gates: [TEST-PLAN](TEST-PLAN.md).
The original `PLAN / NOT_RUN` status above is plan-time state, not current verdict. [CURRENT-AUDIT.md](CURRENT-AUDIT.md) owns unfinished work; [SEP-27-002](../../../adr/SEP-27-002-single-benchmark-orchestrator-and-typed-evidence.md) owns the CLI decision. Historical implementation inventory, migration matrix and closeout remain recoverable at `eff53181`, but their receipts are source-specific and do not qualify current code.

## Purpose

Evaluate replacement of `tools/benchmark/manifest.json` + Python `benchctl.py` with a typed registry and Rust CLI, without rewriting every existing producer or scorer in Rust. Existing Python source freeze, host preflight, baseline admission, and fail-closed validators are functional assets to preserve, not presumed defects.

## Work

1. Define one `tools/benchmark/registry.toml` with stable family/case IDs, purpose and result unit, allowlisted producer/validator/scorer IDs, required external inputs, source closure, host policy, schedule/sample floor, baseline compatibility, gate tier, and owned output type. Reject duplicate IDs, unknown references, unreachable or unregistered producers, and ambiguous profile membership.
2. Implement `benchctl list`, `plan`, `run`, `validate`, `compare`, and `replay` in `benchmarks/benchctl`. `plan` emits a resolved, digest-bound command/input graph without mutation; `run` checks source, inputs, host, binary and output root before calling an allowlisted producer. Arbitrary registry shell fragments and undeclared output files are forbidden.
3. Provide adapters to existing Just/Rust/Python producers. The candidate CLI owns invocation order, source freeze recheck, resource/timeout limits, staging, raw capture, and status; domain scorers own metric mathematics. One real DSL vertical slice tests the interface before broad migration.
4. Make an explicit go/no-go decision: compare candidate versus Python on negative mutations, raw/verdict parity, implementation/maintenance cost, and execution overhead. If Rust is not justified, keep Python as the **single current orchestrator** and still deliver the typed registry/evidence contract. Never maintain both CLIs as live authorities.
5. Keep `tools/benchmark/manifest.json` and Python CLI authoritative only for not-yet-migrated families if the Rust path is chosen. At family cutover, remove the old current registration and direct CI path together. Retain a named historical replay entrypoint only if required for immutable old captures.

## DoD

- `list` reconciles exactly with BM-00 inventory and Cargo/CI policy; `plan` is deterministic on the same frozen inputs; `run` and `replay` refuse source/input/output drift.
- Each command has a machine-readable result and stable nonzero exit on invalid, blocked, failed, interrupted, or incomplete work. `summarize`/inventory cannot label unvalidated files qualified.
- An injected producer that writes a forged green JSON, omits raw data, exits nonzero, times out, or changes HEAD/inputs mid-run cannot promote a passing artifact.
- One existing DSL scenario is run through the candidate CLI and its raw values/behavior verdict and failure refusals are reconciled against the old path before a go/no-go decision. The chosen implementation has exactly one live authority.

## Verification / exclusions

Test registry parsing, command resolution, mutation and process failure modes; then run a representative real producer. CLI replacement is not a license to merge unlike scoring rules into one comparator.
