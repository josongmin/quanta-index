# S30-B02 — independently adjudicate Semble gin 20

Status: `ACTIVE_RESIDUAL`; historical executions are diagnostic. Priority: P0. Depends on S30-B01
source-universe validation. Parent: [Sep 30 plan](../README.md). Contract owner:
[CS-BENCH-01](../../sep-27-code-search-remediation/rfcs/CS-BENCH-01-corpus-gold-and-holdout.md).

## Input and threat to validity

Pin [the 20 annotations](https://github.com/MinishLab/semble/blob/aa634b14dc81ba6925a130cb999081f3240e2a3c/benchmarks/annotations/gin.json),
upstream `repos.json`, tool version and raw byte digests. Check the referenced
gin commit and every annotation path against the selected candidate universe.
The upstream authors state that one model generated both queries/labels and
their verification. The initial annotation is a proposal, not independent
human gold. `relevant`, `secondary`, `seed` and `related` have different roles;
do not mechanically promote all of them into the same relevance grade.

The three symbol strings also occur in the 1,196 exact-name suite, but the
task intent differs. In particular, source-derived `Engine` file judgments
include `binding/default_validator.go`, while the upstream type-lookup label
selects `gin.go`. Preserve separate task IDs, label provenance and scoring.

## Work

1. Define each query's intended information need, accepted result unit and
   grade-0/1/2/3 rubric before viewing new product results. Record source
   paths and evidence spans for proposed relevant/secondary files.
2. Obtain two independent source-grounded judgments for all 20 queries and
   reconcile differences. If reviewers are automated, record that fact and do
   not set `human-reviewed`; human review requires actual human assessors.
3. After the first blind judgment freeze, inspect the **union** of five
   products' admitted top-10 file results, blinded to product identity, for
   additional relevant files. Version the amended qrels and rescore **all**
   products on the same amended qrels. Record pool depth, unjudged count and
   a leave-one-system-out sensitivity check. Product majority is not truth.
4. Keep semantic (11), architecture (6) and symbol (3) query categories
   separate. Define `complete_ranked_pool_v1` only after every admitted ranked
   result is judged; otherwise report incomplete judgment coverage rather
   than assuming unjudged is irrelevant.

## Verification and deliverable

- All 20 queries have source-bound qrels, rationale and review-provenance
  status; disagreements and changed upstream labels are explicit.
- Wrong source SHA, missing gold path, duplicated label, grade/unit mismatch
  and an unjudged ranked result reject qualified scoring in focused fixtures.
- A frozen blind runner pack contains no gold or answerability. The category
  report is descriptive; `n=3` symbol results cannot establish superiority.

Existing [suite schema](../../../../tools/benchmark/retrieval/suite.schema.json)
and [evaluator](../../../../tools/benchmark/retrieval/evaluator.py) own the
format and mathematics. This ticket produces reviewed data, not a new scorer.
The method follows [TREC pooled judgments](https://trec.nist.gov/pubs/trec33/papers/overview_33.pdf)
and [CodeSearchNet's graded human annotations](https://github.com/github/CodeSearchNet),
while retaining TREC's incomplete-pool limitation.


## Execution ownership

Human review and a complete five-product blind pool remain unqualified.
Existing automated labels and prepared human forms are not human judgments.
Current cross-ticket execution is owned once by the
[OCT-04 residual ledger](../../oct-4-parallel-closure/tickets/INDEX.md).
Retain the acceptance above for any new claim; reuse compatible captures.
Past counts, binaries, failures and commands are recoverable from [historical bodies](../../ARCHIVE-INDEX.md#oct-05-benchmark-and-quality-ledger-compaction).
They do not qualify current source.
