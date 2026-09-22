# Slow-test measurement: top-20 cost drivers (SEP-22)

Method: read-only static audit of every `tests/` target plus one live timing
probe. The probe (`cargo test -p quanta-index-searchd-runtime --test
runtime_supervisor_owner_v1 clean_drain_returns_zero_and_drops_guards_last`)
was launched but never got past dependency compilation in the observation
window (datafusion/lance graph, >4 min and still compiling), so **no real
per-test wall times are claimed below**. All second estimates are derived
from sleep durations, poll cadences, and timeout caps read directly from
the cited file bodies. Worst case per site = the cap; typical cost =
cadence x expected iterations (ms).

Cost-driver taxonomy used: F=fixed sleep, P=poll loop, S=process spawn,
B=socket boot, T=tempdir churn, X=large fixture build.

## Ranked top-20 (slowest first, worst-case caps)

### 1. `over_maximum_runtime` — 10,001-row fixture + 600 s client timeout (X) — H
- `crates/quanta-index-searchd-runtime/tests/e2e_top_k_truth_table.rs:748`
- Symptom: `E2eRuntime::boot_with_client_request_timeout(Duration::from_secs(600))`
  then ingests `OVER_MAXIMUM_ROWS = PUBLIC_TOP_K_MAX + 1` rows
  (`crates/quanta-index-contract-base/src/query/top_k.rs:21`: `PUBLIC_TOP_K_MAX = 10_000`).
  Comment at `:740-747` admits the seal "outlasts the client's default wait
  (the harness waits ten minutes)" in a debug build.
- Why bad: single test can legally burn up to ~600 s; 10k-row lexical+semantic
  index + dense-train seal dominates the whole suite's debug-build time.
- Fix: shrink to `PUBLIC_TOP_K_MAX`-boundary probing (e.g. exactly
  `MAX`, `MAX+1` rows of 1-line chunks already suffices — keep row count but
  cut per-row cost via `boot_with_semantic_stream_window_policy` small
  windows), or gate the 10k-row variant behind `#[ignore]` / nightly.

### 2. `publish_with_backoff` — 120 s PATIENCE retry loop (P) — H
- `crates/quanta-index-searchd-runtime/tests/e2e_ingest_idempotency.rs:181`
- Symptom: `const PATIENCE: Duration = Duration::from_secs(120)`; loop sleeps
  50 ms (`:201`) per `SERVER_OVERLOADED` retry; 8 publishers x same loop
  (`:268`).
- Why bad: on a slow/loaded CI worker every publisher can spin the full
  120 s cap; backoff has no jitter/cap scaling and no fast-path assertion.
- Fix: cut PATIENCE to 20–30 s (overload clears in ms locally), assert first
  non-overloaded receipt eagerly, share one `E2eRuntime` (already done) and
  stagger publisher starts.

### 3. Scrub tests — 120 s SCRUB_WAIT at 100 ms pace (P) — H
- `crates/quanta-index-searchd-runtime/tests/e2e_integrity_scrub.rs:44`
- Symptom: `SCRUB_WAIT = 120 s`, `SCRUB_INTERVAL = 100 ms`; one-file-per-step
  policy (`:49-54`) means pass length scales with file count.
- Why bad: worst case 120 s per scrub test; interval pacing serializes steps.
- Fix: shrink `SCRUB_INTERVAL` to 1–5 ms for tests (already a constructor
  arg at `:50-53`), reduce generation file counts, poll receipt file instead
  of sleeping the interval.

### 4. `second_signal_latches_an_immediate_abort` — 60 s + 125 s supervisor deadlines (F) — M
- `crates/quanta-index-searchd-runtime/tests/runtime_supervisor_owner_v1.rs:387`
- Symptom: `SearchdSupervisor::new(Duration::from_secs(60), Duration::from_secs(125), …)`;
  blocking child sleeps 30 s (`:401`); trigger thread sleeps 30 ms + 200 ms
  (`:411-413`).
- Why bad: the 60/125 s deadlines are orders of magnitude above the ~230 ms
  exercised path; a regression hang parks the test binary for minutes.
- Fix: drop deadlines to ~2 s / ~5 s like sibling tests (`:102-104`); keep
  exit-code assertions identical.

### 5. `hard_deadline_escalation_is_not_graceful` — 30 s sleeper + 5 s assert cap (F) — M
- `crates/quanta-index-searchd-runtime/tests/runtime_supervisor_owner_v1.rs:261`
- Symptom: ignoring child `sleep(30 s)` with 80 ms coop / 400 ms hard
  deadlines (`:249-250`); elapsed asserted `< 5 s` (`:281`).
