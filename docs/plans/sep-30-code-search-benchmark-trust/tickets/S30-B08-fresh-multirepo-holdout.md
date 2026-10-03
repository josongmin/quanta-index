# S30-B08 — fresh multi-repository holdout and product decision

Status: C4 `VERIFIED` as diagnostic admission; C5 diagnostic capture in progress;
qualified multi-repository decision `NOT_RUN` (2026-10-03).
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

## C5 diagnostic update (2026-10-03)

The corrected C4 mechanical suites admit 48 ordinary-search cells (5,521
tasks per product), 12 explicit OSA1 typo cells (4,149 positive tasks), and
12 component cells (64 tasks). This is exposed, mechanically labeled
diagnostic data. No human-reviewed relevance judgments, complete external
indexed-universe proof, repository-cluster decision, or qualified default
change follows from these captures.

- Sourcegraph, cs, and OpenGrok completed 48 ordinary-search cells. The raw
  capture is `/private/tmp/qi-c5-external-session-v6-20261003`; independently
  amended scoring is under `/private/tmp/qi-c5-external-score-v6-20261003`.
  The original external scorer failed because it treated the legacy
  representative `gold_paths` as the full multi-file judgment set. The amended
  scorer validates that raw projection separately and uses the frozen
  `file_judgments` for grading. All 144 cell-product rescored Hit@10/NDCG
  outputs were checked against the repository evaluator.
- Quanta explicit OSA1 completed 12 cells: 4,109/4,149 file Hit@10, with six
  typed `LEX_TRIGRAM_PLAN_LIMIT_EXCEEDED` errors. Evidence:
  `/private/tmp/qi-c5-explicit-quanta-20261003-v2`. Quanta components completed
  64 tasks (60 file hits, three producer abstentions and one typed parse
  refusal): `/private/tmp/qi-c5-components-quanta-20261003-v1`.
- Quanta exact declaration capture completed 12 cells:
  `/private/tmp/qi-c5-declaration-quanta-20261003-v6`. The original precommitted
  scorer failed on its zero-error assertion. A separately registered
  post-capture amendment reports 649/658 Hit@10 among completed positive
  tasks, 649/1,101 among attempted positive tasks, and 493 typed
  `SYMBOL_COVERAGE_INCOMPLETE` rows across five repositories. Nine completed
  positive misses are Rust constants or a macro in `zoxide` absent from the
  captured producer. `benchmarks/retrieval/src/symbols.rs` now extracts these
  Rust node kinds; a source-bound recapture has not yet verified the fix.
- The repaired Quanta/Semble ordinary-search pair is still preparing. Do not
  combine partial cells or the earlier failed pair batch with completed
  48-cell results. The cs native `~1` OSA1-wide batch was stopped after one
  completed `attrs` cell (386 tasks); the next cell's staging data remains
  incomplete. Of 4,149 OSA1 tasks, only 1,033 one-character substitutions
  match the verified cs edit class; whole-identifier versus content-window
  matching and ranking still differ. The other 3,116 are outside the shared
  operation class. Preserve the partial native capture and stop receipt at
  `/private/tmp/qi-c5-cs-fuzzy-20261003-v5`; do not report a 12-repository cs
  fuzzy score from it. A new substitution-only frozen suite is required.

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

**VERIFIED, code rails:** `env -u PYTHONPATH .venv/bin/python -m pytest -q
tools/ci/tests/test_retrieval_benchmark.py` passed 443 tests in 366.03
seconds. The decision, benchctl and review preparation set passed 124 tests
in 13.77 seconds. Ruff check/format and `git diff --check` passed. A complete
v3 mixed-suite capture followed by unmocked verdict and C5 replay is still
`NOT_RUN`.

## C5 file-ranking qualification boundary (2026-10-03)

**Code path implemented, native qualification NOT_RUN.** The qualified pair
spec accepts only `code_search_file` with `default_file_search`, schema-v3
repository-disjoint admission and a quality claim. The evaluator requires
source-oracle or reviewed-complete label authority, scored distinct-file order,
native score evidence, complete paired top-10 judgments and no-answer controls.
It emits a separate `file-judgments-complete-v1` report with
`file_ndcg_at_10`, per-query values, paired and stratified deltas, and
uncertainty. The verdict independently re-scores the report; `QUALITY_DELTA`
accepts the file metric only with schema-v3 admission and all existing
qualification gates. Exploratory file reports remain diagnostic. The C5 policy
v3 binds `metric_scope: scored_distinct_file` and
`request_mode: default_file_search`; replay rejects a context report in that
track. Context policy v2 remains `context_span_density`. Typo and exact-content
file requests remain diagnostic because their request and relevance contracts
are different.

**VERIFIED, focused code checks:** source-backed file evaluator fixtures cover
score order, an independent NDCG golden, unjudged result refusal, missing score
evidence, no-answer failure and report tampering. C5 synthetic replay covers
both policy versions across twelve repositories. The decision/benchctl rail
passed 90 tests. A 2-positive-task fixture is correctly refused for insufficient
uncertainty; a 20-positive-task/20-family fixture passes both qualified
uncertainty checks. Direct stage and verdict tests refuse typo and exact-content
file profiles for qualified file scoring. These checks do not substitute for an
unmocked qualified pair capture and verdict replay. The 12-repository file
capture, independently reviewed labels, external indexed-universe attestations
and product-default decision remain `NOT_RUN`.

Post-fix retrieval regression at `main@ff2bae33`: the full
`test_retrieval_benchmark.py` file passed **447/447** in 332.35s using
`env -u PYTHONPATH .venv/bin/python -m pytest -q`.
The subsequent commits through `4f751b64` did not change `run.py`,
`evaluator.py`, `decision.py` or that test file. This is a code regression
check, not an unmocked qualified file capture.

## Frozen C4 v6 and native typo diagnostics (2026-10-03)

**VERIFIED, diagnostic only:** clean `9d38b69d` rebuilt all twelve source-oracle
capsules at `/private/tmp/qi-b08-capsules-v6-anchor-9d38b69d-20261003`.
Their decoded `gold.json` payloads equal the previous twelve capsules. The
source-bound C4 output is
`/private/tmp/qi-b08-c4-bundle-9d38b69d-20261003`; its matrix SHA-256 is
`73c99401d44b98b4f04c5aa2e871775da3a0c30a04c12dae91daf0eeba1edfd7`.
It declares 72 repository/intent cells: 60 admitted diagnostic cells, twelve
no-admission cells, 5,680 selected and 692 excluded tasks. All emitted input
hashes and selected/excluded task partitions matched matrix replay. This is
mechanical declaration-target evidence, not reviewed file relevance.

The frozen Quanta `code_search_typo_file` run used the release/all-features
searchd and runner bound in
`/private/tmp/qi-b08-product-8e8592f4-20261003/build-binding.json`.
All twelve repositories' 4,206 admitted typo queries completed with 4,146
file Hit@10, MRR@10 0.8750 and NDCG@10 0.9009; 60 misses were all `capped`,
and 21 of 1,093 query families had at least one miss. There were no query
execution failures. Per-repository status, native record/report hashes, and
the 17m52s sum of run-plus-evaluator wall time excluding the reused `lo` run
are in
`/private/tmp/qi-b08-product-8e8592f4-20261003/quanta-typo-summary.json`.
The wall sum is not a search-latency benchmark.

The two original `lo` misses had gold files at ranks 14/18 and 15/23/25 in
a top-50 continuation. New controls kept their frozen top-ten `(path,score)`
prefixes unchanged: `bat.osa.182` found its declaration file at rank 11
(`/tmp/u0`), and `typeorm.osa.209` found five declaration files at ranks
11/12/13/26/27 (`/tmp/u1`). These four cases establish ranking and result
limit as their failure stage; they do not prove that every capped miss has the
same cause. The TypeORM first page ranks usages, tests, and fixture files ahead
of declarations with tied scores. A content-file search and a declaration
navigation judgment remain distinct contracts.

An offline join of all 60 misses against the twelve v6 gold capsules and the
raw candidate rows confirmed that each first page has ten distinct files and
none includes an intended file. All 60 source partitions have exactly one
near declaration name and no extra near declaration file; 37 have one gold
file. Thus near-name ambiguity does not explain these specific synthetic
target misses, although their user relevance remains unreviewed. The
per-task paths, scores, source-capsule digests and record/report bindings are
in `/private/tmp/qi-b08-product-8e8592f4-20261003/quanta-typo-miss-partition.json`
(SHA-256 `1b49b9a696c27ee8fc8d50e1840ecb6a1c3ef9866d89c5d7545a106ff04c191d`).
Only the four top-50 continuations above establish their gold ranks beyond
ten; no beyond-window rank is inferred for the other 56 misses.

**VERIFIED, separately scoped native cs diagnostic:** the exact same `lo`
358-task typo suite was submitted as `cs 3.2.0` native `~1` under
`/private/tmp/qi-b08-product-8e8592f4-20261003/cs-fuzzy-lo-9d38`.
Its implementation checks only same-length content windows, so the shared
edit-class report admits 91 ASCII single substitutions: Quanta 91/91 versus
cs 90/91 file Hit@10. The other 267 submitted rows stay visible by operation
but are excluded from this shared class. The source-bound replayed report is
`/private/tmp/qi-b08-product-8e8592f4-20261003/cs-fuzzy-lo-final-report.json`;
its scorer/source binding is adjacent. Source correction commits `ba9cf4c9`
and `239d1907` have focused tests. The cs capture explicitly lacks indexed
universe attestation, so these numbers do not establish a product ranking.

**NOT_RUN:** twelve-repository matched default-file product capture, current
five-product external service capture, independently reviewed subjective
labels, qualified file-ranking C5 contract/capture, and a release decision.
Sourcegraph/OpenGrok localhost endpoints 7080/7081 refused connection during
this diagnostic. Existing `lo` exact-name paired capture at `/tmp/qx` passed
native pair replay with both Quanta and Semble at 98/98 file Hit@10; it remains
`diagnostic_unqualified`.

## Twelve-repository exact-name paired diagnostic (2026-10-03)

**VERIFIED, diagnostic only:** clean `9d38b69d` completed all twelve
`declaration_name_exact` Quanta/Semble native pair captures and replays from
the frozen C4 v6 matrix. The matrix selected 1,252 requests; 1,132 have paired
file judgments. The other 120 requests are separate no-answer/other tasks and
must not enter the file Hit@10 denominator. An initial offline summarizer
assertion exposed that denominator distinction; after correcting the
summarizer, each route's per-query task IDs matched the report's eligible IDs
and the selected matrix IDs. The result is
`/private/tmp/qi-b08-product-8e8592f4-20261003/exact-pair-summary.json`
(SHA-256 `39dab0f3810acae3fff98205c90e67eda26e6ed9f3ea6286a3bb83fb5bd7c96a`).

| Route | File Hit@10 | MRR@10 | NDCG@10 |
| --- | ---: | ---: | ---: |
| Quanta lexical | 1,108 / 1,132 | 0.8723 | 0.8971 |
| Semble lexical-file | 1,111 / 1,132 | 0.8688 | 0.8945 |

The sequential sum of run-plus-replay wall times is 1,945.258s excluding the
previously reused `lo` capture; it is not query latency or a performance
comparison. These source-exposed mechanical labels lack human relevance review
and no product ranking or qualified file decision follows from this diagnostic.

An independent join of all twelve exact suites, native records and scored
rows reproduced every file Hit@10 bit using `(path, file_sha256)` rather than
the evaluator's aggregate. Quanta missed 24 and Semble missed 21; twelve task
IDs overlap. Each Quanta miss returned ten distinct files with `capped`
status; each Semble miss returned ten distinct files with `success` status.
Neither status proves a rank beyond the returned window. Names including
`test`, `label`, `show`, `Wait` and `timeout` are bare and may match many
uses; the source-oracle declaration file is a mechanical target, not a reviewed
user-intent judgment. The task-level gold, top-ten paths, record/report digests
and statuses are in
`/private/tmp/qi-b08-product-8e8592f4-20261003/exact-pair-miss-audit.json`
(SHA-256 `d98dd3e9cd974a6b6f8845efccda9d2d2b6a633a141674917fb545235688e177`).
For example, `mocha.def.041` queries bare `test`: its mechanical declaration
file is `lib/reporters/xunit.js`, while Quanta returned ten test/fixture
files at equal score `142.0`. This is a returned-window ranking observation,
not proof that the declaration file is absent from the index or that a generic
file-search user intended the declaration.

Read-only offline file-evidence replay at evaluator `main@91b18b48` used the
existing exploratory `/tmp/qx` `lo` pair, source commit
`5c6ddcb7063c9908031340db03f27ba7483d1ece`, suite SHA-256
`aa6e7bb9767f593f2db07e45775c98991cae6cd522248136d7559c98864143c2`,
Quanta record SHA-256 `83ef50820d440d62cb2cfca5ec72c45a2f42f6625a8193cce0bef504ad46c6c4`
and Semble record SHA-256
`b6e34a311d8e6b115083d502431edc6c126fc81be861fa150c41b3882ea94edd`.
`merge_records` and `evaluate_complete_scored_file_evidence` rederived 98
paired positive rows under `file_ndcg_at_10` (Quanta minus Semble mean delta
`0.005574468530525119`); both qualified uncertainty-shape checks returned
true. The frozen `/tmp/qx` manifest is exploratory and its verdict has
`QUALITY_DELTA: not_applicable`. All 108 tasks have mechanical source-oracle
labels; schema-v3 admission explicitly refuses a suite with no human-reviewed
task. A focused negative test verifies that refusal. This verifies the
evaluator/replay path only.

The same current scored-file evaluator at `main@ff2bae33` also consumed the
native Quanta/Semble records for **all twelve** exact-name repositories via
`merge_records`, then produced `evidence_unqualified` on each. It revalidated
the source-bound suites and records, scored all 1,132 paired positive tasks,
and retained 20 no-answer route observations (ten tasks) in each repository. The
positive-task-weighted Quanta minus Semble file NDCG@10 delta is `+0.00258515`;
this is a diagnostic arithmetic check, not a qualified effect estimate.
Per-repository deltas, source-file hashes, record hashes and evaluator report
hashes are in
`/private/tmp/qi-b08-product-8e8592f4-20261003/exact-complete-file-evidence.json`
(SHA-256 `c63a0e3785db5c52c57a5541de9008087def30c01e377b1d777dad6b59e2c3ab`).
An independent read-only pass over those twelve roots also called
`replay_complete_scored_file_report` and both qualified uncertainty-shape
checks: **12/12** reports and both checks passed, with per-repository positive
counts exactly matching the frozen summary (1,132 total). It wrote no capture.
No qualified native capture, human review, external index attestation or C5
decision was performed by this replay.

## Current code verification boundary (2026-10-03)

At clean `main@5121711c`, the affected C0-C4 Python rails passed:
`test_corpus_release.py`, `test_gold_oracle.py`, `test_corpus_binding.py`,
`test_code_search_matrix.py`, `test_code_search_workflow.py` (**214/214**);
`retrieval/test_corpus_set.py` (**4/4**);
`test_identifier_robustness_report.py` and `test_source_oracle_suite.py`
(**72/72**); `test_completed_response_timing.py` (**20/20**); and
`test_holdout_review.py` plus `test_holdout_c4.py` (**84/84**).
The full retrieval rail passed **447/447** at `ff2bae33`; the retrieval
runner/evaluator/decision sources and that test file are unchanged through
`5121711c`. `pm.py lint` passed and the checkout is clean. These checks prove
the tested code surfaces, not human label custody, an unmocked qualified pair
capture, B07 host performance or the C5 product decision.

## Twelve-repository partial-name paired diagnostic (2026-10-03)

**VERIFIED, diagnostic only:** clean `9d38b69d` completed and replayed all 36
prefix, infix and components C4 v6 cells across twelve repositories. Every
selected task was paired eligible (222/222); all cell verdicts passed.
The route counts below were also recomputed directly from suite file gold
`(path, file_sha256)` and both native records, independently of the report's
aggregate. The complete per-repository and per-intent output is
`/private/tmp/qi-b08-product-8e8592f4-20261003/partial-pair-summary.json`
(SHA-256 `8e8406b8750da710135fc70c1164a6d4e6ec70b23224c0d687d7ee04c8dda379`).

| Intent | Paired tasks | Quanta file Hit@10 | Semble file Hit@10 |
| --- | ---: | ---: | ---: |
| Components | 71 | 67 | 69 |
| Infix | 74 | 73 | 29 |
| Prefix | 77 | 73 | 48 |

All nine Quanta misses returned ten distinct files with `capped` status.
Of Semble's 45 infix misses, 26 abstained; of its 29 prefix misses, 16
abstained. The remaining returned results did not contain gold; their
beyond-window ranks are unknown. These are
mechanical intended-declaration-file targets and have no human relevance
review. The sequential run-plus-replay wall sum was 3,969.976s on a contended
host; it is not a query-latency comparison. No five-product or C5 qualified
claim follows from this diagnostic.

## Component-query miss audit and qualification boundary (2026-10-03)

The four Quanta component misses in the frozen 12-repository capture are
`cli.com.002` (`test update available no current version`), `mocha.com.006`
(`clean up`), `zellij.com.004` (`version info`) and `zellij.com.005`
(`home unix`). All four returned ten distinct files with `capped` status;
the capture does not locate their mechanical target beyond rank ten. Their
suite gold files and source SHA-256 values match the pinned source view.
`zellij.com.005` illustrates the intent mismatch: the source oracle labels
`zellij-utils/src/lib.rs` because line 10 declares `mod home_unix`, while
the implementation file `zellij-utils/src/home_unix.rs` is returned at rank
two. The other three queries also contain ordinary terms that occur outside
the generated declaration target. These observations require independent
relevance review before a score change can be called an improvement.
The task-level qrels are in
`/private/tmp/qi-b08-c4-bundle-9d38b69d-20261003/{cli,mocha,zellij}/declaration_name_components/suite.json`;
the corresponding original Quanta rows are in
`/tmp/{v03,v12,v30}/rep-00/quanta/strategy-00-fixed_window_strict/record.json`.

An isolated top-50 control at `/private/tmp/qc-ekzx7sj0` bound the original
runner and searchd binary digests, the pinned corpus commits and manifests,
and derived one- or two-query packs with a 50-file window. Two pre-index
attempts were refused by the runner (pack route mismatch, then missing runner
identity). The corrected `cli` attempt reached publish/activate but **FAILED**
with a 30-second IPC read timeout on a contended host (load average above 40).
It produced no query record. `mocha` and `zellij` top-50 controls are
`NOT_RUN`; no off-page rank or candidate-admission conclusion is drawn for
the four misses.

The existing `holdout_review.write` adapter produced two blinded, unjudged
forms per repository for these four tasks at
`/private/tmp/qc-ekzx7sj0/{cli,mocha,zellij}/review/`. Each form contains
frozen source text from both original retrieval routes, a source alternative
and a random control; product membership, scores and ranks are withheld in
owner custody. Form digests replayed for all three repositories. This is
review preparation only: no reviewer identities, grades or adjudication were
created, and the exposed diagnostic cases are not a fresh holdout.

