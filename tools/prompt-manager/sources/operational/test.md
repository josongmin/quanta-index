# Testing

- default Rust test rail: `cargo test --workspace`
- behavior changes require regression tests
- prompt-manager changes require `tools/prompt-manager/tests/test_pm.py`
- policy/validator changes require property tests in `crates/quanta-index-core/tests/property_policies.rs`
- hot-path validator changes require updating `crates/quanta-index-core/benches/policy_bench.rs`

## Test rails

- unit: `just rust-test-unit`
- integration: `just rust-test-integration`
- e2e smoke: `just rust-test-e2e`
- pyramid: `just rust-test-pyramid`
- property invariants: included in the integration rail (proptest)
- benchmark regression guard: `just rust-bench` (criterion)

## Heavy correctness rail

- `just rust-miri` — Miri UB detection (nightly, contract + core only; rusqlite FFI is excluded)
- `just rust-careful` — cargo-careful stacked-borrows / debug-assert run (nightly)
- `just rust-tsan` / `just rust-asan` — ThreadSanitizer / AddressSanitizer (nightly + `-Z build-std`)
- `just rust-mutants` — cargo-mutants on `quanta-index-core`
- `just rust-udeps` — unused-dep detection via rustc (nightly)
- aggregate: `just verify-rust-heavy`
- CI: scheduled nightly + workflow_dispatch via `.github/workflows/correctness.yml`