- Why bad: leaked 30 s sleeper thread per run; fine at 400 ms but the 30 s
  sleep outlives the test and pollutes subsequent timing.
- Fix: replace `sleep(30 s)` with latch `recv_timeout(5 s)` / park on the
  shutdown flag; detached handle already abandoned — make it a scoped,
  instantly-killable wait.

### 6. `p08` lease holder — fixed 3 s hold + 2 process spawns (F+S) — M
- `crates/quanta-index-searchd-runtime/tests/runtime_supervisor_owner_v1.rs:486`
- Symptom: `hold_the_lease` sleeps exactly 3 s while parent spawns two child
  processes of the test binary (`:520-529`, `Command::new(&exe)` at `:522`).
- Why bad: fixed 3 s floor on every run; two full test-binary process spawns
  (~100s of ms each, links whole harness) for a file-lock assertion.
- Fix: replace sleep with a pipe/event: parent probes `second` as soon as
  `holder-report` appears, then signals holder to exit; or fold into an
  in-process two-thread lock-contention test.

### 7. `wait_for_sockets` — 30 s SOCKET_TIMEOUT per real-process boot (P+S+B) — H
- `crates/quanta-index-searchd-runtime/tests/common/searchd_binary_process.rs:15`
- Symptom: `SOCKET_TIMEOUT = 30 s`, 10 ms poll (`:91`); every
  `SearchdBinaryProcess::start` spawns the real `searchd` binary (`:131`)
  and triple-connects sockets until all accept (`:65-99`).
- Why bad: ~10 known call sites (repo_map, crash_matrix, explain, sdk_frontdoor,
  end_to_end, …) each risk 30 s; binary spawn + 3-socket accept is the
  slowest fixture primitive in the repo.
- Fix: share one booted binary per test-binary via `OnceLock` + per-test
  state roots; cut timeout to 10 s; poll at 1–2 ms with `UnixStream::connect`
  fast path (already connect-based at `:117-123`).

### 8. `composite_generation_authority_restart` — 30 s SOCKET_TIMEOUT + full restarts (P+S) — H
- `crates/quanta-index-searchd-runtime/tests/composite_generation_authority_restart.rs:48`
- Symptom: `SOCKET_TIMEOUT = 30 s` (`:48`), `wait_until` at `:75` over three
  sockets, full runtime stop/start cycles per generation (G0/G1/G2).
- Why bad: restart-heavy: each generation boots a driver thread + sockets;
  30 s cap per wait.
- Fix: reuse harness `E2eRuntime::reopen` instead of manual rebuild (as
  `e2e_full_corpus` does at `e2e_full_corpus.rs:2313`), cut SOCKET_TIMEOUT
  to 5–10 s.

### 9. `e2e_generation_activation_concurrency` — 30 s waits x3 barriers (P) — M
- `crates/quanta-index-searchd-runtime/tests/e2e_generation_activation_concurrency.rs:65`
  (also `:343`, `:348`, `:359`), helper at `:157` (5 ms poll).
- Symptom: three separate `wait_until(SOCKET_TIMEOUT, …)` gates on
  generation-visibility atomics; 5 ms cadence is good but cap is 30 s each.
- Why bad: ~90 s combined worst case for an atomic-flag handoff that
  resolves in ms.
- Fix: cut cap to 5 s per gate; replace sleeps at `:166` (5 ms) with
  condition-variable wake on publish.

### 10. `e2e_perf_chaos` — 43 fresh `E2eRuntime::boot()`s (B+T) — H
- `crates/quanta-index-searchd-runtime/tests/e2e_perf_chaos.rs` (43
  `E2eRuntime::boot()` call sites; e.g. `:591`, `:615`, `:649`, …)
- Symptom: every chaos case boots a full daemon (tempdir + 3 sockets +
  lazy driver) from scratch; `boot()` itself is cheap (harness.rs:451 —
  driver starts lazily on first query) but first-query boot + seal repeats
  43x.
- Why bad: highest per-file boot count in the suite; dominates file runtime
  even though each boot is ~100 ms–1 s.
- Fix: share one `E2eRuntime` per file with per-test repo/revision isolation
  (distinct `RepoId`s already used); only boot fresh where policy differs.

