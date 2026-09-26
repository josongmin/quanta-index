# EXT-05 SG Structural Phrase Regex Verdict

> Archive status: `Historical program record`. Current architecture: [JUN-02-001](../../../adr/JUN-02-001-search-dsl-authority-and-runtime-contract.md), [JUN-06-001](../../../adr/JUN-06-001-sourcegraph-compatibility-boundary.md). Archive map: [Completed Plan Archive](../../ARCHIVE-INDEX.md).


Parent RFC: [../rfc.md](../rfc.md)

Status: `landed`

## Landed Result

직접 lexical sibling surface를 `지원됨`으로 넓히지 않았다.

- SG structural route의 quoted `Phrase`와 `/Regex/`는 계속 structural body semantics다
- direct lexical sibling surface는 explicit unsupported로 inventory에 고정했다
- 대체 shipped surface는 lexical text route 또는 `content:` / `file.has.content(...)`

## Objective

Remove the current semantic mismatch for SG structural lexical siblings:

- `Phrase`
- `Regex`

## Current Source Truth

- current SG structural legality:
  - `crates/quanta-index-search-plane/src/lowering.rs`
  - `Phrase -> LowerPhraseBody`
  - `Regex -> LowerRegexBody`
- native structural execution treats lexical siblings as lexical subqueries:
  - `crates/quanta-index-search-plane/src/query_dispatcher.rs`

## Required Decision

Exactly one:

1. preserve `Phrase` / `Regex` as lexical siblings on SG structural route
2. explicit typed-fail them on SG structural route

Current mixed state is not allowed to remain.

## Files To Touch

- `crates/quanta-index-search-plane/src/lowering.rs`
- `crates/quanta-index-searchd-runtime/tests/e2e_dual_syntax_lowering_parity.rs`
- optionally `crates/quanta-index-lq-bridge/tests/golden_bridge.rs`
- `docs/analysis/jun-4-dsl-capabilty.md`

## Concrete First Increment

Pick `Phrase` first. Do not touch `Regex` in the same first PR.

## Red Rail First

- `./scripts/cargow test -p quanta-index-search-plane --lib -- --nocapture`
- `./scripts/cargow test -p quanta-index-searchd-runtime --test e2e_dual_syntax_lowering_parity -- --nocapture`

## DoD

- SG structural `Phrase` and `Regex` each end in either exact parity or explicit typed-fail

## Not Done If

- SG still rewrites them structurally while docs describe them as lexical-equivalent