Qualified `code_search_file` capture and verdict now require every positive
evaluation task to declare `default_file_search` with
`complete_ranked_pool_v1` and no mechanical `source_oracle`. Mechanical
no-answer controls remain admissible. The capture preflight rejects an
unreviewed declaration-derived positive before indexing, and verdict replay
rechecks the same condition. Existing component, exact-name and typo captures
remain diagnostic. The independently reviewed relevance judgments and
unmocked qualified pair remain `NOT_RUN`.

At source base `a867f47b` with this focused overlay, the full
`test_retrieval_benchmark.py` rail passed **448/448** in 666.56s. The
additional verdict-boundary test added after collection passed with the
capture-boundary test in a focused **2/2** run (16.77s); a new full-file
449-test run is `NOT_RUN`. `ruff check`, `ruff format --check`, `git diff
--check` and `pm.py lint` passed. These are code checks, not a qualified
product capture or a completed relevance review.

### Component misses: source-bound scoring RCA (2026-10-03)

The original `retrieval-diagnostic.json` responses at `/tmp/{v03,v12,v30}`
retain an exact candidate count, even though the merged `record.json` shows
only the top ten. The frozen `release-v4/views/{cli,mocha,zellij}/code_only`
files match all 1,909 suite file hashes. A read-only source-level scorer
reimplemented the current CodeSearch literal boundary, capped occurrence,
case-exact and proximity terms. Its top-ten paths and scores matched all
40 original Quanta rows; its matching-file counts equaled the native exact
candidate counts. No native off-page cursor was fetched, so the gold ranks
below are **offline reconstructed ranks**, not captured product ranks.

| Task | Native exact candidates | Gold score | Native rank-10 score | Offline gold rank | Semble gold rank |
| --- | ---: | ---: | ---: | ---: | ---: |
| `cli.com.002` | 14 | 457 | 533 | 12 | 3 |
| `mocha.com.006` | 22 | 165 | 197 | 14 | 1 |
| `zellij.com.004` | 43 | 185 | 222 | 27 | 4 |
| `zellij.com.005` | 13 | 185 | 211 | 12 | 6 |

All four gold sources are ASCII and match their suite SHA-256 values. The
native windows report `capped` with ten distinct files, exact candidate and
examined counts, and continuation. The gold scores are below the observed
tenth-place scores by 76, 32, 37 and 26 points. Thus the current scoring and
top-ten truncation suffice to explain each miss; an index omission is not
required. Index-universe attestation is still absent, and native ranks beyond
ten remain unobserved.

The query generator splits declaration names into lowercase camel/snake
components, while `code_search_file` submits those words unchanged to the
public general file search. `code_search.rs` then requires each substring
somewhere in a file and sums each term's best content/path boundary score;
the proximity contribution is at most 32. It neither joins terms at one
declaration nor consults the indexed symbol local name. Embedded terms in
`cleanUp`, `VersionInfo` and `home_unix` receive weaker boundary scores than
standalone words elsewhere. `zellij.com.005` additionally labels the module
declaration file although the implementation file is already Quanta rank 2.
This is a confirmed query-intent/ranking-contract mismatch; whether a given
top-ten alternative is irrelevant requires independent source-backed grades.

Resolve it on two separate contracts. For declaration-component lookup,
introduce an explicit typed symbol-name component route with bounded
component postings, ordered-component verification, symbol identity and a
defined file projection. Test exact adjacency, case, duplicate names,
module declarations, no-answer, posting cap and cursor order against fixed
source oracles. For default general file search, retain file-level content
and path matching; test any bounded declaration feature or fielded reranker
only on independently reviewed graded file relevance. Do not turn these four
source-exposed cases into a qualified improvement claim or silently apply a
symbol-only preference to every file query.

## Twelve-repository ordinary-input OSA1 baseline (2026-10-03)

**VERIFIED, diagnostic only:** the frozen `9d38b69d` runner completed all 12
projected ordinary-input OSA1 cells. Every pair verdict passed. An external
checker independently joined `(path, file_sha256)` gold to both native
distinct-file records and checked 4,206 task IDs, report scores and binary,
suite, pack, manifest and verdict digests. Its output is
`/private/tmp/qi-b08-default-osa1-20261003/default-pair-summary.json`
(SHA-256 `e41a00d8196ea5490821a4aa8fb9f0c7048c9238adfb032b0e5065ebd5195a50`).

| Route | File Hit@10 | Statuses |
| --- | ---: | --- |
| Quanta ordinary input | 112 / 4,206 | 4,077 abstained; 115 success; 14 capped |
| Semble lexical-file | 3,053 / 4,206 | 3,769 success; 437 abstained |
| Quanta explicit OSA1, separate request | 4,146 / 4,206 | Separate source-bound capture |

The explicit OSA1 capture hit 4,025 of Quanta ordinary-input abstentions.
This measures a request-policy gap in the old default route, not an index
omission or a qualified product ranking. The serialized run walls summed
11,202.907 seconds excluding the preverified `bat` row, under concurrent
workload and repeated setup; they do not support a speed comparison. The
mechanical intended-declaration targets have no human file-relevance review.
The later `12fe7d9f` empty-result fallback is being captured separately;
its results must not be merged with this older baseline as if source and
binary identities were unchanged.

## Next unseen holdout preparation (2026-10-03)

The twelve repositories above have now been exposed to tuning diagnostics,
so a later C5 decision needs a new roster. Before querying the reserve
repositories, the original C0 candidate order was filtered against its
selected/rejected ledger, the development release and the exposed release.
The resulting twelve-cell ordered reserve list is frozen at
`/private/tmp/qi-c5-fresh-20261003/selection-precommit.json`
(SHA-256 `b31c8a5198221585b3c536f7a2d87be79ae471b903243e424fa951e325ed2a70`).
It is a selection rule, not twelve accepted repositories. Each candidate
still needs current license, size, complete-history, overlap and view checks.

The first Go/small reserve, `rs/zerolog@56591163bce358abdb860d3dad599d3ab440621a`,
has a complete Git checkout and a source-replayed candidate release at
`/private/tmp/qi-c5-fresh-20261003/zerolog-release` (release digest
`sha256:b35ff03b121ec5dce0d8051a85ca56e272fcbfb3ce2d72af7e770eee482a1c48`).
Its frozen views contain 82 `code_only` and 99 `developer_search` files.
Preflight found zero identical code blobs and zero shared root commits across
the ten development and twelve exposed evaluation views; the external
preflight is `/private/tmp/qi-c5-fresh-20261003/zerolog-preflight.json`
(SHA-256 `1fd8fa1019035af44cbba2ae946e7940f31bb3a7923f2afd2efb5089bfaf915c`).
Near-duplicate source audit, labels and product captures are **NOT_RUN**.
This candidate is not yet C5-admitted.

The same frozen candidate order has since produced source-replayed releases
for the remaining three small cells: `zoxide` (26 code files), `attrs` (56)
and `immer` (55). Together with `zerolog` (82), these four candidates have
219 `code_only` files and zero identical code blobs or shared root commits
against the 22 prior development/evaluation views. The per-candidate commits,
release digests and preflight limitations are in
`/private/tmp/qi-c5-fresh-20261003/candidate-status.json`
(SHA-256 `839bb8dd898a6dd72ed4d2f874025741ec7a8a44df7c13b0eb3a6bb7d2753903`).
This does not discharge near-duplicate checks or admit any of the four.
The original three Go/medium reserves measured 258, 89 and 181 `code_only`
files, below the preregistered 301-file lower bound. Before checking any
replacement source, an ordered public-metadata amendment was frozen at
`/private/tmp/qi-c5-fresh-20261003/selection-amendment-go-medium-1.json`
(SHA-256 `ee5510f713e32b58583459f9632d1b5f26c1c484c89e2f4eab80775b8dd90a2c`).
Its first candidate, `grpc/grpc-go@de4775bffabedc6674c131d88212a510c32e1ab1`,
replayed to a provisional release with 1,081 `code_only` files and no
identical code blob or shared root commit against the 22 prior views. The
five frozen candidates total 1,300 code files; their digests and the three
size refusals are in `/private/tmp/qi-c5-fresh-20261003/candidate-status-v2.json`
(SHA-256 `0fe6a3231fd4e92a8d213791f1c401852e967afe67b7331d9e25d2de35044e61`).
Near-duplicate audit and the other seven cells remain **NOT_RUN**.

Nine of the twelve fresh candidate cells now have complete-history checkouts
and source-replayed provisional releases. They contain 6,264 `code_only`
files. The source/manifest/release bindings, commits and per-cell counts are
in `/private/tmp/qi-c5-fresh-20261003/candidate-status-v4.json` (SHA-256
`aff99bea5eb2e3be46bd21e6903a73f0980171a2e7ec0d19470f0d340ab7f7ae`).
The existing split-leakage winnowing policy was run over all nine candidates
against the ten development and twelve exposed evaluation repositories, plus
all cross-candidate pairs. It found zero exact copies above the policy's
256-byte floor and zero near-duplicate pairs. The complete census is
`/private/tmp/qi-c5-fresh-20261003/near-audit-v1.json` (SHA-256
`5c75f136e2cab8447cd960fc0a8dbec4c4f2824e4e9b7ee5e2cba1e25be76afd`).
One 26-byte Celery example stub still matches a Django file below that floor;
it remains visible in the preflight and is not counted as a policy violation.
Three cells, independent relevance labels, index attestation and product
captures remain **NOT_RUN**. None of these candidates is C5-admitted yet.

## C3 natural-language relevance review, multi-model AI (2026-10-03)

**Policy amendment (owner decision):** the human-reviewer requirement for this
C3 NL lane was replaced by multi-model AI review. Identities are
`ai:claude-opus-5-5:pass-A`, `ai:claude-sonnet-5-5:pass-B` and adjudicator
`ai:claude-fable-5-1:adjudicator`; no AI decision is recorded as a human
reviewer or human receipt. These labels are **multi-model AI-reviewed, not
human-reviewed, and not qualified**. External root:
`/Users/songmin/Documents/code-new/qi-b08-c3-nl-review-20261003-OmZWCc`
([RESULTS.md](/Users/songmin/Documents/code-new/qi-b08-c3-nl-review-20261003-OmZWCc/RESULTS.md),
SHA-256 `b332021b58aff850b9da6b46d98f7deebc45efc446c1a5af5fbde9f7b2535361`;
file inventory `SHA256SUMS-root` SHA-256
`a144fbf13dd3a457719685f41c98cd53346c2ca259f0261f5813cbc3979e3673`).

- Start binding: `12fe7d9f` clean; 12 frozen checkouts clean; release
  `sha256:cb896b12…`, code_only manifests and v5 gold identities recorded.
  The 106-task mechanical packet was neither used nor changed; it stays a
  separate denominator.
- Relevance rubric (0–3, examples, boundary rules) and area taxonomy were
  frozen before pooling. 240/240 NL/workflow tasks were authored (20 per
  repository; 48/48/48/36/36/24 across six areas; underfill 0).
- Blind packets: BM25 and path/declaration pools, author source anchors and
  two seeded random controls per task, deduplicated by path; every candidate's
  full text and SHA-256 matched the frozen manifest. `holdout_review.write`
  produced two forms per pack (13 packs; zellij split by the 64 MiB export cap).
  Missing-candidate proposals re-issued all 13 packs (+638 files) and tailscale
  once more (+1); both passes graded every added file. Final judged task/file
  pairs: 4,346.
- Pass agreement: exact grade 94.3%, quadratic-weighted κ 0.966,
  answerability 240/240 (same-family models; not evidence of correctness).
  Adjudication: 246 items, 0 unjudged. Final qrels: 240 answerable tasks,
  grades 0/1/2/3 = 2,258/1,246/574/268; 240 included, 0 excluded.
- Final per-repository mixed suites keep the v6 C4 `declaration_name_exact`
  source-oracle tasks unchanged and add 20 reviewed NL tasks
  (`complete_ranked_pool_v1`, `label_review` bound to an evidence SHA over both
  passes and the adjudication). Two schema-v2 annotation receipts and one
  adjudication receipt cover only the subjective tasks in suite order and bind
  the suite SHA and annotation receipt SHAs.

**VERIFIED** at a pinned export of `d1bb1438` (12/12 repositories):
`evaluator.validate_suite`, `holdout_review.validate_completed_forms` (13
packs) and `run._validate_gold_review_receipt` for all three receipts;
`test_holdout_review.py` 38/38 in that export.

**BLOCKED, qualification admission (12/12):**
`run._validate_disjoint_admission_source` requires the suite family set to
equal the split row (130 families including 20 literal). The NL families are
absent from the frozen split, and literal tasks cannot share a file-search
suite. Separately, `fd13216e` changed `tools/benchmark/corpus_release.py`;
release-v4's `generator_digest` matches that file only through `d1bb1438`, so
at current main release-v4 validation fails before the admission comparison.

**BLOCKED, product capture:** the frozen NL queries pass the
`natural_language` request policy (240/240) but only 9/240 pass
`code_search_file`. `natural_language` has no evaluation request mode and
returns chunk units, which file judgments exclude. Queries were not rewritten,
and no product ranking is reported.

**NOT_RUN:** human identity verification (superseded by the amendment), the
full `verify_admission_bundle`, and live index attestation.

## Component route verification and coverage limit (2026-10-03)

At clean `main@19e5fa55`, the explicit `components:"word word"` route and
`code_search_components_file` benchmark policy are wired. A follow-up audit
removed a query-specific source-byte absence exemption: the product's
`SymbolRecord.local_name` contract does not require the name to be a literal
source-byte slice, so raw-byte absence cannot prove that a file with an
incomplete symbol census has no matching declaration. The route now requires
complete symbol coverage for every in-scope file and returns
`SymbolCoverageIncomplete` otherwise. Exact path and language constraints may
exclude an incomplete file before that check. A focused regression proves
that a parse-failed file with no literal query component still causes refusal.

**VERIFIED:** `./scripts/cargow --lane component-check-lane check -p
quanta-index-search-plane -p quanta-index-lexical -p
quanta-index-retrieval-bench --locked`; lexical `l3_exact_source` 22/22;
search-plane component tests 2/2; retrieval-bench component request test 1/1;
live-driver `runtime_risk_suite` component E2E 1/1 under `--all-features`;
`test_holdout_c4.py` 48/48; Rust format check and `git diff --check`.
The E2E publishes four source files, searches through CodeSearch syntax,
checks exact-name precedence and distinct-file order, rejects terms split
across separate symbols, and checks an empty answer. These checks prove the
synthetic product route and benchmark request binding, not a twelve-repository
quality improvement.

**Remaining product limitation:** the frozen diagnostic preflight reports
eleven `parse_failed` Go files in `cli`, two intentionally syntax-error
JavaScript fixtures in `mocha`, and none in `zellij`. If those coverage states
recur, the current unscoped component route refuses the first two repositories;
the old four misses
cannot be declared fixed from the synthetic E2E. Three inspected `cli` Go
parse errors occur at `new(expression)` calls in its Go 1.27 source (for
example `pkg/cmd/codespace/create_test.go:107`); this establishes a parser
compatibility symptom, not the cause of all eleven failures. The separately
frozen release-v4 also fails current-source generator-digest validation after
`fd13216e`, so an actual current-source four-query or full C4 capture
requires a new release/capsule/output root. **NOT_RUN:** that recapture,
parser remediation, partial-coverage response design, independently reviewed
file relevance, and C5 qualification. Do not score a typed coverage refusal
as a top-ten miss or merge any future recapture into the frozen C4 totals.

## Exposed twelve-repository scale diagnostic (2026-10-03)

**VERIFIED for source-bound preparation and admission; `diagnostic_unqualified` for
quality.** Clean `a867f47b8dd4ca5efa36b0cca671c84d258c204a` sampled the
already exposed release-v4 with `scale_diagnostic_v1`, seed `3797249375`.
The frozen ledger is
`/private/tmp/qi-scale-diagnostic-20261003-a/ledger.json` (SHA-256
`bfb5f9fa63c0e675d102dc7af46c645e6e9397ad315685fb485c50e2e6ab1980`).
Its 12 recipes contain 13,823 task rows: 1,200 exact content, 1,200 exact
positive declarations, 1,428 prefix, 1,422 infix, 1,397 components, 4,776
case-folded OSA1, and 1,200 each synthetic and wrong-repository negatives.
The source replay published 12/12 gold capsules at
`/private/tmp/qi-scale-gold-a867-20261003`; all bind the same release digest
`sha256:cb896b12a8faf9569a829469bc39fe8c8bd9869ce908494722c0bf8f829bf1df`.

The C4 matrix at `/private/tmp/qi-scale-c4-a867-20261003/admission-matrix.json`
(SHA-256 `22a23fb51505e172dff4c45d2eb05d09749dbfb65f864ac9ae80f111e667a0ed`)
contains 72 cells, 60 admitted suites and 12 empty case-sensitive OSA1 cells;
the latter intent was not sampled. Exact-file literal admission is separate.

| Mechanical lane | Source candidates | Admitted | Excluded |
| --- | ---: | ---: | ---: |
| Exact declaration, positive | 1,200 | 1,132 | 68 |
| Prefix | 1,428 | 1,215 | 213 |
| Infix | 1,422 | 1,056 | 366 |
| Components | 1,397 | 1,115 | 282 |
| Case-folded OSA1 | 4,776 | 4,206 | 570 |
| Synthetic no-answer | 1,200 | 1,200 | 0 |
| Wrong-repository no-answer | 1,200 | 1,200 | 0 |
| Exact content literal | 1,200 | 1,196 | 4 |

The exact-content matrix is
`/private/tmp/qi-scale-literal-fixed2-a867-20261003/literal-matrix.json`
(SHA-256 `a47dd48e1dd1f87ff14f64a37361ccfb26c8932a9570de379fbc36868f10befa`).
It uses the frozen `a867f47b` producer plus only the current literal adapter
patch (file SHA-256
`4f3ecca0b476d3ee70a7186c535ae4ab38c29f2ab633b8ab2ef8375050f255cd`;
patch SHA-256
`389405ed0f8d209dddc14e202cd7e3b6aec0c52b6a6921b8e2b1625c28293fa0`).
The initial literal admission **FAILED** because `cli.lit.077` and
`cli.lit.099` contain tabs: the sampler accepted stripped lines with internal
control characters, while the independent literal oracle refuses them.
`lo.lit.042` and `zellij.lit.056` were also near-duplicates. The sampler now
checks the literal query contract before selection; the adapter records these
four exclusions without post-score backfill. The superseded second attempt
was intentionally interrupted before admission. Fixed tests passed:
`test_gold_oracle.py` plus `test_holdout_literal.py` 90/90 on the first fix;
the final literal file 22/22 and four focused boundary tests passed after the
duplicate fix. Ruff and format checks passed.

