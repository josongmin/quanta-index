# Deep dive: lexical + core + ipc + embed tests

Scope: `crates/quanta-index-lexical/tests/` (17 files), `crates/quanta-index-core/tests/` (7 files),
`crates/quanta-index-ipc/tests/` (5 files), `crates/quanta-index-embed/src/` unit tests
(`cache.rs`, `openai.rs`, `telemetry.rs`; no `tests/` dir). Every file:line cited below was opened.

## 1. Behavior map

### lexical (17 integration files, ~11.5k lines)
| File | Behavior |
|---|---|
| `tantivy_smoke.rs:390` | Round-trip + keyword/All/regex/RawString/phrase/filter/type/select/count/top-k/collapse (~25 tests, 3627 lines — biggest file) |
| `sealed_manifest.rs:500` | Sealed-generation doors: manifest/overlay/segment integrity, quarantine (~10 tests, 1389 lines) |
| `unicode_normalization_goldens.rs:861` | Golden normalization table on both routes + NFC/NFD agreement + token refusal (~7 tests, 1312 lines) |
| `text_authority_shards.rs:285` | Shard rewrite/link, boundary docs vs rebuild, digest mismatch, crash catch-up (~6 tests, 937 lines) |
| `generation_delta_base_carryforward.rs:1` | Delta inherits base incl. ordering hazard + newly-written-bytes cost cap (400 filler scopes) |
| `g0l_tantivy_snapshot_probe.rs:45` | Vendor gate: segment immutability, hard-link reuse, snapshot pinning, rebuild-vs-delta ranking (2000 docs) |
| `cancellation_inside_search.rs:35` | Budget observed inside collect/scan/regex-verify; 3000-doc seed |
| `execution_budget.rs:36` | Examined-candidate budget: exact-set refuses, pages serve (5 docs, budget 4) |
| `regex_cache_bounds.rs:38` | Bitmap restriction, byte+cardinality eviction, Arc-shared hits (6 docs) |
| `regex_literal_alternation.rs` | Literal-vs-regex alternation semantics |
| `ranked_pages.rs` | Page spec / ranking stability |
| `explain_candidate.rs:1` | Explain output shape |
| `planner_authority.rs` | Planner authority decisions |
| `boot_inventory.rs` | Boot inventory listing |
| `resident_bytes.rs` | Resident-bytes accounting |
| `sealed_commitment_cost.rs` | Sealed commitment cost bound |
| `writer_envelope.rs:131` | Writer heap envelope (3-writer LRU), idle sweep, seal release |

### core (7 files, pure policy, no I/O)
| File | Behavior |
|---|---|
| `hybrid_policy.rs:96` | Joint readiness gating + RRF oracle + top-k/overfetch (~25 tests, 507 lines) |
| `semantic_stream.rs:165` | Window policy, resident source, lease tally (dim 4, ROW_BYTES 16) |
| `read_view_declaration.rs:75` | Every predicate declares its read domain; repo-metadata authorities |
| `semantic_policy.rs:18` | Semantic policy validation incl. max_top_k |
| `ingest_resource_policy.rs:142` | Ingest resource ceilings |
| `generation_serving_policy.rs:81` | Pin-state oracle table (retained × heads × pin → Served/NotReady/Unknown) |
| `lexical_policy.rs:22` | Select/type/rev/predicate filter surfaces (4 tests, 90 lines) |

### ipc (5 files, real UDS servers + stubs)
| File | Behavior |
|---|---|
| `admission.rs:302` | Queue-full → SERVER_OVERLOADED; conn-cap close-at-accept; dispatch-deadline checkpoint |
| `repo_admission_and_slowloris.rs:273` | Repo admission + slowloris/peer-hangup cancellation (`:422` sleeps 120+200ms) |
| `g0r_runtime_cancellation_probe.rs:351` | Head-of-line freedom, disconnect-cancels-budget, private wire type |
| `socket_access.rs:302` | Socket modes/groups via stat, scripted peer creds, shared-mode bind |
| `wire_historical_cbor.rs:60` | v1 fixture rejected by v2 decoder; truncation/malformed-length rejection (137 lines, no server) |

