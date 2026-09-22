# Test Duplication / Overlap Audit (2026-09-22)

Scope: `crates/` Rust tests (`#[test]` / `#[tokio::test]`). Every finding below was
verified by opening the cited file bodies (search hits alone were not used).

## Cluster D1 — error-code string/serde contract copy-pasted across 5 lq crates (duplicate)

Same three behaviors tested once per error-enum crate with near-identical bodies:

- `code_serde_roundtrip_via_ciborium` (5x):
  - `crates/quanta-index-lq-regex/src/errors.rs:409`
  - `crates/quanta-index-lq-bridge/src/errors.rs:335`
  - `crates/quanta-index-lq-structural/src/errors.rs:439`
  - `crates/quanta-index-lq-positions/src/errors.rs:397`
  - `crates/quanta-index-lq-trigram/src/errors.rs:277`
- `code_strs_are_unique` / `code_strs_unique` (same assertion, two names, 8x total):
  - `crates/quanta-index-lq-norm/src/errors.rs:255`
  - `crates/quanta-index-lq-regex/src/errors.rs:361`
  - `crates/quanta-index-lq-positions/src/errors.rs:287`
  - `crates/quanta-index-lq-trigram/src/errors.rs:230`
  - `crates/quanta-index-lq-bridge/src/errors.rs:294`
  - `crates/quanta-index-lq-obs/src/span.rs:369`
  - `crates/quanta-index-lq-obs/src/errors.rs:253`
  - `crates/quanta-index-lq-structural/src/errors.rs:353`
- `code_strs_roundtrip` / `code_roundtrip` / `code_strs_roundtrip_via_from_code_str` (same loop, 7x):
  - `crates/quanta-index-lq-regex/src/errors.rs:371`
  - `crates/quanta-index-lq-obs/src/span.rs:379`
  - `crates/quanta-index-lq-obs/src/errors.rs:263`
  - `crates/quanta-index-lq-trigram/src/errors.rs:240`
  - `crates/quanta-index-lq-bridge/src/errors.rs:304`
  - `crates/quanta-index-lq-structural/src/errors.rs:363`
  - `crates/quanta-index-lq-norm/src/errors.rs:266` (+ `crates/quanta-index-lq-positions/src/errors.rs:298`)
- `unknown_code_returns_none` / `from_code_str_rejects_unknown` / `code_unknown_returns_none` (same 1–2 asserts, 7x):
  - `crates/quanta-index-lq-bridge/src/errors.rs:311`
  - `crates/quanta-index-lq-obs/src/span.rs:386`
  - `crates/quanta-index-lq-structural/src/errors.rs:380`
  - `crates/quanta-index-lq-norm/src/errors.rs:275`
  - `crates/quanta-index-lq-positions/src/errors.rs:307`
  - `crates/quanta-index-lq-obs/src/errors.rs:270`
  - `crates/quanta-index-lq-trigram/src/errors.rs:247`

Symptom: ~20 near-identical unit tests; e.g. bridge `code_roundtrip` (`errors.rs:304`)
is the same 3-line for-loop as structural `code_roundtrip` (`errors.rs:363`) modulo
the enum name.

Why it is bad (duplicate/cheesy): copy-paste contract means a real regression (e.g. a
duplicate `as_code_str`) must be fixed N times; inconsistent test names hide the
shared contract; each crate pays its own compile/test time for the same assertion shape.

Fix: extract one generic contract test. Options: (a) a shared `lq-contract-test`
helper crate with `assert_code_str_contract::<C>()` generic over the code enum trait,
called once per crate (1 line each); or (b) keep one canonical test in
`quanta-index-contract` and delete the per-crate copies. Normalize on one name
(`code_strs_unique` / `code_strs_roundtrip` / `unknown_code_returns_none`).

## Cluster D2 — twin proptest files: positions vs trigram CBOR roundtrip (duplicate)

- `crates/quanta-index-lq-positions/tests/property_cbor_roundtrip.rs:44`
  (`cbor_roundtrip_preserves_value`) vs
  `crates/quanta-index-lq-trigram/tests/property_cbor_roundtrip.rs:38` (same name,
  same shape: build index → serialize → deserialize → `prop_assert_eq`).
- `crates/quanta-index-lq-positions/tests/property_cbor_roundtrip.rs:70`
  (`cbor_encoding_byte_identical_across_builds`) vs
  `crates/quanta-index-lq-trigram/tests/property_cbor_roundtrip.rs:54` (same name,
  same double-build byte-equality shape). Both files also configure
  `ProptestConfig { cases: 256 }` identically.

