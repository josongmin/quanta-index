# Slow, flaky, and test-layer audit (Sep 22)

Audited snapshot: `23bd3d7fa7af1122f904e59f1f514935bd5ffe7e` (`main`,
equal to `origin/main` when frozen).

This is the single owner document for slow-test cost, nondeterminism, and
E2E-versus-unit balance. Findings are retained only when the current source
proves a happy-path wait, a scheduler-dependent oracle, an interruptibility
defect, or redundant heavy coverage. A timeout ceiling is not treated as an
observed duration.

## Evidence boundary

- `just rust-profile-history-summary-json` reported host-local aggregate profile
  history, but it is not keyed by source SHA, cache state, contention, test case,
  or successful runs only. It cannot rank individual tests.
- No uncontended per-test timing run was captured for this audit. A concurrent
  Cargo mutation process was active on the host, so a new timing sample would
  not be clean performance evidence.
- The repository has already removed the old one-Cargo-process-per-target local
  structure: `tools/ci/test-authority.toml:76-81` expands each local scope into
  one nextest process, and `crates/quanta-index-searchd-runtime/Cargo.toml:4-33`
  sets `autotests = false` and declares three composed runtime suites plus two
  owner targets. `Justfile:48-50` exposes the 9-source fast, 30-source risk, and
  49-source exhaustive profiles. Recreating per-file targets is not an open fix.

## Retained findings

### R1 — Two-process lease proof has a fixed three-second happy-path floor

- Evidence:
  `crates/quanta-index-searchd-runtime/tests/runtime_supervisor_owner_v1.rs:467-487`
  writes the holder report and then sleeps for exactly three seconds while
  holding the lease. The parent starts the second process and then waits for the
  holder at `:533-565`.
- Failure mode: every execution of
  `state_root_lease_two_process_exclusion` waits for the full sleep even after
  the second process has already proved `STATE_ROOT_IN_USE`. Process isolation is
  required for the file-lock contract; the fixed hold duration is not.
- Owner: `quanta-index-searchd-runtime`, test harness in
  `runtime_supervisor_owner_v1.rs`.
- Minimum fix: give the holder an explicit release signal. For example, pipe its
  stdin, report `held`, run and verify the second child, then write/drop the pipe
  so the holder releases immediately. Parent death closes the pipe, so the child
  does not need a fixed normal-path sleep.
- Verification rail:
  `just rust-profile test-runtime-supervisor-owner`, plus a focused exact run of
  `state_root_lease_two_process_exclusion` when collecting before/after timing.

### R2 — Embed retry unit tests execute real randomized backoff

- Evidence: `crates/quanta-index-embed/src/openai/retry.rs:11-42` produces
  full-jitter delay up to 250 ms on attempt zero, and
  `crates/quanta-index-embed/src/openai.rs:337-383` sleeps that delay in the
  production retry loop. The unit cases at `openai.rs:1286-1339` each trigger
  one real retry even though their assertions cover response classification and
  attempt counts, not elapsed time.
- Failure mode: two ordinary library tests add a random delay in `[0, 250 ms)`
  each. Their verdict is deterministic, but test-fast wall time varies for no
  oracle value.
- Owner: `quanta-index-embed`, `openai.rs` / `openai/retry.rs`.
- Minimum fix: inject a per-provider retry delay source/sleeper (no
  process-global test hook). Production keeps full jitter and budget-aware
  sleeping; unit tests use deterministic zero delay. Keep separate pure tests
  for status classification and delay bounds.
- Verification rail:
  `./scripts/cargow test -p quanta-index-embed --lib openai::tests::`, followed
  by `just rust-profile test-fast`.

### R3 — Embed concurrency proof uses a 25 ms timing window as its oracle

- Evidence: `crates/quanta-index-embed/src/openai.rs:971-1016` increments the
  in-flight counter and sleeps `self.delay`; the test configures 25 ms at
  `:1041-1052` and requires `peak_in_flight >= 2` at `:1067-1076`.
- Failure mode: overlap is inferred from OS scheduling inside a 25 ms window.
  A sufficiently delayed worker can make a correct four-worker implementation
  report peak one. The test also pays the delay on all ten mock requests.
- Owner: `quanta-index-embed`, test transport in `openai.rs`.
- Minimum fix: make first-wave overlap structural. Gate exactly the first four
  mock calls on a four-party barrier (or latch), let later calls proceed without
  the gate, and retain the peak bound and output-order assertions.