### 11. `sdk_frontdoor` (4563 lines) — 5 s SOCKET_TIMEOUT x ~20 waits + SDK retry loops (P) — H
- `crates/quanta-index-searchd-runtime/tests/sdk_frontdoor.rs:68`
  (`SOCKET_TIMEOUT = 5 s`), `wait_until` at `:240` (10 ms), retry helpers at
  `:1310-1390` (`wait_for_sdk_observation*`, 10 ms sleeps at `:1320`, `:1349`,
  `:1357`, `:1385`).
- Symptom: largest test file; dozens of 5 s-capped waits; each
  `wait_for_sdk_observation` issues a real IPC round-trip per 10 ms tick.
- Why bad: aggregate worst case is minutes; per-tick IPC calls amplify load.
- Fix: cut SOCKET_TIMEOUT to 2 s (local sockets appear in ms), back off
  retry cadence 10 ms → 1 ms fast / 25 ms slow, split file into 2–3 targets
  for parallelism.

### 12. `end_to_end` (3492 lines) — SOCKET_APPEAR/READINESS 5 s waits x ~10 (P+B) — H
- `crates/quanta-index-searchd-runtime/tests/end_to_end.rs:351` (`wait_until`,
  10 ms), `start_runtime` at `:367` (spawns driver thread per test),
  waits at `:377`, `:419`, `:792`, `:928`, `:994`, `:1025`, `:1189`, `:1236`,
  `:1284`.
- Symptom: each test builds config + runtime + driver thread + socket-appear
  wait; ~10 wait sites at 5 s caps.
- Why bad: per-test full-stack boot with no sharing; sleeps at `:360` add
  10 ms floors everywhere.
