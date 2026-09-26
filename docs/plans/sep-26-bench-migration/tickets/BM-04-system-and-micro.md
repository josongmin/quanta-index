# BM-04 — System, load, freshness, and Rust microbenchmark migration

Status: `PLAN / NOT_RUN`. Priority: P1. Depends on: BM-01/BM-03. Common gates: [TEST-PLAN](TEST-PLAN.md).

Implementation/verification/qualification verdicts for this ticket are recorded in `git show eff53181:docs/plans/sep-26-bench-migration/tickets/CLOSEOUT.md` and, where relevant, [BM-00-INVENTORY.md](BM-00-INVENTORY.md), [BM-03-DECISION.md](BM-03-DECISION.md) and [BM-07-MIGRATION-MATRIX.md](BM-07-MIGRATION-MATRIX.md). The original `PLAN / NOT_RUN` status above is the plan-time state, not the closeout state.

Current correction: [CURRENT-AUDIT.md](CURRENT-AUDIT.md) is authoritative over
the historical closeout. Criterion capture/validation/replay and complete-case
custody are implemented; LQ's skipped-stage fixture defect is repaired. Full
clean-source micro integration and native binary/monitored-host qualification
remain separate requirements. A focused test or registration is not BM-04 closure.

## Purpose

Register existing DSL/search-quality/system producers and crate-local Rust microbenchmarks without changing the behavior they measure. Separate human-facing micro diagnostics from product-performance authority.

## Work

1. Register current `dsl-warm`, `dsl-cold`, relevance, ambiguity, snippet, scale, tail, ANN, concurrency, freshness, open-loop, ops, UI, and provider A/B families with their actual producer/scorer/host policy. Preserve route-specific budgets and the existing deterministic scenario truth oracle.
2. Keep Criterion benches at the owning crate. Register each target/input-size case, store raw output, and require correctness fixtures. Introduce Iai-Callgrind only for selected CPU hot paths on supported Linux hosts after pinning runner/toolchain and measuring whether it adds useful signal. Instruction count is not latency qualification.
3. For daemon measurements, bind monotonic timing clock, client request/response boundary, cold vs warm roots/cache, top-k/result shape, query policy, build/instrumentation mode, resource counters, and error/timeout/drop data. Open-loop offered-load results must include offered and completed rates plus generator health; incomplete points are capacity loss, not missing-good samples. Reserve and monitor the canonical host through the full capture, not only preflight.
4. Keep full-build, one-file update, delete/rename, restart/reopen and time-to-searchable distinct. Prevent stale-hit and wrong-generation outputs from receiving a speed verdict.
5. Produce comparison receipts from immutable run IDs. Baseline compatibility must include host class, source/build profile, corpus/config/model/query contract and measurement boundary. An old DSL baseline cannot compare against a changed daemon accept cadence as the same experiment.

## DoD

- Every current system and micro producer is registered or explicitly retired with reason; no CI/Just timing path bypasses the registry after its family cutover.
- Existing scenario truth and typed-error expectations pass before timing; raw sample counts, error/drop/timeout and resource accounting reconcile with summaries.
- A clean-source representative warm/cold run and an open-loop/freshness run through the chosen CLI validate and replay from raw artifacts where the host is available. If the designated host is absent, use a declared diagnostic environment for integration DoD and keep quiet-host qualification `BLOCKED`/`NOT_RUN`.
- Candidate/baseline order and independent repetition units are fixed before the measurement; a contended or unsupported host yields diagnostic or blocked, not a speedup claim. A lost lease, missing capture-time host sample, clock anomaly, or load-generator saturation is exercised by a negative fixture.

## Verification / exclusions

Use existing harness scenario tests and `just rust-bench-build` as cheap gates, then source-bound real-daemon integration and designated host timing. BM-04 does not change search algorithms or convert Criterion p-values into an E2E SLO.
