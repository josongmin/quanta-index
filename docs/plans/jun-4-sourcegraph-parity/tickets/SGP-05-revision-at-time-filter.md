# SGP-05 Revision At Time Filter

Parent RFC: [../rfc.md](../rfc.md)

Status: `planned`

## Objective

Implement:

- `rev:at.time(...)`

as a real revision-time filter surface.

## Current Source Truth

- current bridge/runtime inventory has plain `rev:` but not revision-at-time resolution
- existing history substrate is richer than the old framing implied:
  - `validate_history_timeref_filters(...)`
  - `parse_history_timeref_ms(...)`
  - `resolve_history_since_timeref_ms(...)`
  - refs/tags and `CommitRecord::committer_time_ms` are already materialized
- but lexical text dispatch still rejects `rev:` outright before execution
  - current text route has no revision-selection / pin rebinding surface
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

## Concrete First Increment

Support one RFC3339 timestamp form first, mapped onto the existing history timeref substrate **after** text dispatch gains revision-selection semantics for `rev:`.

Do not start with natural-language timeref parsing.

## Implementation Steps

1. define how text dispatch resolves `rev:at.time(...)` onto an anchor revision and generation pin
2. reuse existing history timeref parsing for the boundary itself
3. land one exact timestamp positive row
4. land one miss row and one invalid-time typed-fail row

## Red Rail First

- exact runtime rows for positive / miss / invalid

## DoD

- `rev:at.time(...)` is not merely accepted syntax
- `rev:` no longer remains fail-closed on the exact text route cell this ticket claims

## Not Done If

- the filter is normalized away into plain `rev:`
- the ticket lands only bridge parsing without revision-selection semantics
- the implementation rebuilds a new timeref parser instead of reusing the existing history timeref substrate
- implementation interprets `at.time(...)` after lexical execution instead of selecting the executable revision/pin up front
