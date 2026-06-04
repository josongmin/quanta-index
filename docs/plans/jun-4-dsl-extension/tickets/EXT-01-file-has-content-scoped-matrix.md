# EXT-01 File Has Content Scoped Matrix

Parent RFC: [../rfc.md](../rfc.md)

Status: `landed`

## Landed Result

- `file.has.content(path:..., <scalar>)`
- `file.has.content(file:..., <scalar>)`

둘 다 exact runtime/front-door/parity row를 확보했고 capability inventory에서 `지원됨`으로 승격됐다.

## Objective

Promote:

- `file.has.content(path:..., <scalar>)`
- `file.has.content(file:..., <scalar>)`

from `부분 지원` to `지원됨`.

## Current Source Truth

- owner seam:
  - `crates/quanta-index-lexical/src/predicate_registry.rs`
  - `crates/quanta-index-lexical/src/lib.rs`
- current exact rails:
  - `file.has.content(lang:rust, /.../)` is green
  - `file.has.content(path:...)` / `file.has.content(file:...)` exact rows were not found in current runtime/front-door/parity inventory
- proof rails:
  - `crates/quanta-index-searchd-runtime/tests/e2e_filter_execution.rs`
  - `crates/quanta-index-searchd-runtime/tests/e2e_dual_syntax_lowering_parity.rs`
  - `crates/quanta-index-searchd-runtime/tests/fixtures/lexical_corpus/runtime_rows.toml`

## Files To Touch

- `crates/quanta-index-searchd-runtime/tests/e2e_filter_execution.rs`
- `crates/quanta-index-searchd-runtime/tests/e2e_dual_syntax_lowering_parity.rs`
- `crates/quanta-index-searchd-runtime/tests/fixtures/lexical_corpus/runtime_rows.toml`
- `docs/analysis/jun-4-dsl-capabilty.md`

## Concrete First Increment

Add exactly these cells first:

1. `file:has.content(path:docs/colors.md, "lemon yellow banana")`
2. `file:has.content(file:colors.md, "lemon yellow banana")`

Each needs positive and miss or exclusion evidence.

## Implementation Steps

1. add fixture rows whose result set changes if the scope is ignored
2. add SG front-door execution tests for `path` and `file`
3. add SG/native parity rows for the same exact shapes
4. only after green, move the two cells to `지원됨`

## Red Rail First

- `./scripts/cargow test -p quanta-index-searchd-runtime --test e2e_filter_execution -- --nocapture`
- `./scripts/cargow test -p quanta-index-searchd-runtime --test e2e_dual_syntax_lowering_parity -- --nocapture`

## DoD

- runtime rows exist for both `path` and `file` variants
- front-door exact execution rail exists for both
- native↔SG parity exists for both

## Not Done If

- only one of `path` or `file` is proven
- scope-ignored behavior could still pass the oracle
