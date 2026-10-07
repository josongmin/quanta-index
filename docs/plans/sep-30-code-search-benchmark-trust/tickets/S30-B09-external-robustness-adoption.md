# S30-B09 — External benchmark adoption and robustness diagnostics

Status: `ACTIVE_RESIDUAL`; existing executions are diagnostic.
Parent: [benchmark plan](../README.md). Owners: existing retrieval intake,
source oracle, runner and evaluator; B03/B05/B08 supply mechanical/statistical
and holdout acceptance. Completed intake/planner/parser contracts are in
[OCT-05-001](../../../adr/OCT-05-001-review-admission-and-result-identity.md);
ranking and experiment boundaries are in
[OCT-05-004](../../../adr/OCT-05-004-cost-capacity-and-qualification-boundaries.md).

## Adoption scope

| Source | Scope retained | Boundary |
| --- | --- | --- |
| [CodeSearchNet judgments](https://github.com/github/CodeSearchNet/tree/106e827405c968597da938f6b373d30183918869) | Commit/CSV-bound natural-language review seed with original repeated grades, fractional means and disagreements | Materialize source snippets/hashes and license approval before capture. Upstream annotators do not invent local reviewer provenance; outside-pool files remain unjudged. |
| [CLARC](https://huggingface.co/datasets/ClarcTeam/CLARC) | Pinned group1 original and neutral-renamed C/C++ snippet lanes | Preserve their versioned actual query admission and shared/new/excluded populations. Positive-only labels are not exhaustive relevance; the local subset is not the whole upstream benchmark. |
| [Memtrace methodology](https://github.com/syncable-dev/memtrace-public/blob/3a3084dd88e4b49adcb185eb09173dbf0bd547d3/benchmarks/fair/run_hybrid_retrieval_benchmark.py) | Independently implemented literal-containment and surviving-identifier-component diagnostics on local source/queries | No copied runner/dataset; retain the pinned [license boundary](https://github.com/syncable-dev/memtrace-public/blob/3a3084dd88e4b49adcb185eb09173dbf0bd547d3/LICENSE). These input properties do not identify an engine's ranking mechanism. |
| [COBE](https://ieeexplore.ieee.org/document/9892610/) | Paired clean/noisy natural-language methodology | It supplies no identifier-typo gold. |
| [CoIR](https://github.com/CoIR-team/coir), [CORE-Bench](https://arxiv.org/abs/2606.11864), [CodeScaleBench](https://github.com/sourcegraph/CodeScaleBench) | Separate document/repository/workflow follow-up | Whole imports/runs are unexecuted here; their units and agent outcomes cannot be merged with identifier navigation. |
| [GitHub Typo Corpus](https://github.com/mhagiwara/github-typo-corpus) | Deferred input adoption | The earlier audit could not retrieve the official archive. Revalidate actual availability/license before use; do not synthesize a replacement or treat that historical observation as current availability. |

These are public, exposed inputs. Import or source replay cannot make them an
unseen holdout or qualified comparison.

## Remaining acceptance

- Materialize and admit the actual source, license, qrels, native request/profile
  and rank unit for each selected external lane. Keep CodeSearchNet language,
  CLARC original/neutral, shared/new queries and exclusions separate. Preserve
  fractional qrel policy rather than rounding into integer-grade authority.
- Use the current [OCT-04 E1/E2 ledger](../../oct-4-parallel-closure/tickets/INDEX.md)
  for final native outcomes, supplemental judgments, admissions and common
  eligibility. Original eight-repository and global twelve-repository cohorts
  retain their own source/gold/task populations; neither fills a different B08
  mixed-track matrix. Existing valid captures need only affected-source rechecks.
- Review ambiguous intended-navigation labels and actual local relevance.
  Declaration-position recovery requires a symbol/name/span-capable response;
  a file hit or nearby use does not establish it. Keep default literal-first,
  explicit OSA1 and stateful Fuzzy Finder request contracts separate.
- Whole-upstream corpus, human qualification and genuinely unused holdout remain
  separate data gates. Apply repository/family inference only after comparison
  admission. Any ranking selection also requires predeclared critical-stratum
  and resource acceptance; public mechanical gold is not general content gold.
- Qualified performance needs [B07](S30-B07-performance-and-indexing.md), actual
  matched output/index boundaries and an admitted host. Semantic/hybrid chunk
  controls do not establish a distinct-file semantic comparison.

## Reusable owner verification

Keep fixed independent positives/negatives for pinned CSV/input refusal,
grade averaging/disagreement, normalization collisions, malformed source URLs,
output isolation, literal/component metadata and historical census replay.
Reaggregation must preserve every original judgment identity and fractional mean.
Fresh planner requests bind the selected Unicode/profile identity; active parser
source cannot differ from the loaded grammar. Native response and source replay
must agree before admitting metrics. Tests are registered through the existing
test authority and source-closure owners.

Completed intake, global-cohort diagnostics, source-locked runtime repairs and
their exact commands/results are recoverable through
[the history index](../../../ARCHIVE-INDEX.md#historical-record-recovery).
No old snapshot/test total is a current-source qualification verdict.
