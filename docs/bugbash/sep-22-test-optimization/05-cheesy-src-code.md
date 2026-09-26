# Actionable production-owner testability audit (Sep 22)

> Archive classification: historical Sep 22 audit input. Use the active [TOPT ledger](../../../tickets/sep-22-test-optimization/INDEX.md), [current closeout](../../../tickets/sep-22-test-optimization/SEP25-CURRENT-CLOSEOUT.md), and [SEP-27-001](../../adr/SEP-27-001-documentation-authority-and-historical-record-custody.md).


Audit basis: `23bd3d7fa7af1122f904e59f1f514935bd5ffe7e`.
These items require a production owner seam; they cannot be fixed honestly by
adding sleeps or test-only shadow logic. Style-only duplication and unprofiled
performance claims were removed.

## PO-1 — Relative time parsing owns the wall clock

- Severity: **M**
- Evidence:
  - `crates/quanta-index-core/src/timeref.rs:169-220` resolves `yesterday`,
    `N units ago`, and duration shorthand by calling `now_ms()` internally.
  - `:222-229` reads `SystemTime::now` directly.
  - Consequently the tests at `:321-332` can assert only `is_some`, not the
    exact timestamp or boundary behavior.
- Reachable failure mode: wrong unit arithmetic or an off-by-one boundary can
  stay green because no deterministic expected value can be supplied. Two
  identical parses at different instants also legitimately return different
  values, preventing an exact semantic oracle.
- Owner/fix: core time-reference owner. Add
  `parse_search_timeref_ms_at(value, now_ms)` (or a small clock port); keep the
  current function as the production edge that samples once and delegates.
- Verification rail: core owner-local tests with fixed `now_ms` for every unit,
  underflow, and exact-boundary cases, then
  `./scripts/cargow test -p quanta-index-core`.

## PO-2 — SDK path resolution hides process-global environment reads

- Severity: **M**
- Evidence:
  `crates/quanta-index-sdk/src/config.rs:156-182` reads
  `QUANTA_INDEX_STATE_ROOT`, `QUANTA_INDEX_CACHE_ROOT`, and `HOME` directly
  inside `ConnectOptions::resolve_state_root`. The searchd config already has
  the owner pattern: an injected environment lookup at
  `crates/quanta-index-searchd/src/app/config.rs:560-578`.
- Reachable failure mode: exact precedence/error tests must mutate global env,
  so they cannot safely run in parallel and can observe another test's value.
  The SDK currently lacks a deterministic test seam for missing/non-Unicode
  variables and platform-default fallback.
- Owner/fix: SDK config owner. Resolve through an injected lookup in an
  owner-local helper and keep `std::env::var` only at the public composition
  edge, matching the searchd pattern.
- Verification rail: SDK table tests for explicit option > state-root env >
  cache-root env > home fallback, including missing and non-Unicode values,
  with no `set_var`; then `./scripts/cargow test -p quanta-index-sdk`.

## PO-3 — Durable idempotency and mutation-lease decisions hard-code wall time

- Severity: **M**
- Evidence:
  - `crates/quanta-index-core/src/domains/idempotency.rs:260-272` exposes a
    free `now_unix_ms()` backed by `SystemTime::now`.
  - `crates/quanta-index-catalog/src/idempotency.rs:919`, `:1263`, and `:1507`
    read it inside claim, recovery, and mutation-lease entry.
  - `SqliteCatalog` stores only the connection/path
    (`catalog/src/connection.rs:31-34`), so no clock can be supplied.
- Reachable failure mode: live/expired/fence recovery boundaries cannot be
  reproduced exactly; tests avoid the boundary with `u64::MAX` leases or
  already-expired values. Clock-step behavior and equality at the deadline are
  therefore not covered by a deterministic state-machine oracle.
- Owner/fix: catalog idempotency owner. Give `SqliteCatalog` a clock port (real
  wall clock in `open`, fixed/scripted clock in tests) and sample once per
  transaction. Do not replace persisted Unix deadlines with a process-local
  monotonic clock.
- Verification rail: owner-local table over `now <`, `==`, and `>` deadline
  for claim/recover/mutation-enter, including clock-step scripts; then
  `./scripts/cargow test -p quanta-index-catalog`.

## PO-4 — Single-flight cancellation is observable only by a fixed 20 ms poll

- Severity: **M**
- Evidence:
  - `crates/quanta-index-search-plane/src/single_flight.rs:16-19` states that
    cancellation does not signal the condvar and fixes the poll quantum at
    20 ms.
  - `:90-110` slices every waiter sleep by that constant.
  - This path is used by both snapshot-registry and history-text cold opens
    (`snapshot_registry.rs:383`, `history_text.rs:307`).
- Reachable failure mode: a cancelled coalesced reader remains blocked until
  the next poll (up to 20 ms) even though its request is already cancelled;
  deterministic tests must wait real time to prove interruption. Reducing the
  constant only trades latency for wakeup load.
- Owner/fix: request-budget/single-flight owner. Add a cancellation wake
  registration (or a waiter notification handle) so cancellation wakes the
  condvar. If that is too invasive, inject the poll policy as an interim test
  seam while retaining the production default; do not add shorter sleeps to
  tests.
- Verification rail: snapshot-registry and history-text owner tests using a
  barrier: cancellation must release a waiter without settling the flight or
  waiting a real 20 ms quantum; then
  `./scripts/cargow test -p quanta-index-search-plane`.
