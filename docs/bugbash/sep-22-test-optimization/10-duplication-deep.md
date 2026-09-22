# Deep Duplication Audit (2026-09-22)

Companion to `01-duplication.md` (clusters D1–D9). This file goes deeper:
every cluster below is grouped by **behavior**, lists **all** `file:line`
members, gives a **keep / delete / merge** verdict per member, and includes an
exact merge sketch. A final section flags **near-duplicates differing by 1–2
lines**. Every cited body was opened (`read`/`sed`); grep-only members are
marked as such.

## Cluster E1 — searchd-runtime socket/config/send/wait helper quad (copy-paste, NEW)

The same four helpers are copy-pasted into (at least) four integration-test
binaries. Bodies verified identical modulo socket-name prefixes and 2-vs-3
socket arity.

- `crates/quanta-index-searchd-runtime/tests/dsl_scenarios.rs:130`
  (`unique_socket_paths`, 3-socket) + `:143` (`build_config`) + `:157`
  (`send_query_request`) + `:164` (`send_ingest_request`) + `:171`
  (`wait_until`)
- `crates/quanta-index-searchd-runtime/tests/end_to_end.rs:307`
  (`unique_socket_paths`, 2-socket) + `:320` (`build_config`) + `:337`
  (`send_query_request`) + `:344` (`send_ingest_request`) + `:351`
  (`wait_until`)
- `crates/quanta-index-searchd-runtime/tests/explain.rs:86`
  (`unique_socket_paths`, 3-socket) + `:102` (`build_config`) + `:116`
  (`send_query_request`) + `:123` (`send_ingest_request`) + `:130`
  (`wait_until`)
- `crates/quanta-index-searchd-runtime/tests/repo_map_end_to_end.rs:76`
  (`unique_socket_paths`, 2-socket) + `:90` (`build_config`) + `:104`
  (`send_query_request`) + `:111` (`send_control_request`, extra) + `:118`
  (`send_ingest_request`) + `:134` (`wait_until`)

Symptom: ~90 lines of identical socket/config/send/poll scaffolding per
binary; only the `qi-<tag>-query/control/ingest` filename prefixes differ
(`qi-dsl-*` vs `qi-query-test-*` vs `qi-explain-*` vs `qi-repomap-*`), and
whether a third (ingest) socket is minted.

Why bad: a harness change (e.g. the AF_UNIX 104-byte `/tmp` workaround comment
kept only in `end_to_end.rs:320`) must be ported N times; the copies already
drifted (2-socket vs 3-socket, `map_or` vs `map(...).unwrap_or(0)`).
Severity: **H**.

Fix (concrete): create `tests/common/searchd_test_config.rs` with
`unique_socket_paths(tag)`, `build_config(state_root, tag, with_ingest)`,
`send_query_request`, `send_ingest_request`, `wait_until`; wire each binary
with `#[path = "common/searchd_test_config.rs"] mod searchd_test_config;`
(the pattern `runtime_fast_suite.rs:7-8` already uses). Keep: the new common
module. Delete: all four per-file copies (keep repo_map's
`send_control_request` as the only per-file extra until generalized).

Merge sketch:

```rust
// tests/common/searchd_test_config.rs
pub(super) fn unique_socket_paths(tag: &str) -> Vec<PathBuf> { /* pid-nanos-seq, qi-{tag}-* */ }
pub(super) fn build_config(state_root: &Path, tag: &str, with_ingest: bool) -> SearchdConfig {
    let mut cfg = SearchdConfig::from_state_root(state_root.to_path_buf())
        .try_with_search_corpus_history_retention_limits(8, 16*1024*1024, 128, 256*1024*1024)
        .expect("valid test retention policy");
    // ... socket overrides from unique_socket_paths(tag) ...
    cfg
}
```

## Cluster E2 — sealed-manifest helper suite + door trilogy, lexical vs semantic (extends 01-D3)

Beyond the three ~100-line trilogy tests (01-D3), the **entire helper
preamble** is duplicated. Body-verified pairs:

| Helper | Lexical | Semantic | Delta |
|---|---|---|---|
| `type TestResult` | `sealed_manifest.rs:51` | `sealed_manifest.rs:49` | identical |
| `type LocateFile` | `:54` | `:52` | identical |
| `repo()` | `:76` | `:63` | fixture id string only |
| `revision()` | `:84` | `:71` | fixture id string only |
| `identity()` | `:140` | `:98` | identical shape |
| `generation_dir()` | `:150` | `:108` | identical shape |
| `struct Doors` | `:378` | `:134` | identical |
| `knock()` | `:384` | `:140` | query shape only (LqQuery vs embedding vec) |
| `typed_code()` | `:405` | `:157` | 1–2 lines: `Option<String>` vs generic `Option<SearchPlaneErrorCodeV2>` |
| `expect_admitted()` | `:419` | `:164` | 1 line: `Ok(1)` vs `Ok(2)` |
| `expect_refused()` | `:440` | `:201` | code param type only (`&str` vs `SearchPlaneErrorCodeV2`) |
| trilogy test 1 | `:1034` | `:543` | adapter/track only |
| trilogy test 2 | `:1213` | `:714` | adapter/track only |
| trilogy test 3 | `:1280` | `:781` | adapter/track only |

