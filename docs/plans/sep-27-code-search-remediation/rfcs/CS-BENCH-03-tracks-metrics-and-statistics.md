# CS-BENCH-03 — Track, unit and statistical acceptance

Status: `ACTIVE_RESIDUAL` for admitted track/holdout execution and policy choice.
Metric/evidence/family-gate decisions are owned by
[SEP-26-003](../../../adr/SEP-26-003-retrieval-evidence-custody-and-qualification.md)
and [OCT-05-001](../../../adr/OCT-05-001-review-admission-and-result-identity.md).
Current qrels/common eligibility/reports are owned by
[OCT-04 E1](../../oct-4-parallel-closure/tickets/INDEX.md#e1).

## Comparison and track contract

Freeze matched semantics or native workflow before capture. Matched semantics
uses the same grammar/case/normalization/path/inventory/unit/limit/completion
contract on the supported intersection. Native workflow retains each product's
actual user mode/setup/order/exclusions. Bind original/submitted requests;
gold-informed rewrites and post-result routing cannot enter the comparison.

| Track | Metrics | Required independent authority |
| --- | --- | --- |
| Exact lexical conformance | Match precision/recall, exact exhaustion, errors | Complete declared-semantics match set; zero known semantic mismatches on the finite suite |
| File locator | Hit@1/5/10, Recall@k, ranked-only MRR | Repository/revision/path qrels |
| Definition locator | Declaration Hit/Recall/MRR, graded-only NDCG | All legitimate declaration/name/span alternatives |
| Context delivery | Required-span coverage, BCY by budget, clipping/bytes/tokens | Source-returned spans and jointly required blocks |
| Updates/operations | Visibility lag/stale hits/recovery/amplification | Ordered mutation, activation and query observations |

Use existing [evaluator](../../../../tools/benchmark/retrieval/evaluator.py),
[lexical comparator](../../../../tools/benchmark/retrieval/lexical_file_comparison.py)
and registry; no alternative scorer. Unranked products get set recall or named
observed-position diagnostics. Chunk top-ten, distinct files, indexed declarations,
selected focus and returned context keep separate units; ranks are assigned after
the declared native transformation and before any gold filtering.

## Required accounting and fixtures

- Report requested/eligible/attempted/completed/unsupported/incomplete/error/
  timeout/unjudged populations by product/track/stratum. Give common-eligible
  quality and full capability/operational coverage; invalid captures block quality
  rather than entering as empty successes or disappearing from denominators.
- Keep any-file hit, all-file recall, legitimate alternative declarations and
  jointly required context distinct. Credit each gold identity once; union source
  ranges before byte coverage. Duplicate/overlapping chunks cannot multiply gain.
  Preview expansion cannot improve indexed-hit metrics retroactively.
- Reviewed grades are required by graded metrics. Declare unjudged-pool policy/
  sensitivity; missing judgments cannot become irrelevant. Correct abstention
  needs complete no-answer scope, including the empty-index case.
- Retain independent tiny fixtures for ranked/unranked output, alternative versus
  required labels, overlap, legitimate zero results, malformed/incomplete statuses
  and same-line declarations. An explicit native symbol/name unit is required for
  declaration credit; rankless legacy rows stay readable and excluded by reason.
- Emit raw numerators/denominators and query identities. Separate grouping,
  name/case ranking and presentation ablations. Research context/agent tracks
  retain their own population and do not become universal lexical prerequisites.

## Statistical and default-decision acceptance

Report paired wins/losses/ties and micro/repository/family macro results on the
same eligible tasks. Resample units under the frozen sampling design. Existing
single-repository qualification resamples whole families within category with
at least twenty independent families and two per category; task-level intervals
remain descriptive. Cross-repository inference needs admitted repositories and
repository-level clustering. Copied queries are not independent observations.

`QUALITY_DELTA=pass` admits matched/blinded/graded evidence and uncertainty.
Default selection additionally requires [decision.py](../../../../tools/benchmark/retrieval/decision.py)
with the exact pre-capture policy SHA, qualified report and actual primary effect,
cluster lower bound, critical-stratum regressions and p95/RSS/index-cost inputs.
Missing inputs refuse; zero/negative or insufficient useful effect cannot issue
a win. An evidence-valid manifest lacking the decision policy stays insufficient
for default admission.

Before tuning/holdout access, freeze metric/track, useful effect, regression/resource
ceilings, confidence procedure and finite ablations from baseline variance/product
needs. Account for multiple policy comparisons; no favorable-k/subgroup/repetition
selection. With too few independent clusters, report descriptive limits. Twenty
queries cannot qualify p99. Compare shared rank math with pinned `trec_eval` and
hand-computed set/range fixtures, retaining metric-definition identity.

[BENCH-01/02](CS-BENCH-01-corpus-gold-and-holdout.md) supply independent/native
inputs; [CS-INT-01](CS-INT-01-integration-and-qualification.md#required-controls)
owns integration. Historical local observations are recoverable through
[the history index](../../ARCHIVE-INDEX.md#oct-05-residual-owner-clarification).
