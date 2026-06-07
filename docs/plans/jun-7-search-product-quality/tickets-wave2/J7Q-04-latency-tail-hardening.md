# J7Q-04 — Latency Tail Hardening

Parent RFC: [../rfc.md](../rfc.md)

Status: `planned`

## Goal

Turn route-critical p95 / p99 regressions from advisory-only noise into explicit
quality signals where they matter.

## Current Code Fact

- p50 compare is blocking
- p95 / p99 are still largely advisory
- warm tail behavior is already visible in existing artifact schema

## Owner Seam

- benchmark artifact producer
- bench support metadata
- compare policy docs

## Source Truth Anchors

- `/Users/songmin/Documents/code-new/quanta-index/tools/benchmark/README.md`
- `/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-searchd-runtime/benches/dsl_query_matrix.rs`
- `/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-searchd-harness/src/bench_support.rs`
- `/Users/songmin/Documents/code-new/quanta-index/docs/plans/jun-7-search-product-quality/COMMAND_AND_ARTIFACT_CONTRACT.md`

## Preferred Implementation Direction

- set budgets by route family first, then harden thresholds selectively
- emit per-row diagnostic metadata sufficient to explain tail cliffs
- keep tail policy honest about warm daemon conditions and variance envelopes
- include candidate-count, regex-complexity, and repo-fanout buckets when they
  explain tail behavior

## Layer Boundary Clarification

- this ticket owns latency-tail quality
- it does not own relevance quality from `J7Q-01` or scale-tier manifests from
  `J7Q-03`

## Concrete Work Items

1. Define per-route tail budgets.
2. Keep p50 blocking but add explicit p95 / p99 thresholds where justified.
3. Emit enough per-row metadata to explain tail regressions.
4. Keep advisory and blocking tail signals separate.

## Required Outputs

- stable command:
  - `just rust-verify-quality-tail`
- canonical artifacts:
  - `artifacts/search-quality/tail/latest/summary.json`
  - `artifacts/search-quality/tail/latest/route_budgets.json`

## First Increment

- document route-family tail budgets before turning any new threshold hard

## Red Rail To Pin First

```bash
just rust-bench-dsl-compare
```

## Worker First Commands

```bash
sed -n '1,260p' tools/benchmark/README.md
rg -n "p50|p95|p99|tail|latency" crates/quanta-index-searchd-harness crates/quanta-index-searchd-runtime tools/benchmark -S
```

## No-Go

- do not convert every advisory into a blocker without route-specific policy
- do not use one global tail threshold for all route families
- do not mix semantic retrieval or hybrid fusion tail into this packet
- do not ship tail verdicts that lack route-local diagnostic metadata

## Reviewer Rejection Checklist

- reject if `p95` / `p99` are made blocking without route-specific budgets
- reject if pass/fail rows do not expose enough metadata for diagnosis
- reject if `p50` improvement is used to hand-wave tail regressions away

## DoD

- tail policy is route-aware and explicit

## Not Done If

- p95 / p99 still have no documented meaning
- tail regressions remain unactionable
- the tail artifact lacks route-local diagnostic fields
