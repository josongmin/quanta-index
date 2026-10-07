# OCT-05-001 — Review, Admission and Result Identity

Status: `Accepted`

Decided: 2026-10-05

Consolidates implemented O4-E1 contracts. It preserves
[SEP-26-001](SEP-26-001-retrieval-query-publication-and-result-proof.md),
[SEP-26-003](SEP-26-003-retrieval-evidence-custody-and-qualification.md) and
[SEP-27-003](SEP-27-003-code-search-source-and-preview-contract.md).
It adds no new public API, metric, relevance policy or provenance claim.
Unfinished execution lives in the [OCT-04 residual ledger](../plans/oct-4-parallel-closure/tickets/INDEX.md#e1).

## Context

Frozen review forms and derived reports previously compared JSON through Python
equality, which aliases boolean, integer and floating values. Returning native
files, preparing blind forms and issuing final labels also represent different
authority boundaries. File recovery does not establish declaration recovery or
complete relevance outside the reviewed population.

## Decision

1. Compare frozen form/task/file bindings and derived report/verdict projections
   through canonical typed JSON bytes. Preserve exact field sets, source text,
   query/rubric, form slot and answerability threshold. `true`, `1` and `1.0`
   cannot substitute for one another. Validate intrinsic field shape as well.
2. Two reviewer executions and an adjudicator execution retain distinct role/run
   identities, model revision/settings, actual request/result and terminal facts.
   Self-reported IDs or process liveness cannot issue judgment authority. This
   does not require three different model products. AI execution remains AI;
   human provenance requires separate actual evidence.
3. Form the supplemental population from replayed native returned-file unions,
   subtract already judged pairs, and hide product/rank/score from reviewer input.
   Reuse valid prior judgment only with its original source/query/rubric binding.
   Unknown, ambiguous, failed and excluded pairs remain explicit; never infer
   grade zero or corpus-wide no-answer from absence in a pool.
4. One canonical suite/pack/split/license/review/proof authority issues each
   repository admission. Changed final labels produce a new admission revision.
   A scoring projection may reuse raw only when the native request/source/unit/
   profile/result binding permits it; never overwrite historical native identity.
5. Name recovery uses independently sourced declaration ID/name bytes/spans and
   the selected native unit. File hit, use-only match, nearby declaration,
   enlarged context or wrong case cannot count as declaration recovery.
   Unsupported units have an explicit population and reason.
6. Source freezing and source split validation do not establish unseen relevance.
   Query/family/exposure, license approval, gold and acceptance criteria remain
   separate inputs. Development and previously exposed corpora cannot become
   holdout through renaming. Preserve ambiguous, excluded and underfilled strata.
7. Bootstrap/cache reuse preserves the declared method, seed, draw ordering,
   reduction and strata, with bounded memory and independent reference parity.
   Further vectorization requires a demonstrated whole-caller bottleneck.

## External input, planner and parser identity

- CodeSearchNet intake binds the exact upstream commit/CSV and retains every
  annotator row, grade/note, disagreement and fractional mean. Its lowercase
  grouping convention cannot silently merge normalization collisions. Outside-pool
  files stay unjudged; intake does not issue source verification or local review.
- Fresh NL planning emits normalized scored keyword OR terms under its explicit
  profile; match-only phrase requests retain their distinct contract. Rust and
  Python request identities are independently derived using pinned Unicode 17
  default lowercase data, rather than host Python Unicode tables. The actual
  versioned token/profile admission preserves shared/new/excluded queries.
- Gold capture/replay package identities come from the producer's source-locked
  runtime. Bind loaded vendored grammar generation to source provenance; a source
  change requires a fresh producer process. Cache reuse binds source bytes/digest
  and parser identity and rechecks each original suite's universe/gold.
- Identifier containment and surviving-component diagnostics describe frozen
  query/name bytes under the oracle tokenizer. They do not identify the engine's
  tokenizer or explain a hit. Complete intended-name file gold and representative
  intended-original file recovery retain separate metrics and source authority;
  neither becomes declaration-span or general content relevance.

Owners: [external qrel intake](../../tools/benchmark/retrieval/codesearchnet_qrels.py),
[query planner](../../tools/benchmark/retrieval/query_plan.py),
[runtime identity](../../tools/benchmark/retrieval/retrieval_contract.py),
[declaration parser](../../tools/benchmark/retrieval/declaration_parsers.py),
[robustness report](../../tools/benchmark/retrieval/identifier_robustness_report.py).
Retain pinned-input/grade/normalization/refusal fixtures and loaded-grammar/source
mutants; historical request identities are replayed under their original profile.

## Review completion and diagnostic units

- `complete_ranked_pool_v1` requires source-bound grades 0–3 for every ranked
  top-10 file/declaration. Missing judgments exclude with `unjudged_ranked_file`
  or `unjudged_ranked_declaration`; operational mean is `not_applicable` with
  `incomplete_ranked_judgments`, while eligible conditional means and observed
  execution-error/timeout penalties retain their separate denominators.
  Historical `unjudged_zero_v1` remains exploratory, not qualified holdout.
- `answerability_min_grade` binds review context and task (default 1). A selected
  threshold 2 permits grade-1 partial clues on an unanswerable task, with no gold
  or grade >=2; answerable tasks need sufficient-answer judgment/gold. This does
  not change graded NDCG or positive-relevance Hit/MRR (grade >0). Keep thresholds
  out of blind packs and revalidate originals/final labels before issuance.
- Native file/symbol/chunk units, effective query intent, indexed source witness,
  score/order and projection are independently bound. Path-ordered constant-score
  prefixes are not relevance rankings. Scored SDK path ties and Semble native
  BM25 ties remain distinct; old records without required explicit rank/score
  evidence cannot inherit newer eligibility. Same-line declarations retain
  independent indexed identity even when their returned context span is shared.
- Objective source oracles exhaustively recompute the declared frozen universe
  and positive grade-3 set. Mechanical labels are not human relevance. Declaration
  absence, folded content absence, content/path absence and content-identifier
  OSA1 absence are distinct negative authorities. A file label cannot establish
  declaration recovery, every returned file's relevance or corpus-wide no-answer.
- Review finalization/supplemental preflight binds every original query/source/
  threshold, rejects supplied decisions and already judged/duplicate pairs,
  preserves ambiguity and issues a new suite after label changes. Actual diverse
  pool/reviewer/adjudicator execution and independent human provenance stay
  external inputs. A subset cannot decide overall answerability.
- External snippet diagnostics retain complete input/admission/source-blocked/
  execution populations. CLARC positive targets do not issue exhaustive negatives;
  CodeSearchNet fractional pool estimates and official judged-only diagnostics
  keep separate metrics. Synthetic snippets do not attest full upstream corpora,
  licensing, enforced isolation or served/indexed universe.

## Qualified review receipt shapes

For `qualified`, the license receipt must be JSON with exactly
`schema_version: 1`, `reviewer_id`, `decision: approved`, `repository_commit`,
`corpus_manifest_sha256`, and a nonempty `rationale`. The reviewer and corpus
identity must match the admission manifest. This verifies the recorded decision
and scope, not the legal correctness of the review.

Each annotation receipt must be JSON with exactly
`schema_version: 1`, `reviewer_id`, `suite_sha256`, and `reviews`. `reviews`
must contain one row per suite task in suite order. Each row has exactly
`task_id`, `query_sha256`, `labels`, and a nonempty `rationale`. `labels`
contains `answerable` and `gold`, plus the same optional scoring-label keys
present on that suite task: `query_intent`, `judgment_policy`, `file_judgments`,
and `declaration_judgments`. The adjudication receipt has the same shape plus
`annotation_receipt_sha256`, the ordered hashes of both annotation receipts;
its labels must equal the final suite labels. Each annotation's proposed labels
are independently validated against the pinned source. The manifest must name
three distinct reviewer IDs. Hashes and IDs establish content and claimed
custody, but cannot establish that three humans actually reviewed independently;
that remains an external qualification check.

For a repository-disjoint admission (schema v3) whose suite mixes
`source_oracle` and reviewed tasks, use annotation and adjudication receipts
with `schema_version: 2`. Their `reviews` contain only the tasks without
`source_oracle`, in suite order. All mechanical tasks remain bound to the full
`suite_sha256` and are recomputed against the pinned source by suite validation.
The two annotation receipts must cover every reviewed task; adjudication must
match the final suite labels. A schema-v1 receipt for a mixed suite is refused.

## Owners and regressions

Blind-pool preparation, frozen form validation and three-role file-label
finalization are implemented in `holdout_review.py`. `run.py` validates admission
receipts at execution/replay entrypoints; `corpus_binding.py` checks actual
development/holdout source overlap. These programs cannot supply missing actual
reviewer judgments, license approval or independent gold/acceptance inputs.

`evaluator.py::mean_ci` validates finite deltas and unique task identities before
pure numeric reuse. It performs 10,000 seeded within-stratum draws, keeps at most
16 cached keys, and bypasses caching above 256 KiB canonical key bytes. Constant
strata preserve the original addition and percentile-interpolation order.
[Frozen bootstrap regressions](../../tools/ci/tests/test_bootstrap_cache.py)
retain pre-optimization goldens, ordering/signed-zero/identity controls and
cache bounds. They do not establish whole-caller speedup; any further
optimization remains conditional on actual paired-call cost and parity.

- [Review producer/issuer](../../tools/benchmark/retrieval/holdout_review.py),
  [corpus/split binding](../../tools/benchmark/corpus_binding.py),
  [source oracle](../../tools/benchmark/retrieval/source_oracle.py).
- [Name records](../../benchmarks/retrieval/src/symbols.rs),
  [five-product scorer](../../tools/benchmark/retrieval/lexical_file_comparison.py),
  [evaluation](../../tools/benchmark/retrieval/evaluator.py).
- Keep independent boolean/numeric binding mutants, wrong source/query/threshold,
  duplicate roles/pairs, partial execution and wrong name/unit/case controls in
  [review tests](../../tools/ci/tests/test_holdout_review.py),
  [native span tests](../../tools/ci/tests/test_retrieval_native_span_projection.py)
  and [source oracle tests](../../tools/ci/tests/test_source_oracle_suite.py).

## Corpus, gold and holdout acceptance

These are standing admission boundaries consolidated from CS-BENCH-01/03 and
S30-B01/02/03/05/06/08/09, not newly completed datasets or product gates.

- Freeze licensed repository commits, complete source/file hashes, parser identity,
  query grammar/case, task/family IDs, rank unit and gold provenance before capture.
  A parser failure is not absence. Exhaustive mechanical gold retains every valid
  alternative; relevant, secondary and seed-related labels are not interchangeable.
- Proper prefix/infix/component and one-edit identifier tasks preserve the declared
  tokenizer, insertion/deletion/substitution/transposition and case policy. Keep
  ambiguity, exact-name collisions and all nearest alternatives explicit. No-answer
  requires an exhaustive oracle for its selected content/path/declaration domain.
  Original queries and variants stay in one statistical family.
- Freeze development/holdout repository, query and family separation before exposure.
  Prespecify candidate/reserve rosters and small/medium/large language strata; reject
  forks, near copies and source/gold leakage. Underfilled strata stay underfilled.
  Public source is not necessarily unseen to a pretrained model. Retire holdout
  independence after using it to select policy. Historical 12-repository/1,200-task
  ambitions are engineering proposals, not admitted power or quality thresholds.
- Natural-language gold needs actual blind reviewer and adjudicator execution plus
  source-bound raw judgments. Product/rank/score remain hidden. Report pool omissions
  and leave-one-system-out sensitivity; a diverse pool is not exhaustive relevance.
- Gin exact-name/variant, Semble Gin20 and ARB remain exposed case-series/workflow
  populations. ARB uses each case's official pre-fix base and complete file universe;
  original versus adapted requests, gold-bearing versus no-gold tasks and context
  token budgets remain separate. Anchors or post-fix source cannot leak into queries.
- External adoption binds actual materialized source, pinned upstream qrels/license
  and local unit/admission. CodeSearchNet fractional means and CLARC original/neutral
  shared/new/excluded populations retain their policies. Methodology references do
  not import whole datasets or runners. Missing official inputs cannot be replaced
  with fabricated data. Public external data cannot fill unused holdout.

## Statistical units and default decisions

- Exact conformance, ranked file/declaration relevance, unranked set recall, context
  delivery and updates have separate scoreboards. Assign original ranks before gold
  filtering. Preserve all requested/eligible/attempted/completed/unsupported/error/
  timeout/unjudged/excluded denominators and raw numerators.
- Compare paired common-eligible units with independent hand-computed set/range
  fixtures and pinned reference metric implementations. Query variants cluster by
  family; multi-repository inference clusters by repository. Small single-repository
  or Gin20 samples retain descriptive limits and cannot establish p99 or generality.
- Before tuning or holdout access, freeze primary track/metric, useful effect,
  critical-stratum regression bounds, resource ceilings, uncertainty and finite
  ablations. Account for multiple comparisons; do not select favorable k, subgroup
  or repetition after observing results.
- PAIR_VALID and QUALITY_DELTA admit evidence. Default selection additionally uses
  the existing decision.py with its exact pre-capture policy and qualified report:
  actual primary effect, cluster lower bound, critical strata and p95/RSS/index
  cost. Missing or insufficient useful effect cannot issue a win. Semantic quality,
  ANN recall and encoder cost retain independent controls under SEP-26-002.

Owners remain the existing source oracle, evaluator, review/admission, corpus
binding and decision modules; no second scoring or labeling stack is introduced.
Current issuance and execution are tracked once in the residual ledger.

## Consequences

Owner regressions establish binding/refusal behavior. They do not complete model
judgment, final qrels, human review, unseen holdout or product quality. Exact old
commands and execution bodies remain in the [plan history index](../ARCHIVE-INDEX.md#historical-record-recovery).
