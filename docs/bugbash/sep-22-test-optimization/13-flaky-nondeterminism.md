# 13 — Flaky / Nondeterminism Audit

Scope: wall-clock sleeps, timeout races, port/socket reuse, global env
manipulation, unseeded RNG, parallel-unsafe shared dirs/statics,
ordering-dependent tests, time-based assertions. Every cited file body was
opened and read; search output alone was not used as evidence.

## F1 — Real `thread::sleep` backoff in the embed path
- File: `crates/quanta-index-embed/src/openai.rs:542-553` (`sleep_within_budget`, `std::thread::sleep(slice)` sliced by `BUDGET_POLL_INTERVAL` at `:50`).
- Symptom: retry backoff sleeps on the wall clock; tests exercise real delays.
- Why bad: suite time scales with retry count; loaded CI stretches every slice, and cancellation-during-backoff assertions depend on winning a wall-clock race.
- Severity: M.
- Fix: inject a `sleep(Duration)` callable (default `std::thread::sleep`) or a fake clock; unit tests assert the *sequence of requested delays*, not elapsed time. Repro: run the retry test under `stress -c N` and watch durations spread.

## F2 — `recv_timeout` poll loop for attempt completion
- File: `crates/quanta-index-embed/src/openai.rs:436-447` (`outcome.recv_timeout(BUDGET_POLL_INTERVAL)`); test-side `recv_timeout(Duration::from_secs(10))` at `:1639` and `:1669`.
- Symptom: cancellation/timeout detection latency is quantized to 25 ms; tests park up to 10 s waiting for the peer thread.
- Why bad: race window = poll interval; a 10 s parked test thread that never gets released hangs the suite instead of failing fast.
- Severity: M.
- Fix: wait on a waker/`Condvar`/cancellation token instead of polling; in tests bound the wait (`recv_timeout(5s)`) and `release.wait()` on failure paths. Repro: kill the releaser thread pre-`send` and observe the 10 s stall.

## F3 — `ConcurrencyProbeTransport` uses `sleep` to force overlap
- File: `crates/quanta-index-embed/src/openai.rs:992` (`std::thread::sleep(self.delay)`, `delay: Duration::from_millis(25)` at `:1051`).
- Symptom: peak-concurrency assertions assume N workers overlap inside a 25 ms window.
- Why bad: on a loaded single-core CI runner workers may serialize and the peak assertion flakes.
- Severity: M.
- Fix: rendezvous with a `Barrier(worker_count)` (entry gate) instead of sleep so overlap is structural, not timed.

## F4 — `thread::sleep(5ms)` to expire a 1 ms budget
- File: `crates/quanta-index-embed/src/openai.rs:1700-1701` (`for_duration(1ms)` then `sleep(5ms)`).
- Symptom: depends on the OS timer firing within 4 ms of slack.
- Why bad: coarse timers (Windows CI, virtualized runners) can report the budget as still alive at the checkpoint, flipping the assertion.
- Severity: H.
- Fix: build an already-expired budget directly (`RequestBudgetV1::until(Instant::now() - Duration::from_secs(1))` or a fake-clock budget); delete the sleep. Same pattern fix applies to F11.

## F5 — Backoff jitter seeded from the wall clock (unseeded RNG)
- File: `crates/quanta-index-embed/src/openai/retry.rs:61-91` (`SystemTime::now()` XOR `SEED_COUNTER` at `:73-81`, thread-local xorshift).
- Symptom: `backoff_delay(attempt)` returns a different value every run; no seed hook.
- Why bad: timing-only today (docs at `:7` confirm output is unaffected), but retry-timing tests cannot reproduce a failing delay sequence.
- Severity: L.
- Fix: `backoff_delay(attempt, &mut impl Rng)` or a `#[cfg(test)] set_jitter_seed(u64)` hook; default path keeps the current seeding. Repro: call `backoff_delay(3)` twice across processes, observe divergence.

