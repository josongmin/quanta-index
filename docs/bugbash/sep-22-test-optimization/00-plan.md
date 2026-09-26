# Test Optimization Remediation Backlog (2026-09-22)

> Archive classification: historical Sep 22 audit input. Use the active [TOPT ledger](../../../tickets/sep-22-test-optimization/INDEX.md), [current closeout](../../../tickets/sep-22-test-optimization/SEP25-CURRENT-CLOSEOUT.md), and [SEP-27-001](../../adr/SEP-27-001-documentation-authority-and-historical-record-custody.md).


Audited snapshot: `23bd3d7fa7af1122f904e59f1f514935bd5ffe7e`
(`main == origin/main` at freeze).

Status: **18 open actions**. This directory now contains only findings with a
current source path, reachable failure or cost, an owning component, a minimum
fix, and a verification rail. The audit changed documentation only; none of the
actions below is implemented or runtime-qualified yet.

The status above describes the frozen Sep 22 audit, not the current checkout.
For the Sep 23 code integration and remaining qualification gates, see the
[current-source receipt](../../../tickets/sep-22-test-optimization/RCA-2026-09-23-current-source.md#committed-tree-integration-receipt).

Structural execution packet:
[`tickets/sep-22-test-optimization`](../../../tickets/sep-22-test-optimization/INDEX.md).

## Owner documents

- `01-duplication.md` — 2 duplicated-owner defects: direct-runtime socket
  configuration drift and umask child-lifecycle drift.
- `02-slow-inefficient.md` — 5 proven wait/cost/layer defects: the three-second
  lease hold, real retry sleeps, a scheduler-window concurrency oracle,
  non-interruptible peer watch, and one redundant daemon E2E.
- `03-weak-assertions.md` — 3 false-green oracle gaps in trigram properties,
  the E2E smoke query, and post-cutover backup completeness.
- `04-cheesy-test-code.md` — 4 fixture/helper defects: two timeout
  misclassifications, persistent pid-only socket paths, and repeated identical
  filter fixtures.
- `05-cheesy-src-code.md` — 4 production-owner seams: two wall-clock owners,
  ambient SDK environment resolution, and polling-only single-flight
  cancellation.

## Closure rule

An item closes only after its owner-side fix lands and the stated focused rail
passes. Timing items additionally require an uncontended before/after sample.
Focused success is not workspace or daemon-wide qualification; run the broader
rail named by the changed surface before declaring product closure.