- Fix: hoist `build_config`/`build_runtime` into a file-level shared runtime
  (like `composite_*`'s `RunningRuntime`), keep one test that boots fresh.

### 13. `cli_smoke` — real CLI subprocess per test + 20x10 ms socket poll (S+P) — H
- `crates/quanta-index-searchctl/tests/cli_smoke.rs:142` (representative;
  15+ `Command::new(env!("CARGO_BIN_EXE_…"))` sites through `:1254`),
  socket poll `for _attempt in 0..20 { sleep(10 ms) }` at `:1071-1076`
  (and `:1273`).
- Symptom: every case spawns the compiled CLI binary (process + mock UDS
  server at `:1056`); 200 ms poll cap per server start.
- Why bad: process spawn (~50–200 ms each) x 15+ tests dominates; mock
  server thread + tempdir per test.
- Fix: batch multiple CLI assertions into fewer processes where possible;
  replace `0..20 x 10 ms` poll with connect-retry at 1 ms; share mock
  dispatcher across cases.

### 14. `repo_map_end_to_end` — 5 s READINESS + 6x 2 s socket waits (P+B) — M
- `crates/quanta-index-searchd-runtime/tests/repo_map_end_to_end.rs:51`
  (`READINESS_TIMEOUT = 5 s`), waits at `:355-365`, `:494-504`, `:569-574`,
  `:652`, `:693-703` (each `wait_until(2 s, …)`), helper at `:134` (10 ms).
- Symptom: 5 tests each wait on 1–3 sockets at 2 s caps against a real
  `searchd` process boot.
- Why bad: ~30 s combined worst case; sleeps at `:143` (10 ms) per poll.
- Fix: single shared `SearchdBinaryProcess` for the file; cut per-socket
  cap 2 s → 500 ms; poll at 1 ms.

### 15. `ipc` slowloris test — fixed 120 ms + 200 ms sleeps (F) — L
- `crates/quanta-index-ipc/tests/repo_admission_and_slowloris.rs:433`
- Symptom: `sleep(120 ms)` + `sleep(200 ms)` around a pipelined-frame hang-up;
  comment (`:429-432`) admits sleeps only bound watch time; plus
  `HANDSHAKE_BOUND = 20 s` (`:43`) and `recv_timeout(HANDSHAKE_BOUND)` at `:158`.
- Why bad: 320 ms fixed floor; 20 s bound per handshake op.
- Fix: replace fixed sleeps with barrier/event (`wait_entered` already
  exists at `:157`); shrink HANDSHAKE_BOUND to 5 s.

### 16. `e2e_process_envelope` — idle 2 s + 4x tick + 5 s bound (F+P) — M
- `crates/quanta-index-searchd-runtime/tests/e2e_process_envelope.rs:211`
- Symptom: `idle = 2 s`; bound = `idle*4 + tick*4 + 5 s` (`:231`, `:288`) ≈
  13 s+; `wait_for_scrape` polls at 10 ms (`:108-124`), sleeps at `:119`.
- Why bad: multi-second real-time waits for timer behavior; scrape loop does
  a full metrics scrape per 10 ms tick.
- Fix: inject a short maintenance tick policy for tests (tick override
  already exists via `rt.maintenance_policy()`), cut idle to 100–200 ms.

### 17. `dsl_scenarios` — 5x/10x repeated identical queries (X) — L
- `crates/quanta-index-searchd-runtime/tests/dsl_scenarios.rs:394`
  (`for _ in 0..5`), `:498` (`for _ in 0..10`), helper at `:171`,
  per-test `start_runtime` + tempdir.
- Symptom: determinism proven by repeating the same query 5–10x over IPC.
- Why bad: linear IPC + serialization cost x10 for a property 2–3 runs
  would show; each test also boots its own runtime (`:195`, `:349`, `:634`).
- Fix: reduce loops to 3x (still catches flakiness), share one runtime
  across the DSL file.

### 18. `e2e_dual_syntax_lowering_parity` (2723 lines) — 4 fresh boots, no sleeps (B+T) — M
- `crates/quanta-index-searchd-runtime/tests/e2e_dual_syntax_lowering_parity.rs:2384,2430,2491,2609`
- Symptom: each parity test boots its own `E2eRuntime` and ingests the same
  corpus; no poll loops — pure boot+ingest+seal duplication.
- Why bad: 4x full ingest/seal of near-identical fixtures.
- Fix: boot once, ingest once, run all four parity assertions against the
  same sealed generation (read-only queries).

### 19. `e2e_boot_quarantine` — SqliteCatalog open + multi-generation seals (X) — M
- `crates/quanta-index-searchd-runtime/tests/e2e_boot_quarantine.rs:367`
  (`SqliteCatalog::open(&state_root, Duration::from_secs(5))`), seals at
  `:116-128`, three `#[test]`s (`:178`, `:298`, `:347`) each booting.
- Symptom: each test seals 2–3 generations then reboots to prove quarantine;
  catalog open carries a 5 s busy-timeout.
- Fix: share one sealed/quarantined state root via fixture OnceLock;
  per-test reopen is cheap, re-seal is not.

### 20. `g0r`/socket-access IPC probes — per-test UDS bind + tempdir + barrier (B+T) — L
- `crates/quanta-index-ipc/tests/g0r_runtime_cancellation_probe.rs:245`
  (`start_server`: tempdir + 0700 chmod + `UdsServer::bind` + server thread),
  `repo_admission_and_slowloris.rs:114` (same shape),
  `crates/quanta-index-searchd-runtime/tests/explain.rs:52-53`
  (5 s READINESS + 5 s SOCKET_APPEAR, `wait_until` at `:130`, sleep `:139`).
- Symptom: every probe builds a fresh socket dir, binds, spawns a server
  thread and a holder thread (`g0r:328`), tears all down per test.
- Why bad: individually cheap (~10–50 ms) but repeated across dozens of
  small tests; explain.rs adds 5 s caps per wait.
- Fix: share one bound server per file (cores already support multi-test
  use via `shutdown_handle`); cut explain.rs caps to 2 s.

## Cross-cutting fixes (highest leverage first)

1. Deduplicate `wait_until` (7 copies: `end_to_end.rs:351`,
   `repo_map_end_to_end.rs:134`, `sdk_frontdoor.rs:240`, `explain.rs:130`,
   `dsl_scenarios.rs:171`, `composite_…:162`,
   `e2e_generation_activation_concurrency.rs:157`) into one harness helper
   with 1 ms base cadence + exponential backoff and a 5 s default cap.
2. Cap audit: no test-local timeout above 30 s except the top-k 600 s case;
   replace `Duration::from_secs(30)` sleeps/deadlines in
   `runtime_supervisor_owner_v1.rs:261,401` with latch waits.
3. Boot sharing: `E2eRuntime::boot` is lazy (harness.rs:447-453) — the cost
   is first-query boot + ingest + seal. Files with >10 boots
   (`e2e_perf_chaos`: 43) should share one runtime per file.
4. Process spawns: `cli_smoke` (15+ CLI spawns) and `searchd_binary_process`
   (real daemon per test) are the two spawn hotspots; share or mock.

## Timing probe record

- Command: `cargo test -p quanta-index-searchd-runtime --test
  runtime_supervisor_owner_v1 clean_drain_returns_zero_and_drops_guards_last`
- Result: no test output in window; toolchain was still compiling
  third-party deps (datafusion-datasource-*, lance-*) after 4+ min.
- Implication: even the cheapest single-test timing costs a full workspace
  build here; use `cargo test --no-run` once to warm the cache, then time
  individual targets, or read caps from code as done above.