Independent accounting matched every C4 candidate ID to exactly one selected
or excluded ID, verified 60 suite/blind-pack hashes, and split the exact lane
into 1,132 positive plus 1,200+1,200 negatives. The same check matched all
1,200 literal IDs to 1,196 selected plus four excluded, verified 12 suites and
their pack hashes. No task was scored as failed merely because its source
census was unsupported or its query contract was invalid. The global counts
exceed 1,000 for the sampled mechanical lanes; per-repository cells remain
uneven, including small `zustand` and parser-limited `tailscale`. Natural
language and case-sensitive OSA1 remain unsampled.

Execution commands, from the frozen source root unless otherwise noted:

```text
python tools/benchmark/retrieval/holdout_sampling.py --holdout-release RELEASE_V4 --development-release DEV_RELEASE_V3 --seed 3797249375 --profile scale_diagnostic_v1 --output /private/tmp/qi-scale-diagnostic-20261003-a
python tools/benchmark/retrieval/gold_capture_batch.py --release RELEASE_V4 --other-release DEV_RELEASE_V3 --sampling /private/tmp/qi-scale-diagnostic-20261003-a --output /private/tmp/qi-scale-gold-a867-20261003
PYTHONPATH=FROZEN_ROOT python tools/benchmark/retrieval/holdout_c4.py --release RELEASE_V4 --capsules /private/tmp/qi-scale-gold-a867-20261003 --checkouts CHECKOUT_ROOT --output /private/tmp/qi-scale-c4-a867-20261003 --expected-repositories 12 --emit-suites
python tools/benchmark/retrieval/holdout_literal.py --release RELEASE_V4 --capsules /private/tmp/qi-scale-gold-a867-20261003 --checkouts CHECKOUT_ROOT --output /private/tmp/qi-scale-literal-fixed2-a867-20261003 --expected-repositories 12
```

`RELEASE_V4`, `DEV_RELEASE_V3`, `CHECKOUT_ROOT` and `FROZEN_ROOT` are respectively
`/Users/songmin/Documents/code-new/qi-s30-b08-holdout-20261002/{release-v4,dev-release-v3,checkouts}`
and `/private/tmp/qi-scale-src-a867f47b`; the final literal command ran from
`/private/tmp/qi-scale-literal-fix-a867-20261003`. Observed walls were about
64 minutes for gold, 79 minutes for C4, and 44 minutes for fixed literal
admission on a host with load average roughly 30–70. These are preparation
costs, not search latency. The separate large-inventory census stdin/stdout
pipe deadlock was fixed with a file-backed stdin and a large-input test.

**NOT_RUN:** product captures on these newly admitted suites, human relevance
review, fresh post-tuning holdout qualification, C5 decision and deployment.
Sourcegraph/OpenGrok local endpoints on ports 7080/7081 refused connections
at this audit; their full indexed universes are unattested. Do not combine
these source-exposed mechanical cases with an independent holdout, infer a
five-product ranking, or promote this admission matrix to a product decision.

## Fresh C5 source preparation (2026-10-03)

**VERIFIED for roster, source release, sampling and split validation;
`diagnostic_unqualified` for quality.** Before any C5 product output, the
selection ledger froze twelve additional complete-history repositories:
zerolog/grpc-go/telegraf, zoxide/tauri/rust-analyzer, attrs/celery/sympy,
and immer/chartjs/svelte. These have 11,695 `code_only` files. The provisional
holdout release is
`/private/tmp/qi-c5-fresh-20261003/combined-release`, digest
`sha256:3ce648056f1e6fc6e927eb5fdcadfacbee69d3aab8d6444fdb59be302c094b56`.
An exact/near-file audit against the ten development and twelve previously
exposed repositories found no overlap above the declared 256-byte exact-file
threshold or near-duplicate threshold. Small identical stubs below that
threshold remain visible in the audit. License source files are pinned but
approval is not attested.

The old ten-repository development release used an earlier generator digest,
so the current split validator correctly refused it. Clean source snapshot
`40fbb710` rebuilt the same ten commits with the current generator into
`/private/tmp/qi-c5-fresh-20261003/development-release-current`, digest
`sha256:179f1f885e2c4f8104903d8624fec52e7f9c0f5148d048bcac32bac664ac1c70`.
All 6,477 development `code_only` files and both release views matched the
older release by repository. The predeclared `baseline_v3` sample was rerun
with the same seed into `sampling-v4`; all 6,628 task rows, census summaries
and lane counts match `sampling-v3`. The 22-repository source-replayed split
validator passed in 337.324 seconds. Binding receipt:
`/private/tmp/qi-c5-fresh-20261003/split-validation-v4.json`.

The sampled tasks are 240 exact content, 1,200 exact declarations, 96 prefix,
96 infix, 84 components, 4,792 OSA1 variants, and 60 each synthetic and
wrong-repository negatives. Eight OSA1 proposals underfilled. Natural-language
and workflow tasks underfilled 240/240. The independent declaration census
refused 298 of rust-analyzer's 1,462 Rust files; those files are unsupported,
not empty gold. Source gold capture and review remain separate admission gates.
The retained first sampling attempt exited with a process crash before output;
two later runs under the same seed produced byte-identical output. The current
source-matched run used a new output root and changed only the split digest.

**NOT_RUN for C5:** product retrieval, human relevance review, five-product
index-universe attestation, repository-cluster decision, quiet-host timing,
and deployment. The separate exposed twelve-repository diagnostics above
cannot be folded into this fresh set.

## Fresh C5 mixed-language correction (2026-10-03)

**VERIFIED for source-only amendment and split; still unqualified for quality.**
The first C5 release used TypeScript declaration gold for all three JS/TS
repositories, while `chartjs` has 659 JavaScript versus 85 TypeScript files
and `svelte` has 3,447 JavaScript versus 86 TypeScript files in `code_only`.
For example, `chartjs.def.020` names a TypeScript declaration in
`src/types/index.d.ts` and JavaScript method implementations in two `src`
files. An unscoped file-search request cannot treat those implementations as
known irrelevant files. The benchmark owner now marks a declaration task
`unjudged` when another supported language could contain a matching name;
the mixed-language fixture and affected gold/C4 tests passed 117/117.

Before C5 product capture, a source-only amendment at
`/private/tmp/qi-c5-fresh-20261003-v5/amendment.json` froze the rule: choose
each JS/TS repository's declaration language by the larger admitted extension
count, with a deterministic tie break. This changes `chartjs` and `svelte` to
JavaScript and leaves `immer` as TypeScript. The twelve repositories, commits,
11,695 `code_only` files, and both materialized views remain unchanged. Clean
`7e942bb0` produced the replacement release at
`/private/tmp/qi-c5-fresh-20261003-v5/combined-release`, digest
`sha256:2ec04aa364c573854dfeb7545144b658219f66b44e64ba05f42198dac32a58f2`.
Its release creation and independent replay passed. The old release and
terminated pre-amendment gold attempt remain separate evidence; no old values
were overwritten.

The same seed/profile generated 6,628 tasks in `sampling-v5`, with the same
lane totals and the prior eight OSA1 and 240 natural-language/workflow
underfills. The ten repositories whose language did not change retained all
non-wrong-repository tasks. The negative candidate pool depends on the other
repositories' declared names; only `immer` changed five such task queries.
The 22-repository split, release identity, source bytes and leakage policy
replayed successfully in 807.031 seconds. Receipt:
`/private/tmp/qi-c5-fresh-20261003-v5/split-validation.json`.

An independent Rust census refusal audit found that 287 of the 298 refused
`rust-analyzer` files are under parser `test_data`; eleven are other source or
utility files. Both checker and tree-sitter refuse some intentionally invalid
fixtures. Ten non-`test_data` files were refused only by tree-sitter; observed
error sites include `dyn` lifetime ordering and macro token patterns. The
exact grammar-version cause is unverified. These remain explicit `unsupported` source
coverage, never negative gold. Details:
`/private/tmp/qi-c5-fresh-20261003-v5/rust-census-refusal-classes.json`.

**IN_PROGRESS:** source gold capture from the replacement release and exact
source. **NOT_RUN:** C5 product search, human review, external index-universe
attestation, repository-cluster decision and quiet-host performance. Source
text overlap in the three JS/TS repositories is a conservative unjudged bound,
not a count of independently reviewed relevant files.

## Scale admission receipt audit and literal split digest correction (2026-10-03)

An independent read-only join of the exposed scale ledger, all twelve recipes,
the twelve gold capsules, the 72 C4 cells, and the twelve literal cells matched
13,823 unique task IDs. Every candidate ID is selected or excluded exactly
once. All 60 emitted C4 suite, blind-pack, and admission hashes and all twelve
literal suite and blind-pack hashes matched their matrix entries. Final selected
counts are 1,132 positive exact declarations, 1,215 prefix, 1,056 infix,
1,115 components, 4,206 case-folded OSA1, 1,196 exact-content literals, and
2,400 exact-declaration negatives. Each sampled mechanical lane therefore
still exceeds 1,000 after source and query admission. This verifies input
accounting, not retrieval quality or an independent holdout.

The same join found a serialization defect: the frozen literal matrix records
`split_manifest_sha256` as `sha256:<64 hex>`, while its sampling ledger and C4
matrix use `<64 hex>`. The underlying 32-byte digest agrees. Commit
`aa5bccfe` makes the literal adapter emit the shared 64-hex form, with a
focused regression against the prefixed form. The original literal matrix and
its hash remain unchanged. Its field is a legacy-format receipt and cannot
pass a strict cross-matrix equality check without a fresh source-bound
admission run.

**VERIFIED:** `.venv/bin/python -m pytest -q
tools/ci/tests/test_holdout_literal.py` (23/23); independent read-only ID,
hash, release and split join over the above three frozen roots (12/12 recipes,
72/72 C4 cells and 12/12 literal cells). **NOT_RUN:** a fresh literal matrix,
product capture, relevance review and scale quality decision. The initial
`python3 -m pytest` attempt used macOS Python 3.9 and failed because
`zip(strict=True)` is unsupported there; the subsequent `python -m pytest`
attempt encountered a host pytest plugin mismatch. Neither failure is a
product-test result; the project `.venv` uses Python 3.12 and passed.

## Diagnostic ranking boundary audit (2026-10-03)

The exposed twelve-repository pair receipts were rejoined read-only from each
source-bound suite's `(path, file_sha256)` judgments and both native records.
The independently recomputed task-level Hit@10 bits sum to the frozen counts:

| Request and intent | Paired positive tasks | Quanta Hit@10 | Semble Hit@10 |
| --- | ---: | ---: | ---: |
| Default file, exact declaration name | 1,132 | 1,108 | 1,111 |
| Default file, prefix | 77 | 73 | 48 |
| Default file, infix | 74 | 73 | 29 |
| Components file, component name | 71 | 67 | 69 |
| Ordinary-input file, OSA1 typo | 4,206 | 112 | 3,053 |

Quanta's **explicit** OSA1 request separately reached 4,146/4,206; it is a
different request policy and is not substituted into the ordinary-input row.
The newer exposed scale suites have at least 1,000 admitted tasks per sampled
mechanical lane, but no corresponding product records. These tables cannot be
promoted to a scale ranking or a C5 decision.

The existing Gin five-product comparison remains the separate, source-exposed
99-file diagnostic in S30-B04. Its five lane summaries and original native
rows were independently rejoined for file Hit@10 on the common eligible task
IDs; all five products' counts match that report. Sourcegraph and OpenGrok
have no request-time indexed-universe attestation. At this audit, their local
service ports 17080/17081 and 7080/7081 were closed, and Docker's daemon was
unavailable. The preserved service state is Gin-specific, not a twelve-repo
index. **BLOCKED:** a fresh five-product scale comparison. **NOT_RUN:** fresh
scale Quanta/Semble capture, human-reviewed relevance, and C5 product decision.

## Typed component route: four-case product diagnostic (2026-10-03)

At build source `b1356784`, the producer publishes an explicit
`raw_ascii_local_name_v1` source policy. Ingest checks every emitted ASCII
local name against its definition bytes. For incomplete symbol coverage, the
component route excludes a file only when its committed source lacks a
requested component; otherwise it returns `SymbolCoverageIncomplete`. Sealed
lexical manifest format 12 forces format-11 generations to be rebuilt.

The four original `code_search_file` misses were rerun as a **different,
explicit** `code_search_components_file` request over the same pinned corpus
commits and unchanged mechanical gold. New suites and blind packs retained only
those four task IDs. The binaries, inputs, original rows, new records and
evaluation reports are joined in
`/private/tmp/qi-component-actual-kYGykT/verification.json`; the CLI and
Zellij successful retries are under `/private/tmp/qi-component-retry-9doaxN`.

| Task | New gold-file rank | New file | Query call |
| --- | ---: | --- | ---: |
| `cli.com.002` | 1 | `pkg/cmd/extension/extension_test.go` | 15.564 ms |
| `mocha.com.006` | 1 | `lib/reporters/base.js` | 11.770 ms |
| `zellij.com.004` | 1 | `zellij-utils/src/web_server_commands.rs` | 59.784 ms |
| `zellij.com.005` | 1 | `zellij-utils/src/lib.rs` | 22.784 ms |

All four records have `success`, `distinct_file`, `score_desc_path_tiebreak`
and native score evidence. `evaluate-diagnostic` replayed 4/4 Hit@10 and
MRR@10 = 1. The original requests remain `capped` ten-file misses; this is
contract-specific recovery, not a revision of those frozen scores. CLI and
Zellij first attempts reached publish but timed out at the 180-second IPC
read limit before producing any query record. Fresh-root retries completed
with larger limits: publish/activate took 168.986 s (CLI), 45.620 s (Mocha)
and 306.428 s (Zellij) on a heavily contended host. Those timings do not
qualify a performance comparison.

**VERIFIED:** contract source-name positive/negative test 1/1, lexical exact
source integration 22/22, manifest format rejection 1/1, and three fresh
SDK-to-daemon captures with four evaluated queries. **NOT_RUN:** full C4
recapture and C5 qualification. Gold remains mechanical and unreviewed;
product quality, external indexed-universe equivalence and a speed ranking
remain unqualified.

## Exposed ordinary-input typo pair completion (2026-10-03)

The source-bound `12fe7d9f` pair finished all twelve previously exposed
repositories. The independent raw-row join at
`/private/tmp/qi-default-auto-12fe-20261003/new-default-pair-summary.json`
has SHA-256 `9d2a36ee8111c91bd2f4421b2a23033db36c71b429540fbaa6e2e7e397d47934`.
It checks all 4,206 selected task IDs, source file hashes, distinct-file rank
units, native records, report scores and the separate explicit-typo capture.
The frozen projection manifest SHA-256 is
`a4b6a37cf269beb6de2537d68215f616910562e547c1688c6b0dcdecc1179bef`.

| Request | File Hit@10 | MRR@10 | NDCG@10 | Native status |
| --- | ---: | ---: | ---: | --- |
| Quanta ordinary input | 4,137/4,206 | 0.8739 | 0.8996 | 3,978 success; 228 capped |
| Semble lexical-file | 3,053/4,206 | 0.5051 | 0.5551 | 3,769 success; 437 abstained |
| Quanta explicit `typo:` | 4,146/4,206 | separate capture | separate capture | separate request policy |

Quanta ordinary input and explicit typo both hit 4,135 tasks; ordinary input
alone hit two, explicit typo alone hit eleven, and both missed 58. Of the 69
ordinary-input misses, 58 were `capped` and eleven `success`. Those 69 variant
tasks belong to 29 repository/name families, not 69 independent examples. The earlier
112/4,206 ordinary-input observation used the pre-fallback source and must not
be mixed with this capture. The new default route falls back to bounded OSA1
only when a folded, unscoped, bare identifier produces no literal file result;
existing literal matches retain their rank and can still crowd out the
intended declaration. These exposed examples are diagnosis, not tuning gold.

The sequential pair wall sum was 13,634.471 seconds excluding the separately
preverified `lo` row; `zellij` alone took 3,304.835 seconds. This includes
indexing, capture and source replay, and is not per-query latency. The frozen
source used an unanchored one-edit textual-exclusion regex during source
validation. Commit `5b1cf2f2` later bounded that search to possible start
positions; its focused parity tests and source-bound 1,336-exclusion audit
passed, but this frozen pair did not execute that later code.

**VERIFIED:** 12/12 pair receipts and independent joined counts.
**NOT_RUN:** fresh C5 product capture, subjective relevance review, matched
five-product index attestation, and qualified product-default decision.

## Fresh C5 gold and Sourcegraph projection boundary (2026-10-03)

The corrected twelve-repository C5 release (`sha256:2ec04aa3…`) produced
12/12 source-derived gold capsules at
`/private/tmp/qi-c5-fresh-20261003-v6/gold-v6`. The exact clean producer was
`dd86ec99`; the receipt is `gold-capture-receipt.json` in the same external
root. It records a 1,152.116-second wall and binds the unchanged sampling
ledger, split manifest, both releases and producer file hashes. The prior v5
attempt failed at the generic 16 MiB control-document limit because
`rust-analyzer/gold.json` was 28,375,487 bytes. The role-scoped 32 MiB gold
bound in `4301e3cc` admits that document without raising the limit for other
control JSON; the published v6 document is 28,375,781 bytes. C4 suite
admission is running under that same frozen source and has not published an
output or result yet.

An independent count of the twelve published `gold.json` task arrays gives
6,628 distinct IDs: 5,262 `mechanical_unreviewed` and 1,366 `unjudged`.
Unsupported source coverage caused all 1,366 unjudged rows; 821 task rows
contain `other_language_possible_declaration`, 692 contain `census_refused`,
and 147 contain both. These are exclusion candidates for C4 admission, not
search misses. The final selected denominator awaits C4's source replay.

The frozen release's 11,695 `code_only` files were independently projected into
twelve new Git repositories at
`/private/tmp/qi-c5-comparators-20261003-v1/repos`. The projection receipt
`projections.json` has SHA-256
`d837b25e4baa93c965ee151386395e720ed8a48b12ef9a96b199d3be1fe05380`.
Each repository's tracked path and SHA-256 set matches its release manifest;
the projection commits are distinct from the original repository commits.
The same exact-file views were copied to an isolated OpenGrok source root at
`/private/tmp/qi-c5-comparators-20261003-v1/opengrok-src`; its source receipt
has SHA-256
`66421616d5d6f5112ded029b00dbc46ecdf9977167763d312d774de42aaf1ca9`.
Sourcegraph capture previously submitted the original `rev:` even for a
projection, which cannot identify that projection commit. Commits `7c7c66c6`,
`d99ee9c6` and `a6370967` add separate source/service revision binding,
replay checks, and committed-blob validation. The Sourcegraph adapter's 37
focused tests pass, including a Git `skip-worktree` counterexample.

**VERIFIED:** 12 gold capsules and both exact-file input projections; Sourcegraph
revision contract and focused tests. **NOT_RUN:** a new Sourcegraph or
OpenGrok service index, request-time indexed-universe attestation, fresh C5
product queries, independent relevance approval, qualified product comparison
and performance measurement. A Git projection is input evidence, not proof
that a service indexed every file.