- Verification rail:
  `./scripts/cargow test -p quanta-index-embed --lib
  openai::tests::embed_batch_dispatches_batches_concurrently_and_preserves_order`,
  then repeat the focused test under normal nextest parallelism.

### R4 — Peer-watch teardown is not interruptible; cancellation tests compensate with sleeps

- Evidence:
  - `crates/quanta-index-ipc/src/server.rs:1324-1365` sets the 50 ms interval;
    its `probe_only` branch uses `thread::sleep`, while the ordinary socket
    poll is bounded by the same interval at `:1431-1470`.
  - `PeerWatch::disarm` sets `stop` and immediately joins at
    `server.rs:1374-1380`. A watcher blocked in `poll` or `sleep` cannot observe
    `stop` until that interval ends, so a completed dispatch can pay watcher
    teardown latency.
  - `crates/quanta-index-ipc/tests/repo_admission_and_slowloris.rs:421-451`
    adds fixed 120 ms and 200 ms sleeps to make the pipelined-then-hang-up
    ordering likely.
  - `crates/quanta-index-ipc/tests/g0r_runtime_cancellation_probe.rs:415-457`
    adds another fixed 200 ms wait even though its module header says ordering
    is handshake-driven.
- Failure mode: the production request path and its tests share a wall-clock
  polling seam. Fast requests may wait for watcher join, the two integration
  proofs carry 520 ms of unconditional sleep, and correctness is inferred after
  elapsed time instead of acknowledged watcher state.
- Owner: `quanta-index-ipc`, `PeerWatch` in `src/server.rs`; the two integration
  targets own their fixture assertions.
- Minimum fix: repair `PeerWatch`, not only the tests. Add an explicit wake FD
  (for example a Unix stream pair) to the watched poll set so `disarm` wakes and
  joins immediately. Expose a test-only observer/channel for the states needed
  by the pipelined and hang-up proofs; replace all three fixed sleeps with bounded
  receives. Keep a timeout only as failure containment.
- Verification rails:
  `./scripts/cargow test -p quanta-index-ipc --lib`,
  `./scripts/cargow test -p quanta-index-ipc --test
  repo_admission_and_slowloris`, and
  `./scripts/cargow test -p quanta-index-ipc --test
  g0r_runtime_cancellation_probe`.

### R5 — One resource-envelope daemon case duplicates lower-layer authority

- Evidence:
  - Core policy tests already prove that chunks and typed sources both count as
    carried rows at
    `crates/quanta-index-core/tests/ingest_resource_policy.rs:140-170`, and
    prove record, text, and vector ceilings at and one past the limit at
    `:173-202`.
  - Search-plane owner tests prove preflight refusal, zero mutation, admission,
    and accounting in
    `crates/quanta-index-search-plane/src/ingest_dispatcher/tests/search_corpus.rs:138-197`.
  - Config parsing proves all three operator fields map to the policy in
    `crates/quanta-index-searchd/src/app/config.rs:2456-2459`.
  - The first daemon test in
    `crates/quanta-index-searchd-runtime/tests/e2e_ingest_resource_envelope.rs:63-155`
    provides the necessary wiring proof: typed refusal leaves no generation,
    then an admitted batch seals and serves. The second test at `:157-190`
    boots another daemon only to re-prove the record-count boundary.
- Failure mode: the second E2E adds a full daemon fixture without covering a
  remaining boundary. Its only assertion is already authoritative below the
  daemon, while the same file's first case covers policy wiring and disk effects.
- Owner: `quanta-index-searchd-runtime`, with retained lower-level authority in
  `quanta-index-core`, `quanta-index-search-plane`, and searchd config tests.
- Minimum fix: remove `the_record_ceiling_counts_every_carried_row`; retain the
  first E2E unchanged and retain all lower-level boundary rows.
- Verification rails: the owning core/search-plane unit tests, then
  `just rust-profile test-daemon-all` for exhaustive daemon closeout. This is a
  coverage relocation, not authorization to weaken workspace nextest.

## Execution order

1. R4: fixes a shared production/test wait owner and removes duplicated sleeps.
2. R1: removes the only multi-second unconditional happy-path wait retained.
3. R2 and R3: make embed unit timing deterministic without changing production
   retry semantics or concurrency limits.
4. R5: remove the one proven redundant daemon boot after lower-layer rails stay
   green.

No production or test source was changed by this audit.
