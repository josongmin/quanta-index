# CHEESY production code audit (testability / perf) — 2026-09-22

Scope: production `src` code that makes tests slow, flaky, or duplicated.
Each finding below was read in the file body cited (search hits alone were not used).

## 1. Ambient env reads in SDK path resolution — tests must mutate process-global env

- File: `crates/quanta-index-sdk/src/config.rs:166-172` (`ConnectOptions::resolve_state_root`)
- Symptom: `resolve_state_root()` calls `std::env::var("QUANTA_INDEX_STATE_ROOT")`, `std::env::var("QUANTA_INDEX_CACHE_ROOT")`, and `std::env::var("HOME")` deep inside resolution. `cfg(target_os)` branch at lines 178-181 adds a second platform-dependent behavior.
- Why bad (cheesy/weak): env is process-global mutable state. Every test of default-path resolution must set/restore env vars, which cannot run in parallel safely and leaks between tests. It also hides the real input (a path) behind an undeclared dependency.
- Fix: take an explicit resolver: `resolve_state_root_with(env: &dyn Fn(&str) -> Option<String>)` (or a small `EnvReader` trait), with the real `std::env::var` passed only at the binary edge. Unit tests then pass a HashMap stub with zero env mutation.

## 2. Wall-clock baked into time-reference parsing — nondeterministic pure functions

- File: `crates/quanta-index-core/src/timeref.rs:169-230` (`parse_human_relative_timeref_ms`, `parse_duration_timeref_ms`, `now_ms` at 222-223 calls `SystemTime::now()` directly)
- Symptom: relative inputs (`"yesterday"`, `"3 days ago"`, `"7d"`) read the live clock inside the parser.
- Why bad (nondeterministic/slow): tests for these parsers are time-bombs — they pass or fail depending on when they run, and the only way to test boundary behavior is sleeping to cross a boundary. Pure parsing logic is fused to an impure clock.
- Fix: `parse_search_timeref_ms(value, now_ms: u64)` (or a `Clock` trait with a `FixedClock` test double); keep a thin `parse_search_timeref_ms_now(value)` wrapper that calls `now_ms()` once at the edge.

## 3. Wall-clock baked into idempotency lease logic — fence/lease paths untestable deterministically

- File: `crates/quanta-index-core/src/domains/idempotency.rs:260-272` (`now_unix_ms()` calls `SystemTime::now()` directly; `lease_deadline_ms`/`deadline_ms` compared against it)
- Symptom: lease-expiry, recovery, and fence-loss decisions all bottom out in a free function reading the live clock.
- Why bad (nondeterministic): expired-vs-live lease tests need real time to pass or injected sleeps; parallel runs near a deadline boundary flake. Same root cause as #2 but on the journal's safety-critical path.
- Fix: inject `now_ms: u64` (or `&dyn Clock`) into claim/recover/commit entry points; keep `now_unix_ms()` only as the production clock implementation.

## 4. Env-gated derivation mode read at call time — hidden global input on ingest path

- File: `crates/quanta-index-search-plane/src/semantic_derive.rs:74-88` (`semantic_derivation_mode_from_env_v1` reads `QUANTA_INDEX_SEMANTIC_DERIVE_MODE` via `std::env::var` on every call)
- Symptom: ingest derivation behavior switches on a process env var resolved deep in the call stack, not at startup.
- Why bad (cheesy/weak): tests covering all three modes must mutate global env around each case (serial-only, order-dependent); production can also change behavior mid-process if env is altered.
- Fix: resolve once at startup into `SemanticDerivationModeV1` and thread the value (or a config struct) through the derive entry points; keep the env read in exactly one `from_env_or_default()` constructor.

## 5. `process::exit` + env read inside library code — crash points kill the test process

- File: `crates/quanta-index-search-plane/src/crash_point.rs:55-67` (`reached()` reads `QUANTA_INDEX_CRASH_POINT` and calls `std::process::exit(86)`)
- Symptom: any test that reaches a crash point with the env var set dies with the process — no unwinding, no destructors, no test-harness reporting.
- Why bad (untestable): the only way to test crash-point wiring is spawning real subprocesses (the slow crash-matrix e2e). Unit coverage is structurally impossible.
- Fix: inject the policy: `reached(point, &dyn CrashHook)` where the production hook exits and the test hook records the call. The `cfg(not(test/debug))` no-op at line 70-71 already admits the seam — make it an explicit dependency instead of a build flag.

## 6. Fixed 20 ms condvar poll quantum on the single-flight hot path — every wait pays the quantum

- File: `crates/quanta-index-search-plane/src/single_flight.rs:19,103-108` (`AWAIT_FLIGHT_POLL = 20ms`; `await_outcome` slices every budget wait into `wait_timeout(..., remaining.min(20ms))`)
- Symptom: cancellation/deadline observation latency is quantized to 20 ms, and every coalesced-reader test must wait real multiples of the quantum to prove interruption.
- Why bad (slow): tests are forced to burn wall-clock time to exercise budget paths; production pays up to 20 ms extra latency on every interrupted wait for no correctness reason (cancellation never signals the condvar).
- Fix: signal the condvar on budget cancel/peer-leave (or accept a `waker`), and/or make the poll interval a constructor parameter defaulting to 20 ms in prod and ~1 ms in tests.

## 7. Readiness snapshot re-reads the whole corpus directory tree — O(corpus) I/O on a control query

- File: `crates/quanta-index-search-plane/src/readiness/search_corpus_history.rs:286-379` (`load_search_corpus_root_snapshot_v1` does `fs::read_dir` per pair dir plus `read_regular_file_nofollow_v1` of every `.cbor` record; same pattern in `readiness/activation_catalog.rs:130-136`)
- Symptom: every `ProcessReadiness` control call walks the full authority directory and reads every history file synchronously.
- Why bad (slow): readiness tests must build full fixture trees and pay real filesystem I/O; in production, readiness latency grows with corpus size and contends with ingest. Tests are forced to exercise the slowest possible path.
- Fix: cache the snapshot behind the pair-mutation coordinator generation counter and invalidate only on mutation; readiness then serves the cached snapshot (fast path) and tests can assert on the cache separately from the loader.

## 8. Duplicated recursive lowering for `And`/`Or` — one bug needs two identical tests

- File: `crates/quanta-index-search-plane/src/lowering.rs:97-116` (`strip_sourcegraph_structural_patterntype` repeats the same recurse-collect-set-flag body for `SgQuery::And` and `SgQuery::Or`)
- Symptom: two near-identical arms; any fix (e.g. flag combination, error propagation) must be applied twice and tested twice.
- Why bad (duplicate): forces duplicated tests for identical logic; drift between the arms is a latent bug.
- Fix: one helper `strip_children(children: Vec<SgQuery>) -> Result<(Vec<SgQuery>, bool), BridgeError>` called from both arms (or a `map_children` combinator on `SgQuery`).

## Out of scope / not cited (no body opened)

- `control_dispatcher.rs:464-482` triple `.clone()` identity conversions and `observability.rs` multi-`Mutex` scrape clones look wasteful but need profiling evidence before claiming perf impact; left for the perf pass.
