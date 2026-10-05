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

## Owners and regressions

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

## Consequences

Owner regressions establish binding/refusal behavior. They do not complete model
judgment, final qrels, human review, unseen holdout or product quality. Exact old
commands and execution bodies remain in the [plan history index](../plans/ARCHIVE-INDEX.md#oct-05-handoff-and-ticket-compaction).
