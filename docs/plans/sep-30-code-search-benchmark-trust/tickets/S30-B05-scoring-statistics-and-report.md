# S30-B05 — independent scoring, uncertainty and final report

Status: `EXECUTED_DIAGNOSTIC` (2026-09-30); see receipt below. Priority: P1. Depends on admitted B02/B03
qrels and B04 native captures for each scored lane. Parent: [Sep 30 plan](../README.md).
Contract owner:
[CS-BENCH-03](../../sep-27-code-search-remediation/rfcs/CS-BENCH-03-tracks-metrics-and-statistics.md).

## Scoring contract

| Lane | Primary reported unit and metric | Separate diagnostics |
| --- | --- | --- |
| Exact bare-name content | Ten-distinct-file Recall/Hit@10 against declaration-derived file qrels; state whether the route is scored or path-ordered (Quanta `literal_file` is path-ordered, correction 2026-10-01) | Native ten-chunk observed-prefix file coverage, same-file duplication and cap |
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

Diagnostic scorecard (v1 root); pytrec_eval agreement on reported metrics; all numbers re-derived by a separate audit. No qualified delta or default decision. Producer `quanta-index@0d21914e` (clean worktree);
results, digests and residuals: [qi-s30-bench-trust-20260930-0d21914e/RESULTS.md](/Users/songmin/Documents/code-new/qi-s30-bench-trust-20260930-0d21914e/RESULTS.md).

Names: the v1 root's `b05/scorecard.json` (0d21914e captures). v2 root `scores/scorecard-v2.json` (`score_v2.py`, f318e832): paired exact Hit@10 vs `keyword_file` over all 1,196 tasks, distinct-file systems only; no pytrec cross-check on v2. [qi-s30-v2-f318e832/RESULTS.md](/Users/songmin/Documents/code-new/qi-s30-v2-f318e832/RESULTS.md).

2026-10-01 source-bound follow-up: independent source-file qrel checks over
1,196 complete records agreed with the evaluator on Semble `lexical-file`
1,190/1,196 and Quanta `keyword_file` 1,192/1,196 top-10 file hits. The
Quanta record now carries the native SDK score for every returned file; the
Semble raw capture keeps every positive-score BM25 chunk before file collapse.
The two scored file modes have different query semantics and tie policies, so
these counts are diagnostic observations, not a paired quality delta. See
[`Semble summary`](/Users/songmin/Documents/code-new/qi-s30-file-modes-20261001-ExtGj6hb/semble/summary.json) and
[`Quanta summary`](/Users/songmin/Documents/code-new/qi-s30-file-modes-20261001-ExtGj6hb/quanta/summary.json).
The independent [audit checker](/Users/songmin/Documents/code-new/qi-s30-file-modes-20261001-ExtGj6hb/audit.py)
re-read the source-file qrels, both records and Semble's full native BM25
lists; its [result](/Users/songmin/Documents/code-new/qi-s30-file-modes-20261001-ExtGj6hb/audit.json)
agrees on every task and projection. No new five-product paired score was
created from snapshots with different request semantics.

The separate 2026-10-01 [robustness file-mode audit](/Users/songmin/Documents/code-new/qi-s30-robust-filemodes-20261001-xh1F3Xoh/audit.json)
recomputed all six lanes from source-file qrels. Semble distinct-file hits are
prefix 257/352, infix 164/339, components 276/278, typo 284/363; Quanta
`keyword_file` hits are 21/352, 14/339 (337 submitted), unsupported on
components, and 0/363. The no-answer-content lane returned zero files for
Quanta and non-empty files on all 99 Semble queries. Request semantics and
completion coverage differ; these are operational diagnostics, not a paired
quality delta.
