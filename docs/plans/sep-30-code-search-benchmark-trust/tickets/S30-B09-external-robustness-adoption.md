# S30-B09 — external benchmark adoption and robustness diagnostics

Owner: retrieval benchmark. This extends B03/B05/B08; the existing suite,
runner, source oracle and evaluator remain the execution authorities.

## Current execution status

- Implemented: source-bound external intake, fractional qrels, explicit query
  admission, distinct-file scoring, parser pin preflight, and fresh five-product
  capture joins with mutation refusal tests.
- VERIFIED diagnostics: 2,624 native responses across CodeSearchNet six-language
  and CLARC original/neutral lanes; the original source-valid eight-repository
  typo cohort has 13,835 five-product responses and a fully replayed join.
- In progress: the new global declaration-authority cohort for all 12
  repositories. Its gold capsules are complete; the public 72-cell admission
  matrix and new product captures must complete before it receives a score.
- These are diagnostic captures. Local human relevance review, an unseen
  holdout, whole upstream corpus execution, and qualified performance remain
  separate gates. The eight-repository scores do not fill the new cohort.

## Adoption decisions

| External source | Adopted scope | Execution boundary |
| --- | --- | --- |
| [CodeSearchNet human judgments](https://github.com/github/CodeSearchNet/tree/106e827405c968597da938f6b373d30183918869) | Frozen natural-language qrel review seed; preserve repeated judgments, fractional mean grades, disagreements, and commit-pinned source URLs | Source snippets and file hashes must be materialized before a product capture. This intake is not a suite or a product score. |
| [Memtrace hybrid benchmark](https://github.com/syncable-dev/memtrace-public/blob/3a3084dd88e4b49adcb185eb09173dbf0bd547d3/benchmarks/fair/run_hybrid_retrieval_benchmark.py) | Independently implement diagnostic slices for literal containment and surviving identifier components on our own queries/source | No copied runner or dataset. The pinned repository's [license](https://github.com/syncable-dev/memtrace-public/blob/3a3084dd88e4b49adcb185eb09173dbf0bd547d3/LICENSE) contains derivative/competitive-use restrictions. |
| [COBE](https://ieeexplore.ieee.org/document/9892610/) | Paired clean/noisy natural-language query methodology | Its task is natural-language code retrieval. It does not supply identifier-typo gold. |
| [CLARC](https://huggingface.co/datasets/ClarcTeam/CLARC) | Separate C/C++ lexical snippet lanes for the pinned group1 original and neutral-renamed source | Both 425/526 admitted-task captures completed. The other101 tasks exceeded the explicit64-token profile and were not truncated. Positive-only targets are not exhaustive relevance labels. This is not the full1,245-pair evaluation in the [2026 paper](https://arxiv.org/abs/2603.04484). |
| [CoIR](https://github.com/CoIR-team/coir) / [CORE-Bench](https://arxiv.org/abs/2606.11864) | Reference for complementary code retrieval and repository-context evaluation | Full upstream imports and runs are NOT_RUN. Their document/repository units must retain separate populations and cannot be combined with identifier navigation. |
| [GitHub Typo Corpus](https://github.com/mhagiwara/github-typo-corpus) | Deferred | The official v1.0.0 archive URL returned 404 in this audit; no replacement dataset was synthesized. |
| [CodeScaleBench](https://github.com/sourcegraph/CodeScaleBench) | Existing workflow-benchmark follow-up | Agent task outcomes have a different unit from identifier-typo retrieval. |

The external datasets are public inputs exposed to implementers. Importing
them does not create an unseen holdout or a qualified comparison.

## Implemented benchmark boundaries

1. CodeSearchNet intake binds the upstream commit and exact CSV bytes. Original
   annotator grades and their row identities remain available; duplicate
   query/language/URL judgments are grouped using the upstream lowercase-query
   convention. Fractional means are preserved, never rounded into the current
   integer-grade suite. Outside-pool candidates remain unjudged. Upstream
   annotations do not invent local reviewer identities or source verification.
2. Identifier diagnostics record whether the casefolded query is a proper
   substring of its intended name, the reverse containment holds, or neither.
   They separately count original components of at least three characters
   that survive in the perturbed query under the source-oracle tokenizer.
   These are properties of the frozen input, not claims about an engine's
   tokenizer, ranking mechanism, or cause of a hit.
3. Historical queries, labels, captures and reports retain their original
   bytes. New metadata is attached to newly generated census data; old census
   files without the new fields remain readable.

## Acceptance and remaining work

- Fixed positive and negative fixtures cover grade averaging/disagreement,
  pinned-input refusal, malformed URLs, CSV errors, output isolation, literal
  containment, unchanged components, and historical census compatibility.
- The full 4,006-row CodeSearchNet CSV must be parsed and independently
  reaggregated; all 2,914 URL judgments and their grade means must agree.
- New input strata must be rederived from existing intended-name/query bytes
  without executing product searches or modifying old artifacts.
- Future product runs must record native requests and rank units. Default
  input, explicit OSA1 recovery and stateful Fuzzy Finder sessions have
  separate contracts. A missing capability is not an HTTP failure or an empty
  relevance result.
- Declaration-position recovery needs a symbol-capable product response;
  current file-only typo scores do not prove it. Ambiguous intended-navigation
  labels still need review. Repository/family-cluster statistics apply only
  after the applicable comparison admission.

## Verification — 2026-10-04 initial intake

Implemented owners:

- `tools/benchmark/retrieval/codesearchnet_qrels.py`: fixed upstream intake,
  raw-judgment preservation, fractional aggregation and external no-overwrite output.
- `tools/benchmark/retrieval/identifier_robustness_suite.py`: source-defined
  component identity and literal-containment metadata in newly generated census.
- `tools/benchmark/retrieval/identifier_robustness_report.py`: policy validation,
  source replay and eligible file-hit breakdowns; historical policy-free input remains valid.
- Their focused test files are enrolled in `tools/ci/test-authority.toml`,
  `Justfile` and the applicable source-closure profiles. Existing policy tests
  check owner enrollment, source-closure inclusion and nonempty collection.

| Scope | Verdict | Observed result |
| --- | --- | --- |
| Full official CodeSearchNet intake | VERIFIED | 4,006 raw judgments; 99 queries; 573 query/language pairs; 2,914 qrels; 575 disagreed qrels |
| Independent CodeSearchNet aggregation | VERIFIED | Separate CSV parser, integer histograms and rational mean calculation agree for every qrel; all original row identities/grades/notes preserved exactly once |
| Existing gin noisy-input strata | VERIFIED | All six frozen lanes rederived from intended-name/query bytes; original suites/census untouched; input distribution only |
| Focused owner/policy/authority tests | VERIFIED | 293 passed in 48.71 seconds; includes historical replay, policy tampering, forged upstream input and symlink output refusal |
| Final enrollment and source-closure tests | VERIFIED | 67 passed in 73.21 seconds after adding the two test owners to the existing execution scope and source profiles |
| Python style and test-authority guard | VERIFIED | Ruff check/format and `check-test-authority.py` passed |
| External review-seed product execution | NOT_RUN | Source snippet materialization, corpus licensing and executable scoring contract remain prerequisites |
| New five-product score or performance comparison | NOT_RUN | No product calls were made by this change; historical scores are not recomputed or combined |
| CLARC product lane | NOT_RUN | Original/neutral-renamed 526-pair inputs inspected; positive-only qrels and long-query admission remain execution constraints |

Input-only gin distribution under `camel-snake-v1`:

| Operation | Tasks | No intact component of length >=3 | Some/all intact components | Query is a proper substring of intended name |
| --- | ---: | ---: | ---: | ---: |
| Insertion | 1,192 | 116 | 1,076 | 0 |
| Deletion | 1,178 | 139 | 1,039 | 180 |
| Substitution | 1,192 | 135 | 1,057 | 0 |
| Transposition | 1,192 | 194 | 998 | 0 |
| Keyboard stress | 1,192 | 133 | 1,059 | 0 |
| Boundary stress | 1,056 | 270 | 786 | 0 |

The groups overlap: intact components and literal containment are independent
axes. Insertion additionally has 166 queries containing the complete intended
name. These observations explain which inputs can be matched through simpler
text overlap; they do not establish how any product retrieved a result.

Per-run evidence is outside the checkout:

- Raw public source audit: `/private/tmp/qi-public-bench-audit-y1tax5mf/manifest.json`.
- Official seed: `/private/tmp/qi-public-bench-audit-y1tax5mf/codesearchnet-review-seed-v2.json`,
  SHA-256 `fb40450f134ba4c95fea35b77932039d9c974a99177319d30f5ed7cd1ed77837`.
- Independent check and six input-only lane outputs:
  `/private/tmp/qi-b09-verification-20261004-ytzcxwj_/verification-manifest.json`.
  This manifest binds script, input and analysis-tool bytes; it is not a product-capture receipt.
- Memtrace code/data/license audit: `/private/tmp/qi-memtrace-audit-bDCppO/memtrace-public`.

Executed commands from the repository root:

```sh
python3 tools/benchmark/retrieval/codesearchnet_qrels.py \
  --csv /private/tmp/qi-public-bench-audit-y1tax5mf/codesearchnet-annotationStore.csv \
  --output /private/tmp/qi-public-bench-audit-y1tax5mf/codesearchnet-review-seed-v2.json
uv run --frozen --extra dev python -c "import runpy; runpy.run_path('/private/tmp/qi-b09-verification-20261004-ytzcxwj_/verify_inputs.py', run_name='__main__')"
uv run --frozen --extra dev python -m pytest -q \
  tools/ci/tests/test_codesearchnet_qrels.py \
  tools/ci/tests/test_identifier_robustness_strata.py \
  tools/ci/tests/test_identifier_robustness_report.py \
  tools/ci/tests/test_source_oracle_suite.py \
  tools/ci/tests/test_benchmark_policy.py \
  tools/ci/tests/test_check_test_authority.py
uv run --frozen --extra dev python tools/ci/lint/check-test-authority.py
uv run --frozen --extra dev python -m pytest -q \
  tools/ci/tests/test_benchmark_policy.py::test_execution_regressions_are_registered_to_the_real_owner_scope \
  tools/ci/tests/test_benchmark_policy.py::test_execution_regression_owners_have_nonempty_live_collection \
  tools/ci/tests/test_benchmark_source_closure.py
```

The imported annotations and new diagnostic report fields are implemented.
Source materialization, local relevance review, declaration-position recovery,
qualified product comparisons and an unseen holdout remain separate unfinished scopes.


## Execution follow-up — 2026-10-04

Additional reusable owners:

- `codesearchnet_materialize.py`: pinned full-file fetch, exact line-span
  materialization, source/snippet hashes, explicit unavailable-source ledger.
- `clarc_adapter.py`: pinned 526-pair original/neutral source admission;
  query-hash task identities prevent ordinal task IDs leaking target filenames.
- `external_snippet_benchmark.py`: six language-specific CSN packs, two CLARC
  packs, fractional/positive-only scoring, commitment-bound sidecars, full
  native source/record replay and expected execution-profile checks.
- `identifier_robustness_multiproduct_report.py`: source-derived input strata
  with independently replayed historical native/external rows; this offline
  historical report is not a fresh five-product run.
- `identifier_robustness_fresh_join.py`: matching suite/pack/source admission,
  actual merged-record/report binding, raw external replay, retry identity and
  separate intended-original-file versus near-name-file scoring.
- `holdout_c4.py`: source-validated ordinary-file projection and strict
  fixed-cohort projection; neither can repair a changed gold contract silently.
- `query_plan.py`, `run.py`, `evaluator.py`, native `main.rs` and pair/runner
  schemas: explicit bounded NL token configuration (1..64, default32),
  effective request/profile binding and exploratory-only nondefault admission.
- Six new Python owners are enrolled in the control scope, Justfile, closure
  profiles and policy collection checks. The required Python inventory adds
  observed NL test identities; SDK identities were sorted without deletion.
  Current exact required inventories are Python634/Rust168/SDK25.

Resolved defects:

1. Unsorted SDK inventory rejected otherwise valid SDK receipts. Sorting the
   canonical 25 identities preserves their membership; seven owner tests and
   eleven affected SDK-path tests passed.
2. Long CLARC queries were refused by the default32 profile (483/526).
   Actual native execution rejected the initial128-token experiment at the
   engine OR-fanout64 boundary before any query. The planner/CLI/schema now
   bound the token budget to64 (Rust imports the engine constant). The fresh
   freeze submits425/526 and preserves101 profile refusals, without truncation.
   Rust/Python tests exercise64 success and65 refusal.
   JavaScript source1308798 bytes is admitted through the existing explicit
   max-file-bytes option, preserving the full frozen file universe.
3. A partial external qrel pool was liable to hide source and denominator
   loss. Full CSN573 = submitted462 + source-blocked111; submitted462 =
   positive-known408 + no-positive-judged54. Those54 have undefined target
   quality, not proven no-answer. Failed execution is separately counted.
4. Native record validation alone accepted a valid but unintended product
   profile. The external wrapper now requires the commitment-bound NL policy
   and exact configuration, or the fixed Semble lexical-file profile. Unknown
   record fields, policy/config tampering and wrong product modes are refused.

Observed source materialization: 2,739 distinct source files, 2,613 fetched,
126 HTTP404, 2,746 admitted spans and 2,781 admitted qrels. A separate GitHub
contents-API cross-check of three unavailable sources also returned404. Fetched
GitHub URL bytes are not an independently verified Git tree/blob attestation.
The full CodeSearchNet corpus and original snippet license attribution are
not admitted for a qualified comparison.

| Executed scope | Verdict | Result |
| --- | --- | --- |
| Integrated new-owner/policy/closure tests | VERIFIED | 99 passed, 201.73s |
| Fresh-join/projection/enrollment/closure integration | VERIFIED | 80 passed,98.37s |
| Final snippet/projection/fresh-join/enrollment/closure slice | VERIFIED | 91 passed,71.02s |
| Gold-runtime/capsule/C4 owner suite | VERIFIED | 199 passed,585.89s; two subsequently added pinfile binding/tamper tests passed separately |
| Pinned-source/adapters/strata focused tests | VERIFIED | 46 passed, 20.76s before final profile negative fixture; profile wrapper separately 6 passed |
| Affected runner/planner Python file | FAILED, then focused repair VERIFIED | Initial full execution499 passed/1 expected-error-text assertion failed in1331.49s; error contract preserved and affected19 passed. The full file was not repeated. |
| Final profile/query-identity slice | VERIFIED | 14 passed,493 deselected,11.65s |
| Rust NL CLI and planner owner units | VERIFIED | 1 lib +1 bin test passed; canonical nextest list observed168 tests; initial admission timeout retained as historical failure |
| Semble external snippet captures | VERIFIED | Fresh NL64 8/8 captures/full source-record replays;1312/1312 execution success |
| Quanta external snippet captures | VERIFIED | Fresh NL64 8/8 captures/full source-record replays;1312/1312 executed/scored,0 execution failures; capped included |
| Fresh three-external-product original C5 typo | VERIFIED for2767; BLOCKED for1382 | Eight source-valid cells completed8301 calls; four stale-oracle cells excluded before capture |
| Fresh native original C5 typo pair | VERIFIED for2767; BLOCKED for1382 | Eight source-valid cells completed5534 responses; four stale-oracle cells excluded before capture |
| Fresh five-product join | VERIFIED for2767 | Full native merged-record/report binding, external raw-row replay and common eligibility agree; source-blocked1382 remain outside the scored cohort |
| Qualified performance / full upstream CSN / human local adjudication | NOT_RUN | Not established by these diagnostic runs |

Fresh common NL64 target results: CLARC original Quanta63/425 versus
Semble121/425; neutral Quanta41/425 versus Semble62/425. CSN positive-known
Quanta307/408 versus Semble387/408. Each CLARC full population remains526.
Historical NL128 Semble results161/526 and91/526 remain separate receipts. CLARC is positive-only, and neutralized identical
code groups retain the original labels. CSN NDCG is pool-estimated. These are
synthetic snippet/file diagnostics, not whole-repository or semantic rankings.

Per-run artifacts (outside the checkout):

- Materialization: `/private/tmp/qi-csn-materialization-20261004-6f8feb90-v1/manifest.json`.
- Eight prepared lanes: `/private/tmp/qi-external-snippet-prep-parent-yhwz2shs/prepared/manifest.json`.
- Semble capture/replay: `/private/tmp/qi-external-semble-run-4jwxqn5b/frozen-verify.json`
  and `score-summary.json` (commands and wall times in adjacent execution ledger).
- Historical strata replay: `/private/tmp/qi-c5-source-strata-offline-20261004-v3.json`.
- Fresh external C5 batch: `/private/tmp/qi-c5-external-source-strata-fresh-20261004-v2/ledger.json`.
- Fresh matched NL64 runs/replay/timings: `/private/tmp/qi-b09-snippet64-20261004-v2/RESULTS.md`
  and `score-summary.json` (valid native-v3 and semble outputs).
- Historical native rejected attempts: `/private/tmp/qi-b09-native-snippets-20261004-v1/`.
- Native C5 batch preparation: `/private/tmp/qc5t-azl3zc/`.

Capture cost audit: the 58,562-file/1.02GiB release is fully rehashed four
boundaries per external cell. Across12 cells this means about2.81million file
opens/49GiB read, plus the initial full Git replay. A fixed1024-file sample
produced identical digests but bounded4-worker hashing was slower on this host
(serial median0.607s,parallel0.914s). No hash-range/check-boundary shortcut or
performance patch was made. Host load and about24GiB swap invalidate any
qualified speed ranking; preparation, compile, validation, index and call
wall boundaries remain separate.

### Oracle freshness repair

Current vendored parsers resolve21 files that old oracle recipes excluded.
Four old C5 suites (1382/4149 tasks) therefore fail current public validation.
These are source-eligibility blocks, not product misses. The current C4 producer
also changes the declaration authority from language-specific contracts to the
`declaration_name_exact` contract over all supported declaration languages.
A fixed-cohort projection must
reject that change even when selected query bytes happen to be identical.
The reusable projector preserves source truth and rejects contract drift;
ordinary-file projection changes only the declared request mode.

The first fresh gold/matrix attempt used tree-sitter0.25.2 with
tree-sitter-language-pack0.13.0, whereas the source-locked environment uses
tree-sitter0.23.2 with language-pack0.9.1. On the same Svelte JavaScript bytes,
the first environment rejects the parse and the locked environment admits it.
This confirms runtime drift; it does not isolate a defect in one package.
The observed59 answerability changes
and11 span-set changes in that attempt are invalid-run diagnostics, not an
accepted source-truth change. Its artifacts are preserved and the matrix was
stopped. Official gold generation with the source-defined parser/tokenizer
pins completed12/12 capsules in1996.263s. The receipt is
`/private/tmp/qi-c5-oracle-pinned-TgLgCP/gold-batch-receipt.json`.
The public72-cell C4 matrix is now running in that same environment; the new
cohort is not scored until matrix/projection admission passes.

Frozen runtime boundaries are explicit: Quanta binaries were built from
clean `d7063ac755916d48867416d4f970b6aebc360abd`; the proposed new Python
driver/gold producer is clean `6f3e04856a03291098497c58c9a240ad217d9ee8`.
The harness `provenance.quanta.source_sha` identifies its driver checkout,
not the binary's build source. Separate binary build binding, executable hashes
and dependency checks are required; no engine6f3 or current-main claim follows
from driver6f3. Old records are never rebound to a new suite or source.

### Original-cohort fresh results

`/private/tmp/qi-c5-osa1-fresh-join-20261004/five-product-8-valid-v3.json`
joins exactly the original source-valid8 repositories and2767 tasks. Every
product has the same2767 eligible tasks. These are default file-search requests,
with product-specific matching behavior, rather than a Fuzzy Finder comparison.
The final join checks committed bytes of the external producer files and
refuses to aggregate pairs from different driver/binary/evaluator/corpus tuples.
Prior helper-version reports remain intact; final quality summaries are equal.

| Product | Intended-original-file Hit@10 | Near-name-file Hit@10 | Calls total seconds | Call p50 / p95 milliseconds |
| --- | ---: | ---: | ---: | ---: |
| Quanta |2734/2767|2734/2767|46.377|6.357 /65.242|
| Semble |1962/2767|1984/2767|85.852|5.578 /174.775|
| Sourcegraph |89/2767|89/2767|442.802|136.583 /296.711|
| cs |87/2767|87/2767|419.426|99.461 /451.979|
| OpenGrok |0/2767|0/2767|56.705|14.335 /51.845|

No speed ranking: host contention and measurement boundaries differ. Native
pair wall total3182.463s includes indexing and validation; it is not the query
sum. Quanta123 `capped` and Semble366 `abstained` responses remain eligible.
Sourcegraph85 of89 and cs83 of87 hits occur among85 queries that are literal
proper substrings of their intended names. In the380 tasks without any intact
original component of length>=3, intended-file hits are Quanta356, Semble25,
Sourcegraph27, cs26 and OpenGrok0. These input strata do not prove each engine's
matching implementation. Repository-cluster confidence intervals are
NOT_APPLICABLE:8 repositories are below the existing12-repository threshold.

### Sourcegraph UI capability control

The actual Sourcegraph6.8.0 Fuzzy Finder was exercised with Chrome154 and
Playwright1.62.1 in isolated browser contexts. Its Symbols panel sent
`patterntype:keyword type:symbol count:50`, rather than the regexp request
inferred from a static bundle. Repositories and Files sent separate `type:repo`
and `type:path` requests; their result units are not declaration results.

Independent CPython `ast` over the commit-bound Sympy source located
`sdm_irref` in `sympy/polys/matrices/sdm.py`, line1675. The prefix control
returned that declaration. Insertion, deletion, substitution and transposition
inputs each produced HTTP200 with zero candidates, both as cold pasted input
and after actual prefix/suffix keypresses in a separate warm context.
These are8 correlated controls for one symbol family and one deployed version.
They establish neither a population score nor behavior of the latest release.
The response exposes name/location, but no stable symbol ID.

Receipt, exact requests, source/file/script hashes and screenshots are outside
the checkout: `/private/tmp/qsgui-fIADFv/ui-receipt.json` and the adjacent
`ui-osa1-*-{cold,warm}.json` files. This UI diagnostic is separate from ordinary
Stream file-search scores; a regex expansion must not be labeled Fuzzy Finder.
