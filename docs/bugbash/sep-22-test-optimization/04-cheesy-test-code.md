# Actionable test-helper and fixture audit (Sep 22)

Audit basis: `23bd3d7fa7af1122f904e59f1f514935bd5ffe7e`.
This file retains helper/fixture behavior that loses failure causality, creates
cross-run interference, or repeats expensive setup without adding isolation.
Sleep-only performance findings owned by the timing audit are not repeated.

## TH-1 — SDK observation wait returns `Ok` when its readiness predicate timed out

- Severity: **M**
- Evidence:
  `crates/quanta-index-searchd-runtime/tests/sdk_frontdoor.rs:1335-1361`
  returns `Ok(value)` when `ready(&value)` is true **or** when
  `start.elapsed() >= timeout` (`:1348`).
- Reachable failure mode: a never-ready response is represented as successful
  helper completion. Current callers generally catch the stale value in a
  later assertion, but report a value mismatch instead of the readiness
  timeout and lose the retry history. The helper contract also permits a new
  caller that only needs an `Ok` value to false-pass.
- Owner/fix: searchd SDK-frontdoor harness owner. Return a typed test error on
  timeout containing the last value/error and elapsed time. The helper should
  return `Ok` only when `ready` is true. Apply the same explicit timeout
  classification to the terminal-error helper instead of treating a retryable
  code as terminal after the deadline.
- Verification rail:
  focused `runtime_fast_suite` SDK-frontdoor target plus unit tests for
  immediate-ready, retry-then-ready, never-ready, and retryable-error-timeout
  scripts.

## TH-2 — Process-envelope scrape wait returns stale data as success on timeout

- Severity: **M**
- Evidence:
  `crates/quanta-index-searchd-runtime/tests/e2e_process_envelope.rs:106-120`
  returns `Ok(scrape)` for either predicate success or timeout. The correct
  local pattern exists in `e2e_integrity_scrub.rs:142-163`, which returns an
  error with the last counters/gauges on timeout.
- Reachable failure mode: current callers usually fail later as a gauge-value
  mismatch, so this is not a proven false green; however, a maintenance/scrape
  timeout is misclassified as a value regression and loses the causal evidence
  needed to diagnose the test.
- Owner/fix: `e2e_process_envelope` helper owner. Split the branches: return
  `Ok` only when the condition holds, otherwise return a timeout error carrying
  the last scrape.
- Verification rail:
  `./scripts/cargow test -p quanta-index-searchd-runtime --test runtime_extended_suite e2e_process_envelope`.
  Add a deterministic never-true helper test and require a timeout error.

## TH-3 — SDK binding tests reuse persistent pid-only socket directories

- Severity: **M**
- Evidence:
  - `crates/quanta-index-sdk/tests/sdk_binding_owner_v1.rs:157-161` creates
    `/tmp/sdk-binding-owner-{tag}-{pid}` with `create_dir_all` and never owns a
    cleanup guard.
  - `:112-131` binds `query.sock`; Unix socket filesystem entries persist
    after the listener is dropped.
  - The helper is used by every test at `:188`, `:208`, `:226`, `:250`,
    `:271`, `:291`, `:321`, and `:337`.
- Reachable failure mode: rerunning a test binary with a reused pid after a
  crash, or leaving a stale directory/socket, makes `UnixListener::bind`
  fail before the behavior under test. The directory is also never removed.
- Owner/fix: SDK test fixture owner. Replace the helper with
  `tempfile::TempDir` (private mode, unique name, cleanup guard) and keep the
  guard alive through the server/client lifetime.
- Verification rail:
  `./scripts/cargow test -p quanta-index-sdk --test sdk_binding_owner_v1`
  twice, plus a pre-created stale `query.sock` at the old path; the new fixture
  must be unaffected and leave no directory behind.

## TH-4 — Filter E2E repeatedly rebuilds identical sealed fixtures

- Severity: **M**
- Evidence:
  `crates/quanta-index-searchd-runtime/tests/e2e_filter_execution.rs` calls
  `boot_with_history` four times (`:76,92,108,124`),
  `boot_with_multi_repo_and_commit_recency` three times (`:731,770,823`),
  `boot_with_multi_repo_and_repo_meta` five times (`:845,885,955,976,1063`),
  and `boot_with_multi_repo` across many read-only filter tests (`:697` and
  `:1723-1941`, among others). Each helper boots, ingests, seals, and activates
  the same fixture before executing a read-only query.
- Reachable failure mode: the risk suite pays repeated storage/index build cost
  proportional to assertion count. This is deterministic duplicated work, not
  a file-size proxy. It also makes a single fixture defect fan out into many
  unrelated test failures.
- Owner/fix: filter-E2E fixture owner. Table-drive read-only cases by fixture
  family so one test owns one boot/seal and executes all family rows. Keep
  mutation/restart cases isolated; do not introduce cross-test global runtime
  sharing.
- Verification rail:
  `./scripts/cargow test -p quanta-index-searchd-runtime --test runtime_risk_suite e2e_filter_execution`.
  Record before/after elapsed time and require the same row-level failure
  context and assertions.