Symptom: ~150 lines of harness + ~300 lines of lifecycle tests maintained
twice; the two `expect_admitted` copies differ by the expected hit count
(`Ok(1)` lexical vs `Ok(2)` semantic), which is exactly the parameter the
merge needs.

Why bad: lifecycle-policy edits must land twice; the hit-count difference is
implicit instead of a named parameter; semantic-only `expect_doors_open`
(`semantic/tests/sealed_manifest.rs:187`) and `counter` (`:234`) show the
suites are already drifting apart. Severity: **H**.

Fix: keep one `tests/common/sealed_manifest_case.rs`-style harness
parameterized over adapter + `expected_hits: usize` (+ code-type via the
existing `typed_code` generic). Keep: semantic's generic `typed_code<T>` as
the canonical form; delete lexical's `typed_code`/`typed_open_code` pair in
favor of it. Merge sketch:

```rust
fn expect_admitted(doors: &Doors, what: &str, expected_hits: usize) -> TestResult { /* Ok(expected_hits) */ }
// lexical call: expect_admitted(&knock(&adapter, g), "intact", 1)?;
// semantic call: expect_admitted(&knock(&adapter, g), "intact", 2)?;
```

## Cluster E3 — twin CBOR proptest files, positions vs trigram (extends 01-D2, bodies fully verified)

- `crates/quanta-index-lq-positions/tests/property_cbor_roundtrip.rs:44`
  (`cbor_roundtrip_preserves_value`) vs
  `crates/quanta-index-lq-trigram/tests/property_cbor_roundtrip.rs:38` (same
  name). Both under `ProptestConfig { cases: 256 }`.
- `:70` (`cbor_encoding_byte_identical_across_builds`) vs trigram `:54`
  (same name, same double-build byte-equality shape).
- Third tests differ (positions `:term_postings_decode_preserves_inserted_positions`
  vs trigram `:intersect_is_subset_of_lookup_of_any_trigram`) — keep both.

Symptom: same two property names/shapes for two index types. Strictness
drift verified in the bodies: positions propagates builder/serde errors via
`TestCaseError::reject`, trigram silently `return Ok(())` on serde error —
the "same" test passes vacuously in trigram when serialization fails.

Why bad: doubles proptest wall-time; the trigram copy can go green while
exercising nothing on serde failure. Severity: **H**.

Fix: merge into one shared `proptest!` body generic over a
`CborRoundtrippable` trait (or a `macro_rules! cbor_roundtrip_tests!`), one
`cases: 256` budget, with the positions `reject` policy as canonical. Keep:
positions' strictness; delete: trigram's silent-`Ok(())` arms. Sketch:

```rust
macro_rules! cbor_roundtrip_tests { ($build:path, $ser:ident, $de:ident) => {
    #[test] fn cbor_roundtrip_preserves_value(/* ... */) {
        let idx = $build(/* ... */)?; // reject on error, never Ok(())
        // serialize -> deserialize -> prop_assert_eq!
    }
}}
```

## Cluster E4 — `*_fails_closed` negative matrix in SDK binding suite (extends 01-D6, bodies verified)

- `crates/quanta-index-sdk/tests/sdk_binding_owner_v1.rs:187`
  (`wrong_variant_fails_closed`), `:207` (`wrong_repo_pin_fails_closed`),
  `:225` (`active_selector_out_of_domain_fails_closed`), `:249`
  (`foreign_candidate_fails_closed`), `:270`
  (`window_disagreeing_with_rows_fails_closed`), `:290`
  (`rows_over_request_cap_fails_closed`). Shared scaffolding
  `scripted_query_server` (`:112`), `serve_one` (`:135`), `temp_dir` (`:157`)
  verified.

Symptom: 6 tests × ~20 lines sharing temp-dir + scripted-server +
`expect_err` + `matches!` skeleton; only the mutated axis differs (plus one
`Transport` vs `Binding` expectation at `:270`).

Why bad: each spins its own scripted socket server; new axis = new 20-line
copy. Severity: **M**.