### embed (unit tests in src)
| File | Behavior |
|---|---|
| `cache.rs:1737` | Dedup/fan-out, revision isolation, poison-eviction, hex keys, file round-trip, damage refusal, same-length rewrite, concurrent writers, eviction/LRU/age/manifest (~27 tests) |
| `openai.rs:898` | Wire shape, unknown-field tolerance, order preservation, concurrency bound, window==round coupling, cancel/deadline/backoff (~15 tests) |
| `telemetry.rs:237` | Telemetry counters (2 tests) |

## 2. Overlap map (merge / parametrize candidates)

1. **IPC triple-harness**: `admission.rs:58` (ControlStub+barrier), `repo_admission_and_slowloris.rs:136`
   (same stub shape), `g0r_runtime_cancellation_probe.rs:270` (probe-wire stub) each rebuild
   UdsServer + holder threads + `HANDSHAKE_BOUND=20s` constants. One shared harness suffices.
2. **`spawn_holder`/`wait_entered` duplicated** in `admission.rs:227`, `repo_admission_and_slowloris.rs:227`,
   `g0r_runtime_cancellation_probe.rs:324` — line-for-line clones.
3. **Collapse-per-path ×4** in `tantivy_smoke.rs:1641,1696,1750,1804`
   (select:file / select:path / type:path / type:repo) — same 3-op build, only filter dim differs;
   parametrize over dim.
4. **Cancelled-vs-expired budget pairs** run twice everywhere:
   `cancellation_inside_search.rs:189` (cancel+deadline in collect), `openai.rs:1633,1664,1689`
   (cancel / cap / backoff) — same checkpoint-string assertion helper, three copies.
5. **`expect_admitted`/`expect_refused` + `knock`/`identity`/`generation_dir` helpers** cloned across
   `sealed_manifest.rs:384`, `unicode_normalization_goldens.rs:1151`, `text_authority_shards.rs` —
   extract to one fixture module.
6. **Golden-table loops** `unicode_normalization_goldens.rs:861` × both routes and
   `text_authority_shards.rs:429` (rebuild-vs-incremental) both rebuild full corpora per test;
   share one sealed-corpus fixture.
7. **Core top-k ceiling asserted in two places**: `hybrid_policy.rs:132` and `semantic_policy.rs:316`
   both pin `TOP_K_OUT_OF_RANGE`; keep one owner.
8. **Embed ledger-vs-disk check** `cache.rs:1721` reimplemented per test instead of one helper call
   (already a helper — just underused in 3 tests).
9. **Socket mode assertions** `socket_access.rs` vs admission conn-cap `admission.rs:373` both prove
   "past-cap connections never queue" via different oracles; fold into one table test.
10. **Wire-shape exact-string tests** `openai.rs:899` + `wire_historical_cbor.rs:60` both snapshot
    encodings; both fine but should live under one `wire/` module, not scattered.

## 3. Slowest top-10 (by mechanism; wall-clock not profiled here)

