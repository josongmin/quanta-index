# S30-B09 — external benchmark adoption and robustness diagnostics

Owner: retrieval benchmark. This extends B03/B05/B08; the existing suite,
runner, source oracle and evaluator remain the execution authorities.

## Adoption decisions

| External source | Adopted scope | Execution boundary |
| --- | --- | --- |
| [CodeSearchNet human judgments](https://github.com/github/CodeSearchNet/tree/106e827405c968597da938f6b373d30183918869) | Frozen natural-language qrel review seed; preserve repeated judgments, fractional mean grades, disagreements, and commit-pinned source URLs | Source snippets and file hashes must be materialized before a product capture. This intake is not a suite or a product score. |
| [Memtrace hybrid benchmark](https://github.com/syncable-dev/memtrace-public/blob/3a3084dd88e4b49adcb185eb09173dbf0bd547d3/benchmarks/fair/run_hybrid_retrieval_benchmark.py) | Independently implement diagnostic slices for literal containment and surviving identifier components on our own queries/source | No copied runner or dataset. The pinned repository's [license](https://github.com/syncable-dev/memtrace-public/blob/3a3084dd88e4b49adcb185eb09173dbf0bd547d3/LICENSE) contains derivative/competitive-use restrictions. |
| [COBE](https://ieeexplore.ieee.org/document/9892610/) | Paired clean/noisy natural-language query methodology | Its task is natural-language code retrieval. It does not supply identifier-typo gold. |
| [CLARC](https://huggingface.co/datasets/ClarcTeam/CLARC) | Candidate for a separate C/C++ snippet lane comparing original and neutral-renamed source | Raw 526-pair group1 data was inspected. Only positive labels are supplied; corpus attribution, long-query admission and target-only scoring must be resolved before execution. |
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

## Verification — 2026-10-04

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
uv run --frozen --extra dev python -m tools.benchmark.retrieval.codesearchnet_qrels \
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
```

The imported annotations and new diagnostic report fields are implemented.
Source materialization, local relevance review, declaration-position recovery,
qualified product comparisons and an unseen holdout remain separate unfinished scopes.
