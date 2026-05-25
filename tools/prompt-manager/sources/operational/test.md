# Testing

- default Rust test rail: `just rust-profile test-fast`
- behavior changes require regression tests
- prompt-manager changes require `tools/prompt-manager/tests/test_pm.py`
- policy/validator changes require property tests in `crates/quanta-index-core/tests/property_policies.rs`
- hot-path validator changes require updating `crates/quanta-index-core/benches/policy_bench.rs`

## Test rails

- unit/local fast: `just rust-profile test-fast`
- integration: `just rust-profile test-integration`
- cli smoke: `just rust-profile test-cli-smoke`
- e2e smoke: `just rust-profile test-daemon`
- shared-surface validation: `just rust-profile validate-shared-surface`
- pyramid: `just rust-test-pyramid`
- property invariants: included in the integration rail (proptest)
- benchmark regression guard: `just rust-bench` (criterion)

## Heavy correctness rail

- `just rust-miri` — Miri UB detection (nightly, contract + core only; rusqlite FFI is excluded)
- `just rust-careful` — cargo-careful stacked-borrows / debug-assert run (nightly)
- `just rust-tsan` / `just rust-asan` — ThreadSanitizer / AddressSanitizer (nightly + `-Z build-std`)
- `just rust-mutants` — cargo-mutants on `quanta-index-core`
- `just rust-udeps` — unused-dep detection via rustc (nightly)
- aggregate: `just rust-profile verify-rust-heavy`
- CI: scheduled nightly + workflow_dispatch via `.github/workflows/correctness.yml`