1. `lexical/tests/cancellation_inside_search.rs:35` — `DOCS=3000` full index build per test. Fix: build once in fixture, share read-only.
2. `lexical/tests/g0l_tantivy_snapshot_probe.rs:46` — `BASE_DOC_COUNT=2000` × multiple full rebuilds + ranking-vs-oracle. Fix: gate behind feature/CI-nightly.
3. `lexical/tests/tantivy_smoke.rs:390` — 3627-line file, ~25 tests each doing tempdir+build+open. Fix: shared seeded adapter.
4. `lexical/tests/sealed_manifest.rs:859` — `segment_files_are_length_proved_at_the_doors` mutates every segment file ×4 with rebuild knocks. Fix: smallest segment subset.
5. `lexical/tests/unicode_normalization_goldens.rs:861` — full corpus rebuild per test × both routes. Fix: one shared fixture.
6. `lexical/tests/writer_envelope.rs:210` — `sleep(800ms)` ×2 in idle test (`idle=400ms`, `writer_envelope.rs:192`). Fix: inject clock / shrink idle to 50ms.
7. `ipc/tests/repo_admission_and_slowloris.rs:433` — `sleep(120ms)+sleep(200ms)` per hangup test. Fix: barrier handshake instead of sleeps.
8. `ipc/tests/g0r_runtime_cancellation_probe.rs:426` — `sleep(200ms)` watch-poll wait. Fix: poll-with-deadline loop (already used at `admission.rs:442`).
9. `embed/src/openai.rs:1051` — `delay=25ms` ×10 batches concurrency probe + `openai.rs:550` real `thread::sleep`. Fix: zero-delay transport, assert overlap via rendezvous barrier.
10. `embed/src/openai.rs:1701` — `sleep(5ms)` + 1ms budget expiry test; plus `cache.rs:2285` reopen/evict ×6 entries with mtime pinning. Fix: keep (cheap), but drop redundant reopen in `:2311` tightened-policy half.

## 4. Weakest top-10 (assertions that prove little)

