# TOPT-01 — Explicit Clock and Environment Ports

Status: `code-landed; qualification pending`

Depends on: TOPT-00

Findings: PO-1, PO-2, PO-3

Aligned S21 owners: S21-04, S21-06, S21-07

## Goal

Move nondeterministic inputs to composition edges. Core parsing, catalog state
transitions, and SDK precedence logic must accept explicit values and remain
fully deterministic under unit tests.

## Design

- Use small capability types/functions, not framework-wide dependency
  injection.
- Sample wall time once per operation/transaction.
- Persist Unix deadlines exactly as today; do not use a process-local monotonic
  clock for durable cross-process lease semantics.
- Public convenience APIs may sample the real clock/environment and delegate to
  deterministic owner functions.
- Tests pass fixed/scripted values. They do not mutate process-global env.

## File-level work

| Owner | Change |
|---|---|
| `crates/quanta-index-core/src/timeref.rs` | add `parse_search_timeref_ms_at(value, now_ms)`; current API samples once and delegates |
| `crates/quanta-index-core/src/domains/idempotency.rs` | retain Unix-millisecond value contract; stop consumers from reaching a free global clock mid-transition |
| `crates/quanta-index-catalog/src/connection.rs` | give `SqliteCatalog` a production clock capability and test constructor |
| `crates/quanta-index-catalog/src/idempotency.rs` | sample once per transaction and thread `now_ms` through claim/recovery/mutation-lease decisions |
| `crates/quanta-index-sdk/src/config.rs` | extract deterministic state-root resolution over an environment lookup/value object; public resolve uses real env at the edge |

## Required tests

1. Every relative unit, exact subtraction, zero, and underflow at fixed time.
2. SDK precedence: explicit option > state-root env > cache-root env > HOME;
   missing and non-Unicode inputs; platform fallback isolated from precedence.
3. Catalog claim/recover/mutation-enter at `now < deadline`, `==`, and `>`.
4. Scripted clock step between independent transactions; no second sample
   inside one transaction.
5. Concurrent SDK tests remain safe without a global env mutex.

## Failure rules

- Clock or environment read failure remains typed; no epoch/default-home guess.
- Poisoned/scripted test clocks cannot produce success by fallback.
- Do not add a second legacy API that duplicates state-transition logic.

## Verification

- `./scripts/cargow test -p quanta-index-core`
- `./scripts/cargow test -p quanta-index-catalog`
- `./scripts/cargow test -p quanta-index-sdk`
- `just rust-profile test-fast`

## Done

All three findings have exact deterministic matrices, no test in scope uses
`set_var`, and production sampling exists only at named composition edges.