## Fresh C5 external index progress (2026-10-03)

An isolated Sourcegraph 6.8 service indexed the twelve exact-file Git
projections. Its native `type:path` V3 streams returned 11,695 paths; each
repository's path set and service revision matched the release manifest and
projection commit. The raw streams and summary are under
`/private/tmp/qi-c5-comparators-20261003-v1/sourcegraph-paths`; the summary
SHA-256 is `db988e088ed1c5afaf17a4c026a92dab9da16d9420fb7aaa6e4993310c00ae78`.
This proves the observed path-index inventory, not every content posting or
request-time index stability. A post-stop Zoekt shard snapshot exists at
`/private/tmp/qi-c5-comparators-20261003-v1/sourcegraph-index-snapshot.json`
(`ff58bed2a5165b33ba27b79d19ce9998ebc5ee63bdd51b9dc62ce467b307d0d8`);
there is no matching pre-query snapshot yet. The service is stopped with its
data preserved so OpenGrok can index within Docker's memory limit.

The isolated OpenGrok 1.14.18 service is still indexing the same 11,695-file
projection. A live, two-sided indexed-file inventory and served-content probe
passed for `attrs` (56/56 exact paths and file SHA-256s), with raw evidence at
`/private/tmp/qi-c5-comparators-20261003-v1/opengrok-attrs-index-probe-v1`.
Other repositories are not promoted until their indexer processes finish and
their full inventories and content probes pass. The affected external-capture
and adapter tests passed 119/119 with `pytest -q` over
`test_live_lexical_external.py`, `test_lexical_file_comparison.py`, and
`test_sourcegraph.py`; this is code verification, not a C5 product score.

## C4 v6 admission audit and gold correction (2026-10-03)

The frozen `dd86ec99` C4 run completed successfully in 1,940.81 seconds.
Its matrix at `/private/tmp/qi-c5-fresh-20261003-v6/c4-v6/admission-matrix.json`
has SHA-256 `e96f5d08ea0b7e8269296913252fca909192533f8108a629f4f6f78afdccc5f7`.
Independent byte-hash and ID-partition checks verified all 72 cells, 54 emitted
suite/pack/admission triples, 6,388 candidate IDs, 4,801 selected IDs and
1,587 exclusions. Exclusion reasons were 1,366 unjudged/unsupported, 157
ambiguous typo targets, 56 near-duplicate queries and 8 incomplete near-name
censuses. The 240 literal tasks are outside this C4 declaration matrix.

Every `rust-analyzer` declaration task was excluded. Its corpus contains
intentionally invalid parser fixtures, and the independent `syn` checker
refused some of those files. The old gold oracle treated a checker-refused file
as unresolved even when the query spelling was absent from its bytes. For
exact-name tasks, 4,070 of 4,082 `census_refused` task/file pairs had no query
bytes; 102 of 110 exact tasks had no text hit in any unsupported file. A
query-specific source-byte absence is a sufficient exclusion proof for an
identifier declaration. Commit `9397c1b1` removes the extra primary-parser
refusal precondition and versions the gold oracle to 5. The independent
positive/negative fixture was red before the change; gold/C4 tests then passed
120/120. This does not assert that every `rust-analyzer` task is now judged:
files that may contain the name remain unjudged.

The v6 C4 result is therefore superseded for C5 admission. A fresh source-bound
gold capture at clean `9397c1b1` is running under
`/private/tmp/qi-c5-fresh-20261003-v7`; its C4 wrapper is prepared but must
wait for the matching gold receipt. No v6 labels or counts are merged with v7.

## C5 checker-identity closure and native OpenGrok index (2026-10-03)

The v7 gold capture failed after 1,896.417 seconds, before publishing any
capsule. Its receipt is
`/private/tmp/qi-c5-fresh-20261003-v7/gold-capture-receipt.json`
(`2b003c1007a583b3fdd20f1dba75c5572081b0776347fccc502bd6b7f9ff5c53`).
The failure was `gold checker identity differs from frozen recipe`: the v5
sampling recipes bound only each repository's primary language, while the
corrected gold oracle audits every supported source language in the unscoped
file-search universe. For example, `chartjs` has JavaScript and TypeScript
sources but its frozen recipe binds only the JavaScript checker. The current
per-language checker binary identities themselves still match the recorded
values. A new sampling run from clean `4bd35d39`, the same release and seed,
was preregistered at
`/private/tmp/qi-c5-fresh-20261003-v8/sampling-precommit.json`
(`ee1897ebf0667aec3f48e139f62f355d8a93b509c308cc4228ec815e673d73fe`)
and completed in 313.966 seconds. Its receipt is
`/private/tmp/qi-c5-fresh-20261003-v8/sampling-receipt.json`; the split SHA-256
is `8adc09ad9fdbf1d0684389ff84f38774ae828999b1080e3bf5097c3b682d5f94`.
All twelve recipe files and the split manifest are byte-identical to a separate
clean `9397c1b1` run. Only the ledger differs: it records the changed source hash
of a formatting-only `gold_oracle.py` edit. The 6,628 task IDs and complete task
records are unchanged from v5. Seven recipes now bind additional supported
language checkers. The current `corpus_binding.py` replay validated the
22-repository split and 6,628 tasks in 435.302 seconds; receipt:
`/private/tmp/qi-c5-fresh-20261003-v8/split-validation.json`. Corrected gold
at clean `9397c1b1` completed in 2,401.944 seconds with twelve capsules;
receipt `/private/tmp/qi-c5-corrected-9397c1b1-20261003/gold-capture-receipt.json`
(`3be3af2d7502b320179bac83ac864fb46589abbef5d9d0ae3914cfee930b8fa9`).
Independent raw capsule comparison found 6,114 `mechanical_unreviewed` and 514
`unjudged` tasks. The 496 state changes from v7 are `unjudged` to
`mechanical_unreviewed`, with identical task IDs, queries and labels. This is
source-eligibility evidence, not human relevance review or a product score.
The v7 failed output remains excluded. Corrected C4 and literal admissions are
running from this gold under separate external roots.

The official OpenGrok `1.14.18` AMD64 image made an incomplete local index
under emulation. Reducing its project workers from sixteen to two did not
complete the three lagging projects. Both attempts and their volumes remain
preserved. The official `1.14.18` source tag `74f9e21e` was then built for
Linux ARM64 in a separate image and data volume. The new service indexed all
12 projects. A full native UID inventory and served-byte probe verified
11,695/11,695 paths and file SHA-256s before/after each repository probe.
Its receipt is
`/private/tmp/qi-c5-comparators-20261003-v1/opengrok-v3-full-probe-v2/summary.json`
(`ae459adbdc11bfaea68ebc5df4f24916e89f9dc6bb83f2d903a9953c255bbf8b`);
the observed wall time was 371.346 seconds, not a qualified indexing or query
latency measure. Raw replay independently checked 23,439 captured files and
the twelve manifests. The architecture/build change explains the successful
operational workaround; it does not by itself isolate the root cause of the
AMD64 stalls.

The prior `text/plain` content probe returned HTTP 404 for a Chart.js path
present in OpenGrok's Lucene UID inventory. In the official source, that route
performs a separate `getDocument(path)` query; the octet route reads the source
file. The capture now requires exact Lucene UID inventories around the probe
and compares octet-source bytes against the release manifest. The previously
failing Chart.js file returned HTTP 200 and its expected SHA-256. A focused
positive/negative probe passed 4/4, and the live three-product capture/replay
fixture passed. This proves indexed path membership and served source bytes,
not equality of every Lucene content posting. Fresh C5 product queries remain
`NOT_RUN`.

Sourcegraph's first restart failed twice because its migrator contacted the
PostgreSQL database during crash recovery. A one-off container using the same
official image opened the preserved database, completed recovery and shut it
down cleanly. The service then started normally. Its second container mounts
the exact Zoekt index directory a second time, read-only, for backend snapshots.
The capture adapter now permits only empty `.indexserver.tmp` and `.trash`
directories and the `indexserver.sock` Unix socket as Zoekt runtime entries;
an active staging file and other special files remain rejected. The focused
positive/negative tests passed 3/3 and the full live-external adapter test file
passed 40/40. After service restart, native Sourcegraph path search again
matched all 11,695 manifest paths in twelve repositories. The v2 path summary
is `/private/tmp/qi-c5-comparators-20261003-v1/sourcegraph-paths-v2/summary.json`
(`0c9dee6621b349808e7dfd2192a3ad5eed209f976abb4bd66617eeb01bca5822`).
Its 13-file, 246,805,099-byte backend snapshot was identical before and after
the path queries; receipt:
`/private/tmp/qi-c5-comparators-20261003-v1/sourcegraph-v2-backend-pre.json`
(`5ebc0c436af8d3b09102cf24cf80cdb755ace62d9b342f6fdd1c01fde9d11b56`).
This brackets the path probe, not the future product queries. Their capture
must take its own before/after backend snapshots.

The first C5 external precommit omitted the ordinary-input OSA1 lane: all
twelve `declaration_name_osa1` C4 cells in the previous matrix were
`no_admission_diagnostic`, while the case-folded OSA1 cells used the explicit
typo request. It was superseded before product queries by
`/private/tmp/qi-c5-external-20261003-v2/precommit.json`
(`b41d02d81881dde6da2a2ecc524cb247a8ecfe07f29c402f15ff6af98327b73a`).
The replacement freezes a source-validated projection of each selected
case-folded OSA1 suite to `default_file_search`: only request mode and suite ID
change; query, task ID, gold, and original C4 admission remain bound. A
386-task `attrs` projection passed `evaluator.validate_suite` for both suite
and blind pack. A single unscored `attrs` `convert` preflight returned ten
in-manifest files each from Sourcegraph, OpenGrok and cs, with HTTP 200, HTTP
200 and exit 0. Its first wrapper failed in summary formatting after the
native requests; independent replay of the preserved responses passed, and
Sourcegraph's backend snapshot matched its pre-query snapshot. Receipt:
`/private/tmp/qi-c5-external-20261003-v2/preflight-attrs-convert/summary.json`
(`3c9c078a5db85cefe5b1808c8bf615bfc53c035e951920f71484e03a0b5e5eea`).
This preflight is not a relevance score or full C5 capture.

## C4 checker refusal boundary and replacement capture (2026-10-03)

The corrected v8 C4 run at clean `9397c1b1` exited 1 after 727.124 seconds;
its receipt and traceback are under
`/private/tmp/qi-c5-corrected-9397c1b1-20261003/`. The independent Rust
checker refused `rust-analyzer`'s
`crates/parser/test_data/lexer/ok/single_line_comments.rs`, but the primary
tree-sitter oracle completed that file's census with zero declarations. C4
incorrectly passed every checker refusal as a primary-parser exclusion, which
the source oracle correctly rejected. No C4 matrix was published from that run.

`holdout_c4.py` now excludes a checker-refused file from the primary oracle
only when the primary parser also refuses it. The checker refusal and its
query-specific textual absence remain in the gold provenance. Focused positive
and forged-exclusion tests passed 7/7; the entire C4 test file passed 51/51,
and Ruff, formatting and `git diff --check` passed. The fix and tests are on
clean `main` at `17b2af2a`. A replacement C4 run from clean frozen
`f2aa7d82` is active under `/private/tmp/qi-c5-c4-f2aa7d82-20261003/c4`;
the literal v9 capture remains active independently. Neither has a successful
final receipt yet.

The C5 external ordinary-input selection was carried forward to a third
precommit, `/private/tmp/qi-c5-external-20261003-v3/precommit.json`
(`2f633ba7a1f99c65f85e583ac356d411920ff39bfb68f7679545a61970fdbf18`).
It supersedes v2 after its three explicitly unscored native preflight queries
and before any scored C5 product capture. It binds the replacement C4 output
and source head; the external adapter source remains clean `a9b4ec6e`.
Between those two commits only the C4 adapter and this ticket changed.
The v3 preparation and batch scripts compile but have not run because the
replacement C4 receipt is pending. **VERIFIED:** C4 cause, focused and full
unit tests, source equivalence for the external adapter. **FAILED:** original
C4 v8 capture. **NOT_RUN:** replacement C4 result verification and all scored
C5 product cells.

## Literal v9 independent replay and C4 source-identity failure (2026-10-03)

The second C4 run from clean `f2aa7d82` also failed, after 1,078.928 seconds.
Its traceback and failed receipt are under
`/private/tmp/qi-c5-c4-f2aa7d82-20261003/`. This failure is a different
boundary: the v8 gold capsule binds `gold_oracle.py` at clean `9397c1b1`,
while `f2aa7d82` includes a formatting-only change to that file. The
source-derived capsule validator rejected the changed producer SHA in
`identity.json`. No C4 matrix or product score was published. A separate clean
`9397c1b1`-based checkout now contains only the current C4 fix and tests at
`2edb982d`; its four gold-producing source hashes match the frozen capsule,
its C4 adapter and tests match current `main`, and seven focused tests pass.
Its precommit is
`/private/tmp/qi-c5-c4-goldbound-20261003-v1/precommit.json`
(`b963362fbc785c574fe82b148236694dd92074111d712cf7b85c283efa2ba6d1`).
A successful full C4 run from that source is still `NOT_RUN`.

The separate literal v9 run from clean `9397c1b1` completed in 2,022.596
seconds. Its receipt is
`/private/tmp/qi-c5-literal-20261003-v9/literal-receipt.json`
(`6dd69b2f045a82ee03b8734aa01d8702ae1fa0feaa9fb1f26fba7b8f37d0ba53`).
An independent Python stdlib replay hashed all 11,695 source files
(86,469,103 bytes), checked every gold literal byte span, rescanned each
selected query against its full file universe, and matched all twelve suite,
blind-pack and admission ID/hash partitions. Of 240 candidates, 239 were
admitted and `zoxide.lit.013` was excluded as a near-duplicate. The replay is
`/private/tmp/qi-c5-literal-20261003-v9/literal-independent-verification.json`
(`af2a08aad300a6264378b87bcf610a0448603958b78eafbdacd243693bacd4f2`).
This proves a mechanical exact-content input contract; product search and
human relevance review remain `NOT_RUN` for this v9 lane.

## Literal product execution and interrupted C4 retry (2026-10-03)

A first Quanta whole-file product run captured seven repository cells, then
`sympy` failed during publish with
`LEX_PHRASE_PLAN_LIMIT_EXCEEDED[POSITIONS_PER_CELL]`: a single term/document
cell would exceed 4,096 positions. This is an index-build failure, not a
top-ten miss. Its failed receipt remains at
`/private/tmp/qi-c5-literal-product-20261003-v1/sympy/receipt.json`; the seven
successful whole-file cells are not pooled with the replacement run.

The replacement froze `fixed_window_strict` at 4,096 bytes with 256-byte
overlap. An independent byte scanner checked that all 239 selected literal
occurrences, each at most 80 bytes, lie within a complete emitted window;
receipt: `/private/tmp/qi-c5-external-20261003-v4/literal-window-coverage.json`.
The first replacement process completed nine repository cells and was
interrupted during `telegraf`; it left no receipt for that cell. A separately
precommitted continuation captured `telegraf`, `zerolog`, and `zoxide` in a
fresh root, preserving the partial original state. The combined independent
raw-row scorer bound both precommits, each cell receipt, suite, manifest,
record and report, then recomputed file metrics from returned paths:

- 12 repositories, 239/239 file Hit@10, mean file Recall@10
  `0.997907949790795`, mean file NDCG@10 `1.0`, one `capped` response.
- The only incomplete-recall task is `rust-analyzer.lit.005`: 10 relevant
  files returned from 20 judged files. All ten returned files are relevant,
  hence its NDCG@10 is 1.0 despite Recall@10 of 0.5.
- Sum of per-cell wall times is 520.158 seconds; SDK query timers sum to
  720.377909 milliseconds. These have different boundaries and the host was
  contended, so neither is a comparative latency claim.
- Source-bound precommits:
  `/private/tmp/qi-c5-literal-product-20261003-v2/precommit.json` and
  `/private/tmp/qi-c5-literal-product-20261003-v3/precommit.json`.
  Independent result:
  `/private/tmp/qi-c5-literal-product-20261003-v3/independent-combined-results.json`
  (`32f09287a43a187cb21dccdac61cad06189d266e5c6ca65894f655460b4dac1c`).

This is `VERIFIED` as a mechanical, diagnostic Quanta literal-file result.
It is not a five-product result or a human-reviewed relevance judgment. The
first gold-compatible C4 rerun was interrupted with no receipt or matrix. Its
replacement is active under `/private/tmp/qi-c5-c4-goldbound-20261003-v2/`;
C4 matrix validation and scored declaration-product capture remain `NOT_RUN`.


## C3 structural RCA and Native file capture repair (2026-10-03)

The original mixed C3 suites remain invalid for qualification. Recomputed raw
family sets confirm that all 12 omit 20 NL families from their split authority;
20–42 split families are absent from the suites, depending on repository. The
split equality gate is correct and was not relaxed. These C3 repositories are
disjoint from fresh C5; their AI qrels cannot qualify C5.

The existing `holdout_review.py` now reissues reviewed `semantic_intent` tasks
under `natural_language_file_search` to a fresh diagnostic suite/pack. It first
validates the complete original suite, preserves all task data except the
request contract, rejects malformed excluded tasks and over-limit queries, and
checks input/tool drift. Original receipts and split authority are not carried
forward; AI identities remain AI identities. All 240 tasks in 12 repositories
were reissued and validated without modifying the original capture.

An actual uvicorn public capture reproduced a recorder defect: Native
`select:file` ranks distinct files but preserves a representative published
chunk. The recorder and Python evaluator wrongly required a CodeSearch `file:`
identity, while the synthetic fixture fabricated one. The fix separates ranked
file units from published source witnesses, preserving source/generation proof,
native scores/order and duplicate-file rejection. The unit negative refuses a
source-valid CodeSearch identity substituted into Native output.

**VERIFIED:** `test_holdout_review.py` 47 passed; focused Python file contract
selector 25 passed (438 deselected); retrieval-bench unit binary 112 passed;
registered `sdk_roundtrip::native_file_public_routes_group_order_scope_and_case`
1 passed, 24 filtered (2.87s). The SDK fixture uses >=15 matching chunks in one
file plus nine others, proves token-OR/folded content semantics, excludes a
path-only match, preserves source identity and score order, and walks cursors.
The required-test inventory follows its renamed selector. Recorder, evaluator,
SDK fixture and inventory bytes on main match the pinned proof source.

**VERIFIED, diagnostic capture/replay:** corrected uvicorn and zustand each
executed 20 tasks; all 40 rows are `capped` with ten distinct files and finite
native scores. Capture wall times were 66.826s and 48.042s, respectively, using
debug/hash-dev lexical-only on a contended host. These are execution observations,
not speed measurements. The failed baseline remains preserved separately.