## F6 — Async budget watcher sleeps on the wall clock
- File: `crates/quanta-index-semantic/src/budget.rs:262-269` (`tokio::time::sleep(budget.remaining().min(SEMANTIC_BUDGET_POLL_INTERVAL))` with `POLL_INTERVAL = 10ms` at `:63`).
- Symptom: cancellation detection lags up to 10 ms; `race_with_budget` (`:280-296`, `biased` select) outcome depends on scheduler timing.
- Why bad: no `tokio::time::pause()`-compatible injection; tests cannot deterministically advance to the deadline.
- Severity: M.
- Fix: race `work` against a cancellation `Notify`/token future (no polling for cancel), keeping the timed sleep only for the deadline; tests use `tokio::time::pause()` + `advance()`.

## F7 — Socket-readiness via `exists()` + `sleep(10ms)` × 20
- File: `crates/quanta-index-searchctl/tests/cli_smoke.rs:1071-1076` and duplicate `:1269-1274` (`socket_path.exists()` poll, `std::thread::sleep(10ms)`).
- Symptom: 200 ms wall-clock budget for the server thread to bind; slow bind = silent proceed to a failing connect.
- Why bad: classic timeout race — failure mode is a confusing downstream connect error, not "server did not start"; the `Duration::from_millis(5)` accept poll at `:1066`/`:1264` adds a second quantum.
- Severity: H.
- Fix: signal readiness over a channel from the server thread (or block on `connect` retry with an `Instant` deadline ≥ 5 s and return the last error). Repro: add `sleep(300ms)` before `UdsServer::bind` and watch both callers fail downstream.

## F8 — Clock-derived socket-file uniqueness
- File: `crates/quanta-index-searchctl/tests/cli_smoke.rs:1278-1286` (`SystemTime::now().as_nanos()` + pid + `NEXT_SOCKET_ID`).
- Symptom: uniqueness leans on nanosecond resolution; same-nanosecond calls rely solely on the counter.
- Why bad: stale `/tmp/qi-searchctl-test-*.sock` files from a crashed run can collide across runs (pid reuse); counter is per-process only.
- Severity: L.
- Fix: create an isolated `tempfile::tempdir()` per test (as `private_socket_dir()` at `:705-710` already does) and bind inside it; remove the manual name scheme.

## F9 — Detached server threads + fixed sibling socket names
- File: `crates/quanta-index-searchctl/tests/cli_smoke.rs:1056-1059` and `:1254-1257` (`UdsServer::bind`, `let _server_thread = spawn(...)` never joined; fixed `query.sock`/`control.sock` names at `:1089-1090`, `:715-716`).
- Symptom: server lifetime is tied to `ShutdownHandle`, not the join handle; parallel tests sharing a dir would fight over the same path.
- Why bad: currently mitigated by per-test tempdirs, but any future dir reuse (or `TMPDIR` collision) turns into `AddrInUse`/stale-socket flakes; leaked threads also serialize port/CI resources.
- Severity: M.
- Fix: keep per-test tempdir (never share), `unlink` stale socket before bind or use `SO_REUSE`-equivalent unlink-if-exists, and join the server thread after `shutdown.trigger()`.

## F10 — E2E readiness timeouts measured in seconds
- File: `crates/quanta-index-searchd-harness/src/harness.rs:110-114` (`READINESS_TIMEOUT = 15s`, `SOCKET_APPEAR_TIMEOUT = 5s`, polls 1 ms / 5 ms); waits at `:2154`, `:2405`, `:2642`, etc.
- Symptom: every e2e pays wall-clock polls; under load the 5–15 s ceilings become pass/fail boundaries.
- Why bad: ordering-dependent suite time; a slow daemon start near the ceiling flakes rather than degrading gracefully.
- Severity: M.
- Fix: readiness channel/event from the driver thread instead of polling; keep the timeout as a fail-fast diagnostic that prints the last readiness state.

## F11 — "Just expired" budgets assume sub-5 ms clock granularity
- File: `crates/quanta-index-core/src/request_budget.rs:188-193` (`Instant::now().checked_sub(5ms)`); same idiom in `crates/quanta-index-semantic/tests/score_candidate.rs:246-250` and `crates/quanta-index-semantic/src/budget.rs:449-450`.
- Symptom: asserts the budget is already expired.
- Why bad: on coarse clocks the deadline can still be ~15 ms in the future and the "refused" test takes the success path.
- Severity: M.
- Fix: subtract a full second (or construct `RequestBudgetV1::until(past)` from a fake clock); never depend on millisecond granularity. Deterministic repro: mock `Instant` resolution to 16 ms and watch the refusal disappear.

