# RB-03 — Benchmark-Owned Chunking Ablations

Status: `planned`

Depends on: RB-00 stage A; integration with RB-02 batch contract

Owner: `benchmarks/retrieval/src/chunking/`

## Goal

Measure how chunk boundaries affect actual Quanta retrieval without putting experimental chunkers into production crates or relying on Semantica at runtime.

## Work

1. Define a deterministic `Chunker` contract over original file bytes. Output path, byte span, one-based line span, text slice, strategy/version/config and stable content-bound chunk ID. A separate validator, not the strategy itself, checks every span against source bytes and line terminators.
2. Implement `whole_file` diagnostic control; fixed byte/token window with explicit overlap; and syntax-aware chunks for the pilot language. Declare parser/version and unsupported-language policy. Avoid pretending fixed windows or whole-file chunks are Semantica-equivalent. Predeclare boundary variants and parameters on the tuning split; do not tune on frozen eval.
3. For every strategy, record total chunks, distribution of sizes, duplication/overlap, uncovered bytes, truncated declarations, parse fallback and indexable-file coverage. Fallbacks are explicit configuration and artifact rows, never silent strategy changes.
4. Hold Quanta query route, ranking, model and corpus constant while varying only chunking. Rebuild dependent semantic sources for each strategy. If a required semantic/structural projection cannot be regenerated, mark that route ineligible for the ablation rather than compare mismatched sources.
5. Keep experiment code in the benchmark package. Promoting a winner into production is a separate later decision with product ownership and validation.

## Planned files

- `benchmarks/retrieval/src/chunking/{mod,whole_file,fixed_window,syntax}.rs`
- `benchmarks/retrieval/tests/chunking_contract.rs`

## Acceptance / verification

- Tests cover UTF-8 multibyte boundaries, CRLF/LF, no trailing newline, empty/binary/oversized files, long declarations, nested syntax, invalid parses and overlap limits.
- Every emitted `ChunkRecord` text equals the original byte slice; every line span covers that slice; IDs and order are repeatable across runs.
- [TEST-PLAN.md](TEST-PLAN.md) T08–T10 test both boundary correctness and stale semantic-source/vector reuse across strategies.
- Missing/duplicated coverage is measured and reported, not hidden behind a passing relevance number.
- `./scripts/cargow test -p quanta-index-retrieval-bench` runs focused strategy and SDK-boundary tests after integration.
