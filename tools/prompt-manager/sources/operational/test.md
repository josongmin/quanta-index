# Testing

- default Rust test rail: `just rust-profile test-fast`
- behavior changes require regression tests
- prompt-manager changes require `tools/prompt-manager/tests/test_pm.py`
- core policy/validator changes require regression tests under `crates/quanta-index-core/tests/` (current files: `hybrid_policy.rs`, `lexical_policy.rs`, `semantic_policy.rs`)
- property coverage for typed-translator / wire-format paths lives in the `lq-*` adapter crates' `tests/property_*.rs` (proptest)

## Test rails

- unit/local fast: `just rust-profile test-fast`
- integration: `just rust-profile test-integration`
- cli smoke: `just rust-profile test-cli-smoke`
- e2e smoke: `just rust-profile test-daemon`
- shared-surface validation: `just rust-profile validate-shared-surface`
- pyramid: `just rust-test-pyramid`
- property invariants: included in the integration rail (proptest) — covers `lq-*` adapter crates; core policy validators are currently example-based
- benchmark regression guard: `just rust-bench` (criterion) — covers `lq-norm` pipeline; core has no current bench rail

## Heavy correctness rail

- `just rust-miri` — Miri UB detection (nightly, contract + core only)
- `just rust-careful` — cargo-careful stacked-borrows / debug-assert run (nightly)
- `just rust-tsan` / `just rust-asan` — ThreadSanitizer / AddressSanitizer (nightly + `-Z build-std`)
- `just rust-mutants` — cargo-mutants on `quanta-index-core`
- `just rust-udeps` — unused-dep detection via rustc (nightly)
- aggregate: `just rust-profile verify-rust-heavy`
- CI: scheduled nightly + workflow_dispatch via `.github/workflows/correctness.yml`
