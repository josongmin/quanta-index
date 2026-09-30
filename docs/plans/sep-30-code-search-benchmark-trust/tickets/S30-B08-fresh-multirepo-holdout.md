# S30-B08 — fresh multi-repository holdout and product decision

Status: `NOT_RUN` (2026-09-30); see receipt below. Priority: P2. Parent:
[Sep 30 plan](../README.md). Contract owners:
[CS-BENCH-01](../../sep-27-code-search-remediation/rfcs/CS-BENCH-01-corpus-gold-and-holdout.md),
[CS-BENCH-03](../../sep-27-code-search-remediation/rfcs/CS-BENCH-03-tracks-metrics-and-statistics.md),
[CS-BENCH-04](../../sep-27-code-search-remediation/rfcs/CS-BENCH-04-comparators-performance-and-incremental.md)
and [CS-INT-01](../../sep-27-code-search-remediation/rfcs/CS-INT-01-integration-and-qualification.md).

## Why a new release is required

The old 1,196 gin names, Semble gin 20 and public ARB are exposed evaluation
sets. Their successful replay is a regression or external pilot, not an unseen
post-tuning holdout. Gin-only family resampling also cannot support inference
over repositories. Follow BENCH-01's engineering target of at least 12 pinned
repositories and 1,200 **fresh** cases across language, size, task and negative
strata; those numbers are local targets, not a universal SOTA sample-size rule.

## Work

1. Freeze repository commits, complete file/encoding/license manifests,
   declaration/literal/context oracles and reviewed qrels before candidate
   capture. Separate development and holdout by repository and query family;
   reject copied files, forks, near-duplicate templates and leaked source spans
   or declare a bounded exclusion.
2. Preregister primary task, metric, minimum useful effect, allowed critical
   stratum regression, resource budgets, uncertainty method and finite ablation
   matrix **before** holdout access. Choose size and thresholds from baseline
   variance and product needs; do not choose them after the winning run.
3. Run current registered producers and raw/native replay on a clean bound
   source, indexed universe, exact binary/model and supported host. Perform
   repository-level as well as query-family-level aggregation; report each
   product's attempted/completed/unsupported and judgment coverage.
4. Apply the existing [decision gate](../../../../tools/benchmark/retrieval/decision.py)
   only to an eligible paired report and the frozen policy SHA. A passing
   `QUALITY_DELTA` alone is not an engine win or default-change approval.

Use [CoIR](https://github.com/coir-team/coir) for varied code retrieval tasks,
[CORE-Bench](https://github.com/zhangfw123/CORE-Bench-Eval) for issue-to-edit
and broader context, and [BEIR](https://arxiv.org/abs/2104.08663) for the
heterogeneous out-of-domain evaluation principle. Pin their exact data and
evaluation code when adopted; do not merge their published leaderboard scores
with locally altered corpora. The ARB full 427 and its no-gold cases can be a
separate external workflow track.

## Completion

- Every holdout run has independently admitted labels, source/index identities,
  raw evidence, replay and a query-family/repository split report.
- The report gives task- and repo-macro results, paired uncertainty, critical
  strata, capability coverage and resource ceilings. Failed or unavailable
  comparators remain explicit; no missing row is silently dropped.
- Only the exact policy decision scope may claim a product-default change.
  Installed release, hosted CI, activation and deployment retain their own
  gates. The holdout is retired from future independent evaluation once used
  to select a policy.

## Execution receipt (2026-09-30)

`NOT_RUN`: needs frozen repositories, fresh reviewed labels and a preregistered policy. Producer `quanta-index@0d21914e` (clean worktree);
results, digests and residuals: [qi-s30-bench-trust-20260930-0d21914e/RESULTS.md](/Users/songmin/Documents/code-new/qi-s30-bench-trust-20260930-0d21914e/RESULTS.md).