1. `lexical/tests/tantivy_smoke.rs:419` — sorts ids then compares; masks ranking regressions. Harden: assert exact order.
2. `lexical/tests/sealed_manifest.rs:907` — same-length flip only asserts validator *admits* (documents a hole, proves nothing). Harden: assert scrub quarantines (covered at `:930`, so fold `:859`'s flip arm into it).
3. `core/tests/hybrid_policy.rs:104` — `assert!(matches!(NotReady))` without message/code. Harden: assert code string like `generation_serving_policy.rs:43` does.
4. `core/tests/lexical_policy.rs:23` — accept-path tests assert `is_ok` only, no state. Harden: assert admitted query shape round-trips.
5. `ipc/tests/wire_historical_cbor.rs:64` — `matches!(Err(Decode))` without asserting *which* code retired. Harden: assert BAD_REQUEST named in message.
6. `embed/src/openai.rs:909` — `matches!(body, Ok(exact))` via boolean assert loses diff. Harden: `assert_eq!` on string.
7. `embed/src/cache.rs:1714` — `assert!(insert.is_none())` inside helper; duplicate-name collision silently passes if helper skipped. Harden: return count, assert at call site.
8. `lexical/tests/writer_envelope.rs:141` — asserts `max_writers()==3` (policy arithmetic, not behavior). Harden: drop; envelope behavior below already covers.
9. `core/tests/read_view_declaration.rs:98` — alias==canonical domain equality is tautological if both call same fn. Harden: pin expected domain per alias.
10. `ipc/tests/socket_access.rs:122` — scripted peer source can only return what test set; kernel contract explicitly disclaimed (`socket_access.rs:9`). Harden: mark `#[ignore]`-able integration or assert kernel error path at least once on Linux.

## 5. Cheesiest top-10 (copy-paste / magic / sleep)

1. `ipc/tests/admission.rs:44` + `repo_admission_and_slowloris.rs:43` + `g0r_runtime_cancellation_probe.rs:45` — `HANDSHAKE_BOUND=20s` triplicated. Fix: shared const.
2. `lexical/tests/tantivy_smoke.rs:261` — `upsert/upsert_with_metadata/upsert_with_source_repo/upsert_with_texts/upsert_symbol` 5 near-clone builders. Fix: one builder with options struct.
3. `lexical/tests/cancellation_inside_search.rs:162` — `checked_sub(1s)` clock-underflow dance for expired budget. Fix: `RequestBudgetV1::for_duration(ZERO)` helper.
4. `embed/src/cache.rs:2295` — `UNIX_EPOCH + 1_700_000_000 + index` mtime-pinning triplicated (`:2295,:2345,:2763`). Fix: `stamp(index)` helper.
5. `embed/src/openai.rs:992` — `thread::sleep(delay)` inside fake transport; timing-as-correctness. Fix: barrier rendezvous.
6. `ipc/tests/admission.rs:88` — `thread::sleep(OVERSLEEP)` in dispatcher stub. Fix: block on test barrier instead.
7. `lexical/tests/sealed_manifest.rs:166` — `repo_metadata_bundle_payload` hand-built CBOR per test. Fix: shared bundle factory.
8. `core/tests/semantic_stream.rs:46` — 30-line `record()` fixture with 20 hardcoded strings per test. Fix: minimal `record(id)` default + overrides.
9. `core/tests/hybrid_policy.rs:51` — `slow_rrf_oracle` hand-rolled O(n²) oracle in test. Fix: keep (genuinely independent) but shrink lanes; flagged only for cost, not correctness.
10. `lexical/tests/unicode_normalization_goldens.rs:1185` — `as_format_one` row-mutator for format-downgrade tests. Fix: keep; cheesiest-looking but load-bearing — do not merge with shard tests.

## 6. Per-finding detail (file:line + symptom + why-bad + severity + fix)

- `crates/quanta-index-lexical/tests/writer_envelope.rs:210` — `sleep(800ms)` twice gates idle-sweep proof. Wall-clock tax ~1.6s+ per run, flakes on loaded hosts. **M** — inject monotonic clock into writer cache or drop idle to 50ms with poll loop.
- `crates/quanta-index-lexical/tests/cancellation_inside_search.rs:35` — 3000-doc corpus rebuilt per test. Slowest lexical file for least unique coverage. **H** — seed once (`OnceLock` tempdir) and share searcher.
- `crates/quanta-index-lexical/tests/tantivy_smoke.rs:1641` — 4 collapse tests differ by one filter enum. 4× build cost, 4× maintenance. **M** — `#[test_case]`/table over `(filter, expected)`.
- `crates/quanta-index-ipc/tests/repo_admission_and_slowloris.rs:433` — sleep-bounded hangup proof (`120ms`+`200ms`). Slow + racy. **H** — replace with barrier + `recv_timeout(HANDSHAKE_BOUND)` join, as `admission.rs:442` already does.
- `crates/quanta-index-ipc/tests/admission.rs:50` — `OVERSLEEP=150ms` vs budget `60ms` sleep race. Passes by margin, not handshake. **M** — park on barrier, release after checkpoint assertion.
- `crates/quanta-index-embed/src/openai.rs:992` — fake-transport sleep as overlap generator. **M** — rendezvous barrier; assert peak via counter without sleeping.
- `crates/quanta-index-core/tests/hybrid_policy.rs:104` — bare `matches!` on `NotReady`. Misses wrong-code regressions. **L** — assert `typed_code_or_debug` equals expected code.
- `crates/quanta-index-lexical/tests/sealed_manifest.rs:907` — flip-admit arm proves the hole exists, not that it closes. **L** — merge into `:930` scrub test; delete standalone arm.
- `crates/quanta-index-embed/src/openai.rs:909` — boolean `matches!` on exact wire string. Failure prints nothing. **L** — `assert_eq!(body.unwrap(), EXPECTED)`.
- `crates/quanta-index-ipc/tests/socket_access.rs:9` — kernel contract explicitly unproven by design. Honest comment, but coverage gap. **L** — document as accepted gap; add Linux-only kernel assertion when CI allows second uid/netns.

## 7. Recommended cuts/merges (concrete)

1. Merge the three IPC harnesses into `ipc/tests/harness.rs` (stub + spawn_holder + wait_entered + consts). Deletes ~150 duplicated lines.
2. Table-drive the 4 collapse tests + the 4 `lexical_policy.rs` surface tests. Deletes ~120 lines.
3. Share one sealed corpus fixture across unicode/text-authority/sealed-manifest happy paths. Biggest wall-clock win.
4. Move G0-L probe (`g0l_*.rs`, 606 lines, 2000 docs) to nightly/ignored gate; it measures Tantivy, not the adapter.
5. Replace all `thread::sleep`-as-synchronization in ipc + writer_envelope + openai probes with barriers/poll loops (6 sites listed above).
