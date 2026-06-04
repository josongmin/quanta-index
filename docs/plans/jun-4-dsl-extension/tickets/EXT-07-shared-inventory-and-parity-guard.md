# EXT-07 Shared Inventory And Parity Guard

Parent RFC: [../rfc.md](../rfc.md)

Status: `landed`

## Landed Result

- shared front-door inventory가 promoted surfaces를 반영하게 넓어졌다
- `sourcegraph_parity.py`가 root keyword bucket뿐 아니라 canonical surface id도 검증한다
- accepted/shipped-but-unverified surface는 `--check`에서 fail-closed 된다

## Objective

Make shared inventory and the Sourcegraph parity guard reflect the same truth as the landed code.

## Current Source Truth

- shared front-door inventory is narrower than the live executable surface:
  - `crates/quanta-index-searchd-runtime/tests/common/frontdoor_scenarios.rs`
  - `crates/quanta-index-searchd-runtime/tests/dsl_scenarios.rs`
- current SG guard is coarse:
  - `tools/benchmark/sourcegraph_parity.py`
  - root-keyword bucket, not full alias-shape or predicate-combination truth

## Files To Touch

- `crates/quanta-index-searchd-runtime/tests/common/frontdoor_scenarios.rs`
- `crates/quanta-index-searchd-runtime/tests/dsl_scenarios.rs`
- `crates/quanta-index-searchd-harness/src/scenarios.rs`
- `tools/benchmark/sourcegraph_parity.py`
- `tools/benchmark/SOURCEGRAPH_PARITY.md`
- `docs/analysis/jun-4-dsl-capabilty.md`

## Concrete First Increment

Upgrade the guard from root keyword to canonical surface ids for at least:

1. `repo.has.file`
2. `repo.has.path`
3. `repo.has.content`
4. `repo.contains.content`
5. `file.contains`
6. `file.contains.content`
7. `file.has.content`

## Implementation Steps

1. make the parity guard distinguish canonical predicate/filter surfaces
2. add or refresh shared front-door scenarios for newly promoted cells
3. keep guard fail-closed on any accepted-but-unverified surface

## Red Rail First

- `python3 tools/benchmark/sourcegraph_parity.py --check`

## DoD

- parity guard can distinguish alias-shape drift
- shared inventory no longer underreports promoted cells

## Not Done If

- guard is still green while alias/cell-level support is invisible
