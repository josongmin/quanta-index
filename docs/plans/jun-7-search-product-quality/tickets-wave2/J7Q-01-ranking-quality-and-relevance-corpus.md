# J7Q-01 — Relevance and external lexical comparison

Status: `ACTIVE_RESIDUAL`
Parent: [quality index](INDEX.md)
Owner: lexical/symbol/structural/history ranking and benchmark evaluator

Current execution plan: [2026-10-03 final audit](#2026-10-03-final-audit-and-action-plan).
Earlier observations below retain their original source and request scope.

The judged seeded fixture, per-route MRR@10/NDCG@10/Recall@20 and top-1,
top-k/hard-negative invariants are implemented in
`crates/quanta-index-searchd-harness/src/relevance`. Recreating a relevance
producer is not the remaining task. The current external overlap output is
explicitly `unprovisioned`; it is not a competitive-quality result.

The frozen gin 300 bare-symbol lexical diagnostic is separate from that
unprovisioned qualified overlap: Quanta returned the generated gold file in
295/300 chunk-top-10 responses. All five missed gold texts were published in
the lexical generation. A two-query same-binary control placed `S040` at chunk
rank 12 (fourth distinct file) and `S273` at chunk rank 13 (twelfth distinct
file). This confirms top-k/rank-unit pressure in those cases; it does not prove
a lexical candidate-generation correctness defect or validate the generated
declaration-only labels as general content-search relevance.

At local `main@9c880476`, the exact-symbol searcher and dispatcher tests for
case-sensitive names plus a typed file anchor each pass. The earlier native
300/300 exact-definition-span result is bound to `a9a43529` and a mechanically
derived, unreviewed declaration suite. The latest HEAD has no new gin native
300-query result. Keep declaration lookup, file-ranked lexical search, and
chunk content retrieval as separate result units and acceptance tracks.

## Remaining acceptance

- Independently judge stable query IDs and graded labels; include near-duplicate
  distractors and hard negatives. Keep deterministic tie-break checks separate.
- Measure lexical, symbol, structural and history-backed families independently;
  retain per-query rankings, top-1, top-k containment and forbidden high ranks.
- Execute a real Sourcegraph lexical comparison on overlapping keyword, phrase,
  regex, constrained-content, repo-metadata and symbol-name surfaces. Bind exact
  queries, corpus/revision, native observations, order and explicit gaps.
- Keep macro/route denominators visible; a stable but poor ranking or a blended
  average cannot hide a route regression. Fixture floors are not external gold.
- For exact-name requests, verify the explicit symbol route through a current
  source-bound consumer capture and reviewed declaration identities; do not
  infer automatic bare-query routing from searcher/dispatcher unit tests.

Output owner: `relevance_matrix`, registered under `quality-core/quality-full`.
The retained `summary.json`, `query_judgments.json` and `sourcegraph-overlap.json`
are interpreted under current artifact admission, not old README status labels.
Independent corpus/gold/holdout and native response acceptance are owned by
[CS-BENCH-01](../../sep-27-code-search-remediation/rfcs/CS-BENCH-01-corpus-gold-and-holdout.md)
and [CS-BENCH-02](../../sep-27-code-search-remediation/rfcs/CS-BENCH-02-native-response-validation.md).
No semantic/hybrid or comparative SOTA claim is closed by this ticket alone.

## 2026-10-03 final audit and action plan

Status: `PLANNED`; source inspection and offline diagnosis `VERIFIED`.
Implementation, engine replay and new comparative capture: `NOT_RUN`.
Audit source: clean `main@e18805150ca3c74377e7f9bee0000306d56ce3d3`.

### Decision

Improve file ranking using source-verified declaration evidence, then select
the change on independently judged retrieval tasks. Retain the existing
full-file trigram candidate path, typed symbol route and benchmark framework.
An exact declaration request and a default bare file request remain distinct
product contracts. Their scores must be reported separately.

The historical Gin results are diagnostic. They used a dirty implementation
based on `53ca51e3` with pinned binaries but no complete source digest. They
do not measure the audited HEAD. In particular, current CodeSearch already
contains an empty-literal-result typo fallback; the old ordinary-input typo
score cannot describe current behavior.

### What the raw rows establish

Source inputs are the per-lane `five-product-spec-*.json`, suites, Quanta
records and Sourcegraph rows under
`/private/tmp/qi-p2-current-file-final15-juqw3g51`. The exact Quanta record is
`/private/tmp/a0/rep-00/quanta/strategy-00-fixed_window_strict/record.json`.
See [B04](../../sep-30-code-search-benchmark-trust/tickets/S30-B04-five-product-capture.md)
for the source/binary limitations and offline external-index audit.

| Common eligible lane | Tasks | Quanta Hit@10 | Sourcegraph Hit@10 | Quanta top-1 | Sourcegraph top-1 | Quanta MRR@10 | Sourcegraph MRR@10 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| Exact name | 1,196 | 1,187 | 1,193 | 1,051 | 1,175 | 0.9242 | 0.9886 |
| Prefix | 351 | 347 | 349 | 275 | 330 | 0.8558 | 0.9629 |
| Infix | 339 | 335 | 338 | 262 | 316 | 0.8593 | 0.9590 |
| Components | 271 | 270 | 267 | 203 | 144 | 0.8517 | 0.7099 |

For exact names, Sourcegraph alone hits eight tasks, Quanta alone hits two,
and both miss one. The eight Sourcegraph-only names are `Err`, `H`, `Name`,
`New`, `Type`, `Use`, `route`, and `x`. These broad names require an explicit
relevance judgment; declaration-derived gold does not express every useful
file for a bare search. Sourcegraph's full `L1291.stream` contains
`render/render.go` at position 14 for `writeContentType`; this is an observed
rank outside ten, not a missing Sourcegraph file.

There are 127 exact tasks where Sourcegraph puts a gold file first and Quanta
puts one at positions 2–10. In Quanta, 75 of these gold files score below the
first result and 52 tie it. The first Quanta result is a `_test.go` file in
88 of the 127 tasks. This is evidence of a ranking difference relative to the
declaration oracle, not proof that every test-file result is irrelevant.

### Source diagnosis and rejected shortcuts

The current `searcher/code_search.rs` uses a boundary score of 100/75/40,
up to six extra points for repeated occurrences, five points for an exact
case occurrence, path bonuses, and multi-term proximity. The ordinary file
path does not add a declaration feature. Its final tie break uses repository,
path, line and candidate identity. Symbol-aware typed components and exact
symbol search already exist and must not be rebuilt in an adapter.

A read-only Python calculation checked all 99 source hashes and reproduced
the archived top-ten path/score pairs for four ASCII queries. It then removed
only the occurrence bonus. The following ranks are **offline formula results**;
they are not daemon cursor observations or a new benchmark run.

| Query | Formula gold rank | Rank with occurrence bonus removed |
| --- | ---: | ---: |
| `mappingByPtr` | 2 | 2 |
| `HandleContext` | 3 | 2 |
| `Err` | 15 | 20 |
| `writeContentType` | 13 | 9 |

Removing the occurrence bonus alone is therefore insufficient. Changing a
path tie break cannot establish relevance either. Blanket test/generated
penalties are not justified: `route` has test-file gold and `Type` has generated
code gold. These files must remain eligible and represented in the judgments.

Quanta already has trigram candidate generation, full-source verification,
one row per file and sort-before-page truncation. New trigram infrastructure
or chunking changes are not supported by this RCA. The nine capped exact
misses still need engine continuation before assigning their actual full ranks.

### Work items and ownership

| ID | Work and files | Required result |
| --- | --- | --- |
| R1 | Reuse the runner, saved activation and cursor path to reproduce the nine exact misses plus representative top-1 reversals. Capture a separate current-source baseline. Read `searcher/code_search.rs` and `query_dispatcher/routes/lexical.rs` stage boundaries. | Per-query classification: source absent, candidate absent, source verification rejection, ranking, response truncation or execution limit. Confirm the first page before using any archived cursor continuation. No inferred full ranks. |
| R2 | Benchmark owner: reuse `tools/benchmark/retrieval/source_oracle.py`, `evaluator.py`, existing holdout review tools and B02/B05/B08 contracts. Freeze separate default-file relevance and typed-declaration tasks. Pool results from multiple products and independently judge them. | Graded default-file judgments with explicit intent; declaration identity/span gold for typed lookup; case/scope and unknown judgments explicit. Human review is recorded as human only when actually performed. |
| R3 | Lexical owner: factor deterministic rank feature extraction/scoring from `crates/quanta-index-lexical/src/searcher/code_search.rs` into a private module if needed. Add declaration-name match features from the existing generation's symbol index. Compare baseline, declaration feature only, occurrence adjustment only, and their combination. | A source-bound exact/prefix/infix declaration match can improve rank without changing the literal match set. Feature extraction is bounded and batched per request, not one symbol query per result. Preserve candidate coverage and execution-budget errors. |
| R4 | Lexical/public-route owner: extend `crates/quanta-index-lexical/tests/l3_exact_source.rs` and the existing public relevance fixtures under `crates/quanta-index-searchd-harness/src/relevance`. Update `CODE_SEARCH_CURSOR_ORDER` in `crates/quanta-index-search-plane/src/query_dispatcher/routes/lexical.rs` when scoring changes. | Independent ranking fixtures, page concatenation/order invariants, stale scoring-cursor rejection, chunk-overlap invariance and public consumer proof. Existing numeric-score tests are updated from a written specification, not copied from observed output. |
| R5 | External-capture owner: reuse `tools/benchmark/retrieval/sourcegraph.py`, `lexical_capture.py`, `lexical_file_comparison.py`, B04 and B07. Bind effective requests and served backend index artifacts before/after capture. Inspect version-matched Zoekt debug scores where available. | Fresh five-product results by supported request family, execution coverage and timing boundary. Backend Zoekt scores and Sourcegraph final order stay separate. Missing services/attestation remain explicit gaps. |
| R6 | Benchmark/integration owner: run the selected configuration on the frozen multi-repository release and a genuinely unexposed holdout under B08. Reuse the current sampler rather than duplicate the benchmark stack. | At least 1,000 admitted tasks in each requested mechanical family, counts after exclusions, repository/family split before transformations, reviewed ranking labels, repository-macro results and clustered uncertainty. Existing exposed 12-repository data is development evidence, not a fresh holdout. |

R3 uses optional, validated symbol evidence for general file ranking. Missing
symbol coverage is an unknown feature; it must not silently exclude a literal
match or claim that no declaration exists. A malformed/stale symbol identity
must follow the existing source/generation validation contract. Typed exact
symbol search retains its stricter authority/completeness requirements.

Start R1 and R2 independently. R3/R4 proceed after the intended ranking contract
is written; R5 environment/index preparation can proceed independently. R6
selects a frozen configuration, with holdout labels unavailable during tuning.
Only the integration owner edits shared schemas, metric definitions and the
proof inventory. No new benchmark manager, rank-unit adapter or parallel IR
is needed.

### Tests and decision gates

1. Candidate correctness: independent source scan agrees with the complete
   literal candidate set; case, Unicode, prefix/infix and cross-chunk matches
   stay correct. Test short/broad queries and cancellation separately.
2. Ranking fixtures: a declaration in `z_definition.go` competes with repeated
   calls in `a_usage_test.go`; rename the paths and vary repetition. Also test
   two real declarations, a relevant generated declaration, a test declaration,
   explicit path/content requests and a usage-intent query. Do not hardcode Gin
   paths or make every declaration win every retrieval intent.
3. Authority: missing symbol metadata, stale source digest, wrong generation
   and same-name symbols in different files cannot manufacture declaration
   evidence. Verify the typed route's existing rejection semantics.
4. Paging: 15 chunks from one file plus nine other files still yield ten files;
   multiple pages concatenate to the unpaged order, and a cursor produced under
   the previous scoring contract is rejected.
5. Scoring/reporting: reuse `tools/ci/tests/test_lexical_file_comparison.py`,
   `test_source_oracle_suite.py`, `test_identifier_robustness_report.py` and
   `test_holdout_review.py` only for affected contracts. Preserve denominator,
   unsupported-query, source-hash and unknown-judgment checks.
6. Quality: choose the simplest ablation that improves preregistered
   repository-macro NDCG@10 on reviewed default-file tasks. Report Hit@1,
   MRR@10, Hit@10, negative-query behavior and every lane delta. If the paired
   uncertainty interval includes no improvement, retain an inconclusive verdict.
   Mechanical exact-symbol correctness and protected fixtures permit no loss.
7. Cost: measure the same release build/API boundary on a quiet host. Before
   seeing candidate results, freeze allowed p95 latency, memory and indexing
   overhead relative to the baseline. Reject a quality win that exceeds those
   budgets; do not compare SDK time directly with in-process BM25 time.

Run owner-local Rust checks through `./scripts/cargow` or the appropriate
`just` recipe; use the existing Python test inventory. Select exact commands
after the implementation paths are known. Focused tests establish behavior,
while fresh product captures establish retrieval quality.

### Primary references and applicability

- [Zoekt design](https://github.com/sourcegraph/zoekt/blob/main/doc/design.md):
  positional trigrams, verification and code-related ranking features. It
  supports the architecture direction, not a causal claim about our captured
  Sourcegraph binary.
- [Zoekt API](https://github.com/sourcegraph/zoekt/blob/main/api.go):
  `DebugScore` supports score diagnosis; `UseBM25Scoring` is an optional mode
  that ignores other scoring signals. Do not assume the capture enabled it.
- [GitHub Blackbird architecture](https://github.blog/engineering/architecture-optimization/the-technology-behind-githubs-new-code-search/):
  ngram indexing and bounded posting-list evaluation. This is a scale reference;
  it does not prove a need to replace Quanta's current index on 99 files.
- [TREC overview, relevance judgments](https://trec.nist.gov/pubs/trec33/papers/overview_33.pdf):
  diverse result pooling and the limitations of incomplete judgments. Use this
  for the independent relevance dataset and held-out evaluation process.
- [LambdaMART overview](https://www.microsoft.com/en-us/research/publication/from-ranknet-to-lambdarank-to-lambdamart-an-overview/):
  a later supervised ranking baseline once adequate independent labels exist.
  The present mechanical Gin oracle is insufficient training/qualification data.

References were checked on 2026-10-03. These are established production and
research methods; this plan does not claim a demonstrated SOTA result.

### Audit scope

Executed: `git status --short --branch`, `git rev-parse HEAD`, targeted `rg` and
`sed` source/test inspection, raw-row Python comparisons, four-query offline
formula/ablation calculation and official-reference reads. The calculation
read pinned Gin bytes and verified all 99 file hashes before scoring.

`VERIFIED`: source findings, diagnostic row counts and four-query offline
top-ten reproduction. `NOT_RUN`: Rust tests, current daemon reproduction,
proposed ranker, five-product recapture, reviewed holdout and performance gates.
This edit is a plan update only. Historical captures and shared product source
were not modified by the audit.
