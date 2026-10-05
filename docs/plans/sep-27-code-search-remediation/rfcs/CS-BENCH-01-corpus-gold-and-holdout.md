# CS-BENCH-01 — Independent corpus, gold and holdout acceptance

Status: `ACTIVE_RESIDUAL`. Owners: existing corpus release/binding, independent
oracles and review issuer. Completed preparation/parsing/admission contracts are in
[OCT-05-001](../../../adr/OCT-05-001-review-admission-and-result-identity.md).
Current inputs/judgments/admissions are owned by
[OCT-04 E1](../../oct-4-parallel-closure/tickets/INDEX.md#e1);
[CS-INT-01](CS-INT-01-integration-and-qualification.md#required-controls) owns
integration. Prepared source, mechanical gold and AI diagnostics retain their
own scope until the requested independent release is admitted.

## Release acceptance

- Use existing [corpus release](../../../../tools/benchmark/corpus_release.py),
  [binding](../../../../tools/benchmark/corpus_binding.py) and
  [candidate census](../../../../tools/benchmark/retrieval/corpus_set.py) owners.
  Reusable schemas, pinned acquisition/recipe/oracle adapters, small fixtures,
  license references and split policy belong in the repository. Materialized
  corpora/gold/models/indexes/raw releases belong outside it. Canonical execution
  cannot depend on an unretained temporary recipe or manually synchronized manifest.
- Bind URL/commit, complete file/hash inventory, language/encoding, view/selection,
  symlink/submodule/binary/generated/vendor policy, size bounds and provenance.
  Comparators prove actual indexed content or report exclusions; matching Git
  revision/file counts alone cannot establish equivalent scope.
- Source/labels/recipe/oracle/parser/split changes issue new immutable identities.
  Preserve original evidence bytes and machine-readable change/exclusion reasons.
  Old scores cannot acquire corrected labels by relabeling a report.

## Task and independent gold acceptance

Every task binds stable ID/release, intent, original query, grammar/regex engine,
case/normalization, scope, unit and label provenance. Runner packs omit labels
and answerability. Native NFC/token/case semantics and original-byte semantics
keep separate cohorts unless a reference proves the chosen common contract;
original source byte spans remain the location oracle.

| Task | Independent truth | Unit |
| --- | --- | --- |
| Literal/identifier content | Exhaustive frozen-source scan under declared semantics | File and matching byte spans |
| Regex | Reference matcher and fixed common-semantics fixtures | Complete match set or typed unsupported |
| Definition | Independent language AST/compiler/declaration census | Kind, qualified/local name and all valid declaration spans |
| File/path locator | Complete frozen inventory plus explicit query policy | Repository/revision/path identities |
| Context/workflow | Task-specific review/outcome and required evidence | Graded labels or jointly required blocks |

Product parsing can cross-check independent expected results. Unsupported syntax,
failed extraction and incomplete source scope stay explicit; they cannot issue
empty gold/no-answer. Docstring examples are content. Ambiguous definitions need
qualified queries or all valid alternatives; jointly required blocks use an
all-required metric. Product majority cannot issue relevance truth.

Keep independent positive/negative raw-span fixtures for Python strings/examples,
Rust const/async/generic functions, TS object methods, decorators, comments,
test/generated declarations, malformed source and duplicate/same-line names.
For each admitted syntax, record actual census coverage and extraction identity;
existing supported parsers are reused, with changes only for demonstrated gaps.
Natural no-answer, wrong-repository and absent identifiers bind complete negative
scope and retain their denominator. Human provenance requires actual assessors.

## Fresh evaluation acceptance

The local expanded target remains twelve additional repositories and 1,200 fresh
cases across independently supported Rust/Go/Python/TS-JS and size strata. Keep
repeated names, definitions versus uses, case/qualified names, long/Unicode files,
path/regex/negative queries, 1–2-character and long names, punctuation, chunk
boundaries and near-matching false friends. These are engineering targets, not
power guarantees. [B08](../../sep-30-code-search-benchmark-trust/tickets/S30-B08-fresh-multirepo-holdout.md)
owns the source-first roster, provisional quotas, exposure audit and C0–C5 gates.

Freeze repository/family splits before variants/tuning; audit shared blobs,
forks/copied fixtures and near-identical query templates. Seal labels from runner
and tuning credentials using existing custody controls. Declare unavoidable
overlap, actual eligible population/underfill and primary cohort sizing from
baseline variance/useful effect. Once used to choose a policy, holdout becomes
development; later selection needs a new challenge. Generated/public tasks do
not establish representative user relevance by themselves.

## Completion

Each admitted file/task has one identity. Duplicate/stale/wrong-source/malformed/
incomplete/leaking inputs refuse. Changed gold has an explicit reason/new identity;
native comparator inventories and exclusions bind the admitted release. Independent
oracle identities, reviewed labels where required, split/pack/critical-stratum
and numerical decision inputs precede policy selection. Unavailable strata stay
visible; missing legal or human inputs block only their dependent claim.

[BENCH-02](CS-BENCH-02-native-response-validation.md) owns native evidence;
[BENCH-03](CS-BENCH-03-tracks-metrics-and-statistics.md) owns units/inference.
Historical preparation and diagnosis are recoverable through
[the history index](../../ARCHIVE-INDEX.md#oct-05-residual-owner-clarification).
