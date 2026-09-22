# Slow / Inefficient Tests Audit (bugbash, Sep 22)

Scope: wall-clock hogs in `crates/*/tests` + in-`src` `#[test]` bodies —
sleep/spawn/network/fs-heavy setup, redundant per-case fixtures, missing sharing.
Every finding below was read in the real file body (search hits alone were not used).

## F1 — 30 s leaked sleeper thread per run of the supervisor hard-deadline test
- File: `crates/quanta-index-searchd-runtime/tests/runtime_supervisor_owner_v1.rs:261`
- Symptom: `hard_deadline_escalation_is_not_graceful` spawns `child-ignoring`, which does
  `std::thread::sleep(Duration::from_secs(30))` and is abandoned at the hard deadline
  (join abandoned by design; assertion only checks `elapsed < 5s`).
- Why it is bad (slow): the test itself finishes in ~400 ms but leaves a live thread
  sleeping 30 s. Under `cargo test` / nextest that thread outlives the test, holds a
  thread slot + stack for 30 s per harness run, and serially delays process exit /
  thread-pool teardown. Repeat runs (this file has ~10 tests sharing the binary) stack up.
- Fix: replace the 30 s sleep with a wait on the shutdown flag with a long cap, e.g.
  park on `context.shutdown()` with `Condvar`/poll capped at the hard deadline + margin
  (e.g. 2 s), or drop the handle via `std::thread::park()`. The escalation assertion
  (`elapsed < 5s`, exit 70) is unchanged.

## F2 — 3 s unconditional sleep in the two-process lease holder child
- File: `crates/quanta-index-searchd-runtime/tests/runtime_supervisor_owner_v1.rs:486`
- Symptom: `hold_the_lease` child does `std::thread::sleep(Duration::from_secs(3))`
  while holding the state-root lock; parent in `run_two_process_lease_parent` (:507–570)
  spawns **two real child processes of the test binary** (`Command::new(current_exe)`)
  and poll-waits with 20 ms sleeps (:541).
- Why it is bad (slow): fixed 3 s floor on one test + 2× process spawn + real `fs` lock
  handshake. This single test costs ≥3 s wall clock, unparallelizable by construction.
- Fix: replace the fixed 3 s hold with an event: parent signals the holder over a pipe /
  second report file once the `second` child outcome is recorded, then holder exits.
  Keep a 10 s watchdog timeout as failure, not as the happy path.

## F3 — `cli_smoke.rs`: one real binary spawn + one UDS mock server per test
- File: `crates/quanta-index-searchctl/tests/cli_smoke.rs:142` (also :182, :226, :271,
  :311, :351, :390, :424, :439, :476, :514, :718, :852, :1092, :1230 — 15+ `Command::new`)
- Symptom: every `*_impl` helper calls `start_server` (binds a fresh UDS socket) then
  `Command::new(env!("CARGO_BIN_EXE_quanta-index-searchctl"))… .output()?`. Each test
  pays: tempdir + socket bind + fork/exec of the real CLI + mock-server thread join.
  Readiness is a fixed poll loop: `for _attempt in 0..20 { … sleep(10ms) }` (:1071–1076),
  duplicated at :1075 and :1273.
- Why it is bad (slow/duplicate): process spawn dominates (10–100 ms each × ~20 tests);
  per-test server setup is near-identical (`unique_socket_path` + `ScenarioDispatcher`).
  No test needs a private OS process per se — most assert JSON stdout shape.
- Fix: (a) hoist the mock UDS server into a shared fixture (one server per scenario,
  or one multiscenario dispatcher keyed by repo-id); (b) replace half the
  process-spawn cases with in-process calls into `quanta-index-searchctl` lib fns
  (the dispatchers already live in-process); keep 2–3 true end-to-end spawns as the
  spawn smoke. (c) Replace the 20×10 ms poll with an eventfd/notify or
  `wait_until(path.exists(), 2s)` helper shared with F5.

## F4 — `sdk_frontdoor.rs` (4563 lines): full 3-socket runtime boot per test
- File: `crates/quanta-index-searchd-runtime/tests/sdk_frontdoor.rs:170`
  (`start_sdk_frontdoor_runtime`), called at :2750, :3090, :3283, :3367, :3655, :3873,
  :3950, :4048, …; boot body at :133–167 and :178–206
- Symptom: each test builds a full runtime (`build_runtime` + `drive` on a new thread +
  query/control/ingest UDS servers + `QuantaIndex::connect`), guarded by a 5 s
  `SOCKET_TIMEOUT` poll (:68, :147, :192), then tears it all down (`stop_runtime`).
  File has ~91 `fn`/`#[test]` items; a large fraction each pay a full boot.
