# EXT-00 Scope Lock And Promotion Bar

Parent RFC: [../rfc.md](../rfc.md)

Status: `landed`

## Landed Result

- capability inventory와 RFC의 partial-cell 목록을 code truth 기준으로 동기화했다
- 모든 former partial cell은 정확히 하나의 owner ticket를 가졌고 최종적으로 `지원됨` 또는 `미지원`으로 닫혔다

## Objective

Freeze:

- which `부분 지원` cells this RFC owns
- what proof is required to promote a cell to `지원됨`
- what evidence is required to demote a cell to `미지원`

## Current Source Truth

- packet source truth:
  - [../rfc.md](../rfc.md)
  - [../../../analysis/jun-4-dsl-capabilty.md](../../../analysis/jun-4-dsl-capabilty.md)
- primary owner seams:
  - `crates/quanta-index-lexical/src/predicate_registry.rs`
  - `crates/quanta-index-lexical/src/planner.rs`
  - `crates/quanta-index-lexical/src/lib.rs`
  - `crates/quanta-index-search-plane/src/lowering.rs`
  - `crates/quanta-index-lq-bridge/src/translator.rs`

## Files To Touch

- `docs/plans/jun-4-dsl-extension/rfc.md`
- `docs/analysis/jun-4-dsl-capabilty.md`

## Implementation Steps

1. enumerate every current `부분 지원` cell exactly once
2. assign each to one ticket and one owner seam
3. freeze the promotion bar in the RFC
4. forbid any unowned partial cell

## Red Rail First

- `rg -n "부분 지원|Sourcegraph 완전 parity" docs/analysis/jun-4-dsl-capabilty.md docs/plans/jun-4-dsl-extension`

## DoD

- every partial cell has exactly one ticket owner
- no ticket mixes proof-gap work with semantic-gap work unless the owner seam is identical
- RFC promotion bar matches the capability inventory wording

## Not Done If

- any partial cell is still described only in prose
- any ticket claims promotion without naming runtime/front-door/parity rails