Fix: single table-driven test over
`(name, request_mutator, response_mutator, expected)` rows; keep the positive
case (`matching_positive_response_passes_binding`, `:320`, body not opened —
grep-only) separate. Keep: `scripted_query_server`/`serve_one`/`temp_dir`
helpers. Delete: the six individual `#[test]` fns, replaced by one
`#[test] fn binding_failures_close_per_axis()` iterating the table.

## Cluster E5 — empty-string rejection multiplied per field (extends 01-D5, bodies verified)

- `crates/quanta-index-lq-obs/src/audit.rs:366`
  (`empty_tenant_id_rejected`), `:376` (`empty_user_id_rejected`), `:386`
  (`empty_action_rejected`), `:396` (`empty_resource_rejected`): identical
  8-line bodies, only the assigned field differs. Verified lines 365–403.
- `crates/quanta-index-lq-obs/src/dim.rs:303`
  (`empty_ticket_id_rejected`), `:313` (`empty_wave_id_rejected`), `:323`
  (`empty_tenant_id_rejected`), `:333` (`empty_repo_id_rejected`): identical
  8-line bodies. Verified lines 302–340.
- Name collision: `empty_tenant_id_rejected` exists in **both** files for two
  different validators.

Symptom: 8 one-field tests, copy-pasted match arms; cross-file name
collision confuses `cargo test` filtering.

Why bad: linear test-count growth per field; reviewers skim identical
bodies. Severity: **M**.

Fix: one test per file looping over field-setter closures, e.g.
`for (name, set) in [("tenant_id", |e: &mut Entry| e.tenant_id = "".into()),
...]`. Rename to `empty_audit_fields_rejected` /
`empty_dimension_fields_rejected`. Keep: `good_entry_validates` /
`valid_dimensions_accepted` as the positive anchors. Delete: the 8
one-field fns.

## Cluster E6 — same-name shard-ordering tests, positions vs trigram `source.rs` (extends 01-D8, bodies verified)

- `crates/quanta-index-lq-positions/src/source.rs:257`
  (`shards_out_of_doc_order_are_corrupt_not_merged`): serves first posting
  (`doc_id` 5), second errors `IndexCorrupted`, asserts iterator fused
  (`postings.next().is_none()`). Verified lines 256–272.
- `crates/quanta-index-lq-trigram/src/source.rs:222` (same name): single
  `intersect_trigrams` errors `IndexCorrupted`, no fusion check. Verified
  lines 221–230.

Symptom: same invariant ("out-of-order shards are corruption, not merged")
at two API shapes; the stronger copy checks fusion, the weaker does not.

Why bad: the "same" test gives different guarantees per index; the next
index type will copy the name again instead of inheriting a contract.
Severity: **M**.

Fix: align trigram on the stronger assertion set (error code + fused/terminal
behavior — assert a second call still errors / iterator is terminal), and
document the shared sharded-source contract once. Keep: both tests (different
APIs); strengthen the trigram body. One-line sketch for trigram:
`assert!(sharded.intersect_trigrams(&trigrams("alpha")).is_err())` a second
time to pin terminality.

## Cluster E7 — twin `serde_roundtrip_via_ciborium` in bridge (extends 01-D9, bodies verified)

- `crates/quanta-index-lq-bridge/src/version.rs:215`
  (`SourcegraphVersionTag` roundtrip, verified lines 214–232) vs
  `crates/quanta-index-lq-bridge/src/candidate.rs:208`
  (`BridgeCandidate` roundtrip, verified lines 207–221): identical
  serialize→deserialize→`assert_eq` shape, ~15 lines each; only the value
  construction differs (2 lines).

Symptom: two tests proving "my type round-trips" with duplicated
scaffolding; each new serializable bridge type invites another copy.

Why bad: boilerplate (`match ... assert!(false)`) dwarfs the assertion.
Severity: **L**.

Fix: one generic helper `assert_ciborium_roundtrip::<T: Serialize +
Deserialize + PartialEq + Debug>(value: &T)`; each type keeps a 2-line test.
Same helper absorbs the `error_serde_roundtrip_with/without_construct` pair
at `crates/quanta-index-lq-bridge/src/errors.rs:349,363` (grep-only, body not
re-opened — per 01-D9). Keep: both test names as 2-line callers; delete: the
inline serde scaffolding.

## Cluster E8 — same-name digest-mismatch tests in catalog suites (extends 01-D4, bodies verified)

- `crates/quanta-index-catalog/tests/idempotency.rs:202`
  (`a_row_that_does_not_match_its_digest_is_refused_typed`): corrupts
  `idempotency_v2.body_sha256` via SQL, expects `CATALOG_ROW_CORRUPT` on
  `claim_prepared`. Verified lines 201–233.
