# J7Q-01 — Relevance and external lexical comparison

Status: `ACTIVE_RESIDUAL`. Parent: [quality index](INDEX.md).
Owner: lexical/symbol/structural/history ranking and benchmark evaluator.
Implemented source/ranking/Explain diagnostic boundaries are in
[SEP-27-003](../../../adr/SEP-27-003-code-search-source-and-preview-contract.md)
and [OCT-05-004](../../../adr/OCT-05-004-cost-capacity-and-qualification-boundaries.md).

## Remaining acceptance

- Independently judge stable query IDs and graded intent labels, near-duplicate
  distractors and hard negatives. Default content/use-example relevance and typed
  declaration identity/span tasks stay separate. Keep test/generated declarations
  eligible when relevant; blanket file penalties and deterministic tie breaks do
  not establish relevance.
- Measure lexical, symbol, structural and history-backed families independently:
  per-query rankings, top-1, MRR/NDCG/Recall, top-k and forbidden high ranks.
  Preserve route/macro denominators and critical-stratum regressions. A blended
  score cannot hide a failing route; seeded fixture floors are not external gold.
- Execute real Sourcegraph overlap for shipped keyword/phrase/regex/constrained
  content/repo-metadata/symbol-name surfaces. Bind source/revision, effective
  query/case/scope, served index/version, native order and gaps. Backend Zoekt
  scores and Sourcegraph final ordering have distinct owners.
- Verify explicit symbol/name lookup through a current source-bound consumer and
  reviewed declaration identities. Do not infer bare-query routing, declaration
  recovery or exhaustive relevance from file-only top-ten diagnostics.
- If a new production declaration policy is selected, batch enrichment once per
  request. Preserve unknown coverage, raw name/definition identity, literal recall,
  typed errors, resource accounting, deterministic order and scoring cursors.
  Current per-file diagnostic extraction is not an unused production ranker.
- Choose defaults only after repository/family splits frozen before variants,
  independently judged development tasks, unused holdout, and prespecified useful
  effect/critical-stratum/resource limits. Preserve actual eligible counts and
  underfill for existing 1,000+ mechanical-family targets. Match latency/memory
  boundaries on an admitted host; public code analysis is not policy qualification.

## Diagnosis and proof boundaries

Classify an observed miss from source/native facts: absent source/candidate,
verification rejection, ranking, clipping or execution limit. Reproduce the
first native page before cursor continuation. A top-ten-only capture has no
complete rank; file hit, source name/span and chunk context are different units.
Use independent full-scan/full-sort/OSA oracles, late winners, ties, path renames,
repetition, two declarations, relevant test/generated files, usage intent,
Unicode/case, all pages, budget/cancel and source/preview goldens.

The diagnostic rank study retains complete-pool baseline/ablation comparisons,
optional refusals and exclusions; it has not selected a new ordinary-file default.
Do not infer production quality from experimental weights or old Gin hit counts.

## Execution owners

Output owner: existing `relevance_matrix` under `quality-core/quality-full`.
`summary.json`, `query_judgments.json` and `sourcegraph-overlap.json` require
current artifact admission. Independent corpus/gold and native response acceptance
are in [CS-BENCH-01](../../sep-27-code-search-remediation/rfcs/CS-BENCH-01-corpus-gold-and-holdout.md)
and [CS-BENCH-02](../../sep-27-code-search-remediation/rfcs/CS-BENCH-02-native-response-validation.md).
Actual current review/capture/reporting and conditional ranking work are owned
once by [OCT-04 E1/E2/E4](../../oct-4-parallel-closure/tickets/INDEX.md).

Historical ranking RCA, Zoekt/Blackbird research, completed Explain repairs and
their exact native/local results are recoverable through
[the plan archive](../../../ARCHIVE-INDEX.md#historical-record-recovery).
They do not qualify semantic/hybrid, a product default or current release.
