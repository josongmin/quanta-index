# S30-B08 — fresh multi-repository holdout and product decision

Status: C4 `VERIFIED` as diagnostic admission; 240 exact-content product tasks
captured and replayed; C5 `NOT_RUN` (2026-10-03).
Priority: P2. Parent:
[Sep 30 plan](../README.md). Contract owners:
[CS-BENCH-01](../../sep-27-code-search-remediation/rfcs/CS-BENCH-01-corpus-gold-and-holdout.md),
[CS-BENCH-03](../../sep-27-code-search-remediation/rfcs/CS-BENCH-03-tracks-metrics-and-statistics.md),
[CS-BENCH-04](../../sep-27-code-search-remediation/rfcs/CS-BENCH-04-comparators-performance-and-incremental.md)
and [CS-INT-01](../../sep-27-code-search-remediation/rfcs/CS-INT-01-integration-and-qualification.md).

## Why a new release is required

The old 1,196 gin names, Semble gin 20 and public ARB are exposed evaluation
sets. Their successful replay is a regression or external pilot, not an unseen
post-tuning holdout. Gin-only family resampling also cannot support inference
over repositories. Follow BENCH-01's engineering target of at least 12 pinned
repositories and 1,200 **fresh** cases across language, size, task and negative
strata; those numbers are local targets, not a universal SOTA sample-size rule.

## Corpus preparation protocol (plan; no release admitted)

### 1. Select repositories before writing queries

Use the existing ten-repository candidate set and all previously inspected gin
queries only for development and regression. Select twelve *additional* evaluation
repositories that do not overlap those ten, Semble's public repositories, or the
chosen external benchmark snapshots. Target three repositories each in Go, Rust,
Python and JS/TS, with small, medium and large admitted-file strata within each
language group. These are sampling targets, not inferred population proportions.
Declare selection rules and a reserve list before inspecting search results;
publish every rejected candidate and reason. Check forks, shared Git ancestry,
identical file hashes, copied fixtures and near-duplicate source against the
development set. Public source can still be present in model pretraining; this
holdout is unseen to *our tuning process*, not guaranteed unseen to every model.

Freeze full upstream URL, commit, license evidence, complete tracked-file
inventory, canonical Git blob hashes, language, encoding, symlink/submodule and
generated/vendor policy, view, file-size exclusions, and materialized checkout
identity. Use `corpus_release.py` and `corpus_binding.py` as owners; the existing
`retrieval/corpus_set.py` candidate manifests are not admitted releases. Do not
quietly turn a benchmark-root subset into a complete-repository claim. Current
`gold_oracle.py` caps one manifest at 4,096 files, so a larger repository needs
an explicitly separate scale track or a bounded oracle implementation change
before admission. Two truly large repositories should form a separate scale and
resource track; their query counts do not inflate the primary 1,200-case score.

### 2. Freeze distinct query contracts and a sampling ledger

Target 100 tasks per evaluation repository: 20 exact-content, 20 exact named
definition, 30 identifier variants (prefix, internal substring, component split,
single-edit typo), 10 no-answer/wrong-repository controls, and 20 natural-language
or developer-workflow tasks. This 480/360/120/240 allocation is a *provisional
engineering design*. Before any product capture, enumerate the eligible source
population and freeze the final per-repository/lane counts, random seed, family
IDs, underfill reasons and inclusion probabilities. Never backfill a lane after
seeing its scores. Keep the base name and all variants in one query family.

Record for every task: intent, literal/regex/native grammar, case and Unicode
normalization, path/repository scope, answerability, result unit, completeness
contract and admissible products. A prefix or infix query has every name matching
the declared rule as gold; a typo is not entitled to the source name alone if
other names are equally close or it collides with an exact name. Track unique,
ambiguous, no-candidate and natural no-answer separately. Wrong-repository
controls require an explicitly bound negative corpus. Definition, occurrence,
file and required-context labels remain separate. A patch's edited files are
evidence for issue-to-edit tasks, not a complete list of files useful to read.

### 3. Establish labels independently of product output

For exact content, enumerate every match in frozen bytes using a reference
scanner plus hand-specified edge fixtures. For definitions, use an independently
pinned language parser/compiler census and verify each byte span; the current
Go-only robustness oracle and bounded named-function oracle cannot be promoted
to complete multi-language definition truth. Reject unsupported syntax instead
of emitting an empty gold set. Independently re-enumerate all valid variant
answers; use product results only as *blind review candidates*.

For natural-language and workflow tasks, capture the pre-fix repository state,
query provenance and a written relevance rubric. Pool deduplicated candidates
from varied retrieval methods, add source-derived alternatives and random
negative controls, then have two independent reviewers judge file/required-span
relevance blind to product identity. Adjudicate disagreements and retain
per-candidate rationale and `unjudged` status. Audit the mechanical lanes with
independently authored fixtures and a prespecified stratified human sample;
human-review absence keeps subjective lanes diagnostic. A pooled but unjudged
result is not automatically irrelevant. Freeze labels and a label-free blind
pack before retrieval capture.

### 4. Admit and connect the release

Use the existing release -> binding -> suite/blind pack -> native capture ->
replay/scorer path; do not create another corpus manager. A release ID changes
when source files, label decisions, oracle/parser versions, query semantics or
split assignments change. Prove suite/pack/manifest identity and source/index
inventory for each executed product. The current five-product lexical scorer
accepts bare symbols only; record unsupported families as such and extend its
input contract only with independent fixtures and real native adapter coverage.
Do not score a missing product cell as zero or compare chunk top-ten with ten
distinct files as an equal file-ranking budget.

Before repository-disjoint admission, repair three existing owner boundaries:

- `retrieval/gold_oracle.py` currently requires both `development` and
  `holdout` in each per-repository recipe. Accept an explicitly single-split
  recipe only when a separately verified corpus-wide split manifest binds its
  repository and rejects any repository/family present on both sides. Add
  positive and leakage-refusal tests. Do not weaken the existing within-recipe
  family check without this global proof.
- `retrieval/source_oracle.py` provides the identifier-variant contracts only
  for Go. Add independently checked language-specific declaration censuses and
  tokenizer rules before admitting non-Go variants; reject unsupported syntax.
- `retrieval/gold_oracle.py` supports exact content and a bounded named-function
  cohort, not reviewed natural-language/workflow qrels. Reuse the existing
  `evaluator.py` file/declaration judgments, `label_review`, and qualified
  two-annotator/adjudication receipts for subjective file-locator labels; add
  only a source-bound preparation adapter if necessary. Required context blocks
  need their own contract and metric before a context-delivery claim. Do not
  encode subjective labels as exact declarations or infer human review from
  automated assessor IDs.

