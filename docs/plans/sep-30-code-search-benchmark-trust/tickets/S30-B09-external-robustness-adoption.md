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

Per-run artifacts belong outside the checkout. Verification results and
commands are recorded below after implementation.
