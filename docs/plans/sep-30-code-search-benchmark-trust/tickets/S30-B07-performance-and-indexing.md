# S30-B07 — equal-boundary performance and indexing measurement

Status: `NOT_RUN` (2026-09-30); see receipt below. Priority: P1. Depends on B04's
correct, complete capture contract and a quiet admitted host. Parent:
[Sep 30 plan](../README.md). Contract owners:
[CS-BENCH-04](../../sep-27-code-search-remediation/rfcs/CS-BENCH-04-comparators-performance-and-incremental.md)
and [MISC-06](../../sep-27-misc/tickets/INDEX.md).

## Work and boundaries

Measure correctness before time. A Quanta SDK/IPC request and a Semble
in-process BM25 function call are different timers; report both as different
profiles, never as one speed ratio. Build the pinned Quanta release profile
required by the selected deployment/benchmark recipe, and bind the actual
binary SHA, Semble package/model assets, compiler options and product versions.

For the full admitted 1,196-query exact-name workload and any newly admitted
lanes, record the following. The 20-query Semble set is too small for a stable
p95 comparison and remains descriptive:

- Client request construction through **complete decoded required output**,
  including transport/process startup where that is the actual product flow.
  Report server-internal stages only where instrumented; absent telemetry is
  `unavailable`, not zero. Time-to-first-result and completed top-k are distinct.
- Per-query p50/p95, distributions, completed/error/timeout/partial counts,
  output bytes and native units. Repeat randomized paired blocks with a frozen
  seed and warmup policy; record host load and resource envelope throughout.
- Fresh-root index construction, parse/chunk/embed/index/publish/activate phases,
  source files/bytes, indexed units, CPU, peak process-tree RSS and disk. Quanta
  and Semble may have different chunk counts and operations; report the
  boundaries rather than a bare indexing-speed multiple.
- Cold process/model/page-cache and warm query states separately. A fresh
  directory alone does not prove a cold OS cache.

Use the same task set and user-visible output unit when making a matched
workflow performance claim. Native-product workflows may differ, but their
requests, work and result limits must be shown. Sourcegraph/OpenGrok remote or
unattested indexes cannot silently share local index-build denominators.

## Verification and deliverable

- Query order/status and required output are equivalent within the chosen
  comparison mode before timing analysis. No survivor-only latency after
  dropping failed requests.
- Timing source, clock boundary, warm/cold state, repetition count, compiler
  profile, model assets, indexed source and product topology are in the raw
  receipt. A busy host or concurrent build makes the performance claim
  `NOT_RUN`/diagnostic rather than `PERF_QUALIFIED`.
- Publish a performance table separate from B05 quality, including every
  product's actual query and indexing boundary. Existing gin release timings
  remain diagnostics and are not reused as equal-work proof.

Extend the current registered capture and resource owners only for an observed
missing boundary; do not add a second benchmark harness.

## Execution receipt (2026-09-30)

`NOT_RUN`: host not quiet (load ~26 on 16 cores, concurrent builds). Producer `quanta-index@0d21914e` (clean worktree);
results, digests and residuals: [qi-s30-bench-trust-20260930-0d21914e/RESULTS.md](/Users/songmin/Documents/code-new/qi-s30-bench-trust-20260930-0d21914e/RESULTS.md).

Reconfirmed `NOT_RUN` 2026-10-01 (v2): host load 25–29 and the data volume ran out of space.