Preregister primary track/metric, no-answer metric, repository and family macro
aggregation, minimum useful effect, critical-stratum regression bounds, resource
ceilings, and repository-cluster uncertainty before opening holdout responses.
Report attempted/completed/unsupported/unjudged counts, indexed-universe
coverage and per-repository/lane results. Keep exact-match conformance, ranked
file/definition retrieval, context delivery, scale, latency and incremental
update costs in separate reports. Once used to choose a policy, retire this
release from independent holdout status.

Method sources: [CodeSearchNet](https://github.com/github/CodeSearchNet) uses
repository-separated data and human relevance judgments;
[TREC](https://trec.nist.gov/howto.html) supplies pooling and assessor precedent;
[ARB](https://github.com/eyuansu62/agent-retrieval-bench) supplies frozen
base-commit, no-gold and context-budget task distinctions;
[CORE-Bench](https://github.com/zhangfw123/CORE-Bench-Eval) separates issue-to-edit
from broader context; [CoIR](https://github.com/coir-team/coir) and
[BEIR](https://arxiv.org/abs/2104.08663) motivate task and domain diversity;
[CodeXGLUE AdvTest](https://github.com/microsoft/CodeXGLUE) and
[CLARC](https://proceedings.iclr.cc/paper_files/paper/2026/hash/10e400a587ff6925e4e26333b419ff55-Abstract-Conference.html)
motivate identifier-dependence controls. None supplies gold for the local
prefix/infix/typo contract. Pin exact versions before using external data.

### Preparation exit gates

1. Repository roster, rejection ledger, overlap audit, licenses, frozen commits,
   full file manifests and exclusions are reproducible from an external root.
2. The final lane/family counts and independent oracle outputs are frozen before
   product capture; every underfilled or unsupported stratum is visible.
3. Subjective qrels have reviewer provenance and adjudication; mechanical qrels
   pass independent span/set fixtures and a stratified source audit.
4. Blind packs reveal no labels; stale source, duplicate/near-duplicate families,
   wrong unit/case, incomplete corpus and changed label/oracle identities refuse.
5. Only then may B08 move beyond `NOT_RUN`; a valid corpus release alone does
   not qualify any product score or default decision.

## Implementation worklist and owner tests

Keep these as ordered changes to the existing owners. Generated corpus, labels,
review forms, models, indexes and captures live under fresh external roots; only
reusable code, schemas, fixtures and this recipe belong in the repository.

| Step | Implementation and output | Decisive tests / stop condition |
| --- | --- | --- |
| C0. Freeze the input design | Publish an external candidate ledger with language/size strata, selection and reserve rules, the provisional lane quotas, source licenses and exclusion policy. Reuse `retrieval/corpus_set.py` for candidate file/hash censuses and `corpus_release.py` for the admitted release. | `test_corpus_set.py` and `test_corpus_release.py`: wrong commit, dirty checkout, file hash/path collision, excluded/symlink/oversize file and changed inventory refuse. Do not assign holdout status to a candidate manifest. |
| C1. Bind repository-disjoint splits | Add a v2 `retrieval/gold_oracle.py` recipe that names one split and the global split-manifest SHA. Validate the manifest in `corpus_binding.py` against release digest, repository IDs/commits, source fingerprints and query-family inventory; bind its SHA into gold-capsule identity and admission. Preserve v1 within-recipe leakage checks. | `test_gold_oracle.py`, `test_corpus_binding.py`, `test_retrieval_benchmark.py`: two disjoint repositories pass; swapped repo/commit, repeated family, identical or near-duplicate source, missing repository and stale release digest refuse. Current `evaluator.validate_experiment_custody()` is same-repository only; do not claim it validates this global split. |
| C2. Build mechanical lanes | Extend `retrieval/source_oracle.py`, `retrieval/gold_oracle.py` and `retrieval/identifier_robustness_suite.py` by supported language and declared syntax. Keep exact content, declaration, prefix, infix, components, typo and no-answer as distinct contracts. Record all alternatives, byte spans, case and parser failures. | `test_source_oracle_suite.py` and `test_gold_oracle.py`: fixed hand-authored positives/negatives for each admitted language, duplicate names, collision/equidistant typo, UTF-8/case, malformed syntax, generated/test files, long files and chunk boundaries. A language without independent declaration coverage is unsupported, not zero gold. |
| C3. Review subjective labels | Produce per-task candidate pools and two blind review forms from frozen pre-fix sources. Feed adjudicated file/declaration grades into the existing `evaluator.py` suite fields and qualified annotation receipt format in `retrieval/run.py`. Keep raw reviewer decisions and adjudication outside the checkout. Add a required-block label/metric only for a separately declared context-delivery track. | `test_retrieval_benchmark.py`: label/source hash and reviewer identity mismatch, incomplete review, unjudged result, missing alternative, wrong required block, and forged human-review claim refuse. Until review is real and complete, the lane remains diagnostic. |
| C4. Produce blind inputs and matrix | Use `corpus_binding.py` to publish release-bound blind packs. Extend `code_search_matrix.py`/`code_search_workflow.py` only for query forms supported by actual adapters; retain the bare-symbol five-product workflow for its current contract. Check natural-language admission against `query_plan.py`'s default 32-token/96-character limits before freezing queries. | `test_corpus_binding.py`, `test_code_search_matrix.py`, `test_code_search_workflow.py`: every declared repository/lane/mode cell has a matching source, unit, request and capture identity; missing/unsupported cells are explicit; changed labels are absent from the runner pack; no silent query truncation. |
| C5. Qualification after corpus admission | First run per-repository replay and report separate objective, reviewed, no-answer and scale tracks. The existing `evaluator.validate_experiment_custody()` and `decision.py` use within-repository custody/query-family clustering; extend their versioned admission/report/decision contracts for repository-disjoint custody and repository-cluster uncertainty before any cross-repository product-default claim. | `test_retrieval_benchmark.py` and focused `decision.py` tests: a failed repo, omitted lane, unsupported comparator, unjudged pool, unmatched indexed universe or negative/zero effect cannot pass. Existing single-repository decision proof cannot be relabeled as a multi-repository conclusion. |

Implementation order: C0 -> C1 -> C2/C3 (independent after the split and source
contract are frozen) -> C4 -> C5. C0-C4 may prepare a corpus and diagnostic
capture; C5 is the separate product-decision gate. The 4,096-file oracle cap
and the distinct large-repository scale track must be resolved before claiming
large-repository quality. Do not loosen the cap merely to make an overlarge
fixture pass; preserve bounded reads and explicit resource refusals.

## Execution after corpus preparation

1. Preregister primary task, metric, minimum useful effect, allowed critical
   stratum regression, resource budgets, uncertainty method and finite ablation
   matrix **before** holdout access. Choose size and thresholds from baseline
   variance and product needs; do not choose them after the winning run.
2. Run current registered producers and raw/native replay on a clean bound
   source, indexed universe, exact binary/model and supported host. Perform
   repository-level as well as query-family-level aggregation; report each
   product's attempted/completed/unsupported and judgment coverage.
3. Apply the existing [decision gate](../../../../tools/benchmark/retrieval/decision.py)
   only to an eligible paired report and the frozen policy SHA. A passing
   `QUALITY_DELTA` alone is not an engine win or default-change approval.

Pin external data and evaluator versions when adopted; do not merge their
published leaderboard scores with locally altered corpora. ARB's full 427 cases
and no-gold controls remain a separate external workflow track.

## Completion

- Every holdout run has independently admitted labels, source/index identities,
  raw evidence, replay and a query-family/repository split report.
- The report gives task- and repo-macro results, paired uncertainty, critical
  strata, capability coverage and resource ceilings. Failed or unavailable
  comparators remain explicit; no missing row is silently dropped.
- Only the exact policy decision scope may claim a product-default change.
  Installed release, hosted CI, activation and deployment retain their own
  gates. The holdout is retired from future independent evaluation once used
  to select a policy.

## Execution receipt (2026-09-30)

`NOT_RUN`: needs frozen repositories, fresh reviewed labels and a preregistered policy. Producer `quanta-index@0d21914e` (clean worktree);
results, digests and residuals: [qi-s30-bench-trust-20260930-0d21914e/RESULTS.md](/Users/songmin/Documents/code-new/qi-s30-bench-trust-20260930-0d21914e/RESULTS.md).

Reconfirmed `NOT_RUN` 2026-10-01 (v2).

## Preparation receipt (2026-10-02)

Still `NOT_RUN` for qualification: no product capture, no human review, no
preregistration. C0–C2 preparation executed for the mechanical lanes. Root:
[qi-s30-b08-holdout-20261002/RESULTS.md](/Users/songmin/Documents/code-new/qi-s30-b08-holdout-20261002/RESULTS.md).

- C0: 12 fresh repositories (3 Go, 3 Rust, 3 Python, 2 TS + 1 JS; small,
  medium and large in each) in holdout release `sha256:cb896b12…`. The ten
  development candidates are in release `sha256:f93ec1c5…`. Both use policy
  `git-text-views-v3`, which no longer counts root code files such as
  `license_test.go` as licenses. The rejection ledger and overlap audit are
  external.
- Amendment: the split-manifest leakage check refused `rich`, because its
  generated Unicode width tables nearly duplicate development `black`. It was
  replaced by the declared reserve `uvicorn`. The final leakage census finds
  0 exact and 0 near duplicates.
- C1:
  - `gold_oracle.py` adds schema v2 single-split recipes that bind a
    split-manifest SHA-256.
  - `corpus_binding.validate_split_manifest` validates every bound release. It
    checks complete repository assignment, the commit and code_only universe,
    globally unique families, distinct upstreams, and refuses cross-split exact
    and winnowed near-duplicate code.
  - The gold capsule identity binds the split.
- C2:
  - `source_oracle.py` adds Rust, Python, TypeScript and JavaScript
    declaration censuses with exact, prefix, infix, components and OSA-1
    contracts. These are wired into the evaluator, the robustness report and
    the robustness builder (`--language`).
  - `declaration_census_audit.py` checks each census file by file against
    CPython `ast`, `syn`, the TypeScript compiler and `go/ast`; checker sources
    and lockfiles are pinned in `census_checkers/`.
  - The `gold_oracle` `declaration_name_*` intents label only audited files.
    A file that one parser refuses or disputes makes the task unjudged, unless
    the query bytes provably cannot match any name in it.
  - Hand-written fixtures cover each language.
- Gate 2: `holdout_sampling.py` (seed `3797249375`) froze 960 mechanical
  tasks in 12 split-bound gold capsules.
  - 855 tasks are judged and 105 are visibly unjudged; there are 11,884
    labels.
  - The ripgrep literal audit agrees on 231 of 231 tasks.
  - The natural-language/workflow lane is 0/240, underfilled until C3.
  - Sampling-ledger erratum: v1 wrote `admitted/population` as
    `inclusion_probability` for filtered and retried lanes. That fraction is
    not the per-query selection probability. The v2 builder leaves those
    probabilities unknown with an explicit reason; only the simple
    exact-definition ranked draw has a nominal uniform-rank probability.
    Frozen v1 ledgers and capsules remain unchanged. Do not use their 72
    affected repository/lane cells as inverse-probability weights.
  - Current oracle v2 adds a conservative OSA-1 text-absence proof. Read-only
    re-derivation over all 12 frozen views changes 25 task states from
    `unjudged` to `mechanical_unreviewed`, with no label-span changes; 80 remain
    unjudged. The v1 capsules were not rewritten. New source-bound capture and
    validation under oracle v2 are `NOT_RUN`.
- Code custody: owner tests pass (658) in the clean clone
  `src-b08@0ac9b449` (`53ca51e3` plus two preparation commits). Those commits
  are not on main.
- Open: C3 human qrels and the stratified human audit; C4 adapter mapping for
  the matrix; C5 preregistration; the scale track.

## C4 diagnostic receipt (2026-10-02)

**VERIFIED, narrow diagnostic:** `holdout_c4.py` converts one frozen,
source-bound declaration-name intent at a time into the existing `code_search_file`
suite and blind pack. It uses the canonical declaration-intent contracts, checks
the source/release/split/capsule binding, rejects unjudged and checker-disputed
tasks, and permits a census-refused file only when the source oracle independently
proves that the requested name cannot occur there. No-answer tasks additionally
require the term to be absent from folded content and paths. The focused adapter
suite passed 28/28; Ruff check, format check and `git diff --check` passed.

The final-source `lo` oracle-v2 capsule was captured and independently
validated in a **new** external root. Its identity SHA-256 is
`509626b28c8dcd689cf49ff4e053e86776b9bc68078233c526145855082e2503`;
the bound gold-oracle and binding-owner source SHA-256 values are
`6620429f75b58d7b4d1cf4a4c27614f61b2fbfbfad6e803f3f6e68147b73fa15`
and `b3a08bbdc1928b792c16375e96f021516132bf110c39d2b9d1d5537dcf5d9021`.
The C4 prefix suite admitted 8 tasks and excluded 72 tasks outside that intent.
The same frozen `lo` source/manifest and final all-features release searchd
(`8c79d66cc83e5ec13c2a5dd1c16808111efa60e35c1e63db44372663c495b242`)
were used for one paired diagnostic. The verdict is selected 16, executed 16,
passed 12, failed 0: four Semble calls abstained; `PAIR_VALID=pass` is not
16 passed calls. Quanta Hit@10 is 8/8, Semble file Hit@10 is 4/8 on these eight
selected prefix queries. The source-bound receipts are
[`/private/tmp/qc4f-Dl0fOV/lo-oracle-v2`](/private/tmp/qc4f-Dl0fOV/lo-oracle-v2),
[`/private/tmp/qc4f-Dl0fOV/prefix/admission.json`](/private/tmp/qc4f-Dl0fOV/prefix/admission.json),
and [`/private/tmp/q7/verdict.json`](/private/tmp/q7/verdict.json).

**NOT_RUN, B08 qualification:** these labels identify case-sensitive declaration
target files; default CodeSearch matches folded content and paths. Therefore
this pair is a declaration-target *diagnostic*, not default-search conformance,
an eight-query product ranking, a speed comparison, or a default-change gate.
The natural-language/workflow lane, human review, preregistered decision,
multi-repository product capture, and scale qualification remain unexecuted.
Historical failed or pre-format capsules were not used in this pair.

A cheap source-derived 12-repository admission preflight is recorded in
[`/private/tmp/qc4f-Dl0fOV/admission-matrix-v2.json`](/private/tmp/qc4f-Dl0fOV/admission-matrix-v2.json).
Only `lo` and `uvicorn` have full-census status. Nine other repositories have
query-specific potential admissions but their frozen capsules were not
recaptured or independently validated under the current oracle; `nushell`
has two checker-disputed files and no eligible declaration-name tasks. These
counts are **not** captured suites, product results, or a 12-repository score.

## C4 two-repository extension (2026-10-02)

**VERIFIED, diagnostic only:** fresh oracle-v2 capsules for `uvicorn` (Python,
complete declaration census) and `bat` (Rust, one refused census file) were
captured from the unchanged frozen release and independently rederived before
admission. The final-source C4 prefix adapter admitted 8/8 `uvicorn` tasks and
6/8 `bat` tasks. The other two `bat` tasks remain unjudged. Each admitted
`bat` task records a query-specific raw-text absence proof for the refused
Rust file; the file remains in the complete source universe. Capsule creation
took 342.09/452.75 seconds and independent validation plus C4 admission took
475.17/392.21 seconds, respectively. The full input, source, binary, command,
failure, and timing binding is in
[`/private/tmp/qb8c4-R5aKMz/RESULTS.md`](/private/tmp/qb8c4-R5aKMz/RESULTS.md).

Both final pairs used all-features release searchd SHA-256
`6f7e9594195f3b3d6e6559aba4ba59593d9f5b4f55eae39a2b19b91bbd0d5663`
and runner SHA-256
`757834786b89ae95b211a55b68e0f7166ce0fe02c58c55e83e18066dc6ac76d8`.
The outputs are [`uvicorn verdict`](/private/tmp/q8b/verdict.json) and
[`bat verdict`](/private/tmp/q9a/verdict.json). The paired diagnostic results
are deliberately kept separate:

| Repository | Selected prefix tasks | Pair executed/passed/failed | Quanta declaration-target Hit@10 / NDCG@10 | Semble declaration-target Hit@10 / NDCG@10 |
| --- | ---: | ---: | ---: | ---: |
| `uvicorn` | 8 | 16/16/0 | 8/8 / 0.839 | 6/8 / 0.441 |
| `bat` | 6 | 12/11/0; one Semble abstention | 6/6 / 0.928 | 5/6 / 0.688 |

The `uvicorn` query `send` has 11 independently labeled declaration files,
so Recall@10 has a ceiling of 10/11 for that task. `bat`'s strict symbol
preflight refused six syntax-highlight fixture files among 79 manifest files;
the failed attempt is preserved in `/private/tmp/q9.staging`. The successful
lexical diagnostic explicitly used `symbol_coverage_policy=allow-incomplete`.
That setting does not assert complete symbol coverage. An initial `uvicorn`
attempt with an empty new Hugging Face cache also failed before capture;
the final pair used a new copy of the pinned model cache and preserved the
failed staging root. The earlier successful `uvicorn` pair under searchd
SHA-256 `75efff76e6ae50b7da3c26604d751e35a7ceedb7dbaf3d27198f620ac10a1bc2`
remains at `/private/tmp/q8a` and is excluded from the final table.

**Still `NOT_RUN` for B08 qualification:** declaration-target labels do not
cover all default folded content/path matches, index-universe attestation is
absent, the subjective/workflow lane and human review are unexecuted, and the
12-repository matrix and preregistered decision do not exist. These 14
selected tasks cannot establish a product ranking or performance effect.

## C3 review preparation pilot (2026-10-02)

**VERIFIED, preparation only:** `holdout_review.py` is a thin adapter over the
existing evaluator's blinded v3 query pack and `SourceSnapshot`. It binds every
pooled file to the frozen source hash and universe, requires two retrieval pools
plus source alternatives and random controls, deduplicates files and permutes
their order independently for two review forms. Pool identities/membership stay
in a private owner directory. The forms contain complete UTF-8 file text but no
product metadata, rank, score or preexisting gold. Every grade, answerability and
reviewer identity remains `null`. The adapter exports no suite, qrels or
annotation receipt. Existing `run._validate_gold_review_receipt()` rejects these
unjudged forms. The pool execution flag is deliberately unattested: declared
pool diversity alone is not proof of native retrieval execution.

Command: `.venv/bin/python -m pytest tools/ci/tests/test_holdout_review.py
tools/ci/tests/test_holdout_c4.py -q` — **49 passed** (21 preparation tests and
28 existing C4 tests). Ruff check and format check passed for both new files.
Coverage includes contaminated label/rank fields, source/hash/universe drift,
partial task/pool coverage, missing alternatives/controls, duplicate identities,
bounded complete source export, partial-write cleanup and refusal to treat a
preparation form as a qualification receipt.

A pilot replayed the existing `lo`, `uvicorn` and `bat` captures through
`run.merge_records()` before pooling their files. It prepared two unjudged
forms per repository for 8, 8 and 6 existing prefix tasks, respectively. The
deduplicated task/file candidate counts are 104, 124 and 70 (298 total); they
include source-oracle alternatives and two seeded controls per task. Controls
have no assumed relevance grade. Source, pack, input record and form digests
are retained outside the checkout at
[`summary.json`](/private/tmp/qi-b08-c3-review-lc21ai1c/summary.json).

**NOT_RUN, independent relevance approval:** this is a prefix-only review-input
pilot over three already captured repositories. It is not the prespecified
stratified mechanical human audit, does not fill the 0/240 subjective lane, and
does not supply either human review or adjudication. The pooled subset cannot
prove exhaustive relevance or corpus-wide no-answer. Two independent people
must make source-backed decisions, inspect missing alternatives, and enter the
existing evaluator judgments and annotation/adjudication receipt contract before
the reviewed lane can qualify. B08 remains `NOT_RUN` for product approval and
the 12-repository decision.

## Twelve-repository source and admission audit (2026-10-02)

The frozen 12-repository `code_only` views contain 13,347 files and
112,244,125 source bytes. An independent SHA-checked byte scan reproduced all
240 literal tasks and 619 labeled spans with zero mismatch
([receipt](/private/tmp/qi-b08-literal-audit-ZSdDEx/summary.json)). A blank,
blinded two-reviewer packet covers 106 stratified mechanical tasks; no person
has entered a relevance decision or adjudication
([packet](/private/tmp/qi-b08-mechanical-review-26Fb6M/README.md)). The frozen
oracle-v1 capsules still require current-source recapture; this audit cannot
upgrade their status.

Six repositories exceeded the previous 2,000,000-membership file-index cap.
Their verified frozen bytes yield 1,362,878–3,379,823 memberships when the
same case-folded content/path candidate index is used for both case modes
([exact census](/private/tmp/qi-file-admission-census/final-policy-census.json)).
The current source changes use that single candidate index per surface, retain
exact original-text verification, admit at most 4,000,000 memberships and
check a 128 MiB builder estimate at seal and cold open. This is a source-level
admission calculation conditional on producer text coverage. Full ingest,
activation, physical RSS, all 12 repository captures and B08 product
qualification remain `NOT_RUN`.

## Mixed objective/reviewed admission repair (2026-10-03)

An admission contradiction blocked the intended C5 suite: the decision gate
required both objective and reviewed positive tasks in each repository, while
`evaluator.validate_suite()` refused their two judgment policies in one eval
split and `run._validate_gold_review_receipt()` refused every source-oracle task
in a qualified suite. The source validator now permits only the explicit
`source_oracle_complete_v1` plus `complete_ranked_pool_v1` combination, with
each policy tied to its corresponding task authority. Other mixed policies
remain invalid. For admission schema v3, a mixed suite requires review receipt
schema v2: two annotations and adjudication cover only subjective tasks in
suite order; the complete suite hash and source validation still bind every
mechanical task. Schema-v2 local admission retains all-task schema-v1 receipts.

The focused source fixture contains an objective positive, reviewed positive
and mechanical no-answer task. It accepts the mixed suite and versioned review
receipts, rejects the legacy mixed receipt, missing subjective review and
adjudication label drift. This establishes the code contract, not human
provenance or a complete v3 product capture.

## Current-source ingest diagnostic (2026-10-02)

The all-features release preflight at `14fdeffb` failed to publish the frozen
Tailscale view: its 2,532 files and 6,257 chunks produced a 75,624,895-byte
CBOR request over the 67,108,864-byte decoded-frame limit. The lexical-only
benchmark batch had copied every chunk's text into both lexical and unused
semantic raw-fallback scopes. This was a producer payload failure before seal,
not evidence that the file candidate index rejected the corpus. The preserved
diagnostic is
[`RESULTS.md`](/private/tmp/qi-b08-ingest-14f-AritdR/RESULTS.md).

Commit `912558b8` makes the existing batch assembler omit semantic scopes for
lexical-only routes while keeping source, lexical chunks, symbols and the paired
generation contract. It adds a unit test comparing lexical-only and semantic
batches. The release binaries built from that clean commit have SHA-256
`fc114f6a183f09c0ef1c648231ecc5e84db1a2da0054d12295632dfb246848a9`
(searchd) and
`f8e889e32660a4eb0b9c9af1f11b4075b867498f7a3a9ebe286ba2fe610d10cf`
(runner). A fresh external run at `/private/tmp/b8fix-lebuDK/q` completed
publish, seal, activation and one lexical query over all 2,532 files. Its
receipt records 2,532 lexical replacement scopes and zero semantic scopes;
the query returned 10 distinct files with `capped`/`lower_bound`, as expected
for 68 matching candidates. Driver elapsed 76.21 s, opaque SDK publish plus
activation 70.28 s, server lexical build observation 62.72 s and sampled
process-tree peak RSS 1,230,405,632 bytes. These are one contended-host
diagnostic, not B07 performance evidence. The run does not prove the general
semantic/hybrid payload path can handle a >64 MiB batch.

One current-source mechanical oracle recapture for `zustand` is at
`/private/tmp/qi-b08-current-oracle-bnqaJn/capsules/zustand`. It retained the
same 80 task IDs/queries/intents and 174 labels, but changed six unsupported
states and five answerability states versus the frozen capsule. The remaining
eleven capsules and independent human relevance review are `NOT_RUN`; the
frozen counts cannot be silently promoted to current-source qualification.

## Current-source batch gold and large ingest follow-up (2026-10-02)

**VERIFIED, diagnostic preparation:** `gold_capture_batch.py` captured all 12
repositories into a new external root at
[`gold-v5-batch-current-source`](/Users/songmin/Documents/code-new/qi-s30-b08-holdout-20261002/c2/gold-v5-batch-current-source).
It validates the global split before derivation, uses the existing gold oracle
for each repository, revalidates the split and source digests, then publishes
the directory atomically. A separate current-source replay verified all 12
capsules and their file/identity hashes. Capture took 863.60 s; independent
replay took 419.53 s. The capsules contain 960 tasks and 11,884 labels. Every
task's label list matches `gold-v4`, but the current oracle changes unjudged
tasks from 105 to 132 and unsupported rows from 574 to 366. These capsules are
`mechanical_unreviewed_diagnostic`; the source policy change and absence of
human review prevent a quality decision.

**VERIFIED, ingest and harness only:** `quanta-index@056b6ac5` all-features
release binaries, SHA-256
`cba8c012ace1c26c27968d95ba00ce3ab246cee1caba442667df0fd5bef5993e`
(searchd) and
`baf196393e16fe2dd672f3e8092f47572556d5cd903778ac81e9434aeeaf995a`
(runner), completed the native lexical, semantic and hybrid Tailscale capture
under a fresh external root
[`qm-emy4i_54/q`](/private/tmp/qm-emy4i_54/q). The producer's single batch
accepted 2,532 lexical file scopes and 6,257 semantic chunk scopes, sealed and
activated generation 1, and returned 10 candidates on each route. Each query
was `capped`, which means the result limit was reached, not that the request
failed. The previous >64 MiB request refusal is resolved by bounded IPC
fragments with sequence and whole-body SHA-256 checks. Driver elapsed 89.82 s;
sampled process-tree peak RSS was 1,368,309,760 bytes. This is one contended
host run and does not qualify B07 performance or memory ceilings.

The same binaries completed a fresh Django lexical capture at
[`qd-4wfc68of/q`](/private/tmp/qd-4wfc68of/q): 2,368 files, 6,537 chunks,
publish, seal, activation and a 10-result `capped` query. The previously
rejected parser row for `tests/test_runner_apps/tagged/tests_syntax_error.py`
now records one source-bounded, explicitly unlocated `syntax_error` diagnostic
instead of `parse_failed` with zero diagnostics. Driver elapsed 92.90 s;
sampled process-tree peak RSS was 1,333,788,672 bytes.

Both runs initially exposed a separate Python timing validator defect: it
alphabetized routes even though the runner executes the suite's route order,
and treated `capped` as an incomplete response. The validator now derives the
first complete route cycle, requires every later cycle to match and accepts
`capped` as a completed response. The first rejected Tailscale output remains
separate at `/private/tmp/qm-_ojl37du/q`; its data were not promoted. Focused
timing tests and the fresh completed run verify this repair.

**NOT_RUN for B08 qualification:** current-source capsules remain mechanical
and unreviewed; complete per-repository product matrices, external service
index bindings, preregistered statistics and the multi-repository decision
gate are absent. Neither diagnostic run supports a product ranking or default
change.

## Current-source census and admission closure (2026-10-02)

**VERIFIED, diagnostic preparation:** clean `main@22320380` captured 12 new
gold capsules at
[`/private/tmp/qi-b08-gold-2232-cEcBHy/capsules`](/private/tmp/qi-b08-gold-2232-cEcBHy/capsules)
from frozen `release-v4` and `sampling-v4`; batch wall time was 449.25 s.
Capsule identity now binds the direct Python gold, source-oracle, census-audit
and corpus-binding owners plus parser/runtime versions. The independent
Rust/Go/TypeScript/JavaScript audits also record the hashes of the executed
checker artifacts. Their cache refuses a changed source digest, artifact or
ready marker, and each audit checks checker identity before and after the
source census. Current-source replay refuses an old capsule (`lo` checked:
`gold.json` differs), so its metadata cannot be mixed with the new evidence.

The 960 frozen tasks have 880 `mechanical_unreviewed` and 80 `unjudged`
states. All 12 task label lists are unchanged from the preceding source-bound
capture. In `nushell`, 52 declaration tasks became mechanically judgeable
only where a query-specific raw-source scan proves no matching name can occur
in all seven refused or checker-disputed files; eight tasks remain unjudged.

The final source-bound C4 matrix is
[`/private/tmp/qi-b08-matrix-2232-ek0Ty8/matrix/admission-matrix.json`](/private/tmp/qi-b08-matrix-2232-ek0Ty8/matrix/admission-matrix.json)
(SHA-256 `8d350ed07fd8ae7a353dd3699a83677a9d3377ccd93381a73a042dc283fb0c4a`).
It replayed the capsules and complete source views in 694.73 s. All 12
repositories × five declaration-name intents have a diagnostic cell: 640
selected tasks, 80 excluded as unjudged/unsupported. Its selected IDs and
exclusion rows are identical to the pre-custody diagnostic matrix. The 240
exact-content tasks are a separate contract and are not in these 720
declaration cells. The matrix explicitly records `product_capture=false` and
`qualified_default_search_conformance=false`; no product score was created.

**VERIFIED, focused tests:** `PYTHONPATH=.:tools/benchmark uv run --frozen
--extra dev pytest -q` over `test_corpus_binding.py`, `test_gold_oracle.py`
and `test_holdout_c4.py` passed 139 tests. `test_declaration_census.py`
passed 31 tests, including independent declaration fixtures, changed-cache
refusals and an audit-time identity-change refusal. Ruff and `git diff --check`
passed. The first combined run observed a source edit during derivation and
failed the source-drift guard; it is excluded. The unchanged-source run above
is the accepted test result.

**NOT_RUN, C5 and product qualification:** no human relevance/adjudication for
the mechanical audit or workflow tasks, no complete product capture of the
matrix, no live Sourcegraph/OpenGrok indexed-universe attestation, no
preregistered repository-cluster decision, and no quiet-host performance
measurement. C4's case-sensitive declaration-target labels do not prove the
default folded content/path search contract. The existing 240 literal tasks
and absent workflow lane cannot be merged into the declaration score.

## Negative custody and exact-content admission (2026-10-02)

**VERIFIED, diagnostic only:** `main@26e6bbae` binds C4 no-answer rows to
`ascii_code_search_absent_casefold_v1`. The adapter uses that existing source
oracle when selecting a negative and when emitting its suite row; replay
rejects a query with no declaration but a content or path occurrence. The
120 negative queries across all 12 frozen source views passed a source-hash
and content/path absence check. A complete `lo` capsule replay selected 30
exact-declaration tasks, including ten rows with the stronger contract.

The refreshed C4 matrix is
[`/private/tmp/qi-b08-matrix-26e6-tNX9i5/matrix/admission-matrix.json`](/private/tmp/qi-b08-matrix-26e6-tNX9i5/matrix/admission-matrix.json)
(SHA-256 `8da046847bb2f7ab7cae62a6b70c028a17c5026f6d012fcf4ee27bea492fe5cf`,
wall 710.43 s). It has the same 60 cells, 640 selected IDs and 80 exclusion
rows as the prior matrix. The 12 exact-declaration suite and blind-pack hashes
changed; no selected ID or exclusion changed. The prior matrix remains a
separate, older-source receipt. Both are `diagnostic_unqualified` with
`product_capture=false`.

**VERIFIED, admission only:** the exact-content adapter reuses the frozen
capsule binder and constructs a Quanta CodeSearch `content:"..." case:yes`
request using the existing `LqQuery` lowering. A direct source preflight
accepted all 240 literal inputs and found zero label or NFC-induced file-set
differences. A complete source-bound `lo` replay admitted 20/20 literals with
zero exclusions in 307.83 s. This adapter does not produce a scored suite or
product capture. Its label replay currently calls the gold producer's literal
scanner, so an independently implemented exact-content oracle remains required
before a qualified score.

**VERIFIED, focused rails:** 83 combined C4/literal/source-oracle/decision tests,
12 request-identity tests, the Rust CodeSearch scoped-quote lowering test and
the existing typo language-eligibility budget test passed. Ruff, Rust formatting
and `git diff --check` passed. The single-repository decision policy now binds
the repository commit and requires graded paired/no-answer coverage; it
explicitly refuses multi-repository requests through its query-family CI.

**NOT_RUN:** product capture for the 12-repository matrix or 240 literals,
independent literal oracle, reviewed/workflow relevance, external indexed-file
attestation, repository-cluster decision, and qualified quiet-host timing. The
current pair quality gate treats CodeSearch file policies as diagnostic; a
future file-quality gate must use an independent file relevance contract rather
than relaxing that refusal on this evidence.

## Exact-content replay and product capture (2026-10-03)

This section supersedes the admission-only and `NOT_RUN` literal-capture status
above. The earlier receipts remain historical, source-bound observations.

**VERIFIED, diagnostic only:** `main@e70add73` adds an independent raw-byte
literal oracle, source-bound suite/blind-pack generation, replay validation,
and an exact-content file request policy. The request uses the existing
`LqQuery` and public CodeSearch route (`content:"..." case:yes`); no second IR
or new engine route was introduced. Shared C4/literal batch admission binds
the direct and transitive oracle/tool sources. All 12 gold capsules were
recaptured after the source-oracle change; selection, recipe, gold, blind, and
split payloads remained byte-identical, while producer source identity changed.

The C4 matrix is
[`/private/tmp/qi-b08-c4-admit-e70-Hb0V1H/c4/admission-matrix.json`](/private/tmp/qi-b08-c4-admit-e70-Hb0V1H/c4/admission-matrix.json)
(SHA-256 `a7b04ac8e4b2e76cd4cb2cfe7d6711d7d787cab3314d3566d5e346a1e9df8423`):
12 repositories, 60 cells, 640 selected tasks and 80 exclusions. The separate
exact-content matrix is
[`/private/tmp/qi-b08-literal-admit-e70-MtjCff/literal/literal-matrix.json`](/private/tmp/qi-b08-literal-admit-e70-MtjCff/literal/literal-matrix.json)
(SHA-256 `0336ea5d91f18d39683c26f36370c40875130154aa94e5e97b660fde90faef9c`):
12 repositories, 240 selected tasks. Both are `diagnostic_unqualified`; do not
merge their tasks or metrics.

**VERIFIED, product capture and replay:** all 240 exact-content tasks ran on
the 12 frozen repositories through the public SDK, release search daemon, and
fresh repository state. Each repository's suite/blind-pack hashes matched the
matrix, its manifest revision matched the recorded source revision, and its
record replayed through `evaluate-diagnostic`. The independent summary is
[`/private/tmp/qi-b08-literal-product-release-mav0u6/independent-summary.json`](/private/tmp/qi-b08-literal-product-release-mav0u6/independent-summary.json)
(SHA-256 `c470a32a4d5d337936f79908e965bafae0b1fe1e61fd03e72ec42db185e5ed2f`);
per-repository `binding.json`, `record.json`, `report.json`, and `phases.json`
are under the same output root. The release runner SHA-256 is
`78aeac8ed59ab7508110af0243fdcff952bfafb90b24f3dcdfadf368c52d32c2`;
the daemon SHA-256 is
`eb9bbb0bdf5815a44fddf51bd8e4c54bab382481ba0316ae785594fbf84a7191`.

There were 238 `success` and two `capped` responses, with zero execution
failures. Hit@10 was 240/240, mean NDCG@10 was 1.0, and mean Recall@10 was
0.994003. The capped tasks had 165 and 20 relevant files respectively;
their ten distinct returned files therefore give Recall@10 of 10/165 and
10/20. Of the 240 tasks, 206 had exactly one relevant file. These numbers
test exact-content file retrieval, not default folded CodeSearch quality or
cross-product ranking.

**VERIFIED, execution timing only:** the `--release --all-features --locked`
binary build took 13m25s. Across 12 captures, summed wall time was 356.84s,
including 303.278s of publish/seal/activate phases. The 240 SDK call
observations summed to 892.691ms, with p50 3.061ms and p95 7.681ms. These
single-run, busy-host timings are not a qualified latency comparison. An
earlier debug `zellij` capture hit its 30s IPC timeout during publish; retry
with 180s completed and replayed 20/20 tasks.

**VERIFIED, focused rails:** the retrieval-bench Rust suite passed 191 tests
with a pinned daemon; the Python retrieval-benchmark suite passed 428 tests;
the C4/literal/source-oracle subset passed 88 tests after the final tool-source
binding edit. Ruff, Rust formatting, and `git diff --check` passed. These
checks cover the exact-content adapter and replay, not full C5 qualification.

**NOT_RUN:** human relevance adjudication, workflow labels, live external
indexed-universe attestation, a preregistered repository-cluster decision,
and quiet-host performance qualification. The new exact-content tasks are
mechanical diagnostic labels and must remain separate from the declaration
matrix and the older gin query sets.

## C5 code boundary audit (2026-10-03)

`main@8f8e6c9f` has a deterministic, language-stratified
`evaluator.repository_cluster_ci()` helper. It gives equal weight to query
families within a repository and to repositories in the release, resamples
repositories within declared strata, and refuses missing repositories,
cross-repository families, fewer than twelve repositories, singleton strata,
and out-of-range metric deltas. Three focused unit tests and Ruff passed.

This helper is not connected to `run.py` qualified admission or `decision.py`.
Those contracts still bind one repository and explicitly reject a
multi-repository product decision. C5 therefore remains `NOT_RUN`. The next
code boundary is a versioned, repository-disjoint qualified input that replays
every declared cell and its source/index/review custody before deriving paired
rows for this helper. No current C4 or exact-content diagnostic capture can be
promoted by calling the helper directly.

`main@ae8f0e15` extends the existing `code_search_matrix.py` lexical-only
pair contract to `code_search_typo_file` and
`code_search_exact_content_file`. Semantic and hybrid cells for those
requests must be marked unsupported, while a lexical-only cell cannot be
silently marked unsupported. The matrix/workflow unit set passed 60 tests.
This validates policy routing only. A source-bound bridge from C4 admission
cells to native capture roots was subsequently added as matrix v3 in
`main@7c4d82dc`. `build_c4_spec()` re-derives the frozen C4 admission,
requires one declared capture root or explicit not-run entry per admitted
lexical cell, and derives `no_admission_diagnostic` cells without invented
suites or packs. `verify()` replays the C4 source/capsules, emitted inputs and
native captures, then reports missing captures separately from source-proven
no-admission cells. The matrix/workflow unit set passed 62 tests, including a
two-repository source-bound C4 fixture. The 12-repository v3 diagnostic replay
subsequently passed from frozen `bd2b8c25` at
`/private/tmp/qi-b08-v3-e2e.kSTE4v`: 12 gold capsules, 72 C4 admission cells
(60 admitted, 12 no-admission), and 216 matrix mode cells. Matrix verification
returned `diagnostic_incomplete` with 0 captured, 60 not-run, 120 unsupported,
and 36 no-admission mode cells. The admission-matrix SHA-256 is
`26fae8295edb9831ab653ad05d12db204fa3a78406f3f4aa74877b0767e48309`.
This result binds `bd2b8c25`, not later `main` commits, and no product capture
or C5 qualification was executed. The public `benchctl code-search
matrix-build-c4` command now writes a fresh external v3 spec from an explicit
capture-root inventory; the matrix/workflow and benchctl focused sets passed
62 and 75 tests respectively.

The C5 report adapter now derives repository-cluster rows from suite-bound
per-query records instead of accepting caller-supplied deltas alone. It refuses
missing or duplicate paired rows, wrong routes, suite/report mismatch, invalid
numbers, missing repositories and an inconsistent reported effect. This is an
input integrity helper. Per-repository qualified capture replay, global
admission binding, human relevance adjudication, and product decision remain
`NOT_RUN`.

Focused `test_retrieval_default_decision.py` passed 9 tests. A broader
`test_retrieval_benchmark.py` run exposed one regression: refusing an ungraded
report before the existing verdict could classify `QUALITY_DELTA=fail`. The
single-repository replay now retains that classification, while the
multi-repository interval requires graded rows. The same broad run also had
one setup error from a noncanonical `PYTHONPATH`. The two affected tests and
the new repository-row test passed 3/3 with `PYTHONPATH` unset after the fix
(`main@e4787852`). The full 439-test file after that fix is `NOT_RUN`; the
pre-fix run was 437 passed, 1 failed, 1 setup error.

## C5 replay and metric-gate status (2026-10-03)

The public `benchctl code-search repository-replay --bundle` path now binds a
predeclared 12-repository policy to the global split, source-bound suites,
qualified per-repository verdicts, selected report digests, and candidate
latency/resource observations. It derives repository-cluster uncertainty from
paired query rows. The metric gate checks the preregistered effect, lower
bound, named critical strata, no-answer abstention, and resource ceilings;
passing returns `eligible_for_human_review` and never sets a product default.
It independently checks the no-answer report mean against per-query statuses
and requires each policy query-family inventory to equal its split inventory.

**VERIFIED, code rails:**
`env -u PYTHONPATH .venv/bin/python -m pytest -q
tools/ci/tests/test_retrieval_default_decision.py tools/ci/tests/test_benchctl.py`
passed 86 tests. The wider `tools/ci/tests/test_retrieval_benchmark.py` run
passed 439 tests in 461.95 seconds. Ruff format/check and `git diff --check`
passed for the changed Python files.

**NOT_RUN, qualification:** the synthetic replay test mocks the native
split/verdict validators. No 12-repository qualified capture bundle, human
adjudication, live external indexed-universe attestation, or product-default
review has run. The split does not independently declare every intended
reviewed/scale category, so matching family inventories alone cannot prove
that those lanes were prepared. C5 cannot be marked complete from these unit
tests.

## Repository-disjoint qualified admission code (2026-10-03)

The qualified runner now accepts a distinct admission schema v3 that binds a
holdout repository, release digest, global split bytes and release-path map.
The split validator rechecks every release and the development/holdout source
leakage policy; the selected suite must use the holdout commit, complete
`code_only` file universe, eval-only tasks and exactly the split's query
families. Pair spec freezing, run-manifest artifacts and verdict replay select
this v3 custody path. Existing same-repository schema v2 custody remains
separate. C5 replay requires v3 and rejects a v2 qualified receipt. The C5
metric gate requires critical category and language strata for every observed
answerable group, alongside every repository and no-answer.

The focused schema/source/freeze tests include negative holdout-side,
file-universe, family and split-digest cases. A separate test admits a real-Git
release and its source-derived holdout suite through the unmocked split
validator, then rejects the wrong repository side. Another test runs the
complete verdict fixture through v3 artifacts with only the split source
validator mocked, and rejects changed split bytes. The retrieval benchmark,
C5 decision and benchctl Python set passed 528 tests in 415.10 seconds;
`test_corpus_binding.py` passed 51 tests in 75.34 seconds. A complete v3 pair
capture followed by unmocked `run.build_verdict()` and 12-repository C5 replay
is still `NOT_RUN`. These changes do not establish human reviewer identity or
an externally indexed file universe.

## Objective and reviewed C5 track gate (2026-10-03)

The schema-v2 repository-disjoint policy now requires separate predeclared
minimum deltas for `objective` and `reviewed`. Each holdout repository must
contribute at least one positive, paired task in both tracks. Track membership
comes from the validated suite's `source_oracle` and judgment policy, or from
reviewed `label_review` plus complete judgments, rather than the task's
self-declared category. The gate reports repository-equal track means and
refuses a below-threshold track. It rejects missing or unreviewed labels.
This is a code-level guard; a review receipt alone cannot establish that the
named reviewer was a human or that the judgments are correct.

**VERIFIED, focused code rail:**
`env -u PYTHONPATH .venv/bin/python -m pytest -q
tools/ci/tests/test_retrieval_default_decision.py tools/ci/tests/test_benchctl.py`
passed 86 tests; Ruff and `git diff --check` passed. The full 528-test Python
set was run before this track-gate change, so it is not current proof for this
new code. The actual 12-repository capture, reviewed qrels and separate scale
qualification remain `NOT_RUN`.
