# LXE-00 - Truth Freeze and Executable Matrix

> Archive status: `Historical execution record`. Current architecture: [JUN-02-001](../../../adr/JUN-02-001-search-dsl-authority-and-runtime-contract.md) and [JUN-06-001](../../../adr/JUN-06-001-sourcegraph-compatibility-boundary.md). Live capability truth: [Lexical Capability Matrix](../lexical-capability-matrix.md). Archive map: [Completed Plan Archive](../../ARCHIVE-INDEX.md).


Status: `completed`
Priority: `P0`
Depends on: none

## Purpose

Create the source-backed truth table for lexical DSL, Sourcegraph syntax,
planner lowering, engine execution, response shape, and proof coverage.

No later ticket may claim completion from docs or parser coverage alone.

## Current live truth (2026-05-27)

- `lexical-capability-matrix.md` exists and is now closed against live-source
  proof rather than parser-only inventory.
- `crates/quanta-index-searchd-runtime/tests/e2e_matrix_inventory.rs` proves
  the harness-backed write -> seal -> reopen -> query path and typed invalid
  request behavior.
- contract- and SDK-surface regressions are fenced by:
  - `cargo test -p quanta-index-contract --test lxe_unified_surface -- --nocapture`
  - `cargo test -p quanta-index-contract --test ipc_query_result_v2_contract -- --nocapture`
  - `cargo test -p quanta-index-sdk --lib`
  - `cargo test -p quanta-index-searchd-runtime --test e2e_matrix_inventory -- --nocapture`

## Owner files

- `docs/plans/may-25-lexical-enhancement/lexical-capability-matrix.md`
- `docs/plans/may-25-lexical-enhancement/tickets/*.md`
- `crates/quanta-index-search-plane/src/lowering.rs`
- `crates/quanta-index-search-plane/src/query_dispatcher.rs`
- `crates/quanta-index-lexical/src/lib.rs`
- `crates/quanta-index-contract/src/query/**`
- `crates/quanta-index-contract/src/results/**`
- `crates/quanta-index-searchd-runtime/tests/dsl_scenarios.rs`
- new `crates/quanta-index-searchd-runtime/tests/e2e_matrix_inventory.rs`

## File-level work breakdown

- `docs/plans/may-25-lexical-enhancement/lexical-capability-matrix.md`: add one
  row per syntax/operator with parser, lowering, planner, executor, response,
  test, and owner-ticket columns.
- `crates/quanta-index-search-plane/src/lowering.rs`: annotate every accepted,
  rejected, dead, and no-op lexical branch against the matrix.
- `crates/quanta-index-search-plane/src/query_dispatcher.rs`: map active runtime
  routes and fail-closed branches into the matrix.
- `crates/quanta-index-lexical/src/lib.rs`: enumerate every live engine path and
  every `NotImplemented` or silent drop branch.
- `crates/quanta-index-contract/src/query/**` and `results/**`: freeze active
  public surface rows and legacy-absent rows.
- `crates/quanta-index-searchd-runtime/tests/*.rs`: inventory current parser,
  unit, and E2E coverage and tag gaps explicitly.

## Work items

- Build a matrix with one row per accepted LQ leaf/filter/option/directive.
- Build a matrix with one row per accepted Sourcegraph expression.
- For every row, record:
  - parser entry
  - normalizer/translator entry
  - active contract carrier
  - lowering call site
  - planner node
  - execution engine
  - result carrier
  - existing unit test
  - existing E2E test
  - status: `executed`, `typed-rejected`, `expected-failing`, or `dead`
- Add a test that fails if a filter is lowered to a no-op without an explicit
  status annotation.
- Add a test that fails if removed public request fields reappear.
- Mark all parser-only rows as not complete until a real-engine E2E row exists.

## Test plan

- `cargo test -p quanta-index-search-plane capability_matrix`
- `cargo test -p quanta-index-contract legacy_query_shape_absent`
- `cargo test -p quanta-index-lexical no_silent_filter_drop`

## E2E plan

This ticket creates inventory tests only. It does not prove runtime behavior.
Runtime scenarios are owned by `E2E-00` through `E2E-07`.

## DoD

- `lexical-capability-matrix.md` exists and links every row to file/line
  evidence or to an expected-failing ticket.
- every accepted syntax row has exactly one owner ticket.
- every silent no-op candidate has been converted into either an executable
  plan item or a typed rejection.
- no ticket is allowed to mark a row `done` without a green unit test and E2E
  reference.

## Failure modes

- Broad grep is mistaken for proof.
- Sourcegraph translator coverage is counted as runtime coverage.
- parser-only tests are counted as E2E.
- fail-closed structural/history behavior is counted as live success.
