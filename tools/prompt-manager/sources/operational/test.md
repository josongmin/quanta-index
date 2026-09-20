# Testing

- default Rust test rail: `just rust-profile test-fast`
- behavior changes require regression tests
- prompt-manager changes require `tools/prompt-manager/tests/test_pm.py`
- core policy/validator changes require regression tests under `crates/quanta-index-core/tests/` (current files: `hybrid_policy.rs`, `lexical_policy.rs`, `semantic_policy.rs`)
- property coverage for typed-translator / wire-format paths lives in the `lq-*` adapter crates' `tests/property_*.rs` (proptest)
- public contract / SDK surface changes require `just rust-public-api`
- IPC decoder / wire-shape changes require `just rust-fuzz-smoke`
- activation / generation / readiness / ingress changes require the owning `U/E/C/H-SP` runtime scenario proof

## Test rails

- unit/local fast: `just rust-profile test-fast`
- bounded integration: `just rust-profile test-integration-fast`
- lexical storage integration: `just rust-profile test-integration-storage`
- semantic storage integration: `just rust-profile test-integration-semantic`
- complete integration: `just rust-profile test-integration`
- cli smoke: `just rust-profile test-cli-smoke`
- fast e2e: `just rust-profile test-daemon-fast`
- risk-focused e2e: `just rust-profile test-daemon`
- exhaustive daemon e2e: `just rust-profile test-daemon-all`
- shared-surface validation: `just rust-profile validate-shared-surface`
- pyramid: `just rust-test-pyramid`
- property invariants: included in the integration rail (proptest) — covers `lq-*` adapter crates; core policy validators are currently example-based
- benchmark regression guard: `just rust-bench` (criterion) — covers `lq-norm` pipeline; core has no current bench rail

The local integration/CLI/daemon rails expand authoritative target IDs from
`tools/ci/test-authority.toml` and execute each selected scope in one nextest
process. `validate-shared-surface` deliberately shares one build lane across
one test-profile `--no-run` compile and its test phases, but keeps library,
integration, and CLI selectors in separate nextest processes so `--lib` cannot
expand across their package union.
Do not treat these local scopes as a replacement for the workspace-wide CI
nextest receipt. Keep the cataloged `test_threads` caps; composed scopes use the
smallest cap, and daemon tests boot real runtimes and must not expand to
CPU-count concurrency.
The runtime's 49 scenario source files are grouped into fast/risk/extended
suite binaries. Their catalog rows retain source-level ownership while mapping
to the shared Cargo target; the authority guard fails if a suite omits a row.
The fast daemon loop excludes the cold DSL matrix. Run `test-daemon` or
`rust-bench-dsl-truth` when benchmark truth is in scope.
The bounded integration slice excludes the slow text-authority shard scenarios,
`quanta-index-semantic`, Lance, DataFusion, and Arrow. The complete profile runs
fast, lexical-storage, and semantic scopes sequentially in one build lane so
their 8/2/4 thread caps remain independent.

## Heavy correctness rail

- `just rust-miri` — Miri UB detection (nightly, contract + core only)
- `just rust-careful` — cargo-careful stacked-borrows / debug-assert run (nightly)
- `just rust-tsan` / `just rust-asan` — ThreadSanitizer / AddressSanitizer (nightly + `-Z build-std`)
- `just rust-mutants` — cargo-mutants on `quanta-index-core`
- `just rust-udeps` — unused-dep detection via rustc (nightly)
- `just rust-llvm-lines` — monomorphization / IR growth budget
- `just rust-public-api` — contract / SDK public API snapshot diff
- `just rust-cargo-modules` — guarded module-tree snapshot diff
- `just rust-fuzz-smoke` — IPC decoder fail-closed smoke
- aggregate: `just rust-profile verify-rust-heavy`
- CI: scheduled nightly + workflow_dispatch via `.github/workflows/correctness.yml`
