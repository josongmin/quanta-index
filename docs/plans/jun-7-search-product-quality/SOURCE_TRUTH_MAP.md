# Source Truth Map

## Baseline Capability and Correctness

- DSL capability inventory:
  - `docs/analysis/jun-4-dsl-capabilty.md`
- Sourcegraph parity guard:
  - `tools/benchmark/sourcegraph_parity.py`
- capability drift guard:
  - `tools/ci/lint/check-dsl-capability-truth.py`
- correctness verification split:
  - `docs/plans/jun-7-verification-hellgates/rfc.md`

## External Competitive Baseline

- Sourcegraph code search overview:
  - `https://sourcegraph.com/docs/code-search`
- Sourcegraph code search capabilities:
  - `https://sourcegraph.com/docs/code-search/features`
- Sourcegraph query syntax reference:
  - `https://sourcegraph.com/docs/code_search/reference/queries`

Use these only as the minimum external floor for overlapping non-semantic
code-search surfaces.

## Ranking / Relevance

- lexical owner seam:
  - `/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-lexical/src/lib.rs`
- route payload and provenance seam:
  - `/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-search-plane/src/query_dispatcher.rs`
- live scenario authority:
  - `/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-searchd-harness/src/scenarios.rs`
- broad quality execution rail:
  - `/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-searchd-runtime/tests/dsl_scenarios.rs`

## Snippet / Explain

- explanation contract:
  - `/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-contract/src/results/explanation.rs`
- query response contract:
  - `/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-contract/src/results/query_responses.rs`
- runtime explain rail:
  - `/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-searchd-runtime/tests/explain.rs`
- corpus snippet assertions:
  - `/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-searchd-runtime/tests/e2e_full_corpus.rs`

## Scale / Tail

- bench harness:
  - `/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-searchd-harness/src/bench_support.rs`
- warm runner:
  - `/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-searchd-harness/src/bin/dsl_warm_matrix.rs`
- cold runner:
  - `/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-searchd-harness/src/bin/dsl_cold_matrix.rs`
- bench readme:
  - `/Users/songmin/Documents/code-new/quanta-index/tools/benchmark/README.md`

## Operator / UX

- CLI surface:
  - `/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-searchctl/src/lib.rs`
- SDK consumer surface:
  - `/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-sdk/src/search.rs`
- query dispatcher for route/provenance details:
  - `/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-search-plane/src/query_dispatcher.rs`

## Existing Verification Rails

- fast correctness:
  - `just rust-verify-hellgate-fast`
- broad daemon lifecycle:
  - `just rust-verify-hellgate-broad`
- cross-repo ingress:
  - `just rust-verify-hellgate-cross-repo`
- perf compare:
  - `just rust-bench-dsl-compare`

## Current Packet Truth

- remaining gaps are mostly product-quality and operator-quality gaps
- this packet must not be used to make new DSL support claims
- semantic retrieval and hybrid fusion quality are deferred to a later packet

## Packet-Wide Implementation Principles

- relevance work uses judged truth and blocking metrics, not score anecdotes
- snippet and explain work uses structured spans, offsets, and provenance
- scale work uses seeded synthetic generators and declared tier manifests
- tail work uses route-family budgets plus diagnostic metadata
- operator UX work uses read-only, machine-readable diagnosis surfaces first
- ambiguity work uses typed repair metadata without semantic fallback
- UI contract work uses versioned DTO proof, SDK proof, and CLI parity
- external comparative claims use overlapping Sourcegraph lexical surfaces only