**BLOCKED, relevance completeness:** 134 returned task/file pairs are absent
from the existing AI qrels (uvicorn 64, zustand 70). Complete-ranked-pool scoring
excludes 38/40 tasks; only 2/40 are eligible. No product ranking is reported.
Two new source-bound unjudged packets include paths, hashes and frozen full text.
Reissue the review pool/forms and adjudication with new suite/receipt digests;
post-result diagnostic labels must not become pre-result qualification.

**NOT_RUN:** remaining C3 200 live tasks, multi-product captures for this NL
contract, and the independent v5 106-task mechanical audit. The old 106 packet
binds v1 capsules and cannot be filled as v5 proof. C5 still needs its own fresh
NL relevance data, family/split issuance and source-bound admission. A qualified
semantic/hybrid ten-file comparison requires its own declared product contract;
this repair establishes a lexical NL file diagnostic only. Full quality, speed,
release and deployment qualification remain unclaimed.

Evidence and commands:
[structural RCA](/Users/songmin/Documents/code-new/qi-b08-structural-rca-20261003-j489jdg8/RCA.md),
`projection-summary.json`, `original-family-audit.json`, `runtime-binding-fixed.json`,
`runtime-summary-fixed.json`, `scoring-summary.json`, source archives and the
per-repository missing-review packets in that fresh external root. Original
path bindings remain in preserved copies; no old result or aggregate was changed.

## C3 benchmark contract and parser follow-up (2026-10-04)

The existing IR now carries an optional `answerability_min_grade` (integer 1–3,
 default 1). The C3 rubric's grade-1 clue / grade-2 sufficient-answer distinction
must explicitly use 2 in both review context and suite task. No-answer validation
and sufficient-answer gold follow this threshold; graded relevance metrics remain
based on positive grades. Orphan/source-oracle fields and threshold changes in
completed forms are refused. This does not fabricate human review or independent
AI identities.

`capture_review_pool` exports source-validated distinct-file captures as rankless,
scoreless review candidates, including tasks with empty/abstained output. Under
`complete_ranked_pool_v1`, missing returned-file/declaration judgments now make
operational means `not_applicable` with `incomplete_ranked_judgments`; they cannot
become search failure zeros. Explicit grade zero and execution failures keep
applicable operational treatment. Common eligible cohorts are still required.

The dedicated local/formal retrieval rails now include `test_holdout_review.py`;
the required Python identities are regenerated from actual collection. TypeScript
and TSX census/named-definition gold use the producer vendored compatibility
grammars, rather than an unpatched language-pack parser. Capsule producer bindings
include the parser factory and actual C/header bytes. Linked source directories,
wrong cache identity/binary and absent compilers fail explicitly. Frozen v5
capsules were not rewritten. TypeORM's pinned index source parses with the repair;
Go `new(expr)` and zustand overload syntax remain unsupported boundaries.

An additional live-receipt RCA confirmed that the old direct driver accepted
`query_warmup_passes: 1` but executed zero warmups. Direct `quanta` now forwards
explicit warmup/measurement counts through the existing shared query protocol and
checks the returned protocol/pass counts. It refuses silently ignored multiple
fresh roots; `pair` owns those repetitions. Exploratory specs can explicitly
request zero warmups; qualified speed still requires at least one. Existing pinned captures retain their
actual zero-warmup observations. A two-file probe executed cold 1, warmup 2 and
measured 6 calls, returning both fixed expected files. Complete current-driver
capture failed the compiled symbol policy guard because its runner was from
`f9454987` while current producer inputs had changed; the guard was not weakened.
This is partial schedule proof, not current-main Rust/product qualification.

**VERIFIED, focused local:** holdout owner 63 passed (16.11s); judgment/cohort
selector 17 passed (8.56s); capture pool selector 5 passed (15.35s); parser/gold/
corpus owners 165 passed (291.38s), followed by six focused parser checks (1.55s)
covering subsequent compiler/link/source additions; direct/pair schedule selector
16 passed (2.62s), including the explicit-zero spec/schema case; proof rail owner
36 passed, later inventory/local selector seven passed (3.11s). Ruff,
test-authority lint and diff checks passed. Repeated selectors
are not summed into a whole-suite claim.

**VERIFIED / BLOCKED, frozen v5 sample:** all 106 raw manifest/source bindings
and 89 present label byte spans matched. Merged independent literal/AST/name-
relation checks verified 86 tasks; 20 originally unjudged tasks remain blocked.
Python, Go and TypeScript checks use their compiler/stdlib ASTs. The Rust replay
uses the same pinned syn frontend as the original gold guard, independently
replaying raw byte spans/name relations; it is not a third parser. A separate
Rust build attempt was not admitted within its 1,200s bound; that attempt was
not reported as compilation success.

**Diagnostic capture progress:** 220/240 C3 rows completed source-bound
capture and replay; all completed rows are `capped` with ten distinct files.
1,189 returned task/file pairs are missing from the old qrels. The original batch
finished; TypeORM failed before search when the corpus-wide 120-second symbol
preflight deadline expired at file 2,210 of 3,608. The first timed-out file parses
cleanly in isolation. The existing Rust timeout option is now exposed through
the Python spec, verified against the actual preflight policy, and frozen in
paired protocol locks. A controlled retry uses a fresh root, the same frozen
Rust source/binaries and a 600-second budget; its state is recorded separately
in `typeorm-budget-control-result.json`: **VERIFIED**, 802.210 seconds wall,
3,608/3,608 complete symbol files and 20 validated distinct-file query responses.
The original batch plus this separately bound control cover all 240 unique C3
tasks; original failed TypeORM evidence is retained. New qrels missing from the
old labels total **1,324 task/file pairs**; only **2/240** tasks are presently
eligible for scoring. Original per-repository state is in
`capture-summary.json`. Process wall and contained publish/seal/activate times
are reported separately from call sums and compile/admission waits. These
contended debug/hash-dev runs do not qualify speed or semantic/hybrid quality.

**Remaining data/admission work:** source-bound blank forms/batches include the
new candidates and threshold 2; no completed independent reviews are invented.
The old external `scripts/finalize_repo.py` uses `grade > 0` and drops the new
threshold, so it must not be reused unchanged for this rubric. The canonical
`holdout_review.finalize_file_review_labels` now preserves the threshold, validates
two completed forms and a third adjudicator, and issues source-bound NL file
labels with new commitments. Sixteen focused fixed-golden/negative tests passed;
actual new completed review/adjudication data is still required. Missing
qrels, original C3 family/split mismatch, full admission bundle, current-source
binary proof and other-product NL/index-universe evidence remain open. C3 labels
cannot qualify the repository-disjoint C5 corpus. Full CI/release/deployment
qualification and product rankings remain unclaimed.

Evidence: [follow-up results](/Users/songmin/Documents/code-new/qi-b08-nl-completion-20261003-p8hky9bm/RESULTS.md),
`execution-observations.json`, `reports-complete-label-contract/`,
`v5-sample-final/merged-summary.json`, `verification-current-score-contract.json`,
`verification-parser-contract.json`, `single-capture-schedule-rca.json` and
`direct-protocol-control-v3/RESULT.json` in the same fresh external root. Original
captures, frozen sources and original review forms are preserved.

**C3 Query compilation RCA (2026-10-04):** a one-second native stack sample from
the frozen TypeORM control reached per-file `Query::new` in 68 of 70 samples.
The source compiled the same grammar/query for every admitted file. The producer
now keeps one immutable compiled Query per grammar, separates TS and TSX, uses
fresh cursors, and retains deadline/cancellation checks. Both Rust and Python
producer policy digests include the new cache module. This is a benchmark symbol
producer repair, not a lexical search ranking change. The stack sample does not
quantify whole-run cost, and the frozen control does not measure this repair.

Current Python checks: review owner **79 passed (26.71s)**; requested timeout,
CLI/replay and cache-source tamper selectors **15 passed (16.49s)**; proof inventory
and local rail selectors **7 passed (1.04s)**. These are separate scopes. The Rust
symbols unit rail was not admitted within its first 300-second bound (exit 124,
zero tests executed). The same-lock retry was admitted after 768.223s, compiled
in about 140s and **passed all 21 symbol units in 0.18s** (94 other library tests
filtered). `rust-symbol-unit-result.json` binds relevant sources and test binary.
Actual nextest collection contains 167 tests across the library, chunking and L5
parser owners. The required authority was missing 15 existing test identities;
it is now synchronized to the actual collected list. The same owner scope then
ran through canonical nextest: **167 passed, 0 skipped, 5.213s** across three
test binaries, with one test thread. `rust-contract-owner-result.json` records
the command and outcome; this is the Rust contract owner, not workspace CI.
Actual cache speed and current-main Rust E2E remain
unrun; the frozen control does not measure the repaired producer.

All 12 repositories now have new threshold-2, source-bound blank review forms,
including the new TypeORM, tailscale, uvicorn and zustand pools. Their fields
remain unjudged; no reviewer identity, grade or adjudication was synthesized.
See `execution-completion.json`, `typeorm-budget-control-replay-summary.json`,
`review-preparation-index.json`, `verification-budget-label-contract.json` and
`rust-symbol-unit-admission.json` in the follow-up external root.

## C5 Sourcegraph typo score contract and full recapture (2026-10-04)

The C5 ordinary-input OSA1 lane submitted bare identifiers to Sourcegraph's
`type:file patternType:keyword count:all` Stream content search. It did not invoke
an edit-distance or fuzzy symbol request. A fresh, source-bound Sourcegraph-only
recapture of all 4,149 tasks matched the original top-10 file paths on
4,149/4,149 tasks: 145 nonempty results and 132 all-relevant file hits. All
requests returned HTTP 200 with validated complete streams; the 13 Zoekt index
files had identical before/after hashes. The run took 693.649 seconds wall time,
with 565.388 seconds summed HTTP request timing. These timings are diagnostic.

Of the 132 hits, 128 were deletion variants whose query remains a substring of
the intended name. None of the 2,068 same-length-edit tasks hit a gold file.
A SymPy control returned zero results for the typo, 21 for its intended spelling;
the same typo also returned zero results with `type:symbol`, while the correct
spelling returned one symbol. The separate Sourcegraph UI fuzzy finder was not
measured. The observed 132/4,149 is reproducible for this **keyword content
request**, not a product-wide fuzzy-search score.

Future B08 reports must label the native request beside each score and keep
ordinary-input experience separate from explicit OSA1 capability. They must
not rank these policies as feature-equivalent. The existing Quanta `typo:` and
cs `~1` captures remain separate request lanes. No Sourcegraph adapter or
scorer arithmetic defect was found, so the frozen captures and aggregate were
not rewritten. Full RCA, raw recapture, source and index bindings, and controls:
`/private/tmp/qi-sourcegraph-osa1-full-yzpjy5ut/RCA.md`.

**VERIFIED:** Sourcegraph 4,149-task Stream recapture and original top-10
agreement. **NOT_RUN:** Sourcegraph UI fuzzy finder end-to-end, full content-posting
attestation, human-reviewed C5 relevance, or post-fix Quanta full recapture.

### Sourcegraph Fuzzy Finder request probe (2026-10-04)

The running Sourcegraph 6.8.0 frontend's `FuzzyFinderSymbols` bundle issues
GraphQL `search(patternType: regexp, query: $query)` with
`repo:^<name>$@<revision> type:symbol count:100 <input>`. Its symbol cache
starts empty; the browser-side fuzzy matcher ranks only symbols returned by
these requests. The separate Files tab fetches file names and therefore does
not test recovery of a misspelled declaration name from source content.

A fresh, isolated GraphQL probe used that exact Symbols query shape against
`benchmark/sympy@02ae2b456e5ea40de3b36e8381457f6211361318`.
`sdm_irref` and the deletion/substring `sdm_irre` each returned the
`sdm_irref` declaration in `sympy/polys/matrices/sdm.py`; insertion
`sydm_irref`, substitution `sdm_irrex`, and transposition `sdm_irerf`
returned no symbol candidates. All five HTTP responses were 200 with no
GraphQL errors. The frontend bundle and raw responses are under
`/private/tmp/qi-sg-fuzzy-finder-rca-20261004/`; bundle SHA-256 is
`64b24fe2a9a3c267bc6ab0f30c6da8b9fc7afdc4c458fd82455b9fe845479465`.

This confirms that simply switching the cold-query benchmark adapter to the
Symbols tab's GraphQL request would not supply candidates for these three
non-substring edits. Interactive typing can accumulate earlier prefix-query
candidates in the browser cache and must be evaluated as a separate,
stateful UI session if that behavior is the intended contract. These five
controls do not establish a full-suite Fuzzy Finder score or product-wide
lack of typo recovery. **VERIFIED:** frontend request shape and five GraphQL
controls. **NOT_RUN:** interactive UI trajectory and full OSA1 suite.

## C3 oracle alignment and actual AI review continuation (2026-10-04)

The current-source oracle used the older Python language pack for Rust and
Python while the Rust producer already accepted raw references and Python
3.14 template strings. The canonical Rust 0.24.2 and Python 0.25.0 grammars
are now source-bound path dependencies, generated with the same pinned CLI
0.24.4 / ABI 14 used for Go and TypeScript. Rust and Python oracle adapters
consume the same C/scanner sources; malformed inputs remain typed failures.
Actual vendor bytes enter both producer grammar identities. The generic
compiler/cache adapter is reused; no per-language parser implementation or
legacy IR was added. Main commits `6c4c7713` and `93953ecb` contain this cutover
and the fixed source-span proofs.

The independent `syn` checker also omitted attributed, bodyless function
signatures (`#[ref_cast_custom] fn ref_cast(...);`) retained as `Item::Verbatim`.
It now parses the complete token stream as `ForeignItemFn` before counting
that source-written declaration; arbitrary macro tokens are not inferred as
names. The actual checker positive/malformed test passed. This repairs the
reference census, not a product ranking algorithm.

Verification scopes (separate, overlapping checks, not an additive test total):

| Command / scope | Observed result |
| --- | --- |
| `./scripts/cargow --lane b08-nl-proof-lane test -p quanta-index-retrieval-bench --lib symbols:: --locked -- --test-threads 1` | **VERIFIED**, 21 passed; 18.52s compile, 0.16s tests |
| `.venv/bin/python -m pytest -q tools/ci/tests/test_source_oracle_suite.py tools/ci/tests/test_gold_oracle.py` | **VERIFIED**, 118 passed, 265.91s; before the later independent-checker repair |
| `.venv/bin/python -m pytest -q tools/ci/tests/test_retrieval_benchmark.py -k 'symbol or preflight'` | **VERIFIED**, 82 passed, 63.28s |
| `./scripts/cargow --lane b08-nl-proof-lane nextest run -p quanta-index-retrieval-bench --lib --test chunking_contract --test l5_parser_regressions --all-features --locked --test-threads 1` | **VERIFIED**, 167 passed / 0 skipped, 10.695s tests; run `03c9c5d2-5d29-4b01-8744-1808b80c8f42` |
| `.venv/bin/python -m pytest -q tools/ci/tests/test_retrieval_contract_proof.py -k 'inventory or local'` | **VERIFIED**, 7 passed, 2.75s; before the new checker test identity was added |
| Source-oracle selector including producer grammars, cache refusal and independent Rust signature census | **VERIFIED**, 9 passed, 2.84s; before formatting-only checker changes |

Fresh external work root:
`/Users/songmin/Documents/code-new/qi-b08-closeout-20261004-2i72kj91/`.
Original captures, qrels, v5 packet and frozen checkouts remain unchanged.

- **VERIFIED, release generation:** holdout 12 repositories (195.23s) and
  development 10 (114.35s) were reissued with the current generator digest.
  Every original view and tracked inventory is unchanged. Status is
  `source_reissued_not_admitted`, not benchmark qualification.
- **VERIFIED, v5 source audit:** all 20 originally unjudged tasks were
  rederived diagnostically. Sixteen now have complete mechanical labels,
  including a genuinely empty negative task. Four remain explicitly unjudged:
  `bat.pre.002` (ANSI-highlighted output fixtures), `mocha.pre.007` (intentional
  malformed fixture), `typeorm.osa.003` and `zellij.inf.003` (matching
  declarations in another language under an unscoped request). None received
  fabricated negative gold. Old components `sensitive` metadata was corrected
  to the current `casefold` contract only in separately issued recipes; these
  outputs must not be scored as unchanged original v5 commitments.
- **Actual AI review in progress:** the installed CLI supports the existing
  amended Opus 5.5 / Sonnet 5.5 / Fable 5.1 roles. Fresh blind full-source
  judgments and separate adjudications are running in the new root, with
  actual model usage, input/output hashes and exact source quotes. The two
  independent decisions are committed before the adjudicator sees them.
  No old grades are carried forward; no result is described as human review.
- **Split validation in progress:** the new release mapping and authored C3
  suite family sets have been written to `c3-split-preparation/`; complete
  release/source/near-duplicate verification is running. The equality gate
  remains unchanged. This is preparation; no full admission is claimed.

Evidence: `release-reissue-progress.json`,
`v5-remaining-final-source-audit.json`, `v5-nushell-reference-corrected/result.json`,
`ai-review-current/`, `ai-review-rest-precommit.json` and the fresh process logs.

**BLOCKED / remaining:** actual completed reviews for all 240 tasks, canonical
form validation and qrels issuance, explicit treatment of the four v5
ineligible/ambiguous tasks, capsule/suite/split/receipt reissuance and the full
admission bundle. **NOT_RUN:** current-source qualified product captures,
complete external indexed-universe proof, same application-facing API boundary
performance qualification and a qualified five-product ranking. The concurrent
C5 pair batch remains bound to its own older source and corpus; its results
cannot be composed with this C3 source/data cutover.

**Follow-up completion:** `c3-split-preparation/result.json` reports
**VERIFIED** source/split validation in **1004.73s** across all 22 repositories,
including full release replay, code-only source binding and cross-split
near-duplicate checks. Split manifest SHA-256 is
`4ef1d4d4b56c5236bc73ead14263b057ca57511b67d201def253d0e07255a00d`;
release-map SHA-256 is
`8f00c18e960ddec2d722bb60f0896f18c77c2d309ab7f8522734454e68186462`.
This clears preparation's family/release/source gate, not the complete
qualification bundle or the pending reviewed suite commitments.

After formatting the checker, its actual signature test passed again
(1 passed, 58.79s, including the new source-keyed checker build). The existing
declaration/refusal/disagreement selectors passed 7 tests (1.10s), and final
proof inventory/local selectors passed 7 (2.01s). Rustfmt, Ruff format,
`git diff --check` and the then-current repository test-authority check passed.
Earlier broad authority failures occurred during concurrent CI edits; they are
not a remaining failure in this observed current source.

### C3 track reissuance and live comparator catalog audit

The next canonical producer execution is pinned to clean Git archive
`36970d9fc3315096a35f233933381e72c6e56ed1` at the external root's
`pinned-src-36970d9f/`. Shared-main edits after this revision are not part of
that execution. The main working tree remains the implementation location;
this archive is an evidence boundary, not another implementation worktree.

