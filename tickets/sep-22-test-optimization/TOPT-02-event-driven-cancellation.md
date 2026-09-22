# TOPT-02 — Event-Driven Cancellation and Wakeup Ownership

Status: `planned`

Depends on: TOPT-00

Findings: R4, PO-4

Aligned S21 owners: S21-05, S21-09

## Goal

Cancellation must wake blocked work immediately. Poll intervals may remain as a
defensive fallback, but cannot be the primary correctness or test oracle.

## IPC PeerWatch design

1. Add an explicit wake primitive to the watched poll set, preferably a Unix
   stream pair or equivalent FD with clear ownership.
2. `disarm` sets stop, signals the wake FD, joins, and returns the terminal
   watcher result.
3. Disconnect and stop become distinct typed terminal reasons.
4. The watcher owns and closes both peer and wake resources exactly once.
5. Expose a bounded test observer for `armed`, `peer-disconnected`, `stopped`,
   and `joined`; do not expose production mutation hooks.

## Single-flight design

1. Extend request-budget cancellation with waiter registration/notification, or
   introduce a cancellation listener owned by the flight wait.
2. `await_outcome` waits on outcome-or-cancellation, not a fixed 20 ms slice.
3. Settle, fence, and cancel races have one terminal result per waiter.
4. Registration is removed by RAII on every return/panic path.
5. If the final implementation retains a poll fallback, tests prove the wake
   path and the fallback remains a last-resort liveness bound only.

## Owned paths

- `crates/quanta-index-ipc/src/server.rs`
- IPC peer-watch owner tests and
  `tests/{repo_admission_and_slowloris,g0r_runtime_cancellation_probe}.rs`
- `crates/quanta-index-search-plane/src/single_flight.rs`
- snapshot-registry and history-text owner tests

## Required race matrix

- cancel before registration, during wait, and concurrent with settle;
- peer closes before/after disarm and while response dispatch completes;
- repeated disarm is impossible or explicitly idempotent by type;
- poisoned outcome/watcher failure remains error, never cancellation success;
- no waiter, thread, FD, or registration remains after terminal return.

## Verification

- `./scripts/cargow test -p quanta-index-ipc --lib`
- `./scripts/cargow test -p quanta-index-ipc --test repo_admission_and_slowloris`
- `./scripts/cargow test -p quanta-index-ipc --test g0r_runtime_cancellation_probe`
- `./scripts/cargow test -p quanta-index-search-plane`
- `just rust-profile test-integration-fast`

## Done

The three fixed IPC sleeps and the 20 ms single-flight correctness dependency
are gone. Barrier/channel proofs pass without relying on elapsed time, and
resource counts return to baseline.
