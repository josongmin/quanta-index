# SGP-05 Revision At Time Filter

Parent RFC: [../rfc.md](../rfc.md)

Status: `landed`

## Objective

Implement:

- `rev:at.time(...)`

as a real revision-time filter surface.

## Current Source Truth

- current bridge/runtime inventory has plain `rev:` plus revision-at-time resolution on the text route
- existing history substrate is richer than the old framing implied:
  - `validate_history_timeref_filters(...)`
  - `parse_history_timeref_ms(...)`
  - `resolve_history_since_timeref_ms(...)`
  - refs/tags and `CommitRecord::committer_time_ms` are already materialized
- lexical text dispatch now resolves `rev:at.time(...)` before lexical execution:
  - explicit `rev:<ref|tag|sha>` remains the anchor when present
  - otherwise the selected pin revision or materialized `HEAD` ref anchors the history walk
  - the dispatcher selects the latest reachable commit whose `committer_time_ms <= boundary`
  - lexical execution then rebinds to the activated lexical generation for that revision
- Sourcegraph docs expose `rev:at.time(...)` as a distinct surface
- this is a revision-selection/history owner problem, not a lexical predicate problem

## Files To Touch

- `crates/quanta-index-lq-bridge/src/translator.rs`
- `crates/quanta-index-search-plane/src/query_dispatcher.rs`
- activation / pin-selection owner chosen for text dispatch resolution
- `crates/quanta-index-sdk/src/history.rs` only if a fixture/contract gap appears during time-resolution proof
- history or revision-selection owner chosen for time resolution
- `crates/quanta-index-searchd-runtime/tests/e2e_filter_execution.rs`
- `crates/quanta-index-searchd-runtime/tests/fixtures/lexical_corpus/runtime_rows.toml`

## Current Increment

Current tree has landed the executable increment:

1. detect `rev:at.time(...)` shape on lexical/text route
2. resolve the boundary with the shared timeref parser
3. walk the materialized history DAG from the anchor commit
4. rebind lexical execution to the selected revision pin
5. pin this with owner-local, targeted runtime, and SDK/front-door rails

## Implementation Steps

1. define how text dispatch resolves `rev:at.time(...)` onto an anchor revision and generation pin
2. reuse existing history timeref parsing for the boundary itself
3. land exact timestamp, named-date, and human-relative runtime rows
4. preserve typed-fail on invalid timeref and fail-closed activation gaps

## Red Rail First

- owner-local dispatcher red rail is closed
- targeted runtime/front-door red rails are closed
- shared inventory followthrough moves to `SGP-08`

## DoD

- `rev:at.time(...)` is not merely accepted syntax
- `rev:` no longer remains generic/opaque fail-closed on the exact text route cell this ticket claims
- human timeref support (`yesterday`, `june 25 2017`, `1 year ago`) is executable on the Sourcegraph text route
- invalid timeref still fails closed with `HISTORY_INVALID_TIMEREF`

## Not Done If

- the filter is normalized away into plain `rev:`
- the ticket lands only bridge parsing without revision-selection semantics
- the implementation rebuilds a new timeref parser instead of reusing the existing history timeref substrate
- implementation interprets `at.time(...)` after lexical execution instead of selecting the executable revision/pin up front
