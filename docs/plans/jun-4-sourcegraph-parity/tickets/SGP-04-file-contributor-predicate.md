# SGP-04 File Contributor Predicate

Parent RFC: [../rfc.md](../rfc.md)

Status: `planned`

## Objective

Implement:

- `file:has.contributor(...)`

with file-level contributor authority.

## Current Source Truth

- no registry/bridge/runtime owner exists
- contributor search is separate from ownership and should not piggyback on it
- history ingest already carries raw contributor-adjacent material:
  - commit author/committer in `HistoryBatch`
  - diff hunks per file
- but no file -> contributor materialization exists today
- this ticket is authority-blocked until a producer-side file-contributor aggregation owner is chosen

## Files To Touch

- `crates/quanta-index-lq-bridge/src/translator.rs`
- `crates/quanta-index-search-plane/src/query_dispatcher.rs` once contributor authority exists
- `crates/quanta-index-sdk/src/history.rs`
- producer-side contributor ingestion/materialization owner
- `crates/quanta-index-searchd-runtime/tests/e2e_filter_execution.rs`
- `crates/quanta-index-searchd-runtime/tests/e2e_dual_syntax_lowering_parity.rs`
- `crates/quanta-index-searchd-runtime/tests/fixtures/lexical_corpus/runtime_rows.toml`

## Concrete First Increment

Support one contributor identity form first:

1. exact email or exact normalized username regex

Do not mix email aliasing, fuzzy matching, and code-host identity mapping in the first PR.

## Implementation Steps

1. define contributor authority contract
2. add one positive and one miss oracle
3. add SG/native parity

## Red Rail First

- `e2e_filter_execution`
- `e2e_dual_syntax_lowering_parity`

## DoD

- contributor filter is backed by real file-level authority

## Not Done If

- implementation silently degrades to repo-level author search
- search-plane shells out to git or blame, or reads source history outside producer-owned ingest
