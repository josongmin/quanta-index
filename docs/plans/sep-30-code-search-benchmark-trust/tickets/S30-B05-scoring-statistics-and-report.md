# S30-B05 — independent scoring, uncertainty and final report

Status: `EXECUTED_DIAGNOSTIC` (2026-09-30); see receipt below. Priority: P1. Depends on admitted B02/B03
qrels and B04 native captures for each scored lane. Parent: [Sep 30 plan](../README.md).
Contract owner:
[CS-BENCH-03](../../sep-27-code-search-remediation/rfcs/CS-BENCH-03-tracks-metrics-and-statistics.md).

## Scoring contract

| Lane | Primary reported unit and metric | Separate diagnostics |
| --- | --- | --- |
| Exact bare-name content | Ten-distinct-file Recall/Hit@10 against declaration-derived file qrels, on a file-ranked route | Native ten-chunk observed-prefix file coverage, same-file duplication and cap |
| Typed exact symbol | Indexed declaration identity Hit/MRR@10 where a genuine symbol-ranked route exists | Same-line distinct declarations and returned-context span checks; do not fold into the five-product content score |
| Semble gin 20 | Reviewed graded file NDCG@10 plus file Recall@10 by semantic/architecture/symbol category | Secondary-file treatment, pool coverage, judgment sensitivity; descriptive only at 11/6/3 tasks |
| Identifier robustness | Per-family change in exact-vs-variant file/declaration success, by prefix/infix/split/typo | Ambiguity classes, no-answer false-positive/abstention, query length/case strata |
| ARB gin | Official positive file Recall@20, MRR and token-budget BCY | Per-workflow subset and budget, never mixed with 99-file gin results |

Use exactly the same eligible task IDs for paired quality comparisons. Report
requested, eligible, attempted, completed, unsupported, incomplete, error and
timeout counts by product and stratum; keep operational score separate from
conditional retrieval quality. A capped complete window is not an execution
failure, while a partial/timeout response is not an empty successful ranking.
NDCG requires reviewed grades; unjudged ranked files block the affected
score under the declared policy. Avoid giving multiple gains to duplicate
chunks, and avoid treating a context window as an indexed declaration span.

Compare both matched-semantics and native-workflow results only within their
own mode. If indexed-universe or result-unit equivalence is unproved, publish
the rows as diagnostic and exclude them from a qualified cross-product delta.
An unranked native result gets set recall or explicitly observed order, not a
fabricated relevance MRR.

## Statistics, decision and verification

1. Recompute every numerator, denominator, rank and status from raw/native-bound
   rows using an independent small checker. Compare selected rank metrics to
   pinned `trec_eval` on fixed qrel fixtures and to hand-calculated goldens.
2. Give per-query paired wins/losses/ties and family-cluster bootstrap intervals
   on identical tasks. The Semble 20 and its three symbol cases remain a case
   series: show individual outcomes and no superiority/significance claim.
3. Report macro by repository once multi-repository data exists; one gin
   repository cannot provide repository-level confidence. Predeclare primary
   effect, critical strata and resource ceilings before opening B08's holdout.
4. Keep `PAIR_VALID`, `QUALITY_DELTA`, and default-decision status distinct.
   `QUALITY_DELTA=pass` is evidence admission, not proof of a positive delta.
   Apply existing `decision.py` only with a frozen policy SHA and a valid
   qualified report.

Focused tests must reject wrong route/unit/case, reversed or duplicate ranks,
missing task, altered gold, unjudged file, partial response, no-answer leakage,
and unsupported-result denominator drift. Golden fixtures include grades
`[3,1]`, duplicate chunks from one file and same-line distinct declarations.
Publish a report with raw IDs, exclusions, qrel version and manifest digests;
do not rewrite original 300-query or 1,196-query reports.

Implement needed scoring changes only in the existing
[evaluator](../../../../tools/benchmark/retrieval/evaluator.py),
[lexical comparator](../../../../tools/benchmark/retrieval/lexical_file_comparison.py)
and their test owners. No second scoring stack.

## Execution receipt (2026-09-30)

Diagnostic scorecard v2; pytrec_eval agreement on reported metrics; all numbers re-derived by a separate audit. No qualified delta or default decision. Producer `quanta-index@0d21914e` (clean worktree);
results, digests and residuals: [qi-s30-bench-trust-20260930-0d21914e/RESULTS.md](/Users/songmin/Documents/code-new/qi-s30-bench-trust-20260930-0d21914e/RESULTS.md).