## F12 — Process-global env vars as crash-point switches
- File: `crates/quanta-index-repomap/src/object_store.rs:69-73` (`QUANTA_INDEX_REPOMAP_CRASH_BOUNDARY`), `crates/quanta-index-semantic/src/build.rs:98`, `crates/quanta-index-searchd/src/app/state_format.rs:941-947` (`QUANTA_INDEX_STATE_MIGRATION_CRASH`), read via `std::env::var` at request time.
- Symptom: any test that sets these mutates process-global state (`config.rs:1382` notes `set_var` is unavailable in this workspace's tests, so suites fork subprocesses — good — but parallel subprocesses inherit the parent env).
- Why bad: two parallel crash-matrix tests with different boundaries can cross-contaminate through the inherited environment.
- Severity: M.
- Fix: scope env to the child `Command::env()` only (as `e2e_umask_hardening.rs:41-52` already does), never `set_var` in-process; or mark the matrix tests `serial`. Repro: run two boundaries in parallel with a shared parent env and observe the wrong boundary firing.

## F13 — Proptest suites run unseeded
- File: `crates/quanta-index-lq-trigram/tests/property_from_prior_equivalence.rs:44-45` (`cases: 256, ..default()`), same shape in `property_idempotent_upsert.rs:52-53`, `crates/quanta-index-lq-positions/tests/property_upsert_idempotent.rs:73-74`, `property_delete_invariant.rs:58-59`, `property_plan_limits.rs:37-38` (`cases: 64`).
- Symptom: RNG seed comes from the ambient proptest default; a failure's seed is only recoverable from the printed regression file.
- Why bad: "passed 256 cases" is not a fixed corpus — reruns explore different inputs, so a flaky failure may not reproduce without `PROPTEST_CASES`.
- Severity: L.
- Fix: document the `PROPTEST_CASES=<seed>` replay command in each file header and check in failing seeds as regression cases; optionally pin `ProptestConfig { cases, failure_persistence: File, .. }`. The seeded Poisson schedule in `open_loop.rs:130-171` (explicit `seed`, local xorshift at `:122-128`) is the good pattern to copy.

## F14 — Shared statics across parallel tests (ordering dependence)
- File: `crates/quanta-index-semantic/src/build/tests.rs:51-58` (`static BUDGET: OnceLock`, `static TALLIES: DenseLaneTalliesV1` shared by every build test via `unbounded_watch()`); `crates/quanta-index-semantic/src/durable_write.rs:47` (`ATOMIC_WRITE_SEQUENCE`), `crates/quanta-index-lexical/src/index_store.rs:96` (`TEMP_SEQUENCE`), `crates/quanta-index-embed/src/openai/retry.rs:68` (`SEED_COUNTER`), `crates/quanta-index-searchd-harness/src/harness.rs:110` (`NEXT_SOCKET_ID`).
- Symptom: tallies/sequences accumulate across tests in one process; any assertion counting tallies or assuming sequence 0 depends on execution order and parallelism.
- Why bad: `cargo test` runs tests on multiple threads — shared counters make assertions order-dependent.
- Severity: M.
- Fix: per-test `RequestBudgetV1`/`Tallies` instances (drop the `OnceLock`); keep the atomic sequences (they are collision-avoidance, correctly using pid + sequence) but never assert their exact values.

## Non-findings (checked, deliberately clean)
- `StructuralLeafCache` (`crates/quanta-index-search-plane/src/query_dispatcher/routes/structural/eval.rs:25-30`) is the single `HashMap` in hot code and carries an explicit `#[expect(clippy::disallowed_types)]` with reason "transient, not part of any persisted or external ordering surface" — correct; iteration order never escapes.
- Posting-list builders (`lq-trigram/builder.rs`, `lq-positions/builder.rs`) and CBOR encoders use `BTreeMap` with sorted output and `sort_unstable()` before emit — deterministic by construction.
- `SystemTime::now()` in `timeref.rs:223`, `integrity.rs:87`, `sealed_generation/scrub.rs:67` feeds production timestamps (mtime/catalog times), not test assertions — out of scope for test flakiness.
- `open_loop.rs:120-159` Poisson schedule is seeded and local (no ambient thread RNG) — deterministic replay; no fix needed.
