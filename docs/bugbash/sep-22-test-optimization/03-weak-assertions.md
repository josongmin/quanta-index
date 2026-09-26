# Actionable weak-oracle audit (Sep 22)

> Archive classification: historical Sep 22 audit input. Use the active [TOPT ledger](../../../tickets/sep-22-test-optimization/INDEX.md), [current closeout](../../../tickets/sep-22-test-optimization/SEP25-CURRENT-CLOSEOUT.md), and [SEP-27-001](../../adr/SEP-27-001-documentation-authority-and-historical-record-custody.md).


Audit basis: `23bd3d7fa7af1122f904e59f1f514935bd5ffe7e`.
This file retains only tests that can stay green while the behavior named by
the test is broken. Setup assertions, intentionally polarity-only contract
tests, and cases already covered by an exact sibling oracle were removed.

## WA-1 — Trigram property tests treat the operation under test failing as success

- Severity: **H**
- Evidence:
  - `crates/quanta-index-lq-trigram/tests/property_cbor_roundtrip.rs:42-50`
    returns `Ok(())` when either `serialize_cbor` or `deserialize_cbor`
    fails.
  - `property_cbor_roundtrip.rs:58-68` also returns `Ok(())` when either
    deterministic encoding fails.
  - After the legitimate input-domain skips at `:81-90`, the valid
    intersection path at `:91-94` silently `continue`s when
    `intersect_trigrams` returns `Err`.
- Reachable failure mode: a regression that makes serialization,
  deserialization, or valid intersection fail for every generated case still
  produces a green 256-case property run. The property never reaches its
  equality/subset oracle.
- Owner/fix: `quanta-index-lq-trigram` property-test owner. Convert unexpected
  operation errors to `TestCaseError::fail` (including the generated case and
  error); retain only the explicit `< 3 bytes` and over-query-limit input
  skips.
- Verification rail:
  `./scripts/cargow test -p quanta-index-lq-trigram --test property_cbor_roundtrip`.
  Also mutation-check one forced error in each of serialize, deserialize, and
  valid intersection; each mutation must make the property target fail.

## WA-2 — E2E harness smoke accepts any non-empty in-corpus result

- Severity: **M**
- Evidence:
  - `crates/quanta-index-searchd-runtime/tests/e2e_matrix_inventory.rs:18-52`
    queries the literal `smoke_needle_rust`, then checks only that results are
    non-empty and that at least one returned path occurs anywhere in
    `SMOKE_CORPUS`.
  - `crates/quanta-index-searchd-runtime/tests/common/e2e_corpus.rs:41-63`
    shows the unique expected row is
    `alpha` / `src/lib.rs`; the other in-corpus rows are `beta` and `gamma`.
- Reachable failure mode: a dispatcher/index regression that ignores the
  query and returns `beta` or `gamma` passes the current smoke oracle because
  those paths are also in the ingested corpus.
- Owner/fix: searchd E2E harness owner. Assert the exact candidate-id/path set
  for this query (`alpha`, `src/lib.rs`) and reject extras. Keep the separate
  typed-error assertion.
- Verification rail:
  `./scripts/cargow test -p quanta-index-searchd-runtime --test runtime_fast_suite e2e_matrix_inventory::harness_smoke_scenarios_share_one_reopened_fixture`.
  A mutation that replaces the query result with `beta` must fail.

## WA-3 — Post-cutover crash test does not prove the published backup is complete

- Severity: **M**
- Evidence:
  `crates/quanta-index-searchd-runtime/tests/state_migration_owner_v1.rs:455-475`
  injects `AfterCutoverRename`, then checks only that the manifest file exists,
  `manifest.objects` is non-empty, and the staging directory is absent.
- Reachable failure mode: a cutover that publishes only one advertised object,
  omits another object, or publishes an object with the wrong digest remains
  green. `non-empty` is not the crash-safety invariant named by
  `leaves_the_complete_new_root`.
- Owner/fix: state-migration test owner. Run `run_offline_verify_v1` against the
  published destination and compare its manifest object set/digests with the
  expected backup manifest. Preserve the staging-absence assertion.
- Verification rail:
  `./scripts/cargow test -p quanta-index-searchd-runtime --test state_migration_owner_v1 a_crash_after_the_cutover_rename_leaves_the_complete_new_root`.
  Mutating the cutover to omit one object must fail this test.
