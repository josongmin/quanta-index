# TOPT-06 — Fail-Closed Wait Helpers and Temporary Resource Custody

Status: `planned`

Depends on: TOPT-01, TOPT-03

Findings: TH-1, TH-2, TH-3

Aligned S21 owners: S21-07, S21-10

## Goal

Test helpers return success only when their predicate is true, and filesystem
resources live under an RAII owner that guarantees uniqueness and cleanup.

## Wait result contract

Use a test-local result type carrying:

- predicate satisfied value; or
- terminal operation error; or
- deadline exceeded with elapsed bound, attempts, last value/error, and
  expected predicate description.

Do not return the last value as success. Do not convert a retryable error into a
terminal error merely because the deadline elapsed.

## Work

1. Repair SDK observation and terminal-error helpers in `sdk_frontdoor.rs`.
2. Repair process-envelope scrape wait using the fail-closed scrub helper
   contract as the local model.
3. Add immediate-ready, retry-then-ready, never-ready, terminal-error, and
   retryable-error-timeout unit seams without real sleeps.
4. Replace pid-only SDK binding directories with `tempfile::TempDir`; keep the
   guard alive across server/client lifetime.
5. Ensure Unix socket entries and directories disappear on success, assertion
   failure, and server-thread error.

## Verification

- `just rust-profile test-sdk-binding-owner`
- `just rust-profile test-sdk-binding-owner-lib`
- `./scripts/cargow nextest run -p quanta-index-searchd-runtime --test runtime_fast_suite --all-features --locked -E 'test(/^sdk_frontdoor::/)'`
- `./scripts/cargow nextest run -p quanta-index-searchd-runtime --test runtime_extended_suite --all-features --locked -E 'test(/^e2e_process_envelope::/)'`
- `just rust-profile test-daemon-all`

## Done

Never-ready and never-true scripts return typed timeout evidence, stale socket
paths cannot collide with a rerun, and cleanup is guard-owned rather than a
best-effort tail call.