Symptom: two heavyweight proptest binaries proving the same two properties for two
index types.

Why it is bad (slow/duplicate): doubles proptest wall-time for one logical property
(deterministic CBOR roundtrip); drift risk — positions version uses
`TestCaseError::reject` on builder/serde errors while trigram silently returns
`Ok(())`, so the "same" test has different strictness.

Fix: parametrize over a `CborRoundtrippable` trait (or a macro in a shared test
helper) with one `proptest!` body; or merge into a single integration test crate
that runs both index types under one `cases: 256` budget. Align the reject-vs-skip
policy while merging.

## Cluster D3 — lexical vs semantic `sealed_manifest.rs` door/quarantine trilogy (duplicate)

Same three test names, same scenario outline, only the adapter/track differs:

- `a_door_finding_is_quarantined_only_by_the_adapters_re_proof`:
  `crates/quanta-index-lexical/tests/sealed_manifest.rs:1034` vs
  `crates/quanta-index-semantic/tests/sealed_manifest.rs:543` (~100 lines each:
  intact → foreign → damage → restore → re-proof quarantine → ask-again → discard).
- `a_scrub_resumed_over_a_reclaimed_generation_is_refused_not_quarantined`:
  `crates/quanta-index-lexical/tests/sealed_manifest.rs:1213` vs
  `crates/quanta-index-semantic/tests/sealed_manifest.rs:714`.
- `an_interrupted_reclaim_is_out_of_the_namespace_and_finished_once`:
  `crates/quanta-index-lexical/tests/sealed_manifest.rs:1280` vs
  `crates/quanta-index-semantic/tests/sealed_manifest.rs:781`.

Symptom: ~300 lines of quarantine lifecycle logic maintained twice.

Why it is bad (duplicate/slow): both spin tempdirs and full seal/damage cycles;
a lifecycle-policy change must be edited in two large files in lockstep; failures
in one track easily mask drift in the other.

Fix: extract a shared track-parameterized harness (e.g.
`tests/common/sealed_manifest_case.rs` generic over `LexicalAdapter |
SemanticAdapter` with `TrackKind` as a parameter) and keep one test body with two
one-line instantiations. If adapters cannot share a trait yet, at minimum extract
the `expect_admitted` / `expect_refused` / inventory-shape assertions into common
helpers.

## Cluster D4 — same-name digest-mismatch tests in two catalog suites (overlapping)

- `crates/quanta-index-catalog/tests/idempotency.rs:202`
  (`a_row_that_does_not_match_its_digest_is_refused_typed`: corrupt
  `idempotency_v2.body_sha256` via SQL, expect `CATALOG_ROW_CORRUPT` on
  `claim_prepared`)
- `crates/quanta-index-catalog/tests/auxiliary.rs:319` (same test name: corrupt
  `auxiliary_rows_v1` + `auxiliary_tracks_v1` values via SQL, expect
  `CATALOG_ROW_CORRUPT` on `all_rows`/`track_rows`)

Symptom: identical test name proving "bit-rot behind SQLite's back → typed corrupt"
for two catalog tables.

Why it is bad (duplicate/weak): the shared invariant (stored-hash verification on
read) is tested per-table instead of once; adding a third table means a third copy.
The name collision also confuses `cargo test <name>` filtering (both match).

Fix: rename to `idempotency_row_digest_mismatch_is_corrupt` /
`auxiliary_row_digest_mismatch_is_corrupt`, and extract the corrupt-via-SQL +
`typed_code == CATALOG_ROW_CORRUPT` pattern into a common catalog test helper.
Consider one parameterized test over a `CorruptibleTable` fixture.

## Cluster D5 — empty-string rejection multiplied per field (parametrizable repeats)

- `crates/quanta-index-lq-obs/src/audit.rs:366,376,386,396`
  (`empty_tenant_id_rejected`, `empty_user_id_rejected`, `empty_action_rejected`,
  `empty_resource_rejected`): identical 8-line bodies, only the field differs.
- `crates/quanta-index-lq-obs/src/dim.rs:303,313,323,333`
  (`empty_ticket_id_rejected`, `empty_wave_id_rejected`, `empty_tenant_id_rejected`,
  `empty_repo_id_rejected`): identical 8-line bodies, only the field differs.
- Note `empty_tenant_id_rejected` exists in *both* files
  (`audit.rs:366` vs `dim.rs:323`), testing two different validators under one name.

Symptom: 8 one-field tests with copy-pasted match arms.

Why it is bad (cheesy/duplicate): linear test-count growth per field; reviewers
skim identical bodies; cross-file name collision confuses test filtering.