- Why it is bad (slow/duplicate): runtime boot (socket bind ×3, thread spawn, connect
  handshake) is the most expensive fixture in the repo and it is rebuilt ~10+ times
  with only `thread_name` differing. Failure mode is also slow: a hung boot costs 5 s.
- Fix: share one booted runtime per test binary via `OnceLock`/`LazyLock` + per-test
  repo/revision isolation (distinct `RepoId`s already exist, e.g. `repo()` at :70).
  Keep the with-ingest variant (:178) as the single shared instance and derive clients
  per test. Expected saving: N boots → 1 boot.

## F5 — sleep-poll readiness loops instead of events (10 ms everywhere)
- Files:
  - `crates/quanta-index-searchd-runtime/tests/common/searchd_binary_process.rs:91`
    (`thread::sleep(10ms)` inside `wait_for_sockets`, cap `SOCKET_TIMEOUT` = 30 s at :15)
  - `crates/quanta-index-searchd-runtime/tests/sdk_frontdoor.rs:249`, `:1320`, `:1349`,
    `:1357`, `:1385` (`thread::sleep(10ms)` retry polls)
  - `crates/quanta-index-searchd-runtime/tests/e2e_generation_activation_concurrency.rs:166`
    (`sleep(5ms)`), `repo_map_end_to_end.rs:143`, `explain.rs:139`, `e2e_crash_matrix.rs:247`
    (all `sleep(10ms)`)
- Symptom: readiness / cross-thread rendez-vous done by fixed-sleep polling.
- Why it is bad (slow/weak): each poll adds mean ~5 ms latency per wait even on a fast
  machine, and the 30 s cap in `wait_for_sockets` turns any real-binary startup
  regression into a 30 s hang per test (multiplied by every test using
  `SearchdBinaryProcess::start`).
- Fix: poll on `UnixStream::connect` success with 1 ms base + exponential backoff capped
  at ~50 ms, and shrink the hard cap for the in-process runtimes (5 s → 2 s already
  exists in sdk_frontdoor; apply to `searchd_binary_process` too via env override).
  For in-process cases prefer `Condvar`/channel notification from the drive thread.

## F6 — fixed 120 ms + 200 ms sleeps as test synchronization (slowloris/admission)
- Files:
  - `crates/quanta-index-ipc/tests/repo_admission_and_slowloris.rs:433` (`sleep(120ms)`)
    and `:435` (`sleep(200ms)`) — comment admits "the sleeps only bound how long it had"
  - `crates/quanta-index-ipc/tests/admission.rs:51` (`OVERSLEEP = 150ms`, slept at :88)
  - `crates/quanta-index-ipc/tests/g0r_runtime_cancellation_probe.rs:426`
    (`sleep(200ms)`)
  - `crates/quanta-index-ipc/src/server.rs:2382` (`sleep(60ms)` in
    `DelayedTestResponseEnvelope`), `:1784` (`sleep(30ms)`), `:2574` (`sleep(10ms)`)
- Symptom: cancellation / overload / deadline proofs sleep fixed durations then assert
  counters.
- Why it is bad (slow/weak): fixed sleeps make the suite slow on fast machines (320 ms
  floor in the pipelined-hangup test alone) and flaky on slow/loaded ones — the classic
  sleep-as-synchronization antipattern. `server.rs` sleeps are inside `#[cfg(test)]`
  doubles, so they tax every IPC test run.
- Fix: replace sleeps with barrier/latch handshake (`wait_entered` already exists in the
  slowloris harness — extend it with `wait_polled`/`wait_cancelled` hooks); assert with
  `recv_timeout(HANDSHAKE_BOUND)` instead of sleep-then-count. Shrink `OVERSLEEP` to
  `SHORT_DISPATCH_BUDGET + 10ms` (60 → ~70 ms instead of 150 ms).

## F7 — scrub test does boot → ingest → seal → restart → poll-metrics per test
- File: `crates/quanta-index-searchd-runtime/tests/e2e_integrity_scrub.rs:161`
  (`std::thread::sleep(SCRUB_INTERVAL)` in `wait_for_scrape`), test body at :172–240+
- Symptom: `a_byte_defect_…_quarantined_by_the_scrub` boots an `E2eRuntime`, ingests,
  seals, activates, waits for a background scrub via metric polling, `reopen()`s,
  flips a byte, restarts, and polls again. `end_to_end.rs` (3492 lines) and
  `e2e_full_corpus.rs` (2459 lines) repeat the same boot+ingest+seal prefix per test.