Two additional seams are confirmed:

1. **Old exclusions are no longer valid.** Executing `evaluator.validate_suite`
   on the old C3 zustand mixed suite fails because the current parser admits
   `src/middleware/subscribeWithSelector.ts`, which the old suite excluded.
   Do not remove the validator or retain these old labels. Reissue the full
   mechanical capsule and C4 suites with the canonical current producer.
2. **One suite has one request mode.** `declared_evaluation_contract` rejects
   mixed request modes. Exact identifier file search and independently reviewed
   NL file search therefore receive separate suites, score denominators and
   track split commitments. Repository-disjoint corpus/leakage authority is
   retained for both tracks; family equality is not weakened. The existing
   `c3-split-preparation/` manifest matches the old authored mixed suites and
   is preparation proof, not the final NL or newly admitted C4 family binding.

`reissue_mechanical_capsules.py` starts canonical `capture_gold_batch` for
12 original v6 recipes / **6,612 candidate tasks**, followed by canonical C4
matrix emission. Candidate count is not a scored/product-executed denominator.
Every original query remains in the new recipe; old checker assertions are
removed so that fresh actual reference identities enter the issued gold.
The isolated Python 3.14.5 environment keeps CPython t-string reference parsing
and pinned retrieval dependencies outside the shared venv. Two startup attempts
failed for missing dependencies before any capsule execution; their logs are
retained. Attempt 3 passed dependency preflight but subsequently exited with
`gold task has unsupported or ambiguous query semantics`. Session `48760` is
terminal; no completed capsule/matrix is claimed.

Actual C3 judgments continue under the existing three-model amendment. Fresh
`issue_reviewed_labels.py` invokes the canonical issuer only after all actual
completed forms exist. `issue_reviewed_nl_suite.py` then issues NL-only suite,
blind pack, two annotation receipts and adjudication receipt, preserves query
bytes, uses sufficient-answer threshold 2 and validates the resulting source
and receipts. These scripts are prepared/syntax checked; actual issuance is
pending. New records are required; old captured records cannot be rebound.

**VERIFIED, live catalog scope:** authenticated API reads from the current
Sourcegraph (`127.0.0.1:17080`) and OpenGrok (`127.0.0.1:17083`) return the
12 C5 comparator repositories (`attrs`, `svelte`, etc.). Neither authorized
catalog exposes any of the 12 C3 holdout repositories. OpenGrok's zustand
file inventory is empty. Complete raw catalog responses and missing-name sets
are in `current-external-catalog/result.json` under the external root. This is
an observed API availability boundary, not a content-postings attestation.
These current services cannot support a C3 holdout comparison as configured;
prepare dedicated source-bound holdout indexes, then verify paths, bytes,
native index authority and query execution. Do not relabel the C5 development
batch as C3 holdout proof or mutate a concurrently captured backend.

Concurrent C5 pair processing was observed alive (runner and daemon processes),
with 27/48 cells recorded at observation. The old external ledger's `running`
string does not prove a live process. Qualified performance remains **NOT_RUN**
while builds, indexing and these diagnostic jobs contend on this host.

**Review failure and bounded recovery:** zustand task 18, Sonnet pass, returned
one wrong frozen file SHA. The original process exited with a typed source
binding failure; its raw response and `invalid.json` are retained. The resumed
execution revalidates terminal successful calls against their raw model output,
model identity, input/decision hashes and exact source quotes. It reissues only
the failed call in a fresh retry directory, with JSON-schema constants binding
each allowed path/SHA pair. The real retry passed; no grade or identity was
repaired by hand. `zustand-resume-precommit.json` binds the old and resumed
scripts. Process `49652` / session `57884` continues tasks 18–20. The other
actual review process remains `93265` / session `35655`.

`continue_actual_review_issuance.py` (session `21859`) waits on those specific
live processes and requires all 20 actual completed forms for each of the three
roles before invoking canonical label issuance and NL-only suite/receipt
validation. A partial form or terminal failed review does not become qrels.
Its outputs are fresh, external, source-pinned, and explicitly unqualified.

The v5 merged audit also carried stale fields from the old Nushell rows despite
using the corrected result's label state. The new
`v5-remaining-final-source-audit-corrected.json` reads every task's actual gold
document and binds each document SHA. Six stale fields were corrected,
including `nushell.com.006` answerability and five census exclusion inventories.
The old summary is retained. The corrected 20-row tally remains 16 complete
mechanical tasks and four unjudged tasks; no current proof is inferred from
the old per-row timing fields.

**First actual issuance completed:** zustand has 20 independently authored
queries, 360 explicitly judged task/file pairs, two actual model passes and
separate actual adjudication. Canonical `finalize_file_review_labels` passed
and issued qrels SHA-256
`38dd5b3867af5ed45e3b7b87676d061c965be3c7007b3b62fbc3bf65c82bcd6f`.
All 20 tasks have sufficient answering files; five tasks had reviewer or
adjudicator differences. There are 60 validated model calls and one preserved
failed call. No failed response was included as a successful review.

The new NL-only suite, blind pack, both annotation receipts and adjudication
receipt passed canonical source/contract validation at frozen `36970d9f`.
Suite SHA-256 is
`1a901b83451323db53c63bd9a28bf8a199be8eb67456e411ecf75a264dc83ccb`.
Queries are unchanged, sufficient-answer threshold is 2, and whole-file gold
is used solely as a file witness. Evidence is
`ai-review-current/zustand/nl-suite-current/validation.json`. This proves
reviewed file-label/suite/receipt issuance; complete admission, index authority,
fresh retrieval quality and qualified timing remain unproved.

The first issuance watcher exited after qrels success because its isolated
runtime lacked pytest imported by the existing `run.py` receipt validator.
That failed startup log is retained. Installing the observed main pytest
version (9.1.1) into the isolated environment allowed the actual NL validator
to pass. `continue_actual_review_issuance_remaining.py` / session `79578`
continues the other 11 repositories and retains the completed zustand output;
it does not rerun or silently overwrite the issued qrels. The mechanical
capsule execution requires a fresh invocation after the recorded failure.

### Current-source and concurrent-work audit — 2026-10-04

Observed main was clean at `8fe282c04dc676206b09004e9d1c82009fe33c3a`,
matching the locally observed `origin/main`. Existing C3 issued artifacts remain
bound to frozen `36970d9f`; publication does not rebind those artifacts.

- **VERIFIED, focused producer repair:** commit `f3d2ae29` validates every gold
  recipe before the expensive corpus/split replay. Command
  `.venv/bin/python -m pytest -q tools/ci/tests/test_gold_oracle.py -k gold_batch`
  passed **8 tests**, with 66 deselected, in **186.99 seconds**. Ruff passed for
  `corpus_binding.py` and `test_gold_oracle.py`. Malformed JSON, a wrong raw type
  and an invalid later recipe cannot start corpus replay; the existing valid
  batch and source-drift checks remain covered. Full qualification is unproved.
- **VERIFIED, new input preparation only:** all 12 amended recipes validate.
  Their 84 components tasks use the current `casefold` contract. All 6,612
  task IDs, query bytes and family sets are retained; no old artifact is
  overwritten. Evidence: `mechanical-capsule-preparation-amended/amendment.json`
  under the external root. **NOT_RUN:** amended full capsule/C4 emission.
- **Actual C3 review in progress:** PID `93265` is alive. At observation the
  progress document contains 19 complete mocha tasks, each with two actual
  AI reviews and actual adjudication. Zustand's 20-task suite/receipts are
  already issued; thus **20/240 tasks have issued suite/receipt proof**, while
  another 19 have completed decisions but no issued suite yet. Watcher PID
  `73141` is alive and waits for all three completed 20-task forms. These are
  AI reviews under the recorded policy amendment, not human review receipts.
- **VERIFIED, comparator preparation:** fresh source-identical projections
  contain **13,347 files / 12 repositories**, prepared in **101.448 seconds**.
  Dedicated C3 containers use loopback ports 18080 (Sourcegraph) and 18083
  (OpenGrok), leaving the concurrent C5 services untouched. Both HTTP health
  checks passed and fresh Sourcegraph site initialization completed.
  **NOT_RUN:** Sourcegraph repository registration, complete indexed-universe
  attestation, fresh C3 product capture and qualified timing. Container health
  is not indexed-content proof. Sourcegraph uses an emulated amd64 image;
  deployment and fair performance qualification are not claimed.
- **Remaining v5 scope:** the corrected 20-task audit has 16 complete and four
  unjudged tasks: `bat.pre.002`, `mocha.pre.007`, `typeorm.osa.003`,
  `zellij.inf.003`. Invalid syntax/highlight fixtures and unscoped cross-language
  declaration matches need explicit source/eligibility or query contracts;
  do not silently exclude files or fabricate negative gold.
- **BLOCKED, complete C3 admission:** final NL-only track split families and
  license/model/contract/SDK custody must be bound to the newly issued suites.
  The old mixed-suite split is preparation proof; keep exact family equality.
  No full `verify_admission_bundle` result or qualified C3 comparison exists.

Concurrent work was inspected read-only:

- **「조사 lexical 실패 5건 (2)」** has a live C5 pair runner, with
  **29/48 cells and 3,572/5,521 tasks** in the raw ledger. The validated cell
  wall-time sum is **7,168.093 seconds**. This diagnostic batch covers a
  different corpus/track and cannot close the C3 NL review/admission work.
- **「ㅔ벤치 준비 - 코퍼스」** is inspecting this same external root for label
  admission and five-product binding, and inspecting request-wide declaration
  lookup in core/SDK/search-plane code. Admission ownership overlaps this
  ticket; a second executing C3 issuance/index pipeline was not observed.
  Reuse the current issuance artifacts rather than starting another pipeline.
- **「벤치 - 엔진문제」** is handling CLARC, CodeSearchNet and the older
  12-repository typo/external capture track. Its release build is alive.
  These datasets and commits are separate from this C3 holdout. Its reported
  focused SDK checks are not a full C3 admission result.

Audit commands: `git status --short`, `git rev-parse HEAD`, `git log`,
`ps -axo pid,ppid,etime,command`, `docker ps`, targeted JSON reads of progress,
amendment, projections, v5 audit and C5 ledger, plus read-only application
thread status/history. No new heavy batch or duplicate product run was started
for this inventory. Full five-product quality ranking and fair latency remain
**NOT_RUN**.

### Canonical reissuance and dedicated index progress — 2026-10-04

All paths below are relative to the external root
`/Users/songmin/Documents/code-new/qi-b08-closeout-20261004-2i72kj91`.

- **VERIFIED, actual reviewed issuance:** mocha's 20-task canonical NL suite
  and all three review receipts passed. Together with zustand, **40/240 tasks
  and 871 task/file judgment pairs** have issued suite/receipt proof. The
  actual remaining-review process and issuance watcher remain live; the lo
  review has begun. AI provenance remains explicit and unqualified.
- **VERIFIED, NL family preparation only:**
  `c3-nl-split-preparation/preparation.json` binds the 12 original authored NL
  sets, 240 task/query commitments and exactly 20 families per repository.
  Both issued suites agree with these families, source commits and universes.
  Development/holdout corpus assignments are retained. **NOT_RUN:** canonical
  full validation of this new split and complete admission. Do not substitute
  the older mixed-suite split or weaken exact family equality.
- The failed mechanical attempt is replaced by a fresh invocation using clean
  `git archive f3d2ae2900a7c5728789b8ddeb3e333f473318fa`, preserved at
  `pinned-src-f3d2ae29/`. Its actual `corpus_release.py` digest equals both
  reissued releases' generator identity. All 12 amended recipes passed cheap
  preflight before canonical `capture_gold_batch`. Session `14063` / observed
  PID `52644` is live. Outputs are fresh
  `mechanical-capsules-amended-f3d2ae29/` and
  `mechanical-c4-amended-f3d2ae29/`; no completed capsule/matrix is claimed.
  Source/archive/launcher/recipe bindings are in
  `mechanical-amended-execution-precommit.json`. The old attempt is retained.
- **VERIFIED, Sourcegraph path-index scope:** the dedicated C3 backend exposes
  exactly **13,347 indexed paths across all 12 repositories** at their expected
  projection commits. Full `type:path` streams ended normally with no skipped
  entries; their path sets match the release manifests. The canonical native
  read-only mount snapshot and runtime/port binding are identical before and
  after the probe. Evidence:
  `c3-comparators/sourcegraph-paths/summary.json`, raw streams and native
  snapshots. This is path-index proof; content postings are not independently
  decoded, and no product quality or fair latency verdict is issued.
- OpenGrok native inspection observed 12 project shards, 84 files and
  136,566,270 bytes. The first full-view caller failed with HTTP 401 because
  the copied caller incorrectly targeted **C5 port 17083** with the C3 token.
  A direct authenticated C3 request at **18083** passed; no service-side auth
  defect is established. The original failed output/log is preserved.
  `run_opengrok_full_probe_retry.py` uses a fresh output and asserts the
  container's exact loopback port binding. Session `4247` is live. At
  observation bat **79 files / 7.163 seconds** and cli **1,014 files /
  121.786 seconds** passed canonical bracketing UID inventories and per-file
  served-byte checks. Complete probe, after-index hash and comparison remain
  unproved. These elapsed times are validation costs, not search latency.

Commands actually executed: isolated Python canonical capsule launcher;
`create_sourcegraph_token.py`, `register_sourcegraph.py`,
`attest_sourcegraph_paths.py`; the terminal failed OpenGrok full probe followed
by its fresh, port-bound retry; and `prepare_c3_nl_split.py`. Credentials are
stored externally with mode 0600 and were not printed. Existing C5 service and
captures remain unchanged. Complete admission, the four residual v5 judgments,
fresh five-product quality and quiet-host API timing remain outstanding.

**VERIFIED, cross-source revalidation:** the 40 issued zustand/mocha tasks,
both blind packs and all six review receipts also passed the canonical
validators at frozen `f3d2ae29` in **12.951 seconds**. Suite bytes and hashes
are unchanged; the original artifacts are not relabeled as newly issued.
`reviewed-nl-revalidation-f3d2ae29.json` records the precise revalidation scope.
Both development and holdout release generator identities match this source.
This does not supply full split/admission custody or product execution proof.

### 2026-10-04: Current-source gold audit and actual-review recovery

This observation supersedes the preceding live-process and residual-v5 counts.
All run artifacts remain under the existing external root
`/Users/songmin/Documents/code-new/qi-b08-closeout-20261004-2i72kj91/`.

- The unscoped declaration-gold repair is in main (`ee17986a`). An unfiltered
  file-search query needs the independently agreeing declaration census of all
  supported source languages, not just the query author's language. The source
  oracle, gold producer, C4 consumer and suite schema share this domain. The
  eight frozen owned-overlay file digests still match main at this audit.
  `pinned-src-unscoped-declarations/` is explicitly base `78085865` plus the
  owned overlay, not an unmodified archive of that commit.
- All 20 previously unjudged v5 tasks were rederived with that same source:
  **18 mechanical_unreviewed / 2 unjudged**. The latter are `bat.pre.002`
  (ANSI-highlighted output fixtures) and `mocha.pre.007` (intentional JS syntax
  error). No guessed empty gold or silent corpus deletion is admitted.
  `v5-rederived-unscoped-current/progress.json` is authoritative. Mechanical
  derivation is not independent relevance review or full split admission.
- **VERIFIED**, dedicated OpenGrok UID/served-byte scope: all **13,347 files**
  across 12 repositories passed in **936.615 seconds**. The before/after native
  index digest is `38971d8ec7e83a17d3a32b2bd9fe7e8fe4087a82f8bcd7d38759b95c3ab76d6e`.
  See `c3-comparators/opengrok-full-probe-retry-1/summary.json`. Content terms
  remain independently unproved; this does not qualify product comparisons.
- Both original blind-review input sets passed frozen source-byte, query/rubric
  and candidate-set checks: **240 tasks / 5,670 task-file pairs per pass**.
  `actual-review-both-inputs-source-validation.json` proves input custody only.
- Actual C3 suite issuance remains **40 tasks / 871 file pairs**, zustand and
  mocha. The old rest reviewer and then cohorts 2/3 failed on invalid model
  evidence. Failed inputs/raw responses are retained. The sqlalchemy failed
  call passed a fresh validated retry; zellij's retry again invented an exact
  source quotation and was rejected. This is review-output failure, not a
  product search failure. No manual quote or grade repair was made.
- A new external review launcher, `run_ai_review_source_anchors.py`, asks the
  actual assessor to select a numbered original source line. The exact quote
  is materialized from that line; grades and rationale remain model-authored.
  Raw output, original input, numbered full-source input and normalized
  decisions are separately bound. Cached anchored decisions are rederived
  from the raw line selection. Tools remain disabled and AI provenance remains
  explicit. Focused positive checks and six rejecting mutations passed;
  `source-anchor-focused-verification.json` records their narrow scope. The
  actual Sonnet zellij retry (four files, 15.391 seconds) also passed independent
  raw-response replay in `source-anchor-actual-call-replay.json`.
- At observation, live review PIDs are 58431 (lo/cli/uvicorn), 77835
  (sqlalchemy/bat/nushell) and 87534 (zellij/tailscale/typeorm/django). The idle
  old watcher was stopped only after verifying that it owned no live issuer
  child. `continue_actual_review_issuance_cohorts_retry_1.py` binds exact worker
  commands, avoiding stale PID reuse. Original scripts/logs are preserved.
  The original f3 mechanical capsule PID 52644 is also live; its older gold
  semantics must not be relabeled as the new unscoped contract.

**Remaining:** complete actual reviews/issuance; explicit disposition of the two
invalid-fixture tasks; current-contract mechanical capsule/C4 reissue; canonical
NL split and full admission validation; actual license/model/contract/SDK
custody; fresh five-product holdout capture and equal-API, quiet-host timing.
Current `run.py` explicitly permits qualified file pairs only with
`code_search_file`; `natural_language_file` is available for exploratory file
capture but is not thereby admitted for qualified comparison. Do not bypass
that gate or treat the old grammar blockage as a still-missing NL file route.

Follow-up observation at 04:05 KST: canonical f3 `capture_gold_batch` completed
all 12 capsules in **2,391.114 seconds**; their actual identity-file hashes
match `mechanical-capsule-result-amended-f3d2ae29.json`. The same live launcher
has entered canonical C4 matrix validation. This is f3 source-bound capsule
generation, not current unscoped-contract qualification. C3 has 40 issued tasks
plus 13 completed lo task reviews and one completed sqlalchemy task review;
the zellij first-task adjudication is live. The actual source-anchor response
also passed the launcher's cached-response rederivation without another model
call. Full C3 issuance and admission remain incomplete.

### 2026-10-04: Latest-contract capsule execution and chunk-status RCA

