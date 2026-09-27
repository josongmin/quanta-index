# CS-BENCH-03 — Task tracks, metric units and statistical admission

Status: acceptance **OPEN**. Existing metric helpers and separate timing-layer
labels are present; independently admitted task-track/statistical/holdout
execution is **NOT_RUN** in the current remaining-work audit.
Category: benchmark evaluation. Findings: F04/F09; depends on BENCH-01/02.

Current `lexical_file_comparison` reports hit rate and macro file recall
separately. This does not establish native-derived result authority, independent
gold or equivalent work. Close BENCH-02 before qualification scoring; then run
the declared track units, statistical admission and ablation. Latest boundary:
[CS-INT-01](CS-INT-01-integration-and-qualification.md#current-remaining-work-audit--2026-09-27).

## Purpose

Expose developer-visible outcomes and engine diagnostics without mixing units
or turning an observed native order into a relevance ranking. A chunk top-10, ten
distinct files, a definition and a returned context window are not interchangeable.
The historical 200-query run is diagnostic; corrected independent labels and
equivalent work are needed for stronger comparative claims.

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

## DoD

- [ ] Each registered profile declares track, semantics, unit, ordering, cutoff,
  exclusions, completeness and metric eligibility before capture.
- [ ] Independent tiny fixtures verify ranking, alternative/required gold,
  overlap, no-answer and malformed/incomplete denominators.
- [ ] Reports expose raw numerator/denominator and per-stratum query identities;
  unsupported products/tasks cannot improve an aggregate by disappearing.
- [ ] Development ablation isolates grouping, name/case ranking and presentation.
- [ ] Primary metrics, effect/regression limits and resource budgets are frozen
  before a fresh holdout; source-bound results determine default admission.
- [ ] Research context/agent tracks are labeled separately and reuse corpus/control
  owners without becoming prerequisites for core lexical conformance.

Research [R01–R04](../references.md) informs additional task types, not a universal
lexical benchmark. Established qrel/metric precedent: [S07](../references.md).