- Why it is bad (slow): full daemon lifecycle ×2 plus background-scrub wait inside one
  test; the metric-poll loop wakes at `SCRUB_INTERVAL` granularity regardless of actual
  scrub speed.
- Fix: (a) notify scrub completion via the existing receipt file
  (`SEMANTIC_SCRUB_RECEIPT`) with `notify`-crate watch or 1 ms stat poll instead of
  metrics-scrape polling; (b) share one sealed generation across the scrub tests
  (seal once, clone state-root per test with hardlinks); (c) split the "defect
  survives boot" half (no scrub needed) from the "scrub quarantines" half.

## F8 — 15× per-test `tempfile::tempdir` in embed cache tests; 7× in core generation tests
- Files:
  - `crates/quanta-index-embed/src/cache.rs:1924`, `:1963`, `:2037`, `:2069`, `:2158`,
    `:2207`, `:2245`, `:2268`, `:2286`, `:2329`, `:2403`, `:2563`, `:2621`, `:2698`,
    `:2745` (each `#[test]` creates its own tempdir + cache root)
  - `crates/quanta-index-core/src/domains/generation.rs:1110`, `:1133`, `:1157`,
    `:1175`, `:1219`, `:1320`, `:1374` (same pattern)
- Symptom: every tiny unit test (round-trip, damaged-entry, ledger match) pays
  tempdir create + namespace dir create + real `fs` writes + Drop cleanup.
- Why it is bad (slow/duplicate): fs-heavy setup dominates sub-millisecond assertions;
  on macOS tempdir creation is notably slow. Nothing here needs distinct filesystems
  except the damage/persistence cases.
- Fix: pure round-trip/validation tests → in-memory `FileCache` backend or a shared
  `TempDir` via `OnceLock` with per-test subdirs (`dir.join(test_name)`). Keep real
  isolated tempdirs only for the damage-removal and cross-instance persistence tests.

## F9 — real-process `searchd` spawn per e2e test via shared helper
- File: `crates/quanta-index-searchd-runtime/tests/common/searchd_binary_process.rs:131`
  (`Command::new(env!("CARGO_BIN_EXE_quanta-index-searchd"))`), spawned by
  `SearchdBinaryProcess::start` (:23–37) used across `end_to_end`, `e2e_crash_matrix`
  (652 lines), `composite_generation_authority_restart` (742), `e2e_umask_hardening`
  (`Command::new("sh")` at `:33` + sleeps at :91, :192)
- Symptom: each e2e test forks the real daemon binary and waits for 3 sockets.
- Why it is bad (slow): binary spawn + triple-socket handshake per test; the crash-matrix
  and restart tests additionally kill/restart it mid-test. This is the wall-clock
  bottleneck of the whole `test-daemon` lane.
- Fix: default e2e tests to the in-process `E2eRuntime`/`build_runtime` harness (as
  `sdk_frontdoor` already does); reserve `SearchdBinaryProcess` for the 2–3 tests that
  genuinely need process semantics (crash, umask, lease exclusion). Gate the
  real-process subset behind a feature/ignore flag so `just test-fast` skips it.

## F10 — network-adjacent retry sleeps in embed provider (+ test-only delay fakes)
- File: `crates/quanta-index-embed/src/openai.rs:550` (`std::thread::sleep(slice)` in
  `sleep_within_budget`), `:992` (`std::thread::sleep(self.delay)` in the fake), `:1701`
  (`sleep(5ms)` in a test), `:50` (`BUDGET_POLL_INTERVAL = 25ms`)
- Symptom: production retry path sleeps on-thread; tests use real sleeps to simulate
  latency/backoff.
- Why it is bad (slow/weak): on-thread sleep blocks a runtime worker per in-flight
  embedding request; tests pay real time for fake latency and cannot run in parallel
  against a shared fake without interleaving.
- Fix: (prod) replace `thread::sleep` with async timer / cancel-aware wait on the
  request budget; (tests) replace `sleep(delay)` fakes with a manual clock /
  `tokio::pause()` or a latch the test controls. Not the biggest saver, but it removes
  the only network-shaped waits from unit tests.

## Suggested order (biggest wall-clock win first)
1. F4 (share one sdk_frontdoor runtime) + F9 (in-process by default) — removes most
   daemon boots/spawns.
2. F2 + F1 (event instead of 3 s / 30 s sleeps) — removes fixed multi-second floors.
3. F6 + F5 (latches instead of fixed sleeps/polls) — removes ~0.5–1 s of sleeps and
   most flakiness.
4. F3 (shared CLI mock server; fewer spawns), F7 (share sealed generation), F8
   (share tempdirs) — removes per-test fs/process duplication.
5. F10 (async retry wait) — code-health follow-up.