- **VERIFIED**, cheap current-contract admission: all 12 amended recipes and
  6,612 tasks pass the frozen unscoped producer's validator. The eight owned
  source digests match main; both actual release `generator_digest` fields
  match the source generator. `mechanical-unscoped-precommit.json` and
  `mechanical-unscoped-release-binding.json` retain the exact bindings.
  `reissue_mechanical_capsules_unscoped.py` has started canonical generation
  into new `mechanical-capsules-unscoped-current/` and then
  `mechanical-c4-unscoped-current/`. Session 9848 is live at observation;
  neither capsule completion nor C4 success is inferred. The previous f3
  capsule/C4 execution is retained as separate source-bound evidence.
- **VERIFIED**, original missing-return coverage: all **1,324 task-file pairs**
  from the 12 original missing-review packets occur with the same task/path/SHA
  in both current blind assessor input sets. Exactly **167** have canonically
  issued grades from the two issued repositories; **1,157** are not yet issued.
  `returned-pair-coverage-20261004.json` counts original returned pairs, not the
  larger 5,670-pair candidate pool. Input inclusion is not completed relevance.
- Cohort 2's next terminal condition was not another invalid quotation:
  sqlalchemy task 3 reviewer 1 explicitly reported confidence in all six file
  grades, but set `unresolved=true` because that *chunk* lacked an answering
  file. Its rationale explicitly identified a candidate-set gap rather than
  uncertain grades. Task answerability is calculated only after all chunks.
  The previous launcher correctly withheld forms/qrels on the unresolved flag.
- New external `run_ai_review_chunk_assessment.py` clarifies the existing
  assessment contract: `unresolved` reports uncertainty about a candidate
  grade, not absence of an answer in one chunk. A retained unresolved response
  is never reused as resolved; a fresh actual `.followup-1` call is required.
  Default cache reuse still rejects unresolved responses. If the follow-up
  remains genuinely unresolved, execution stops rather than retrying to force
  a verdict. `chunk-assessment-focused-verification.json` checks the rejecting
  default, required fresh-call dispatch and unchanged prior artifacts.
- **VERIFIED**, actual follow-up replay: Opus produced a fresh resolved task-3
  response over the identical original input. Its raw model-selected source
  lines regenerate the recorded decisions; the original unresolved response
  remains unchanged. `chunk-assessment-actual-followup-replay.json` records
  both raw hashes and observed grade equality. Cohort 2 has continued to the
  independent second assessor; no flag/grade was manually changed.
- The watcher now reads `review-live-workers.json` and checks exact live
  PID/command identities, so an authorized retry can be recorded atomically
  without stale PID reuse. `continue_actual_review_issuance_live_registry.py`
  replaced the idle prior watcher only after checking no issuer child was
  live. Current reviewer PIDs at handoff: 58431, 31383 and 87534. The registry
  and watcher binding is in `review-live-registry-precommit.json`.

All AI judgments remain explicitly nonhuman and unqualified. The current
source capsule, complete C3 issuance, full admission and final comparison
remain outstanding; this update does not convert pending rows into success.

Follow-up observation at 04:17 KST supersedes the live-review claims above:
all three actual reviewer processes are **terminal FAILED**, and the live
registry watcher correctly terminated before incomplete forms could be issued.
All three raw responses have `is_error=true`, `terminal_reason=api_error`, no
model usage, and the exact service message `You've hit your session limit ·
resets 8:40am (Asia/Seoul)`. Failed calls are lo task 18 second assessor,
sqlalchemy task 3 adjudicator, and zellij task 3 first assessor. This is a
**BLOCKED C3 external-model execution scope**, not an engine search failure or
a relevance grade. `c3-review-session-quota-20261004.json` binds those raw hashes.
No retries are submitted before the service-reported reset. At termination,
additional complete three-role assessments are lo 17 tasks, sqlalchemy 2,
zellij 2; issued suites still total 40 tasks / 871 pairs. Other completed
partial-role calls remain preserved for verified resume. The old f3 C4 job
(PID 52644) and latest-contract capsule job (PID 8999) remain live. Source,
split/admission, invalid-fixture dispositions and fair timing are separate
remaining scopes; the overall objective is not complete or blocked by this
single external-model quota while mechanical work can still progress.

### 2026-10-04: Go syntax regression closure and reviewed NL file admission

This section supersedes the earlier statement that qualified file pairs admit
only `code_search_file`. It does not qualify any pending capture or AI review.

- **VERIFIED**, Go grammar repair: builtin names may be shadowed, so syntax
  parsing must accept user-defined `new(a, b)`, `make(a, b)` and variadic
  `new(values...)`. Operand types and builtin arity belong to Go type checking.
  The shared vendor grammar now uses one argument rule rather than a separate
  single-operand `new` rule. The independent isolated audit retained eight
  parser fixtures and three actual Go compiler acceptances under
  `/private/tmp/qi-go-call-grammar-fix-20261004-q6q_73wj/`. Main Python grammar
  tests passed **3/3**; the actual Rust producer owner test passed **1/1** with
  `./scripts/cargow --lane code-search-rank-lane test -p
  quanta-index-retrieval-bench --lib symbols::tests::go_functions_methods_and_types
  -- --exact`. An earlier `--bin` selector selected zero tests and is not proof.
- **VERIFIED**, reviewed natural-language file admission: commit `589719ad`
  evolves the existing planner/driver contract. Loading, direct pair capture
  and staged capture now share `_validate_file_pair_contract`; reviewed
  `code_search_file` and `natural_language_file` require repository-disjoint
  admission, a quality claim, lexical-only Quanta and Semble lexical-file.
  Explicit typo/components/exact-content modes remain diagnostic. Qualified
  NL file labels require the declared NL request mode, semantic intent,
  distinct-file gold/results, and independent complete labels for both
  answerable and no-answer tasks. Preflight and verdict enforce the same label
  boundary; complete-file reporting and QUALITY_DELTA use the admitted policy
  set. No semantic/hybrid qualification or relaxed model/isolation/receipt gate
  was introduced. Non-default NL token budgets remain exploratory.
- **VERIFIED**, focused driver checks: the first selected pytest rail passed
  **24** cases; the broader file/profile/qualification rail passed **25** cases.
  These selections overlap and must not be summed as unique test coverage.
  The rails reject wrong rank units, mechanical NL labels including no-answer,
  wrong intent, non-disjoint admission and direct/staged bypass attempts.
  Ruff and owned diff checks passed. Full portable contract/SDK receipts are
  still outstanding.
- **VERIFIED**, unchanged issued labels under the current driver: zustand and
  mocha remain **40 tasks / 871 pairs**. Suite, blind pack, two annotation
  receipts and adjudication receipts pass current validation, and default
  `natural_language_file` planning accepts every query. No original artifact
  or grade was changed. The source-hash-bound result is
  `nl-file-contract-revalidation-xw1nflrz/result.json` under the external
  closeout root, with a 3.011-second validation scope. AI provenance remains
  explicitly nonhuman and unqualified.
- **VERIFIED**, independent fixture classification: bat's three remaining
  blocking paths are generated ANSI terminal-output snapshots, verified
  against the frozen generator's AST/source and actual escape bytes. Mocha's
  remaining file explicitly declares an intentional syntax error and an
  independent `node --check` refuses it. See
  `v5-invalid-fixture-audit-gdvb0241/result.json`. The two gold tasks remain
  `unjudged`; no empty gold, corpus deletion or ANSI-stripped scoring input
  was fabricated. Canonical exclusion/denominator reconciliation remains
  outstanding. This audit is not an engine-quality verdict.
- **VERIFIED**, NL input preflight: all **240 unchanged authored queries** pass
  the default current NL-file planner and match their NL-only split families;
  both issued suites preserve those commitments. A clean `git archive` of
  `09d8a843f5143cb082eda1ee653e1517b2b46cd1` starts canonical full split validation
  for all **22 repositories** in `nl-split-current-am26ey_3/`; its source,
  script and inputs are bound in `precommit.json`. Session **37492** is live
  at this observation. Full split completion is not yet claimed.
- **BLOCKED**, fresh public SDK execution: the canonical daemon build request
  was not admitted after its 300-second wait on the shared build/test lock.
  No compiler or SDK test ran from that request. Existing public SDK fixtures
  already include the NL-file projection, but their source presence is not
  execution proof. Do not bypass admission or call this an engine failure.

**Still required:** remaining actual C3 reviews/issuance; canonical current-source
C4 and NL split completion; the two fixture exclusions in the final denominator;
full license/model/contract/SDK admission; fresh eligible five-product holdout
capture and equal-API timing. Older live mechanical jobs retain their own frozen
source bindings and do not prove this newer Go/driver source.

Follow-up at 04:50 KST: the frozen unscoped producer completed all **12**
mechanical capsules in **2,087.271 seconds** and entered canonical C4 validation.
All actual identity-file hashes match
`mechanical-capsule-result-unscoped-current.json`; this is completion of that
frozen producer, not current Go/driver or qualified comparison proof.

The original v5 audit and these capsules use different query sets. In
particular, reused local ID `bat.pre.002` is `lin` / `bat.name.line_range_2_3`
in v5 but `parse_` / `bat.name.parse_less_version` in the new capsule.
`mocha.pre.007` is `sho` / `mocha.name.shouldEscapeHtmlChar` in v5 but `retri` /
`mocha.name.retries` in the new capsule. Both new capsule tasks are admitted;
neither closes the original unjudged v5 task. The independently bound
`v5-invalid-fixture-audit-gdvb0241/capsule-query-identity-crosswalk.json` records
this distinction. The original v5 exclusion denominator and new C4 denominator
must remain separate, keyed by their suite/gold bytes and query commitments,
never joined by local task ID alone.

### 2026-10-04: routed no-model admission and original v5 sample closeout

- **VERIFIED**, the reachable lexical-only qualification defect is repaired on
  main (`3a22dfa9`, `d73292f8`). Both manifest issuance and verdict replay use
  `_quanta_admission_model_revision` across every repetition. Lexical/symbol
  routes require their exact `none:<route>` / `not-applicable` identity;
  semantic/hybrid require one real consistent model identity. Missing routes,
  unrouted captures, sentinel misuse, mixed models/revisions and whitespace
  revisions are refused. The quality embedder restriction now applies only
  when semantic/hybrid execution is actually declared. The paired fixture
  matches the public producer's lexical no-model tuple. The focused command
  `.venv/bin/python -m pytest -q tools/ci/tests/test_retrieval_benchmark.py -k
  'quanta_encoder_selector_binds_semantic_capture_revision or
  verdict_quality_gates or qualified_verdict or model_parity or strategy_model'`
  passed **7** tests (520 deselected, 43.02 seconds). Ruff check/format passed.
  This is focused behavior proof, not final admission qualification.
- **VERIFIED**, all **240** unchanged authored NL queries and the exact family
  split passed canonical validation over **22 repositories** on clean pinned
  `09d8a843` in **841.495 seconds**. See
  `nl-split-current-am26ey_3/result.json` in the external closeout root.
  Only **40** tasks have issued reviewed suites; full C3 issuance is unchanged.
- **VERIFIED**, the original v5 **106 query identities** were retained in
  `v5-independent-final-ca0jiwoh/`. The original 20 unjudged tasks have separate
  rederivation bindings: **18** pass independent repository-wide AST/name/span
  completeness checks and **2** have proven fixture exclusions. The audit
  denominator is **104 eligible / 2 excluded**. Original gold and product
  captures remain unchanged; rederived labels are not human relevance labels.
  `final-sample-audit.json` and `sample-exclusion-receipt.json` bind the exact
  original/revised packets, gold, manifests and fixture evidence. The retained
  86 tasks keep their frozen contracts; the repaired 18 have repository-wide
  declaration labels. No historical benchmark aggregate is rescored.
- The first Rust-only replay for `zellij.inf.003` was **FAILED** because its
  checker omitted **48 JavaScript declarations** from a repository-wide gold
  set. That raw failure is retained. `audit_cross_language_completion.py`
  verifies all **18** repaired tasks with the union of supported languages;
  it does not silently scope gold to Rust. Go/Python/TypeScript use independent
  standard frontends; Rust syn remains the original guard's reference frontend,
  not an invented third parser. `finalize_sample_audit.py` then verifies source
  and byte bindings and integrates both exclusions into the sample denominator.
- **VERIFIED**, offline retained Semble model revision and asset replay in
  `model-binding-verified-sw50i89d/result.json`: revision
  `e9d2a44ca6a05ac6685f3b23709ea57eb7352d5b`, asset digest
  `ea909b7defe7804ce18bf003ef60a437b54782541819ab6b7004c36fd9eea5d0`.
  An independent logical-path/raw-byte hash matches the adapter. The cache was
  read-only; a new C3 capture/admission is not claimed.
- **VERIFIED**, 12 repositories' **14 license source files** and tracked notice
  identities are bound in `license-review-inputs-kfyhcv1f/result.json`.
  Symlink Git blobs are distinguished from resolved license content. No approval
  reviewer or decision is fabricated; actual scoped approval receipts remain
  absent.
- **FAILED**, portable contract production on clean `8d499543` refused its
  Python collection because the source-controlled proof inventory lacked newly
  added tests. Main `38f73d3a` subsequently includes the inventory update; that
  newer commit still needs its own complete proof. The failed root is retained
  as `current-portable-proof-model-fix-0eybjt42/contract/`.
- **VERIFIED**, the older pinned `f3d2ae29` mechanical run completed its **72**
  C4 cells in **3,783.660 seconds** after **2,391.114 seconds** capsule production.
  It remains diagnostic and bound to that older producer. The unscoped producer's
  separate C4 process and the `8d499543` SDK proof are live at this observation.

**Remaining:** full actual C3 model review/issuance (quota reset reported as
08:40 KST); current-source C4 completion; final same-source contract and SDK
receipts; actual scoped license decisions; full admission; external indexed
content attestation integration; reviewed holdout capture and equal-API timing.
The new v5 sample disposition does not satisfy these separate qualification gates.

Follow-up at 05:29 KST supersedes the SDK-live observation above:

- **FAILED**, the clean `8d499543` SDK run executed **25** tests: **24 passed,
  1 failed**. The actual public NL-file OR fixture passed. The failure was the
  SDK test's obsolete flat sum of schema-3 phase values, which counted both
  the publish parent and its `sdk_publish`/`sdk_activate` children. Producer
  timings and the Python verifier already distinguish nested children; no
  evidence of producer double-counting is asserted.
- Commit **`32fbdb2d`**, pushed to `origin/main`, repairs that existing SDK
  assertion. It independently requires finite nonnegative measurements,
  child sum within the publish parent, and the disjoint partition equal to
  total. Existing Python phase golden/refusal tests passed **2/2** (4.06 seconds),
  and `cargow fmt -p quanta-index-retrieval-bench --check` passed. Actual repaired
  SDK RED-to-GREEN execution is still pending, not inferred from those tests.
- The old SDK/contract failures are preserved. Both fresh portable rails now
  use the same clean **`32fbdb2d`** worktree and new output root
  `current-portable-proof-nested-sdk-fj8nngov/`, with commands/source/environment
  bound before launch. Sessions **42229** (SDK) and **44800** (contract) are
  confirmed live; resource admission serializes Rust work. The previous source
  is not silently upgraded and cross-source receipts are not composed.

### 2026-10-04: native Sourcegraph contents and scoped license decisions

- **VERIFIED**, all **13,347** manifest files across **12** repositories were
  read from the actual deployed Zoekt binary through its native
  `/print?format=raw` API. Every returned document body matches its frozen
  manifest SHA-256. This reads indexed document payloads, not Git-server source
  content. The native shard inventory and container identity remain unchanged
  before/after the **22.935-second** probe. The copied and deployed reader binary
  hashes match. Only the temporary read-only probe process was stopped; the
  existing Sourcegraph service and index were preserved. The earlier unsupported
  JSON API attempt remains recorded and was not relabeled as successful.
  `sourcegraph-native-content-8l9gxy35/` under the external closeout root retains
  every body, request worker, input binding and result. Independent full replay
  passed, with **8** receipt mutations refused, and **12** bounded index-scope
  receipts were issued. Posting-level correctness, query-capture integration,
  qrels and fair performance are separate requirements, not asserted here.
- **VERIFIED**, actual primary-agent AI review issued **12** license decisions
  scoped to local source-bound benchmark ingestion and internal evaluation,
  covering the exact **13,347-file** manifests. Root and applicable nested terms
  were inspected, explicit SPDX headers were checked, and all tracked notice
  objects plus resolved symlink content were preserved. Bat's selected Go
  fixture uses Apache-2.0; selected TypeScript/TSX fixtures use MIT. Django's
  Python and vendored notices, Nushell crate notices, Tailscale's distinct
  notices and Zellij termwiz notices were included. These are actual AI
  decisions, never human-review or legal-counsel attestations. Public code-only
  dataset redistribution and deployment clearance are outside their scope.
  `license-scoped-review-xuozw4ln/` retains the decisions and sidecars. Canonical
  receipt validation on the pinned `32fbdb2d` helper accepts all **12** inputs
  and refuses **84** independent field mutations. New admission manifests must
  consume these exact receipts; no old release or capture was overwritten.
- **FAILED**, the pinned `32fbdb2d` portable contract Python rail executed
  **661** tests in **710.682 seconds**, with **655 passed / 6 failed**. Each
  failure reaches the host-monitor digest mismatch: the canonical `RawFile`
  digest includes `sha256:` while this driver compared it with bare hex. Main
  already contains the producing/consuming boundary repair from another worker;
  the failed receipt is retained. The old failed cases plus the new monitor
  refusal cases pass on the current working tree: **13 passed**, 522 deselected,
  **114.68 seconds**. The current diagnostic-v7 pair and clock-overhead fixtures
  each pass separately (**3.56** and **2.99 seconds**). These focused results do
  not substitute for fresh portable proofs on one complete pinned source.
- Required-test reconciliation found **5** committed host-monitor refusal IDs
  plus **2** committed diagnostic-v7 replay IDs missing from the Python proof
  authority. They were added without removing existing requirements. Canonical
  collection now matches **671** identities. Its external reconciliation root
  is `proof-inventory-reconcile-cwq0a2s8/`. The SDK `32fbdb2d` process remains
  live under resource admission; it has not supplied a passing SDK receipt.

**Remaining:** complete the actual C3 judgments/issuance after the reported
08:40 KST quota reset; connect scoped license/index receipts to fresh admission
and query captures; freeze the integrated source and produce complete matching
contract/SDK proofs; finish C4 and the reviewed five-product comparison with
equal public API timing. No qualified ranking or final performance claim is made.

Follow-up: fresh contract and SDK portable producers were started on one clean,
pinned `9e27b8ba18282cd9c8c6c72afb63ada26868176e` checkout at
`/Users/songmin/.codex/worktrees/b08-integrated-admission-proof/quanta-index`.
Their external root is `integrated-portable-proof-9e27-sj_x_b3k/`; its
`precommit.json` binds commands, source and environment before execution.
Sessions **72447** (contract) and **89355** (SDK) are confirmed live. A fresh
isolated compiler target avoids mutating the earlier still-live `32fbdb2d` SDK
target. This is a verification checkout; fixes remain on main. Neither new
producer has yet issued a passing receipt, and later main commits are not
silently included in this pinned proof.

