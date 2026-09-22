# CHEESY test-code audit (bugbash, Sep 22)

Scope: copy-paste, magic values, flaky patterns, global state, ordering
dependence, huge inline fixtures, dead helpers. Every finding below was
verified by opening the cited file body (grep hits alone were not used).

## F1 — Fixed `thread::sleep` to "give the watch a poll" (slow + flaky)

- File: `crates/quanta-index-ipc/tests/repo_admission_and_slowloris.rs:433,435`
- Symptom: `thread::sleep(Duration::from_millis(120))`, then
  `thread::sleep(Duration::from_millis(200))` between pipelining a frame,
  hanging up, and asserting the cancellation count. The comment admits
  "the sleeps only bound how long it had."
- Why bad: wall-clock sleeps make the test slow by construction and flaky
  under load (too short = false failure, generous = slow suite).
- Fix: wait on the watch/notify primitive the server already exposes
  (cf. `server.wait_entered()` used two lines above), or poll the
  `cancelled_observed` counter with a deadline instead of two fixed sleeps.

## F2 — Busy-poll loops with 5 ms sleeps (cheesy synchronization)

- Files:
  `crates/quanta-index-ipc/tests/repo_admission_and_slowloris.rs:442-443`,
  `crates/quanta-index-ipc/tests/admission.rs:413-419`
- Symptom: `while <counter> == 0 && Instant::now() < deadline {
  thread::sleep(Duration::from_millis(5)) }` and a `loop { match send(..)
  { Err(_) if Instant::now() < deadline => thread::sleep(5ms), .. } }`
  retry, both bounded by a 20 s `HANDSHAKE_BOUND`.
- Why bad: spin-sleep polling is slow on failure (waits out the whole
  bound) and imprecise on success; duplicated in two files.
- Fix: share one `wait_until(deadline, predicate)` helper in the IPC test
  support module (backoff + deadline, returns the last snapshot on
  timeout for diagnostics); or block on a condvar/oneshot the server
  signals when the counter advances.

## F3 — `sleep(idle * 2)` couples test to wall clock (flaky)

- File: `crates/quanta-index-lexical/tests/writer_envelope.rs:210,230`
- Symptom: `std::thread::sleep(idle.saturating_mul(2))` before calling
  `release_idle_writers()`, twice in one test.
- Why bad: correctness of the assertion depends on the scheduler sleeping
  "long enough"; loaded CI either flakes or forces `idle` to be inflated,
  slowing every run.
- Fix: inject a fake clock into the writer cache, or replace the sleep
  with a deadline poll on `writer_cache_stats().open_writers == 0` after
  `release_idle_writers()`.

## F4 — Wall-clock elapsed assertion with magic window (weak + flaky)

- File: `crates/quanta-index-catalog/tests/idempotency.rs:285-293`
- Symptom: `let started = Instant::now(); ... expect_err(...); let waited
  = started.elapsed(); if waited < 60ms || waited > 5s { fail }`.
- Why bad: asserts on scheduler timing rather than behavior; the
  60 ms..5 s window is a magic value that is both too tight (slow CI can
  exceed it on the `expect_err` path setup) and too loose to prove the
  60 ms busy budget is honored.
- Fix: assert the typed `CATALOG_BUSY` code (already done) plus that the
  configured budget value reached the lock-wait call (spy/fake clock);
  drop the elapsed window or keep only a generous upper bound as a
  hang-guard.

## F5 — Copy-pasted `SqliteCatalog::open` with magic timeouts (duplicate)

- Files: `crates/quanta-index-catalog/tests/operation_journal.rs:141`
  (repeated ~20x with `Duration::from_millis(200)`, plus 60/500 variants
  at :410,:759-:817),
  `crates/quanta-index-catalog/tests/idempotency.rs:99` (repeated ~10x
  with `Duration::from_millis(100)` plus a 60 ms variant at :278).
- Symptom: every test inlines `SqliteCatalog::open(temp.path(),
  Duration::from_millis(N))` with unexplained 60/100/200/500 ms values.
- Why bad: duplicate setup hides intent (what does 200 ms mean?) and a
  timeout-policy change requires editing dozens of call sites.
- Fix: add `fn open_test_catalog(dir) -> ...` helper with a named const
  (`TEST_BUSY_BUDGET`) and one dedicated test for the 60 ms busy-budget
  edge; use the helper everywhere else.

## F6 — Ten `boot_with_*` full-runtime fixtures in one file (slow + duplicate)

- File: `crates/quanta-index-searchd-runtime/tests/e2e_filter_execution.rs:39`
  (`boot_with_history`), plus `boot_with_lexical:139`,
  `boot_with_multi_repo:191`, `..._and_commit_recency:265`,
  `..._and_repo_meta:296`, `..._and_repo_topic:334`,
  `..._and_file_ownership:399`, `..._and_file_contributor:443`,
  `boot_with_rev_at_time_generations:576`, `boot_with_metadata:2462`.
- Symptom: each filter family gets its own full `E2eRuntime::boot()` +
  ingest + seal + activate helper; the seeded `E2eHistoryFixtureSpec`
  block (:43-:57, magic sha/timestamps) is re-typed per variant.
- Why bad: slow (a whole runtime boot per test) and copy-paste heavy; a
  fixture change must be mirrored across N helpers.
- Fix: single `boot_with_fixture(spec: &FilterFixtureSpec)` builder with
  small per-test spec structs; share one sealed generation across tests
  in the file where isolation allows.

