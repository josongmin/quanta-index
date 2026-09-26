# EXT-06 SG Structural Predicate Matrix

> Archive status: `Historical program record`. Current architecture: [JUN-02-001](../../../adr/JUN-02-001-search-dsl-authority-and-runtime-contract.md), [JUN-06-001](../../../adr/JUN-06-001-sourcegraph-compatibility-boundary.md). Archive map: [Completed Plan Archive](../../ARCHIVE-INDEX.md).


Parent RFC: [../rfc.md](../rfc.md)

Status: `landed`

## Landed Result

Shipped SG structural predicate sibling subset은 repo gate family로 고정됐다.

Supported exact subset:

- `repo.has.file`
- `repo.has.path`
- `repo.has.content`
- `repo.contains.content`

Contexts:

- `AND`
- `OR`
- `AND NOT`

Non-repo predicate siblings는 explicit typed-fail로 닫혔다.

## Objective

Expand SG structural predicate sibling proof beyond the current narrow proved subset.

## Current Source Truth

- current legality table preserves `Predicate` leaves:
  - `crates/quanta-index-search-plane/src/lowering.rs`
- current green subset is narrow:
  - `repo.has.file(...)` mixed with structural body
- wider predicate family is not fully proved:
  - `repo.has.content(...)`
  - `repo.has.path(...)`
  - `repo.contains.content(...)`

## Files To Touch

- `crates/quanta-index-search-plane/src/lowering.rs` only if legality cells change
- `crates/quanta-index-searchd-runtime/tests/e2e_dual_syntax_lowering_parity.rs`
- `crates/quanta-index-searchd-runtime/tests/fixtures/lexical_corpus/runtime_rows.toml`
- optionally `crates/quanta-index-searchd-runtime/tests/e2e_filter_execution.rs`
- `docs/analysis/jun-4-dsl-capabilty.md`

## Concrete First Increment

Widen one family at a time:

1. `repo.has.content(...) AND structural`
2. `repo.has.path(...) AND structural`
3. `repo.contains.content(...) AND structural`
4. only then `OR`
5. only then `AND NOT`

## Implementation Steps

1. add one new predicate sibling family
2. add hit/miss parity row
3. add runtime row
4. leave all remaining cells explicit

## Red Rail First

- `./scripts/cargow test -p quanta-index-searchd-runtime --test e2e_dual_syntax_lowering_parity -- --nocapture`

## DoD

- every widened predicate sibling family has explicit exact proof
- still-unsupported sibling/context cells typed-fail clearly

## Not Done If

- one green family is used to overclaim the entire predicate matrix