Fix: collapse each file's group into one test looping over field-setter closures,
e.g. `for (name, set) in [("tenant_id", ...), ...]`, or use `test_case` /
`rstest`. Rename to `empty_audit_fields_rejected` and `empty_dimension_fields_rejected`.

## Cluster D6 — `*_fails_closed` negative matrix in SDK binding suite (parametrizable)

- `crates/quanta-index-sdk/tests/sdk_binding_owner_v1.rs:187,207,225,249,270,290`
  (`wrong_variant_fails_closed`, `wrong_repo_pin_fails_closed`,
  `active_selector_out_of_domain_fails_closed`, `foreign_candidate_fails_closed`,
  `window_disagreeing_with_rows_fails_closed`, `rows_over_request_cap_fails_closed`):
  each builds a scripted server via `run_scripted`/`scripted_query_server`,
  sends one malformed response, asserts one `Binding{axis}` (or Transport) variant.

Symptom: 6 tests sharing the `temp_dir` + `text_request` + `text_response` +
`expect_err` + `matches!` skeleton; only the mutated axis differs.

Why it is bad (cheesy/slow): each spins its own scripted socket server; adding a
new axis means another ~20-line copy. The suite already has the right shape for a
table test (`coverage_table_exact_matches_sdk_surface` at line 366 proves the team
uses table-driven coverage).

Fix: convert to a single table-driven test over
`(name, request_mutator, response_mutator, expected_axis)` rows, keeping the
positive case (`matching_positive_response_passes_binding`, line 320) separate.
One server-setup helper per row instead of six binaries' worth of setup.

## Cluster D7 — same-name constructor-guard tests, different strictness (overlapping)

- `crates/quanta-index-embed/src/cache.rs:2495`
  (`zero_ceilings_are_refused_at_construction`: 5-arg
  `EmbeddingCacheRetentionPolicy::new` + DEFAULT invariant, 8 asserts) vs
  `crates/quanta-index-core/tests/ingest_resource_policy.rs:246` (same name,
  3-arg `IngestResourcePolicy::new`, 4 asserts).

Symptom: same test name, same "zero ceilings refused" idea, different types and
different exhaustiveness (embed also checks `Duration::ZERO`, total-vs-namespace
ceiling, DEFAULT invariant).

Why it is bad (duplicate/weak): name collision across crates; the weaker copy
(core, 4 asserts) under-tests relative to the stronger copy — suggests the guard
pattern lacks one shared constructor-policy test.

Fix: rename per type (`retention_policy_zero_ceilings_refused`,
`ingest_policy_zero_ceilings_refused`); if both policies share a ceiling-validation
helper, test the helper once and keep one smoke assert per constructor.

## Cluster D8 — same-name shard-ordering tests in positions vs trigram `source.rs` (overlapping)

- `crates/quanta-index-lq-positions/src/source.rs:257`
  (`shards_out_of_doc_order_are_corrupt_not_merged`: asserts first posting served,
  second errors `IndexCorrupted`, iterator fused) vs
  `crates/quanta-index-lq-trigram/src/source.rs:222` (same name: single
  `intersect_trigrams` errors `IndexCorrupted`).

Symptom: same invariant ("out-of-order shards are corruption, not merged") proven
at two API shapes with no shared assertion.

Why it is bad (duplicate/weak): the stronger positions copy checks fusion
(`postings.next().is_none()`); the trigram copy does not — the "same" test gives
different guarantees per index.

Fix: align both on the stronger assertion set (error code + fused/terminal
behavior), and note the shared sharded-source contract in one place (or a shared
trait test) so the next index type inherits it instead of copying the name.

## Cluster D9 — twin `serde_roundtrip_via_ciborium` in one file (mergeable)

- `crates/quanta-index-lq-bridge/src/version.rs:215`
  (`SourcegraphVersionTag` roundtrip) vs
  `crates/quanta-index-lq-bridge/src/candidate.rs:208`
  (`BridgeCandidate` roundtrip): identical serialize→deserialize→`assert_eq` shape,
  same `ciborium` calls, ~15 lines each.

Symptom: two tests in the same crate proving "my type round-trips" with duplicated
scaffolding.

Why it is bad (cheesy): each new serializable bridge type invites another copy;
boilerplate (`match ... assert!(false)`) dwarfs the assertion.

Fix: one generic `assert_ciborium_roundtrip::<T>()` helper in the bridge test
module; each type keeps a 2-line test calling it. Same helper can absorb D1's
`error_serde_roundtrip_with/without_construct` pair at
`crates/quanta-index-lq-bridge/src/errors.rs:349,363`.