## F7 — Identical `make_query_with_filters` in two test files (copy-paste)

- Files: `crates/quanta-index-lexical/tests/tantivy_smoke.rs:378-387`,
  `crates/quanta-index-lexical/tests/planner_authority.rs:130-139`
  (line-for-line identical `LqQuery { lq_version, expr, filters,
  options: defaults, directives: vec![], source_span: eof(0) }`).
- Symptom: same 10-line constructor duplicated; `planner_authority.rs`
  additionally duplicates it as `make_query_with_options:141-150`.
- Why bad: classic copy-paste helper; the two copies will drift (one
  already grew a sibling variant the other lacks).
- Fix: move `make_query_with_filters` / `make_query_with_options` into a
  shared lexical test-support module and `use` it from both files.

## F8 — Copy-pasted `unique_socket_paths` pid+nanos+sequence (duplicate)

- Files: `crates/quanta-index-searchd-runtime/tests/repo_map_end_to_end.rs:76-88`,
  `crates/quanta-index-searchd-runtime/tests/explain.rs:86-99`,
  `crates/quanta-index-searchctl/tests/cli_smoke.rs:1278-1286`,
  `crates/quanta-index-searchd-runtime/tests/sdk_frontdoor.rs:112-114`
  (same pid + `SystemTime::now` nanos + `NEXT_SOCKET_ID` recipe).
- Symptom: 4+ copies of the same socket-uniqueness recipe with slightly
  different prefixes; each file also keeps its own `static
  NEXT_SOCKET_ID: AtomicU64`.
- Why bad: duplicated global-state machinery; a collision/robustness fix
  (e.g. `tempfile`-backed sockets) must land in every copy.
- Fix: one shared helper (e.g. in
  `crates/quanta-index-searchd-runtime/tests/common/searchd_binary_process.rs`,
  already the common harness) returning a guard that owns and unlinks
  the socket paths.

## F9 — `assert!(false)` + `spin_loop()` fallback (hang on failure + dead code)

- File: `crates/quanta-index-lq-bridge/tests/property_translator_total.rs:20-31`
- Symptom: on `SourcegraphVersionTag::supported()` error the helper does
  `assert!(false, ...)` then `if let Ok(t) = new("sg-0.0.0") { return t }
  loop { core::hint::spin_loop(); }`.
- Why bad: if the assert is ever disabled/ignored the test hangs forever
  spinning a core instead of failing; the fallback contradicts the assert
  above it (dead logic either way).
- Fix: `supported().expect("supported pin must parse")` (or return the
  `sg-0.0.0` fallback deliberately without the assert/spin); delete the
  `spin_loop` arm. Same `spin_loop` idiom appears in
  `golden_bridge.rs` — fix both.

## F10 — Shared/fixed temp paths + pid-only uniqueness (global state, ordering)

- Files: `crates/quanta-index-searchd-runtime/tests/e2e_process_envelope.rs:352-355`
  (fixed `temp_dir().join("quanta-index-envelope-probe")`),
  `crates/quanta-index-sdk/tests/sdk_binding_owner_v1.rs:157-161`
  (`temp_dir().join(format!("sdk-binding-owner-{tag}-{}",
  process::id()))` + `.expect("create temp dir")`).
- Symptom: one test uses a fixed shared path in the system temp dir; the
  other uniquifies only by pid, so parallel tests in the same process
  (threads) or a stale dir from a crashed run collide; `.expect()` in the
  helper panics instead of returning `TestResult`.
- Why bad: ordering dependence + cross-test interference; stale state
  from run N fails run N+1.
- Fix: `tempfile::tempdir()` (auto-unique, auto-cleaned) everywhere;
  make helpers return `Result` instead of `.expect()`.

## F11 — Huge single-file e2e fixtures (slow compile + weak failure signal)

- Files: `crates/quanta-index-searchd-runtime/tests/sdk_frontdoor.rs`
  (~4563 lines), `crates/quanta-index-lexical/tests/tantivy_smoke.rs`
  (~3627), `crates/quanta-index-searchd-runtime/tests/end_to_end.rs`
  (~3492), `crates/quanta-index-searchd-runtime/tests/e2e_filter_execution.rs`
  (~2582), `crates/quanta-index-searchd-runtime/tests/e2e_dual_syntax_lowering_parity.rs`
  (~2723).
- Symptom: thousands of lines of inline scenarios, each booting full
  runtimes, in one compilation unit per file.
- Why bad: slow to compile, slow to run, one failure buries the signal;
  shared inline data cannot be reused by other suites.
- Fix: split each into one file per scenario area with a shared
  `common/` boot helper, and move large static corpora to data files
  (or `fixtures/`) loaded once.

## F12 — Wall-clock `SystemTime` seed + `max_bytes: 1` step loop (slow/flaky)

- File: `crates/quanta-index-lexical/tests/sealed_manifest.rs:1151-1153`
  (`SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs()` as the
  "before" bound), `:1162` (`loop { scrub(max_bytes: 1) ... }`).
- Symptom: the test's time bound comes from the real clock while the
  scrub is driven one byte per iteration in an unbounded `loop`.
- Why bad: clock-dependent assertion plus a very slow stepping loop
  (one-byte steps over real data); a second-boundary tick or slow disk
  flakes the bound check.
- Fix: capture the bound from the adapter/catalog clock (or assert
  relative ordering `last_completed >= before - tolerance`), and step
  with a realistic budget instead of `max_bytes: 1` except in one
  dedicated resumability test.
