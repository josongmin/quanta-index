# BM-07 — CI, baselines, and authority cutover

Status: `PLAN / NOT_RUN`. Priority: P0 closeout. Depends on: BM-04/BM-05/BM-06. Common gates: [TEST-PLAN](TEST-PLAN.md).
The original `PLAN / NOT_RUN` status above is plan-time state, not current verdict. [CURRENT-AUDIT.md](CURRENT-AUDIT.md) owns unfinished work; [SEP-27-002](../../../adr/SEP-27-002-single-benchmark-orchestrator-and-typed-evidence.md) owns the CLI decision. Historical implementation inventory, migration matrix and closeout remain recoverable at `eff53181`, but their receipts are source-specific and do not qualify current code.

## Purpose

Finish migration with one executable authority and no dangling direct CI path, stale baseline, unverifiable `latest` artifact, or old current writer.

## Work

1. Change timing-bearing benchmark jobs in `.github/workflows/{ci,correctness}.yml` and Justfile wrappers to call named registry profiles through the **chosen single CLI**. Keep PR cheap gates (schema/inventory, correctness, bench compilation) separate from canonical host timing and externally admitted retrieval/agent evaluation. Direct compile-only commands may remain if inventoried and labeled non-measurement; a workflow that calls individual DSL timing producers/comparators after cutover is a bypass and fails policy.
2. Implement baseline admission as an explicit operation over immutable run IDs. Verify complete raw samples, correctness, compatible host/source/corpus/config/model/binary/measurement boundary, predeclared margin and uncertainty method, owner review, and atomic publication. Remove hard-coded `dsl-latency`-only comparator policy once all affected families have typed comparators.
3. Preserve historical artifacts/readers by named replay command; disable former current writers and `latest`-based comparison paths at each family cutover. Do not delete user data or old benchmark receipts as a side effect.
4. Add one migration matrix: old family/path, new ID/producer/scorer, legacy-reader status, raw parity result, exact verification receipt, and cutover commit. Update `tools/benchmark/README.md`, retrieval README, Justfile help and CI documentation from this matrix.
5. Re-freeze source and external inputs after all normative docs/code changes; run source-bound tests and available clean-host benchmarks. Final claims report implementation, contract proof, actual measurement, and excluded external qualification separately. Do not require an unavailable external gold/model/host to close the infrastructure migration.

## DoD

- Policy rejects any unregistered bench target/producer, duplicate live scorer/comparator, CI direct bypass, stale baseline, or missing raw dependency. `benchctl list/plan/validate/replay` work from a clean exact-source snapshot. BM-03's Rust/Python go/no-go decision and the losing path's retirement are recorded.
- Existing and new validators agree on shared frozen raw evidence or every difference has a documented contract change and independent oracle test. No unexplained score or sample-count drift.
- Representative micro and DSL/system producer types have a fresh-process real-run replay when their host is available; retrieval and recorded-only paths have source-bound contract/fixture replay and actual captured-input replay where those inputs exist. Every result records command, HEAD, dirty state, input/binary/host identities, raw output, artifact digest, covered and excluded scope. Qualified external quality/performance remains separately gated and can be `BLOCKED`/`NOT_RUN` after infrastructure closeout.
- The migration matrix identifies one current authority per family; old current control plane is removed only after that family's acceptance, with historical replay retained as needed.

## Verification / exclusions

Run cheap policy/unit checks, then scoped Rust and Python integration, source-closure proof, CI-equivalent commands, and finally canonical-host/external measurements. Do not mark the migration complete from green compilation or a valid but unmeasured artifact. Baseline approval is not deployment or activation approval.
