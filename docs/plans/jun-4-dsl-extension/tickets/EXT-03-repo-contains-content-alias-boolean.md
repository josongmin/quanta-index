# EXT-03 Repo Contains Content Alias Boolean

Parent RFC: [../rfc.md](../rfc.md)

Status: `landed`

## Landed Result

- `repo:contains.content("...") OR ...`
- `... NOT repo:contains.content("...")`

alias boolean exact execution/parity row가 landed했다.

## Objective

Promote alias boolean shapes:

- `repo:contains.content("...") OR ...`
- `... NOT repo:contains.content("...")`

from `부분 지원` to `지원됨`.

## Current Source Truth

- owner seam:
  - `crates/quanta-index-lexical/src/predicate_registry.rs`
  - alias rewrite to canonical `repo.has.content`
- current proof:
  - alias positive surface exists
  - canonical `repo.has.content(...) OR/NOT` is proved
  - alias boolean exact rails are not present in the current inventory

## Files To Touch

- `crates/quanta-index-searchd-runtime/tests/e2e_filter_execution.rs`
- `crates/quanta-index-searchd-runtime/tests/e2e_dual_syntax_lowering_parity.rs`
- `crates/quanta-index-searchd-runtime/tests/fixtures/lexical_corpus/runtime_rows.toml`
- `docs/analysis/jun-4-dsl-capabilty.md`

## Concrete First Increment

Add exactly two rows:

1. `repo:contains.content("gate-a only") OR missing_corpus_token`
2. `shared_oracle_needle NOT repo:contains.content("gate-a only")`

## Implementation Steps

1. add exact SG execution tests for alias boolean shapes
2. add alias↔canonical parity rows
3. add runtime rows if shared inventory should carry the alias shape directly

## Red Rail First

- `./scripts/cargow test -p quanta-index-searchd-runtime --test e2e_filter_execution -- --nocapture`

## DoD

- alias boolean rows are exact, not inferred from canonical rows
- alias→canonical rewrite still preserves candidate set

## Not Done If

- only canonical boolean rails are green
