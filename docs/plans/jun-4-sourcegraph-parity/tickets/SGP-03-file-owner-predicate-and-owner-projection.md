# SGP-03 File Owner Predicate And Owner Projection

Parent RFC: [../rfc.md](../rfc.md)

Status: `planned`

## Objective

Implement:

- `file:has.owner(...)`
- `file:has.owner()`
- `-file:has.owner()`
- `select:file.owners`

with a real ownership authority, not a parser-only placeholder.

## Current Source Truth

- no ownership predicate family exists in current registry
- no ownership result projection exists in current `select:` contract
- Sourcegraph docs treat ownership as both query input and result projection
- query-side filter and projection-side result contract may land in different owner seams
- repo-map `owner_path` / `RepoMapNodeRef` surfaces are structural ownership, not people ownership
  - reusing repo-map ownership as if it were CODEOWNERS / person ownership is incorrect
- no people-ownership ingest/materialization owner is visible in the current search-plane tree
- this ticket is authority-blocked until producer-side people-ownership authority is chosen

## Files To Touch

- `crates/quanta-index-lq-bridge/src/translator.rs`
- `crates/quanta-index-search-plane/src/query_dispatcher.rs` once ownership authority exists
- `crates/quanta-index-contract` if query/projection DTO changes are required
- producer-side people-ownership ingestion/materialization owner
- `crates/quanta-index-searchd-runtime/tests/sdk_frontdoor.rs`
- `crates/quanta-index-searchd-runtime/tests/e2e_filter_execution.rs`
- `crates/quanta-index-searchd-runtime/tests/fixtures/lexical_corpus/runtime_rows.toml`

## Concrete First Increment

Land query-side filtering before result projection:

1. `file:has.owner(alice@example.com)`
2. `file:has.owner()`
3. `-file:has.owner()`

Only after that, add `select:file.owners`.
If projection needs a distinct contract/result-shape change, split it into a
second PR inside this ticket.

## Implementation Steps

1. define ownership authority source and normalization
2. land query-side owner filter exact rows
3. add empty-arg any-owner / no-owner semantics
4. add owner projection contract and explicit result shape

## Red Rail First

- shared front-door exact scenario for owner filter
- dedicated projection scenario for `select:file.owners`

## DoD

- query-side filter and projection-side contract are both explicit

## Not Done If

- owner filter works only for one identity spelling
- projection reuses file candidate contract without an explicit owner result contract
- the ticket claims complete ownership parity while only query-side filtering is landed
- implementation reuses repo-map `owner_path` / `RepoMapNodeRef` as if they were person ownership authority
