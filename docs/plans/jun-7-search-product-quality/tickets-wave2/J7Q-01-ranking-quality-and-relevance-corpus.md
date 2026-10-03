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

Historical audit status: `PLANNED`; source inspection and offline diagnosis `VERIFIED`.
Implementation status is updated in the execution section below. Historical
engine replay and comparative captures remain separate from that execution.
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

## Zoekt and Blackbird implementation analysis (2026-10-03)

Scope: read-only source/design analysis. Zoekt was inspected at
`153817f643cde8b229ee388c1dddbcf07f4798af`; selected source files are in the
external directory `/tmp/qi-zoekt-reference-_5z3qz_2`. Quanta was rechecked at
`fabbe589866047e2c586e7218d91d5f57d3cee29`; the inspected file-search,
file-authority and normalizer files are unchanged from `e1880515`.
This Zoekt revision is not established as the version inside the earlier
Sourcegraph benchmark image. Blackbird conclusions below use dated public
engineering descriptions; its current full engine/ranking implementation was
not available in the inspected material.

### Zoekt: directly inspectable mechanisms

- [Candidate selection](https://github.com/sourcegraph/zoekt/blob/153817f643cde8b229ee388c1dddbcf07f4798af/index/indexdata.go#L337)
  selects low-frequency ngrams and adjusts overlapping choices; a positional
  distance iterator rejects incompatible offsets before source comparison.
  `matchtree.go` stages cheap checks before content and regex work. Unlike
  Quanta's file-ID trigram intersection, this can reject spatially unrelated
  grams without scanning the whole candidate file. It costs positional storage.
- [Default scoring](https://github.com/sourcegraph/zoekt/blob/153817f643cde8b229ee388c1dddbcf07f4798af/index/score.go#L99)
  uses the strongest matching fragment and matched query atoms. An occurrence
  inside a stored symbol receives full/edge/overlap bonuses. Filename basename
  matching has analogous distinctions. Repeated body occurrences do not enter
  this default file formula as Quanta's occurrence bonus does.
- [Constants and symbol kinds](https://github.com/sourcegraph/zoekt/blob/153817f643cde8b229ee388c1dddbcf07f4798af/index/contentprovider.go#L589):
  full symbol bonus 7,000, edge 5,500, overlap 4,000; whole-word bonus 500,
  partial-word 50. Kind/language adds another signal. These numbers establish
  their relative priorities; copying their numeric scale into Quanta is not a
  justified calibration.
- [Index-time document order](https://github.com/sourcegraph/zoekt/blob/153817f643cde8b229ee388c1dddbcf07f4798af/index/builder.go#L887)
  considers generated/vendor/test flags, path length and other properties.
  It affects the file tie break and which files are visited before limits.
  Thus the system's behavior depends on both query scoring and index order.
- The optional BM25 branch in `index/score.go` uses constant IDF, byte-length
  normalization, boosted filename/symbol term frequency and a low-priority
  file adjustment. The executable code retains those code-specific signals;
  the broad API comment about ignoring other scoring signals is not a complete
  description. Record the actual option before attributing a result to BM25.
- `DebugScore` explains score components. Search limits still apply; source
  inspection does not establish exhaustive retrieval or reproduce the captured
  Sourcegraph service's final ordering.

### Blackbird: public design and its limits

The [2021 engineering account](https://github.blog/engineering/architecture-optimization/a-brief-history-of-code-search-at-github/)
explicitly describes boosting definitions and complete matches, with lower
priority for tests and partial matches. Symbol extraction uses Tree-sitter.
This supports adding declaration features to ordinary search. It does not
publish the current weights or justify universally demoting test code.

The [2023 architecture](https://github.blog/engineering/architecture-optimization/the-technology-behind-githubs-new-code-search/)
uses content/symbol/path indices, blob-based sharding/deduplication, incremental
compaction, and commit-consistent visibility. Sparse grams select variable-length
grams deterministically at index/query time to reduce false candidates.
Posting iterators exploit document priority; matches are source-verified and
scored, then results are merged across shards. These are scale techniques,
not evidence that a different gram layout improves Gin relevance.

The [2026-07-31 case-folding article](https://github.blog/engineering/architecture-optimization/dont-stop-early-case-folding-source-code-at-memory-speed/)
describes an ASCII pass that can be vectorized and reuses the input allocation,
plus compact Unicode simple-fold tables. Its reported throughput is a folding
microbenchmark, not end-to-end query performance. Quanta explicitly specifies
NFC plus per-character Unicode lowercase. Simple folding differs for cases
such as final sigma and dotted I: a replacement would require an intentional
contract/version change and rebuilt generations. An ASCII-only fast path can
instead be evaluated for exact equivalence to the existing contract.

### Additions to the execution plan

| Priority | Action | Gate |
| --- | --- | --- |
| P1 ranking | R3 compares best-match declaration features, symbol match extent, case and current occurrence signal. Use existing generation-bound symbol evidence and a deterministic rank function. | Independent declaration/usage fixtures and R2 reviewed holdout; no missing literal candidates or fabricated symbol evidence. |
| P1 explainability | Provide one internal score decomposition used by both ranking tests and diagnostics; avoid duplicating the score formula in a report generator. | Native captured score equals its decomposition; distinct candidate, verification, scoring and preview work counts. |
| P2 candidate cost | Measure posting visits, prefilter files, verified files and bytes scanned on long/common literals and regex. Compare current file postings with a positional prototype or sparse grams only if false candidates dominate. | Source-scan recall invariant, memory/index-build cost and identical API p95; no early top-k stop without a safe score upper bound or an explicitly partial contract. |
| P2 capacity | Audit full-file source residency and index opening before increasing scale limits. Current `file_authority.rs` bounds source bytes at 128 MiB and posting memberships at 4,000,000; short-literal scanning has separate file/byte caps. | Explain admitted corpus size, cold-open peak RSS, index bytes and rejection behavior; do not simply raise constants. |
| P2 normalization | Profile normalization allocations and ASCII throughput while preserving the current normalizer contract. | Byte-for-byte differential tests, source-offset mapping, Unicode edge cases and measured whole-index/query benefit. |
| Later distributed scale | Consider blob deduplication, persistent compressed postings and sharding only after single-node costs and real repository sizes require them. | Repository/path/ACL identity, deletion and generation visibility survive deduplication and shard merging. |

The immediate priority remains ranking and its evidence. Positional indexing,
sparse grams and normalization optimization address measured cost; they do not
substitute for independently judged relevance. Default ranking, optional BM25,
typed symbol search and typo recovery are separate configurations in ablations.

Verification: GitHub API pinned the Zoekt commit; selected files were downloaded
to a new external directory and inspected with `rg`/`sed`. Official GitHub
articles were read directly. Quanta relevant-file diff versus the prior audit
was empty. `VERIFIED` covers these source/design observations. Zoekt execution,
Blackbird execution, performance comparison and proposed Quanta changes are
`NOT_RUN`.

### Deeper comparison and adoption decisions (2026-10-03)

Rechecked Quanta at `195f5aadb3aabe814b54d8e58d7213f69cf86d4f`.
The relevant code-search, file-authority, adapter-open and normalizer diff
against `fabbe589866047e2c586e7218d91d5f57d3cee29` is empty. The checkout
was clean before this documentation addition. The Zoekt revision above is
unchanged. The following are source observations, not new benchmark results.

#### Additional findings that change implementation choices

1. **Identifier boundary scoring is a separate opportunity from tokenization.**
   Zoekt's [byteClass](https://github.com/sourcegraph/zoekt/blob/153817f643cde8b229ee388c1dddbcf07f4798af/index/bits.go#L210)
   separates lowercase, uppercase, digits and other bytes; `scoreLine` compares
   classes on both sides of an occurrence. Quanta's `boundary_score` uses
   `is_token_char`, which deliberately keeps camelCase and snake_case intact.
   Thus a component can receive a stronger boundary signal in Zoekt without
   being a whole lexical token. Implement any analogous rank feature on the
   original source with verified offset mapping: the folded buffer has already
   lost camel-case transitions. Preserve query acceptance and literal recall.
   Do not copy the byte classifier as a Unicode segmentation algorithm.

2. **Zoekt's final order is not always descending score.**
   [SortFiles](https://github.com/sourcegraph/zoekt/blob/153817f643cde8b229ee388c1dddbcf07f4798af/index/contentprovider.go#L971)
   sorts by score and then promotes a file extension absent from the first two
   results into third place, provided its score is at least 90% of the current
   third result. `TestCollectSenderDocumentLimitKeepsNovelExtension` expects
   this behavior. It cannot explain rankings on an all-Go file universe.
   Defer such diversity reranking until mixed-language, multi-intent evaluation
   shows a benefit. Record final returned order separately from score order.

3. **Symbol metadata is useful; symbol location quality is not automatic.**
   [tagsToSections.Convert](https://github.com/sourcegraph/zoekt/blob/153817f643cde8b229ee388c1dddbcf07f4798af/index/ctags.go#L126)
   finds the first occurrence of a tag name on the reported line. Its comment
   explicitly acknowledges wrong offsets for short names; missing names and
   overlapping sections are skipped. The stored section covers the name,
   not the entire function body. `buildShard` parses symbols before ordering
   documents and may continue after parser errors unless configured otherwise.
   For Quanta, distinguish a verified file-level declaration-name feature from
   an occurrence-level definition hit. The latter requires an authoritative
   name range, not a context snippet or the whole indexed function span.
   Existing typed symbol identity/source revision checks remain mandatory.
   Missing symbol metadata must not suppress ordinary literal results.

4. **Exploration limits and display limits have different completeness effects.**
   [limit.go](https://github.com/sourcegraph/zoekt/blob/153817f643cde8b229ee388c1dddbcf07f4798af/index/limit.go#L10)
   sorts before applying display caps. `search/aggregate.go` maintains a bounded
   ranked collection as batches arrive; `FlushWallTime` can switch to streaming.
   In contrast, shard/repository match limits stop document exploration, and
   `TotalMaxMatchCount` stops scheduling further shards while pending searches
   finish. These limits are not an exact global top-k proof. Preserve Quanta's
   current verified-candidate sorting before page truncation. If collection
   changes, separately expose exhaustion, timeout and display truncation;
   require stable ordering, late high-score admission and cursor continuity.

5. **Regex optimization must preserve a candidate superset.**
   Zoekt's [regex tests](https://github.com/sourcegraph/zoekt/blob/153817f643cde8b229ee388c1dddbcf07f4798af/index/eval_test.go#L82)
   cover OR branches, optional literals, anchors, repetition and patterns with
   no useful grams. For example `(foo|)` needs a brute-force candidate path;
   selecting only `foo` would miss valid matches. A Quanta optimization must
   match an independent source-scan oracle, or return its declared resource
   refusal when exhaustive verification exceeds budget. An unsupported or
   budget-refused query is never evidence that the corpus has no match.

6. **Index lifecycle and query scheduling are operational references.**
   Zoekt uses mmap-backed searchers, atomically renamed tombstone metadata,
   shard replacement with delayed close until old readers are gone, and a
   cooperative interactive/batch scheduler (`search/shards.go`, `index/tombstones.go`,
   `search/sched.go`). These mechanisms do not establish crash durability from
   rename alone. Quanta already has sealed generation authority; preserve it.
   `adapter_open.rs:145` calls `from_verified_files`, which normalizes sources
   and builds file trigram postings at generation load. This establishes a
   cold-open cost to measure, not a per-query rebuild. Measure source residency,
   open/reopen time and reader retirement before proposing persisted postings.

7. **Blackbird's product scope is narrower than exhaustive source retrieval.**
   GitHub's [official documentation source](https://github.com/github/docs/blob/main/content/search-github/github-code-search/about-github-code-search.md)
   documents generated/vendor exclusions, file-size and long-line limits,
   default-branch-only search, 100 returned results and no exhaustive search.
   The [symbol qualifier](https://docs.github.com/en/search-github/github-code-search/understanding-github-code-search-syntax#symbol-qualifier)
   finds definitions, not references, with language/type coverage limitations.
   These constraints prohibit treating GitHub results as a complete oracle for
   our fixed manifest, especially generated-code gold. Sparse grams and blob
   deduplication remain scale references, not evidence of typo recovery or
   stronger ranking on this corpus. Full engine weights remain unverified.

8. **Reference tests do not replace independent relevance judgments.**
   Zoekt's `internal/e2e/e2e_rank_test.go` pins several repository snapshots,
   checks golden result order and reports recall/MRR. It even chooses an older
   Zoekt snapshot to avoid matching its own golden files. Adopt snapshot pinning
   and fixture-contamination prevention. Snapshot output alone only detects
   change; independently reviewed relevance determines whether change is good.

#### Work order and decisive checks

| Order | Owned boundary | Concrete work | Independent completion check |
| --- | --- | --- | --- |
| 1 | `searcher/code_search.rs`, existing symbol identity conversion and rank/cursor contract | Add internal score decomposition; compare declaration-name evidence, original-source identifier boundaries and the existing occurrence feature as separate ablations. Audit available name-location authority before adding occurrence-level definition boosts. | Exact name vs usage; repeated usage; camel/snake/acronym/digit boundaries; same-line duplicate names; missing/stale metadata; tests/generated files that are genuinely relevant. No candidate recall change and no body hit falsely labeled as a definition. |
| 2 | Existing retrieval evaluator, suite builder and report | Freeze intent lanes and eligibility. Report file Hit/MRR/NDCG separately from declaration-name recovery. Add candidate completeness and final-order provenance. | Same paired eligible IDs; refusals/failures retained in coverage/operational score; fixed metric goldens. Group derived queries by seed symbol and hold out repositories to avoid treating 1,000 variants as 1,000 independent cases. |
| 3 | Existing candidate planner, file-authority and budget accounting | Instrument posting visits, candidate/source verification counts, bytes and stage timing; compare positional verification only if false candidates dominate. | Independent full-source oracle over literals/regex/Unicode; gram collisions and short/optional patterns; explicit limit refusal. Measure warm queries separately from cold-open and indexing. |
| 4 | Existing file-rank collector, response projection and cursor tests | Validate score order versus final presentation, chunking-independent file projection and bounded collection. | Late better candidates; insertion/batch permutations; page concatenation equals exhaustive deterministic order; old rank-version cursor rejected; partial results labeled. No approximate early stop concealed as exact top-k. |
| 5 | Normalizer and generation lifecycle owners, only after profiling | Evaluate equivalent ASCII normalization and persistent postings; later consider sparse grams, deduplication and scheduling changes if measured costs justify them. | Byte/offset equivalence, Unicode contract fixtures, activation with concurrent readers, delete/reopen, memory bounds and same-boundary latency. Contract changes require rebuilt generations. |

Do not adopt Zoekt's magic score constants, first-string symbol offsets or
extension-diversity policy wholesale. Do not adopt Blackbird's indexing
exclusions into our manifest contract. Default literal search, explicit symbol
search, component search and edit-distance recovery remain distinct evaluated
behaviors even when a public search entrypoint dispatches between them.

Verification for this addition: `git rev-parse HEAD`, `git status --short`,
the relevant-file `git diff fabbe589866047e2c586e7218d91d5f57d3cee29 HEAD`,
`rg`/`sed` source and test inspection, pinned GitHub raw-source downloads and
official documentation reads. `VERIFIED`: the cited source/design findings;
`NOT_RUN`: upstream tests, native comparison runs, runtime lifecycle/performance
checks and proposed product changes. No benchmark counts were replaced.


## 2026-10-03 execution: score authority and experimental features

Status: `DIAGNOSTIC_IMPLEMENTED`. Selected production ranking is unchanged.
This section does not promote a retrieval-quality, performance, release or
five-product qualification claim.

The work started at `main@88c1366e764e4ae69fd2bc93c31c3b148da6b895`.
Other tasks committed overlapping shared-main changes during execution; the
reported local checks exercised live main and its then-current overlay.
They are not a frozen-source qualification receipt. Existing captures and
`/private/tmp/g3` were not modified or replayed into a new aggregate.

### Confirmed defects repaired

1. Backend explanation treated a CodeSearch `file:` identity as a stored chunk
   document ID and returned `NotIndexed`. The sealed adapter fixture failed
   at this exact assertion before repair. File presence now resolves the
   immutable file authority; ordinary matched file explanation uses the same
   selected scorer, independent of the search page cap. A `file:`-prefixed
   legacy chunk ID still falls through to document lookup when no file matches.
2. Public lexical explain refused CodeSearch at the shared planner before
   reaching the backend. A real SDK/runtime test failed with `INVALID_REQUEST`
   after backend repair. Lexical explain now admits the same CodeSearch plan
   as lexical search. Hybrid and symbol admission stay route-specific.
3. Fresh complete-pool execution refused 391 explanations on the NFC-normalized
   112,684-byte `context_test.go`. Rank features unnecessarily reserved a full
   Unicode provenance map, exceeding the retained-byte limit of 64 MiB. A
   120-KiB fixed NFC fixture with a folding-expansion offset reproduced the
   refusal. Rank features now reuse the preview's bounded NFC offset walk;
   non-NFC input still uses the accounted mapper. The budget was not increased,
   and selected membership/scores are unchanged. The fixed fixture also checks
   folded/sensitive score goldens and original camel boundaries.

Owners: `searcher/code_search.rs`, `searcher/port.rs`,
`query_dispatcher/planning.rs` and `routes/explain.rs`.

### Implemented boundaries

- `CodeSearchScoreComponentsV1` is the selected scorer's additive authority:
  boundary/path, occurrence, exact case and proximity. Native explanation
  refuses engine, boost, total or emitted-score contradictions.
- `searcher/code_search/ranking.rs` extracts file-level producer declaration
  names from the pinned symbol index and verifies identity and raw source
  range. It does not infer a declaration-name occurrence span from a body hit.
  One batched symbol query is used per explained file. This is diagnostic
  extraction; a production multi-file ranker still needs per-request batching.
- Optional declaration evidence distinguishes zero from unknown. Unspecified
  display names stay unknown even when they occur incidentally in a function
  body; non-ASCII names also lack this raw-ASCII authority. A promised raw ASCII
  name absent from its definition range or a stale identity is an error.
- Original-source camel/snake/acronym/digit boundary signals use verified
  normalization provenance for Unicode. Literal membership, tokenizer and
  selected ranking remain unchanged.
- Native study exposes baseline, declaration-only, boundary-only,
  half-occurrence, no-occurrence and combined scores. Provisional declaration
  weights are 64/32/16; original-boundary weights are 16/8. These are experimental
  constants, not holdout-selected defaults. Only baseline is marked selected.
- Cursor order and explanation rank fingerprint share one policy authority;
  ordinary, explicit typo and components use distinct fingerprints. No cursor
  scoring-version bump is needed for these diagnostics because selected scores
  and their order did not change.
- `CodeSearchExecutionStatsV1` records successful ordinary-page work: literal
  prefilter execution, source-verification attempts, verified literal files,
  final candidate visits, all verified matches, cursor-eligible matches and
  fetched files. Public trace adds the actual returned-file count after response
  fitting. Counts are validated against the native page; conflicting counts
  are refused. This is not raw posting-visit or distinct pre-verification count.
  Exact-path/regex-only zero literal counts mean no literal prefilter execution.
  Explicit recovery/components remain unobserved; limits/cancellation remain
  typed errors rather than fabricated complete statistics.
- Existing runner diagnostics retain native planner traces. The optional
  `--rank-study-out` runner path now retains native pinned pages and per-file
  explanations after all measured requests. It reproduces the measured first
  window before following the original page-size cursors. Total/count drift,
  duplicate files, non-progressing cursors, limits and refusals remain explicit.
- `code_search_rank_study.py` binds the diagnostic to original record bytes,
  blind pack, effective request/profile, generation and source-file universe.
  It reuses existing file NDCG/Hit/MRR on proven complete pools. Each baseline
  comparison uses the same admitted tasks and lists exclusions/coverage.
  Unknown declaration census excludes declaration policies; a refused explain
  or stopped walk never becomes a zero or a full-rank claim. File-level features
  are explicitly not declaration-span recovery metrics.
- The driver accepts explicit bounded `code_search_rank_study` limits and
  retains original captures when optional study validation fails. A speed claim
  is refused with this mode: query timers exclude diagnostics, but whole-process
  CPU/RSS includes them. Separate performance captures are required. Deadlines
  are checked between SDK calls; the SDK I/O timeout bounds an individual call.
- The public pair-spec schema now admits this optional diagnostic configuration
  for both ordinary file policies and rejects unsupported routes, extra/invalid
  limits and speed claims. The exact-content profile is included in the schema.
- Actual native artifact replay exposed a source-binding omission: ordinary
  `code_search_file` records did not emit the capture's source repository and
  revision, although those values were available to the record producer. The
  producer now emits the pair. The evaluator checks candidate/capture identity;
  partial pairs are refused. Historical rows remain replayable without the pair,
  but cannot qualify a new complete-pool study, including zero-hit tasks.

### Development input refresh on 2026-10-04

The historical 1,196-query v1 declaration oracle suite is not accepted by the
current v3 oracle. Changing only the contract string also failed canonical gold
validation; neither refusal was weakened. A new suite and blind pack were
derived from the same 99 immutable Gin files and the same 1,196 ordered queries
in `/private/tmp/qi-rank-study-gin-1196-20261004-3l7aoaqo/`.
The file qrels changed for 15 tasks; canonical source-line gold changed for 86.
The current independent `census_checkers/go_checker.go` verified all 1,196 file
qrels over 99 files using Go's AST parser. No human relevance review is claimed.
This exposed declaration dataset is development-only, diagnostic and separate
from historical scores and any future unexposed holdout. Original input and
capture files were not changed. The historical corpus-manifest path no longer
exists; the new external manifest was rebuilt from the validated frozen file
universe and retains SHA-256
`d4e1ea025c067f344af640568b5bfd0835ed09c2bfbcf9668b8e9782bc68bd67`.
The first fresh native execution completed all 1,196 candidate pools but refused
391 explanations on the retained-byte defect above. Its 805 eligible samples
cannot represent the full suite. Those original diagnostic artifacts are
preserved; the repair is replayed in a separate state/output root.

### Fresh complete-pool replay after the retained-byte repair

`VERIFIED`: new native execution and source-validated consumer completed in
`/private/tmp/qi-rank-study-gin-1196-nfc-fixed-20261004-lxv_hswi/`.
The runner wall time was 170.982 s; post-measurement diagnostics consumed
87.370 s. These are development-build diagnostic costs on a shared host, not
qualified product latency. The binding file records exact binary/input hashes
and argv; no frozen compiled-source snapshot is claimed.

- Original records: 1,109 `success`, 87 `capped`, zero execution failures.
- Complete native pools: 1,196/1,196, 1,431 pages, 4,608 candidates and 4,608
  returned explanations. All five ablations admit all 1,196 tasks; no exclusions.
- `baseline-equality.json` compares status, entire candidates, query identity,
  rank unit, ordering and score evidence against the pre-repair native capture.
  All 1,196 result rows are identical. The fix changes diagnostic availability,
  not production retrieval quality.
- `ablation-report.json` validates native paging/exhaustion, original first-page
  reproduction, source identity and selected/experimental score algebra.
- `baseline-miss-rca.json` retains full source hashes, observed native ranks and
  additive features for all nine baseline misses. Every miss's positive file
  is present in the exhausted pool: these are ranking/top-ten misses in this
  development capture, not missing indexing or literal recall.

| Diagnostic policy | File Hit@10 | MRR@10 | File NDCG@10 | Regressions versus baseline |
| --- | --- | --- | --- | --- |
| Selected baseline | 1,187/1,196 | 0.924163 | 0.937654 | Reference |
| Declaration only | 1,195/1,196 | 0.997910 | 0.998290 | Zero Hit/MRR regressions on this exposed set |
| Original boundary only | 1,191/1,196 | 0.894187 | 0.916485 | One Hit regression (`L0137`), 92 MRR regressions |
| Half occurrence | 1,187/1,196 | 0.923536 | 0.937396 | Three MRR regressions |
| No occurrence | 1,187/1,196 | 0.914066 | 0.930258 | Two Hit regressions (`L0070`, `L0294`), 93 MRR regressions |
| Declaration + boundary, no occurrence | 1,196/1,196 | 0.998421 | 0.998815 | Zero Hit/MRR regressions on this exposed set |

| Task/query | Best positive native rank | Declaration-only rank | Combined rank |
| --- | --- | --- | --- |
| `L0978 Type` | 32 | 1 | 1 |
| `L1291 writeContentType` | 13 | 13 | 9 |
| `L0101 Err` | 15 | 1 | 1 |
| `L0187 H` | 14 | 1 | 1 |
| `L0245 Name` | 18 | 1 | 1 |
| `L0248 New` | 11 | 1 | 1 |
| `L0984 Use` | 13 | 1 | 1 |
| `L1193 route` | 11 | 1 | 1 |
| `L1293 x` | 17 | 1 | 1 |

`writeContentType` shows why these experiments do not authorize a default:
the request is folded, so exported `WriteContentType` declarations in other
render files receive the same declaration bonus (64) as the lowercase helper
in `render/render.go:37`. Both have exact-case *content* occurrences. Gold's
occurrence contribution is 4 versus 6 in earlier files; removing occurrence
leaves tied scores and the helper at rank 9. This is observed case/intent and
tie-order evidence, not proof that the combined policy is generally optimal.
Several common-name misses similarly tie at selected score 111 and are resolved
by stable path order. The development declaration gold does not evaluate useful
content/use-example ranking, precise declaration-name span recovery, typo,
prefix/infix or independently judged production intent.

### Local checks

All commands ran from this checkout; output was retained in the chat, not as
one-off repository evidence files.

| Command | Observed outcome | Scope |
| --- | --- | --- |
| `./scripts/cargow --lane code-search-rank-lane test -p quanta-index-lexical --test l3_exact_source code_search_explanation_uses_the_same_file_score_outside_top_k -- --exact` | RED: `NotIndexed`; then GREEN, 1 test | Confirmed backend defect |
| `./scripts/cargow --lane code-search-rank-lane test -p quanta-index-lexical -p quanta-index-search-plane --lib code_search` | `VERIFIED`: 23 + 14 tests passed | Native feature, score, grammar, count and cursor contracts |
| `./scripts/cargow --lane code-search-rank-lane test -p quanta-index-lexical --test l3_exact_source` | `VERIFIED`: final 28 tests passed | Sealed source/symbol index, unpromised display-name/body collision, Unicode, independent source oracle, gram collision, late better candidate, paging and exact-symbol separation |
| `./scripts/cargow --lane code-search-rank-lane test -p quanta-index-searchd-runtime --test runtime_fast_suite e2e_explain_score_trace::explain_score_traces_share_one_indexed_fixture -- --exact --nocapture` | RED: public planner refused CodeSearch; then GREEN. Final SDK extension `VERIFIED`: 1 passed | Real SDK/Unix IPC/runtime wiring, pinned native page, work counts and fixed 109/107/105 source score goldens |
| `uv run --frozen --extra dev python -m pytest -q tools/ci/tests/test_retrieval_benchmark.py -k 'independent_file_ndcg or declaration_judgment_requires or cross_suite_experiment_custody or v3_query_family_split or intended_name_source_oracle or code_search_file_refuses_context or code_search_file_policy_binds'` | `VERIFIED`: 7 passed | Existing file/declaration metrics and leakage guard |
| `uv run --frozen --extra dev python -m pytest -q tools/ci/tests/test_identifier_robustness_report.py -k 'scored_family_separately or fixed_golden_preserves or clean_to_typo or operation_and_length or policy_refusal or no_answer_success or rederived_from_source'` | `VERIFIED`: 7 passed | Existing lane, eligibility, no-answer and intended-name reporting |
| `git diff --check`; `python3 tools/ci/lint/check-module-discipline.py`; `just rust-hexagonal` | `VERIFIED` | Hygiene and dependency/facade boundaries |
| `just rust-cargo-modules` | `VERIFIED`: corrected core/contract snapshots match both native module trees | Module inventory, not behavioral qualification |
| `python3 tools/ci/lint/check-test-authority.py` | `VERIFIED` | Existing test targets remain registered |
| `./scripts/cargow --lane code-search-rank-lane test -p quanta-index-retrieval-bench --bin quanta-index-retrieval-bench` | `VERIFIED`: final 10 passed, including all 4 rank-study tests | Stable counts/order, duplicate/drift refusal, bounded work, ordinary-policy separation and existing runner guards |
| `./scripts/cargow --lane code-search-rank-lane build -p quanta-index-searchd-runtime --bin quanta-index-searchd --locked` | `VERIFIED`: fresh build after source-name guard | Standalone development-profile daemon for the explicitly pinned runner E2E; not release/performance qualification |
| `uv run --frozen --extra dev python -m pytest -q tools/ci/tests/test_code_search_rank_study.py tools/ci/tests/test_identifier_robustness_report.py` | `VERIFIED`: 63 passed | Complete-pool independent golden, artifact mutations, refusal/unknown/partial exclusions, zero-hit source binding, original exhaustion status and existing intent contracts |
| `uv run --frozen --extra dev python -m pytest -q tools/ci/tests/test_retrieval_benchmark.py -k 'code_search_file_policy_binds or exact_content_file_policy_binds or v3_pair_spec_schema or g0_schema_files_are_closed or g0_receipt_shape_matches_canonical_receipt_schema or v5_capture_schema'` | `VERIFIED`: 6 passed | Public diagnostic schema, ordinary/exact-content capture identity and forged-pin refusal |
| `./scripts/cargow --lane code-search-rank-lane test -p quanta-index-retrieval-bench --bin quanta-index-retrieval-bench record::tests::quanta_capture_binds_source_pin -- --exact` | `VERIFIED`: 1 passed | Both ordinary file policies emit source repository/revision |
| `./scripts/cargow --lane code-search-rank-lane test -p quanta-index-retrieval-bench --test sdk_roundtrip actual_runner_binary_emits_receipt_bound_v5_record -- --exact --nocapture` | `VERIFIED`: 1 passed, 84.40 s | Explicitly pinned standalone daemon; actual SDK/native complete and capped study artifacts |
| Source-validated Python replay of both actual runner artifacts | `VERIFIED`: complete and one-page-limited artifacts accepted with independent three-file gold | External output: `/private/tmp/qi-rank-study-source-pin-20261004.NhQYZU/`; limited pool remains excluded |
| `uv run --frozen --extra dev python -m pytest -q tools/ci/tests/test_code_search_rank_study.py tools/ci/tests/test_retrieval_benchmark.py -k 'code_search_rank_study or spec or server_observation or hybrid_fetch or quanta_strategy or query_protocol'` | `VERIFIED`: 38 passed | Study and affected driver/protocol contracts on live main; overlaps the preceding row |

Some first attempts had test-code compilation errors, corrected before the
passing runs. An accidental system Python 3.9 invocation failed on the existing
`zip(strict=True)` fixture; the frozen repository Python command passed.
One native command was not admitted after 300 seconds of another task's build
lock; a subsequent admitted attempt passed. Build/admission time is not query
or indexing latency.
The later SDK extension's raw paging call omitted its required generation pin;
that test fixture was corrected to the sealed identity. This was a test setup
failure, not a newly reproduced product defect. The corrected SDK extension
passed. The source-name authority guard and runner bounds tests passed, and the
standalone development-profile daemon was rebuilt afterwards for the runner
E2E. Rust checks waited at the shared resource admission lock,
including another task's full workspace run. An admission timeout does not
execute the product check.

### Remaining production-selection work, ordered by dependency

The actual runner E2E and fresh source-validated complete-pool replay are done.
No historical top-ten-only capture was promoted to a complete experiment.

1. Freeze development/holdout by repository and seed family before expanding
   exact/prefix/infix/components/typo/no-answer tasks. Reuse the existing source
   oracle, evaluator and review tools. Prepare default-content/use-example and
   typed-declaration pools separately; obtain genuine independent judgments.
2. Batch declaration enrichment once per ranked request before experimenting
   with production selection. Preserve unknown evidence, literal recall, typed
   errors, budget accounting, deterministic ordering and score-version cursors.
3. Select a frozen policy only after reviewed, unexposed holdout improvement;
   report regressions by intent and repository. Run comparable API/build/input
   latency and memory measurements on an uncontended host.
4. Bind served external inventories/versions and run the existing five-product
   capture per supported lane. Qualification does not follow from the historical
   Gin counts or from this local explanation fixture.

`VERIFIED`: diagnostic implementation, focused contracts, actual SDK/native
interop, all 1,196 complete development pools/ablations and unchanged original
retrieval rows after repair.
`NOT_RUN`: fresh expanded five-product capture, independent human relevance
qualification, unexposed holdout, comparable performance, release/deployment
and scale optimizations. No production default relevance improvement has been
established by this execution.

## 2026-10-04 follow-up: intent reporting and literal conformance

Status: `VERIFIED` for the scoped diagnostic code and execution below. Production
ranking/default selection remains unchanged and is not qualified by this work.

### Reporting repair

Owner: `tools/benchmark/retrieval/code_search_rank_study.py`; tests:
`tools/ci/tests/test_code_search_rank_study.py`.

- Paired file means now include intent/source-oracle-contract breakdowns with
  attempted counts, admitted IDs, exclusions, coverage and explicit regression
  IDs for Hit, MRR and NDCG. A fully excluded group has no quality mean.
- The report also computes an equal-family macro mean on admitted families.
  Missing family identity is explicit `not_available`; no artificial family
  assignment or repository-level inference is made.
- A study missing a suite task is refused, rather than shrinking its denominator.
- An independent four-task golden has two related declaration successes, a
  content regression and a refused content task. Overall candidate Hit is 2/3;
  equal-family Hit is 1/2; content candidate Hit is zero with 1/2 coverage. This
  proves that a declaration gain cannot conceal the content regression.

`VERIFIED`: `uv run --frozen --extra dev python -m pytest -q
tools/ci/tests/test_code_search_rank_study.py
tools/ci/tests/test_identifier_robustness_report.py` passed 67 tests on live main
and its overlay. Ruff, `git diff --check` and test-authority registration passed.
The original 1,196 raw artifacts were replayed with the new reader to the new
`ablation-report-intent-family.json` in their external output root; historical
reports were not overwritten. Original metrics and task coverage are unchanged.

### Additional 1,000 exact-content tasks

External output:
`/private/tmp/qi-rank-study-content-1000-dedup-20261004-z3_o0cua/`.
This remains a Gin development diagnostic, not a new independent holdout.

- Same immutable 99-file universe and Git commit as the declaration diagnostic.
- Sampling froze seed `content-lines-20261004-v1` and hash-ranked printable
  12..80-byte source lines, excluding bare names, function/type headers and
  comment/import/package headers. Population: 6,797 distinct lines. The first
  1,000 eligible normalized/near-deduplicated queries were selected before
  product execution; 11 near and eight normalized duplicates were skipped.
- An initial unfiltered preparation was correctly refused by the existing
  near-duplicate gate. The accepted preparation applied that same threshold;
  no evaluator admission rule was weakened.
- Gold is every file with a raw, case-sensitive literal occurrence. Existing
  `LiteralSourceOracleIndex` and `source_oracle_gold` generated the suite; a
  separate repeated-byte-find scanner matched all query occurrence sets.
  All tasks passed raw-versus-indexed-NFC file-membership validation.
- 64 queries have multiple positive files; maximum 21. This intentionally does
  not grade which matching file contains the most useful example.
- Public request policy is `code_search_exact_content_file`, with original
  top-k 10, native paging and generation/source-bound explanations. A new state
  root was used. Binary hashes, inputs and exact argv are in
  `capture-binding.json`; the previous verified development binary is reused
  and no frozen compiled-source snapshot or speed claim is made.
- Native execution: exit zero, 133.214 s runner wall time, 17.416 s additional
  diagnostics. Records: 999 `success`, one `capped`, no execution errors.
- All 1,000 pools exhausted. All 1,148 explanations returned. The entire native
  candidate file sets match independently derived oracle sets with zero missing
  or extra files (`literal-conformance.json`). The source-validated ablation
  reader admits all tasks with zero exclusions.

All policies have file Hit/MRR/NDCG of one on this literal contract because every
matching file is equally positive. This is a conformance result, **not evidence
of improved content/use-example ranking**. Meaningful graded relevance and
declaration-role retrieval need distinct reviewed judgments. Do not combine
these 1,000 tasks with the declaration score to inflate a quality claim.

### Remaining input and operational boundaries

- Existing B08 work already prepared ten development and twelve additional
  repositories. The current holdout release has 13,347 `code_only` files.
  Reuse that pipeline rather than constructing another release manager.
- At inspection, `/Users/songmin/Documents/code-new/qi-b08-closeout-20261004-2i72kj91/c3-split-preparation/result.json`
  reports `split_source_verified_review_labels_not_admitted`. A verified source
  split does not supply admitted relevance labels or current C4 product inputs.
  Genuine independent review and a release/source-bound C4 handoff remain gates
  for production-selection evidence. Automated reviewers are not human review.
- One symbol query per explained file is the current diagnostic mechanism.
  Production request-wide batching remains necessary **if** a declaration policy
  is selected; this experiment does not introduce an unused production ranker
  or change ordinary search's latency path.
- Host load averages were approximately 60 on the shared execution host. Fair
  performance qualification is `NOT_RUN`; diagnostic wall times above must not
  become product-speed comparisons.
- Fresh five-product qualification remains `NOT_RUN`: the active release's
  indexed universes, supported lane requests and served versions still require
  their own binding. Historical external rows and a legacy OpenGrok index dump
  do not establish that proof for the new release.

### 2026-10-04: Optional rank diagnostics must not invalidate selected-score Explain

A separate one-query SDK diagnostic reproduced a remaining operational defect:
`/private/tmp/qi-nfd-explain-red-20261004-qkgknzn1/`. The independently source-
validated Go fixture contains a decomposed combining accent and 120,000 spaces
before `func fooBar() {}`. Ordinary search succeeded with one file, score 105;
the complete pool also returned that file. Its Explain was refused with
`LEXICAL_COLLECTION_BUDGET_EXCEEDED` (64 MiB retained-byte limit). The runner
completed in 132.051 seconds on the contended host. This is a development binary
reproduction, not a qualified performance measurement or frozen build-source
claim; `binding.json` records input, fixture revision and binary hashes.

Cause: `explain_code_file` computed the valid selected score, then propagated
`ranking::study(...)?`. Full non-NFC provenance allocation for the optional
experiment could consequently invalidate the otherwise valid score explanation.
The preceding NFC fast path did not cover this fixture.

Repair, present in main commit `2d7c9370`:

- Core lexical trace represents an optional diagnostic refusal separately from
  the selected score. A returned study and a refusal are mutually exclusive.
- Catch only the optional study's typed collection-budget refusal, then check
  request cancellation/deadline. Identity, storage and interruption errors still
  propagate. Search scoring and resource limits are not changed.
- Public Explain retains the selected contribution and emits
  `explain.code_search_rank_study_v1.refused=LEXICAL_COLLECTION_BUDGET_EXCEEDED`.
  Dispatcher validation rejects contradictory studies, wrong engines, missing
  selected decomposition and other refusal codes.
- The benchmark verifies the selected score before excluding the task from
  experimental comparisons. It preserves original results, exclusions and
  coverage; no neutral or zero experimental score is synthesized.
- Extend the existing sealed Unicode fixture, the existing dispatcher score
  golden, and the consumer mutation test. No new production ranking policy is
  selected.

Verification on current main:

| Command / scope | Observed result |
| --- | --- |
| `uv run --frozen --extra dev python -m pytest -q tools/ci/tests/test_code_search_rank_study.py tools/ci/tests/test_identifier_robustness_report.py tools/ci/tests/test_identifier_robustness_strata.py tools/ci/tests/test_identifier_robustness_multiproduct_report.py` | `VERIFIED`: 83 passed in 9.53 s; the rank-study subset is 30 tests |
| Source-validated `code_search_rank_study` replay of the existing 1,196 declaration and 1,000 literal captures | `VERIFIED`: all comparison structures unchanged; new outputs and equality check in `/private/tmp/qi-nfd-explain-green-20261004-163xq3sz/`, original artifacts untouched |
| Ruff, test-authority inventory and `git diff --check` | `VERIFIED` |
| System Python 3.9 / mistaken test path attempts | `FAILED`: unsupported `zip(strict=True)` runtime / absent paths; not the project test verdict; superseded by the frozen environment command above |
| `./scripts/cargow --lane code-search-rank-lane test -p quanta-index-lexical --test l3_exact_source code_search_rank_study_recovers_original_boundaries_after_unicode_normalization -- --exact` | `VERIFIED`: 1 passed; compile 21.97 s, test 5.65 s; resource admission waited 2,600.19 s separately |
| `./scripts/cargow --lane code-search-rank-lane test -p quanta-index-search-plane --lib code_search_score` | `VERIFIED`: 2 passed; compile 64 s, test below 0.01 s |
| `./scripts/cargow --lane code-search-rank-lane test -p quanta-index-lexical --test l3_exact_source code_search_explanation_preserves_constraints_cancellation_and_global_auto_typo_gate -- --exact` | `VERIFIED`: 1 passed; compile 1.72 s, test 0.78 s |
| `./scripts/cargow --lane code-search-rank-lane build -p quanta-index-searchd-runtime --bin quanta-index-searchd -p quanta-index-retrieval-bench --bin quanta-index-retrieval-bench --locked` | Initial `FAILED`: runner referenced nonexistent `GenerationPin.source_repo_id`; repaired single-repository binding; rerun `VERIFIED`, build 20.28 s |
| `./scripts/cargow --lane code-search-rank-lane test -p quanta-index-retrieval-bench --bin quanta-index-retrieval-bench` | Initial test build `FAILED`: three `let-underscore-drop` violations in the NL token-limit fixture; named previous-value bindings repaired it. Final `VERIFIED`: all 11 passed, compile 6.61 s, tests 0.06 s |
| `uv run --frozen --extra dev python /private/tmp/qi-nfd-explain-green-20261004-163xq3sz/run_control.py` | `VERIFIED`: fresh-state SDK control plus independent source-bound Python reader; runner wall 22.243 s. Selected candidates, score and source span equal to RED; Explain returned with explicit diagnostic refusal; all ablation coverage 0, no fabricated quality mean |

Independent holdout/externals are concurrent work, not duplicated here. The
current Sourcegraph path-inventory receipt observes 12 repositories and 13,347
files but explicitly does not prove content postings. Actual AI relevance review
remains partial and unqualified; it is not human review. These observations do
not authorize production ranking selection, a fresh five-product ranking or a
performance comparison. Selected ranking remains baseline.

The build failure was introduced in shared main commit `62b4f2d0`, after the
original Explain repair. `GenerationPin` has only repo/revision/generation;
this benchmark's existing record authority requires the producer source repo
to equal the search repo. The runner now checks those real fields, retaining
revision/generation and first-page source/preview equality checks. This repair
does not introduce an unimplemented multi-source pin contract.

Final focused verification totals: 83 Python + 4 lexical/dispatcher + 11 runner
unit tests = 98 passing tests, plus the actual SDK RED-to-GREEN control and
unchanged 2,196-query artifact replays. Compilation and admission waits are
separate from test time. The Green artifact's `binding.json`, `execution.json`
and `red-green-verification.json` record the development binary/input binding,
22.243-second runner wall time and observable assertions. This is still
`diagnostic_unqualified`; no full benchmark, default-policy promotion or
repository-wide/release qualification is claimed.
