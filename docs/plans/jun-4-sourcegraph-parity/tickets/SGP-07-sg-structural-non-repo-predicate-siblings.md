# SGP-07 SG Structural Non-Repo Predicate Siblings

Parent RFC: [../rfc.md](../rfc.md)

Status: `planned`

## Objective

Resolve SG structural mixed predicate gaps outside the shipped repo-gate family.

Primary candidates:

- `file.contains(...)`
- `file.has.content(...)`
- `symbol.has.name(...)`

## Current Source Truth

- SG structural route preserves only repo gate predicates:
  - `repo.has.file`
  - `repo.has.path`
  - `repo.has.content`
  - `repo.contains.content`
- non-repo predicate sibling is typed `BridgeTranslateFail`
- native structural evaluator has a generic lexical-leaf path, but shared exact inventory for non-repo families is absent

## Files To Touch

- `crates/quanta-index-search-plane/src/lowering.rs`
- `crates/quanta-index-search-plane/src/query_dispatcher.rs`
- `crates/quanta-index-searchd-runtime/tests/e2e_dual_syntax_lowering_parity.rs`
- `crates/quanta-index-searchd-runtime/tests/fixtures/lexical_corpus/runtime_rows.toml`
- `docs/analysis/jun-4-dsl-capabilty.md`

## Concrete First Increment

Pick one family first:

1. `file.contains(...)`

Do not mix `file.has.content(...)` and `symbol.has.name(...)` into the same first PR.

## Implementation Steps

1. prove native structural mixed exact row for the chosen family
2. decide SG preserve vs explicit unsupported
3. add parity or stronger demotion rail

## Red Rail First

- new exact native structural mixed row for the chosen family
- SG parity or SG typed-fail rail for the same family

## DoD

- each non-repo family is either exact green or explicit unsupported

## Not Done If

- native generic code path is mistaken for shipped exact support
