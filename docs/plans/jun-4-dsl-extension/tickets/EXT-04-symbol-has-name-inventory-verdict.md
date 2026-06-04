# EXT-04 Symbol Has Name Inventory Verdict

Parent RFC: [../rfc.md](../rfc.md)

Status: `landed`

## Landed Result

`symbol.has.name(...)`는 shared shipped DSL inventory로 승격됐다.

- native canonical: `symbol.has.name(...)`
- Sourcegraph surface: `symbol:has.name(...)`
- shared runtime/front-door/parity inventory에 모두 반영됨

## Objective

Make `symbol.has.name(...)` honest in the capability inventory.

Allowed end states:

- promote it to shipped shared DSL inventory
- or classify it as a separate owner-route surface, not `부분 지원`

## Current Source Truth

- planner seam exists:
  - `crates/quanta-index-lexical/src/planner.rs`
- registry explicitly excludes it:
  - `crates/quanta-index-lexical/src/predicate_registry.rs`
- current shared capability inventory does not include it as a shipped DSL row

## Files To Touch

- `crates/quanta-index-lexical/src/planner.rs` only if behavior changes
- `crates/quanta-index-searchd-runtime/tests/common/frontdoor_scenarios.rs`
- `crates/quanta-index-searchd-runtime/tests/fixtures/lexical_corpus/runtime_rows.toml`
- `crates/quanta-index-searchd-harness/src/scenarios.rs`
- `docs/analysis/jun-4-dsl-capabilty.md`

## Decision Rule

Promote only if all are true:

1. product scope wants it in the shipped DSL surface
2. runtime/front-door rails can assert it directly
3. benchmark/scenario inventory should treat it as part of the common DSL path

Otherwise demote it to:

- `별도 route`
- or `미지원 DSL inventory cell`

## Red Rail First

- planner unit around `predicate_symbol_has_name_plans_through_symbol_route`
- inventory search:
  - `rg -n "symbol.has.name|select:symbol|type:symbol" crates/quanta-index-searchd-runtime/tests crates/quanta-index-searchd-harness/src/scenarios.rs`

## DoD

- one final classification only
- no lingering `부분 지원`

## Not Done If

- the doc still says `부분 지원` for `symbol.has.name(...)`