- `crates/quanta-index-catalog/tests/auxiliary.rs:319` (same test name):
  corrupts `auxiliary_rows_v1` + `auxiliary_tracks_v1` via SQL, expects
  `CATALOG_ROW_CORRUPT` on `all_rows`/`track_rows`. Verified lines 318–369.

Symptom: identical test name proving "bit-rot behind SQLite's back → typed
corrupt" for two tables; name collision confuses `cargo test <name>`
filtering (both match).

Why bad: shared invariant tested per-table; third table = third copy.
Severity: **M**.

Fix: rename to `idempotency_row_digest_mismatch_is_corrupt` /
`auxiliary_row_digest_mismatch_is_corrupt`; extract corrupt-via-SQL +
`typed_code == CATALOG_ROW_CORRUPT` into a common catalog test helper
(parameterized over a `CorruptibleTable` fixture). Keep: both scenarios as
one-line instantiations; delete: the duplicated open-corrupt-reopen
scaffolding (~30 lines each).

## Cluster E9 — same-policy tests that should STAY duplicated (verified, no merge)

- Cancellation-inside-search: `crates/quanta-index-lexical/tests/cancellation_inside_search.rs`
  (322 lines, native-collect/unindexed-scan/regex lanes) vs
  `crates/quanta-index-semantic/tests/cancellation_inside_search.rs` (410
  lines, exact/approximate dense lanes + process-wide hold). Heads verified:
  same "already-interrupted budget is observed inside execution" policy, but
  the lanes under test are disjoint and the semantic file carries unique
  machinery (`HOLD_TURN`, `DenseLaneHold`, `unit_vector`). Verdict: **keep
  both**; at most share `repo()`/`revision()` one-liners. Severity: **L**
  (no action).
- Error-code string/serde contract family across lq crates (01-D1):
  spot-verified `bridge/src/errors.rs:294-380` region and
  `structural/src/errors.rs:353-439` (`code_strs_unique`, `code_roundtrip`,
  `unknown_code_returns_none`, `code_serde_roundtrip_via_ciborium` shapes
  match modulo enum name). Verdict: **merge** via one generic
  `assert_code_str_contract::<C>()` helper (per 01-D1 fix); keep one 1-line
  caller per crate. Severity: **M**.

## Near-duplicates differing by 1–2 lines (flagged, not full clusters)

1. `expect_admitted`: lexical `:419` (`Ok(1)`) vs semantic `:164` (`Ok(2)`) —
   parameterize `expected_hits` (see E2). Severity M.
2. `typed_code`: lexical `:405`+`:412` (two concrete fns, `Option<String>`)
   vs semantic `:157` (one generic fn, `Option<SearchPlaneErrorCodeV2>`) —
   adopt the generic form everywhere (see E2). Severity L.
3. `expect_refused` code param: lexical `:440` (`code: &str`) vs semantic
   `:201` (`code: SearchPlaneErrorCodeV2`) — unify on the typed enum (see
   E2). Severity L.
4. Proptest error policy: positions `TestCaseError::reject(...)` vs trigram
   `return Ok(())` on serde failure — adopt `reject` (see E3). Severity H
   (silent pass).
5. `unique_socket_paths` arity/prefix: 2-socket (`end_to_end.rs:307`,
   `repo_map_end_to_end.rs:76`) vs 3-socket (`dsl_scenarios.rs:130`,
   `explain.rs:86`); prefixes `qi-dsl-*` / `qi-query-test-*` /
   `qi-explain-*` / `qi-repomap-*` — unify with `tag` + `with_ingest` params
   (see E1). Severity M.
6. `map_or(0, ...)` (`dsl_scenarios.rs:130-136`) vs
   `.map(...).unwrap_or(0)` (`end_to_end.rs:307-313`) for nanos — same
   expression, two spellings; the merge picks one. Severity L.
7. `seeded_runtime()` shape: `e2e_exact_count_window.rs:28` (5-file loop) vs
   `e2e_read_view.rs:36` (2-file literal) — bodies verified; same
   boot→ingest→seal→activate shape with different fixtures.
   `e2e_semantic_scope_cap.rs:42`, `e2e_snapshot_registry.rs:49`,
   `e2e_top_k_truth_table.rs:208` share the name (signature-confirmed via
   grep; bodies not opened). Consider a `seeded_runtime_with(rows)` helper;
   keep per-file fixtures. Severity L.
8. `unknown_code` literal: `"NOT_A_CODE"` (structural `:380`-region, per
   01-D1) vs `"granted_v2"` (`lq-obs/src/audit.rs:354`, verified) — same
   1-assert shape, domain-specific literal is correct; unify only the test
   name (`unknown_code_returns_none`). Severity L.
