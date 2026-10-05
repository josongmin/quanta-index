# CS-BENCH-03 — Task tracks, metric units and statistical admission

Status: acceptance **OPEN**. Existing metric helpers and separate timing-layer
labels are present. The qualified verdict now independently resamples whole
query families within category, requires at least 20 independent families and
at least two per category, and refuses correlated-task pseudoreplication.
Independently admitted task-track/holdout execution is **NOT_RUN** for this
acceptance scope.
Category: benchmark evaluation. Findings: F04/F09; depends on BENCH-01/02.

Current `lexical_file_comparison` reports hit rate and macro file recall
separately. That scorer alone cannot attest native agreement, independent gold
or equivalent work. Close BENCH-02 before qualification scoring; then run
the declared track units, statistical admission and ablation. Latest boundary:
[CS-INT-01](CS-INT-01-integration-and-qualification.md#required-controls).

## Purpose

Expose developer-visible outcomes and engine diagnostics without mixing units
or turning an observed native order into a relevance ranking. A chunk top-10, ten
distinct files, a definition and a returned context window are not interchangeable.
Independent labels, declared units and equivalent work are required before
comparative quality/statistical admission.

Keep metric mathematics in the existing
[retrieval evaluator](../../../../tools/benchmark/retrieval/evaluator.py) and
[lexical comparator](../../../../tools/benchmark/retrieval/lexical_file_comparison.py).
The [registry](../../../../tools/benchmark/registry.toml) registers their profiles;
shared evidence does not become an alternative domain scorer.

## Two comparison modes

- **Matched semantics:** same literal/regex/case/normalization/path scope, admitted inventory,
  output unit, limits and completion contract on the supported intersection.
- **Native workflow:** each product's documented user-facing mode. Report its
  semantics, ranking/order, setup and exclusions; do not call hybrid versus lexical
  a matched lexical comparison or use capability differences as silent failures.

Freeze mode before capture. Query adapters retain original and submitted request
identities. No gold-informed query rewrite or post-result route selection.

## Track contracts

| Track | Developer-visible outcome | Core metrics | Required authority |
| --- | --- | --- | --- |
| Exact lexical conformance | Actual requested bytes/regex matches are found | Match precision/recall, exact-exhaustion and error rate | Independent complete match set |
| File locator | Useful file appears early | File Hit@1/5/10, Recall@k; file MRR only for ranked output | Unique repo/revision/path qrels |
| Definition locator | Correct declaration appears early | Declaration Hit/Recall@k, MRR; NDCG only with admitted grades | Independent declaration alternatives |
| Context delivery | Returned bounded context covers required evidence | Byte-span coverage, BCY by token budget, bytes/tokens and clipping | Source-bound returned spans and required-block labels |
| Updates/operations | New source is searchable consistently | Visibility lag, stale-hit rate, recovery and amplification | Ordered mutation and query/activation observations |

No-answer and unsupported/incomplete scopes are distinct. Require zero known
semantic mismatches on the finite exact-conformance suite; that is not a proof of
perfect recall on all future repositories. Speed is a separate qualified dimension.

For unranked products, report set recall and explicitly named observed-position
metrics if useful. Do not include stream order or map iteration in a ranked MRR
leaderboard without an attested relevance-order contract. Ranks and top-k count
after the declared native/grouping transformation, never after gold filtering.

## Accounting invariants

- Report query count, eligible count, attempted/completed count, unsupported,
  incomplete, error and timeout counts by product/track/stratum. No missing row
  becomes zero hits or drops silently from a denominator.
- Report both common eligible intersection quality and full requested capability
  coverage. Failures on supported required tasks count against completion; invalid
  captures block quality qualification rather than becoming successful empty sets.
- Single-file hit and multi-file recall are different. Alternative definitions
  count as legitimate answers; jointly required context blocks are not alternatives.
- Deduplicate relevance credit per gold identity. Overlapping chunks cannot
  multiply NDCG gain or coverage; union source byte ranges before counting bytes.
- Indexed-hit metrics, selected-focus coverage and returned-context coverage are
  separate. ENG-04 preview expansion cannot improve indexed-hit MRR retroactively.
- Missing reviewed grades makes NDCG inapplicable. Unjudged pooled results need
  a declared treatment/sensitivity analysis, not automatic irrelevance.
- Preserve no-answer behavior even when an index is empty; require complete scope
  evidence before crediting correct abstention.

## Statistics and default admission

Report micro results and macro aggregates by repository and query family. Use
paired per-query deltas on identical tasks, plus wins/losses/ties and stratum counts.
Bootstrap at the repository/query-family clustering level consistent with the
sampling design; many copied queries are not independent samples. When independent
clusters are too few, report descriptive intervals/limitations, not significance.
The current single-repository qualified gate implements the within-repository
query-family boundary. It does not establish inference across repositories;
multi-repository macro inference still needs independently admitted repos and a
repository-level procedure. The task-level interval remains descriptive and
cannot alone pass `QUALITY_DELTA`.

In `retrieval/run.py`, `QUALITY_DELTA=pass` currently means the matched,
blinded/graded evidence and uncertainty are admissible. The gate does not test
whether the candidate delta is positive or meets a minimum useful effect.
Default admission therefore needs a separate decision against predeclared
effect, critical-stratum regression and resource limits. Negative or zero
qualified deltas must not be described as a product win because this gate passes.
The separate `tools/benchmark/retrieval/decision.py` gate requires the exact
policy SHA-256 in the pre-capture qualification admission manifest, replays the
qualified verdict, binds the selected scored report, and compares the primary
delta, query-family cluster lower bound, declared critical strata, and captured
candidate p95/RSS/index bytes with the frozen policy. Missing policy identity,
qualified proof or observed dimension refuses; no numeric defaults are supplied.
Existing admission manifests without `decision_policy_sha256` remain valid
evidence but cannot admit a product default.

The within-repository family gate has fixture coverage, but tests do not
establish independent labels or a qualified benchmark run. Historical local
test counts are recoverable through the [plan archive](../../ARCHIVE-INDEX.md).

Before tuning or opening holdout, freeze: primary metric/track, minimum useful
effect, tolerated regressions per critical stratum, latency/memory/index-cost
budgets, confidence procedure and the finite ablation matrix. Numeric limits are
chosen from baseline variance and product requirements, recorded as acceptance
inputs; missing limits block default admission, not unit-test development.

Do not cherry-pick the winning k, subgroup or repetition. Report secondary metrics
as secondary; account for multiple policy comparisons in the declared selection
procedure. A failed holdout does not become a new tuning split while retaining
its name. P99 from twenty queries is not a defensible tail qualification.

Use an independent standard implementation such as pinned `trec_eval` for shared
rank metrics on small qrel fixtures, and hand-computed sets/ranges for coverage.
Do not compare incompatible metric definitions under identical column names.

Current-main adversarial check: historical `exact_symbol_name` records may omit
`rank_unit` and may collapse distinct declarations sharing a returned context
line. The independent declaration diagnostic now requires an explicit recorded
`rank_unit: symbol`; a rankless legacy record remains readable but is excluded
from that metric with `rank_unit_mismatch`. This closes policy-only rank-unit
promotion, not BENCH-01 independent labels or BENCH-02 native capture authority.

## DoD

- [ ] Each registered profile declares track, semantics, unit, ordering, cutoff,
  exclusions, completeness and metric eligibility before capture.
- [ ] Independent tiny fixtures verify ranking, alternative/required gold,
  overlap, no-answer and malformed/incomplete denominators.
- [ ] Reports expose raw numerator/denominator and per-stratum query identities;
  unsupported products/tasks cannot improve an aggregate by disappearing.
- [ ] Development ablation isolates grouping, name/case ranking and presentation.
- [ ] Primary metrics, effect/regression limits and resource budgets are frozen
  before a fresh holdout; source-bound results determine default admission. A
  qualified negative/zero-effect control refuses a claimed product improvement.
- [ ] Research context/agent tracks are labeled separately and reuse corpus/control
  owners without becoming prerequisites for core lexical conformance.

Research [R01–R04](../references.md) informs additional task types, not a universal
lexical benchmark. Established qrel/metric precedent: [S07](../references.md).
