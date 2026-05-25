# E2E-00 - Live DSL Matrix Harness

Status: `proposed`
Priority: `P0`
Depends on: [LXE-00](LXE-00-truth-freeze-and-executable-matrix.md)

## Purpose

Create the E2E harness that writes records into real index/storage paths,
opens the search runtime, issues public requests, and asserts responses. Parser
or lowering tests do not satisfy this ticket.

## Owner files

- new `crates/quanta-index-searchd-runtime/tests/common/e2e_harness.rs`
- new `crates/quanta-index-searchd-runtime/tests/common/e2e_corpus.rs`
- new `crates/quanta-index-searchd-runtime/tests/e2e_matrix_inventory.rs`
- `crates/quanta-index-searchd-runtime/tests/dsl_scenarios.rs`
- `crates/quanta-index-search-plane/src/query_dispatcher.rs`
- `crates/quanta-index-lexical/src/lib.rs`

## File-level work breakdown

- `crates/quanta-index-searchd-runtime/tests/common/e2e_harness.rs`: create,
  ingest, seal, reopen, query, and assert helper APIs.
- `crates/quanta-index-searchd-runtime/tests/common/e2e_corpus.rs`: hold
  machine-readable corpus rows and expected result fixtures.
- `crates/quanta-index-searchd-runtime/tests/e2e_matrix_inventory.rs`: ensure
  each matrix row is either green, expected-failing, or intentionally deferred.
- `crates/quanta-index-search-plane/src/query_dispatcher.rs`: expose the public
  request/response boundary the harness calls.
- `crates/quanta-index-lexical/src/lib.rs`: use the same runtime execution path
  the harness will exercise in later E2E tickets.

## Work items

- Build a reusable tempdir-backed runtime harness.
- Insert records through the same ingest/channel/build path used by runtime
  tests, not by direct in-memory vectors.
- Seal or activate a generation before querying.
- Reopen indexes through runtime APIs.
- Issue public lexical/semantic/hybrid IPC/search requests.
- Capture response candidates, typed errors, explanations, and engine traces.
- Support expected-failing rows linked to implementation tickets.
- Emit row-level artifacts for failures:
  - query text
  - syntax
  - indexed corpus rows
  - expected IDs
  - actual IDs
  - typed error code
  - `SearchExplanation`

## Test plan

- harness self-test that one content query is stored, reopened, and retrieved.
- harness self-test that an invalid query returns typed parse error.
- harness self-test that missing shard returns typed unavailable.

## E2E plan

This is the parent harness for:

- `E2E-01`
- `E2E-02`
- `E2E-03`
- `E2E-04`
- `E2E-05`
- `E2E-06`
- `E2E-07`

## DoD

- at least one passing E2E proves write -> seal -> open -> query.
- expected-failing rows are machine-readable and linked to tickets.
- no E2E scenario can bypass storage by injecting result candidates directly.
- all later E2E tickets use this harness or explain why a lower-level harness
  is required.

## Failure modes

- building fake result lists instead of indexes.
- querying before generation activation.
- using parser/lowering tests as E2E proof.