### 2026-10-04: index-scope consumer integration and SDK proof result

- **VERIFIED**, the native Sourcegraph scope now has a reusable consumer in
  `retrieval/sourcegraph_index_scope.py`. A live external spec may supply
  `sourcegraph.indexed_scope_receipt`; this requires both `backend_snapshot`
  and `projection_git_root`. The consumer independently checks the selected
  manifest, native stored bodies, complete V3 indexed-path response, Git
  projection, deployed-reader custody, and unchanged container/index identity.
  The same consumer executes before/after queries and during offline replay.
  Missing/duplicate paths, altered bodies, changed process/index identities,
  partial/skipped path responses, and rehashed forged claims refuse. A path
  response reporting more matches than returned paths also refuses.
  `sourcegraph_index_scope` records this bounded scope; overall indexed-universe
  and comparison qualification flags remain false. Posting correctness, other
  products, subjective labels and performance are separate scopes.
- **VERIFIED**, the final external-capture owner file passed **71 tests** in
  **182.25 seconds**, including a local HTTP/process capture, offline replay,
  forged scope metadata, and native-body mutation during queries. The existing
  test-authority/source-closure checks passed **3 tests** in **3.31 seconds**.
  Commands: `.venv/bin/python -m pytest -q
  tools/ci/tests/test_live_lexical_external.py`; and the two authority/closure
  test files with selector `live_code_search_owners_are_enrolled or
  live_workflow_and_owner_tests_are_bound`. Ruff check, format check and diff
  hygiene passed. The module is covered by the existing retrieval-directory
  source closure; owner tests stay in the existing capture rail.
- **VERIFIED**, the final consumer replayed **12 repositories / 13,347 files**
  against the unchanged live query backend in **57.891 seconds**. Artifacts:
  `sourcegraph-index-scope-final-ekct8xn0/` under the external closeout root.
  Its result binds consumer hashes and every scoped receipt; the standalone
  command was `PYTHONPATH=/Users/songmin/Documents/code-new/quanta-index
  .venv/bin/python <external-root>/replay.py`. `launch-observation.json`
  preserves the actual live argv: the system-Python bootstrap had recorded its
  own interpreter in `precommit.json`, so that bootstrap command is not an
  exact execution receipt. This is diagnostic verification, not a formal
  product/performance run. No original bodies, indexes or captures changed.
- **VERIFIED**, the clean pinned `9e27b8ba` SDK producer exited zero: **25
  selected/executed/passed, zero failed**. Independent canonical verification
  passed with `portable_proof.py verify --receipt
  <external-root>/integrated-portable-proof-9e27-sj_x_b3k/sdk/execution-context.json`.
  The initial verification invocation incorrectly selected `sdk_receipt.json`
  and was refused as an invalid execution-context shape; selecting the actual
  context closed that command-selection error without a code change. This SDK
  proof does not include the later index-scope or other main changes.
- The older `32fbdb2d` SDK producer is **terminal, exit 124**: its resource
  admission wait expired after 1,800 seconds. It supplied no passing SDK
  receipt. The new `9e27b8ba` contract producer remains live, session **72447**;
  no passing contract result is inferred from the SDK result.
- **VERIFIED**, the unscoped C3-holdout mechanical producer finished **72 C4
  cells** in **4,383.392 seconds**, including **2,087.271 seconds** capsule
  production. Sixty cells selected **5,971 tasks**; twelve have no tasks for
  the distinct non-casefold OSA1 intent. All **180** emitted suite/pack/admission
  byte digests match the matrix. This retains its `78085865`-plus-owned-overlay
  source binding and is diagnostic, not a current-main result.
- The other live C4 job uses the separate attrs/celery/chartjs/grpc-go/immer/
  rust-analyzer/svelte/sympy/tauri/telegraf/zerolog/zoxide release. Its repository
  set is disjoint from this C3 holdout. These are separate corpus jobs, not
  duplicate execution or interchangeable matrices.

**Remaining:** actual C3 review is still **40/240 issued tasks**, with **1,157**
of the original **1,324** returned task/file pairs unissued. No review worker
is live; the external service reported 08:40 KST quota reset. Final reviewed
suite/split/license/model/proof admission, matching proofs for the final
producer source, fresh five-product captures and equal-boundary repeated
performance remain incomplete. This update does not close those scopes.

Follow-up at 06:22 KST: the pinned `9e27b8ba` contract Python stage completed:
**671 passed, zero errors/failures/skips**, JUnit time **667.835 seconds**
(pytest wall report **667.98 seconds**). The original host-monitor failures no
longer reproduce on this pinned source. The overall producer remains live for
the Rust stage and final canonical receipts; Python success is not whole
contract success. The index-scope changes and this ticket update are present
on main and pushed through `f5b3b55e`; unrelated dirty owner work is preserved.

### 2026-10-04: NL comparator admission and native parser probe

- **VERIFIED**, reapplying the original 240 unchanged NL queries to current
  planners admits 240/240 through Quanta `natural_language_file`, but only
  3/240 through the conservative Sourcegraph adapter. The other 237 refusals
  are 229 query-shape guards and 8 reserved-word guards; they occur before HTTP
  submission and do not prove an engine parser or retrieval failure.
- **VERIFIED**, the first issued task in each of mocha and zustand was submitted
  unchanged through the scoped native keyword endpoint. Both returned HTTP 200,
  completed progress, no skipped scopes, and zero matches. Independent positive
  controls (`retry`, `createStore`) returned native matches in the respective
  repositories. Query text was not rewritten or selected using gold. This
  establishes native acceptance for these two queries, not acceptance of all
  240 or a quality comparison. Native keyword behavior and Quanta token-OR NL
  planning have different semantics; equal HTTP/SDK timing alone cannot make
  them equivalent-work performance rows.
- Artifacts: `nl-sourcegraph-auth-probe-tg8c0wk7/` under the external closeout
  root, including the 240-task census, raw requests/streams, completed progress,
  scoped stored-body/path verification, and unchanged before/after backend.
  The command was `PYTHONPATH=/Users/songmin/Documents/code-new/quanta-index
  .venv/bin/python <external-root>/probe.py`; total wall time was **10.380s**.
  The earlier unauthenticated attempt returned four HTTP 401s and is retained
  separately with a BLOCKED parser-acceptance interpretation. No credential
  values were emitted or copied into request artifacts.
- **NOT_RUN**, safe general NL query construction for Sourcegraph, controlled
  common-predicate NL comparison, and final C3 comparison. Declare native
  workflow versus matched semantics before adapting queries; do not silently
  quote whole questions, synthesize OR rewrites, or count adapter refusal as
  an observed native zero-result search. Existing C3 labels/captures are intact.

### 2026-10-04: current pinned admission-input prerequisites

- **VERIFIED**, the clean exported `08549763cda5daed3d6e95cdf0b1315f22fad917`
  source validated all 240 original query/family/repository assignments, the
  issued mocha/zustand suites and blind packs, their two actual AI annotation
  receipts and separate adjudication receipts, 12 scoped license decisions,
  and the retained Semble revision/actual model bytes. Four forged suite or
  annotation commitments refused. The prerequisite check took **2.975s**.
- It confirms **40 issued tasks / 871 expanded-pool file judgments** and **200
  pending tasks**. Expanded-pool judgments are not the original 1,324 returned
  pairs; original-pair coverage remains 167 issued / 1,157 unissued.
- Artifacts: `nl-admission-input-preflight-wex9_qy3/` under the external closeout
  root. `input-readiness.json` is explicitly `input_prerequisites_verified_not_admitted`,
  not a qualification manifest. No missing proof digest or review was invented.
  Runtime pins are regex 2025.10.23, tree-sitter 0.23.2, language-pack 0.9.1 and
  unicodedata2 17.0.0. The complete 22-repository canonical split/leakage check
  is running in the same pinned source; its terminal result is still pending.
- Command: `PYTHONPATH=/Users/songmin/Documents/code-new/quanta-index
  .venv/bin/python <external-root>/validate.py`. Initial archive extraction
  used unsupported system-Python-3.9 `filter`; workspace Python 3.12 completed
  extraction. The archive source commit was independently recovered with
  `git get-tar-commit-id`, rather than inferred from subsequently moving main.
- **NOT_RUN**, full final admission and product/performance comparison. Existing
  `9e27b8ba` proofs are recorded as a different source, not rebound to `08549763`.
  Remaining actual reviews, final matching proofs, host/cache/lockfile profile
  and declared NL comparator semantics are required before capture qualification.

### 2026-10-04: fresh OpenGrok UID scope and retained-body replay

- **VERIFIED**, a fresh native UID enumeration for all **12 repositories /
  13,347 files** exactly matches their release manifests. Both fresh Lucene
  snapshots match the original full-probe snapshots and runtime: **84 files /
  136,566,270 bytes**, index SHA `38971d8ec7e83a17d3a32b2bd9fe7e8fe4087a82f8bcd7d38759b95c3ab76d6e`.
- The original full probe's raw inventory, bracketing UID responses, all served
  body/transport responses and release view bytes independently replayed under
  the pinned `08549763` capture validator. Missing/duplicate UID inventories
  and altered body bytes refused in three negative controls. Total wall time
  was **45.261s** before final result serialization (reported script completion
  **45.378s**); original captures and index volumes were unchanged.
- Scope is deliberately bounded: UID lists are fresh; served body responses
  are historical and replayed, not newly fetched for all files. Unchanged native
  index bytes/runtime do not independently decode Lucene content terms or prove
  every current served source body. This supplies no quality/speed ranking.
- Artifacts: `opengrok-native-scope-replay-c_99hi7s/` under the external closeout
  root. Command: `PYTHONPATH=/Users/songmin/Documents/code-new/quanta-index
  .venv/bin/python <external-root>/replay.py`. **NOT_RUN**, fresh full body fetch,
  posting decoding, product search and performance qualification.

Follow-up: the same pinned `08549763` full split producer exited zero.
`nl-admission-input-preflight-wex9_qy3/split-validation.json` reports **22
repositories / 240 NL families**, complete release/source/exact-and-near-copy
validation, **651.663s** split time (**654.644s** including prerequisite checks).
Independent post-exit replay matched all **50 input hashes and 5 owner hashes**.
This closes the selected split/source prerequisite; **200 actual task reviews,
final matching proofs and complete admission remain incomplete**. No qualified
product/performance result or missing label is inferred from split success.

Follow-up: the pinned `9e27b8ba` Contract producer is terminal, exit zero:
**Python 671/671 and Rust 168/168 selected/executed/passed, zero failed**.
Independent `portable_proof.py verify --receipt
<external-root>/integrated-portable-proof-9e27-sj_x_b3k/contract/execution-context.json`
passed from the unchanged pinned checkout. Together with the verified SDK
**25/25**, this closes that source's proof production. It does not include
later main changes or provide the final producer admission/comparison.

### 2026-10-04: Sourcegraph literal-data submission and proof-inventory closure

- **VERIFIED**, the Sourcegraph adapter now compiles printable single-line
  query data as whitespace-delimited literal content terms with AND semantics.
  Syntax-bearing terms use `content:` JSON quoting; caller text cannot inject
  repository filters, boolean operators, negation, regexes or whole-query
  phrases. Previously admitted safe bare terms retain their exact request
  bytes. Capability admission delegates to the same compiler instead of a
  second regex/operator predicate. Empty/control/surrogate inputs still refuse
  before HTTP and receive neither fabricated latency nor HTTP success.
  Official syntax: <https://sourcegraph.com/docs/code-search/queries> and
  <https://sourcegraph.com/docs/code-search/queries/language>.
- **VERIFIED**, 11 native controls matched independent case-insensitive literal
  AND path sets computed from frozen zustand contents. Controls cover repeated
  content conditions, a missing first term, reserved words, filter text, quotes,
  backslashes and punctuation. Runtime/index identities and scoped stored bytes
  were unchanged. `sourcegraph-literal-and-probe-ghuiqvmy/` under the external
  closeout root retains requests, raw streams and oracle path sets; **7.607s**.
- **VERIFIED**, all **240 unchanged C3 queries** passed the current compiler
  and request-target preflight (largest **766 bytes**, below the existing 8KiB
  limit). Actual native submissions returned **240 HTTP 200 responses**, each
  accepted by the canonical response validator. All 12 repositories consumed
  their existing native stored-content/path receipts against unchanged
  bracketing runtime/index snapshots. Capture took **232.263s** before result
  serialization; this includes scope verification and is not query latency.
  Offline replay accepted all 240 responses and refused four forged request
  bindings in **12.763s**. Artifacts: `sourcegraph-c3-literal-submission-fc1p78td/`.
  Commands: `PYTHONPATH=<source> <workspace-python> <root>/capture.py` and
  `<root>/replay.py`. Replay exports clean **2266b8cb** because later main edits
  changed a scorer digest; all four exported owner hashes match the capture.
  The current compiler bytes match that executed compiler. No qrels were used
  for scoring, and no original query, capture or index was overwritten.
- **VERIFIED**, compiler/offline owner **37 tests plus 11 subtests** passed;
  source-closure/test-authority checks **3 passed**, and the authority CLI,
  Ruff and diff hygiene passed. The joint three-owner run was **FAILED**:
  **179 passed / 2 failed**, **385.70s**. The reserved-word fixture still expected
  an unsupported-only report field; its corrected focused run passed in
  **76.18s**. The other failure reached capture identity validation while main
  changed. Both failed cases were re-executed from clean **e7a26a69** and passed
  **2/2**, **65.09s**, in `sourcegraph-owner-failure-recheck-_09tt092/` with JUnit.
  This is focused closure, not a fresh passing whole-owner or whole-repository
  receipt. The updated offline test also refuses OR/phrase/filter substitution
  even when the raw-query hash is recomputed.
- **VERIFIED**, an additional admission prerequisite was repaired: clean
  **e7a26a69** collected **677** Contract Python identities while its required
  authority named **673**. The four missing IDs cover guarded source-closure
  reuse, final verification preservation, unsupported Python refusal and
  driver-versus-unattested-binary provenance. They were added to the existing
  authority without deleting entries or changing Rust/SDK inventories.
  Canonical collection now matches **677**; the four tests passed (**2.43s**).
  The external recheck root retains the original refusal, set difference and
  reconciled collection. Full current-source Contract/SDK execution is separate.

**Remaining:** C3 actual review is still **40/240 issued tasks**, with **200**
pending tasks and **1,157/1,324** original returned task/file pairs unissued.
No review worker is running before the service-reported **08:40 KST** reset.
Full final admission, reviewed five-product captures and repeated performance
remain **NOT_RUN**. This closes the conservative Sourcegraph submission guard;
native literal AND search is not Quanta NL token-OR or semantic retrieval.
Declare product workflow versus matched predicates before interpreting quality
or performance. All new executions remain diagnostic and unqualified.

### 2026-10-04: native OpenGrok full-posting coverage

- **VERIFIED**, read-only Lucene enumeration separates **13,347 live file
  documents** from **4,268 auxiliary documents** across the 12 C3 repositories.
  File paths and unique UIDs match every release manifest; each corresponding
  frozen source hash matches its manifest. Native `full` postings are present
  for **13,346 files**, rather than all 13,347 manifest paths.
- The sole file without `full` postings is
  `bat/tests/syntax-tests/highlighted/TypeScriptReact/app.tsx`, SHA
  `17b4dfd05ccec0f28dd0f97fcd4851740aac3e0b1e874a2e19e7618c39e3d696`.
  Its frozen 6,499 bytes contain 494 ANSI escape bytes; OpenGrok retains its
  path/UID metadata as type `file`, without the stored `t` field. This is direct
  evidence of metadata-only inclusion, not an observed query execution failure.
  Do not silently remove this path, rewrite the release, or claim full content
  search coverage from UID/path counts. Final admission must retain this scope
  exception and determine whether any final task requires that file.
- Both bracketing runtime/index snapshots are identical, including 84 native
  files / 136,566,270 bytes and index SHA
  `38971d8ec7e83a17d3a32b2bd9fe7e8fe4087a82f8bcd7d38759b95c3ab76d6e`.
  Missing-file, duplicate-file and forged-UID controls all refuse. Independent
  post-exit replay rechecked the raw output hash, helper hash, counts and
  bracketing snapshots. Command: `.venv/bin/python
  <external-root>/opengrok-native-full-postings-y0o5m0qe/probe.py`;
  elapsed **16.564s**. Artifacts are under the existing external closeout root;
  original captures, frozen checkouts and index files are unchanged.
- **FAILED**, the preceding `opengrok-native-file-universe-9cjsur69` wrapper
  incorrectly required every primary document to have `t=p` and assumed one
  auxiliary stored-field shape. Native extraction succeeded, but those wrapper
  assertions did not. Its raw output is preserved; the new posting probe uses
  manifest/UID identity and actual postings instead. The earlier field-helper
  compilation failure and corrected retry also remain separate artifacts.
- **NOT_RUN**, complete analyzer-term/source equivalence, reviewed C3 scoring
  and final comparison qualification. Posting presence does not prove all
  expected tokens, ranking, or search responses. The 40/240 review issuance and
  reported 08:40 KST external-model quota reset remain unchanged.

Matching proof production is live from clean **89058da8** in
`integrated-portable-proof-89058da8-8d6s3e1w/`: Contract PID 4092 and SDK PID
4227 were confirmed alive, both waiting for canonical Rust resource admission.
Contract Python collection matches **677** required identities; no terminal
test success is inferred from collection or waiting. These jobs do not include
later main changes to external fresh-join or execution batching. Retain the
source distinction when assembling final admission; do not relabel the old
9e27b8ba receipts or duplicate the live jobs because observation yielded.

Follow-up native controls confirmed the metadata-only distinction through the
actual OpenGrok API: fixture path-only search returns HTTP 200 / one file;
fixture `full=import` returns HTTP 200 / zero files. A positive
`full=hiddenfileextension` control returns exactly `bat/src/assets.rs`. Its
expected path was independently derived from every hash-checked frozen bat
source before submission. Index/runtime snapshots stayed identical; **2.139s**.
The earlier positive control incorrectly assumed `path=src/assets.rs` meant an
exact file restriction; it returned seven paths and the wrapper failed. Those
responses remain in `opengrok-metadata-content-controls-l2nlb1sr/`; the corrected
unique-term controls are a separate run in
`opengrok-metadata-content-controls-unique-25ypkztd/`.

The corrected root's `scope-disposition.json` binds all 20 original bat C3 NL
tasks and both prepared 20-task independent review forms. None references the
metadata-only fixture in task labels or candidate paths. This is a bounded
scope disposition, not completed relevance review: preserve the release and
all task denominators, declare native content coverage, and recheck newly
issued qrels rather than silently excluding a product's omitted file. The
final 200 reviews and admission are still incomplete.
