# BM-01 — Cargo placement and dependency direction

Status: `PLAN / NOT_RUN`. Priority: P1. Depends on: BM-00. Common gates: [TEST-PLAN](TEST-PLAN.md).

The original `PLAN / NOT_RUN` status above is plan-time state, not current verdict. [CURRENT-AUDIT.md](CURRENT-AUDIT.md) owns unfinished work; [SEP-27-002](../../../adr/SEP-27-002-single-benchmark-orchestrator-and-typed-evidence.md) owns the CLI decision. Historical implementation inventory, migration matrix and closeout remain recoverable at `eff53181`, but their receipts are source-specific and do not qualify current code.

## Purpose

Keep benchmark code in the same repository and root Cargo workspace for source/lockfile identity, while separating its dependency graph and lifecycle from shipping product crates. Physical layout is not a substitute for a measured build/test-cost decision.

## Work

1. Retain small hot-path `benches/` in their owning product packages, initially `quanta-index-lq-norm` and `quanta-index-searchd-runtime`. Keep Criterion as a microbenchmark/diagnostic tool; compile-only CI is not a latency gate.
2. Put new protocol and CLI packages under `benchmarks/`. Keep `benchmarks/retrieval` as the Quanta SDK producer. Separate system producers into a benchmark-only package when its ownership is clear.
3. Audit every consumer of `quanta-index-searchd-harness`. Because runtime tests also depend on it, **default to keeping it in place**. A move to `testing/searchd-harness` is optional and requires evidence of a dependency/ownership/build-cost benefit; if approved, make it a mechanical path/package-reference change with all test and bench consumers updated. Preserve package name unless a separate compatibility audit justifies renaming.
4. Add an automated dependency-direction guard: production `crates/*` normal dependencies may not point to `benchmarks/*`; `dev-dependencies` on `testing/*` are allowed. Detect cycles and unapproved benchmark-only dependencies in shipping binaries.
5. Decide `workspace.default-members` from measured root-command behavior. Document that explicit `--workspace` continues to select all members. Do not create a nested workspace merely to hide compilation cost.

## DoD

- `Cargo.lock`, edition/toolchain, and root profile remain single-authority; `./scripts/cargow metadata --no-deps` resolves every package exactly once.
- Product binaries and normal product library builds do not acquire benchmark-only dependencies. Scoped product tests and full bench target compilation both compile after moves.
- The existing daemon/test harness scenarios and retrieval SDK runner retain identical externally observable behavior. Any optional path-only move has no metric or verdict changes; a no-move decision records the reason.
- CI's fast product lane and bench compile lane use explicit package/target selection, and the measured before/after compile cost is recorded; no unmeasured speedup claim.

## Verification / exclusions

Run Cargo metadata/dependency guard, scoped harness tests, `just rust-bench-build`, and affected test-authority rails through `./scripts/cargow`. Do not run timing-bearing benches on a contended shared checkout. A separate workspace is a future option only after a documented toolchain/lockfile conflict or unmanageable measured build cost.
