# SGP-06 SG Structural Direct Phrase Regex

Parent RFC: [../rfc.md](../rfc.md)

Status: `planned`

## Objective

Resolve the direct SG structural lexical sibling gap for:

- `Phrase`
- `Regex`

## Current Source Truth

- current SG structural legality:
  - `Phrase -> LowerPhraseBody`
  - `Regex -> LowerRegexBody`
- local analysis currently classifies both as explicit unsupported direct surfaces

## Files To Touch

- `crates/quanta-index-search-plane/src/lowering.rs`
- `crates/quanta-index-searchd-runtime/tests/e2e_dual_syntax_lowering_parity.rs`
- `docs/analysis/jun-4-dsl-capabilty.md`

## Required Decision

Exactly one per surface:

1. preserve as lexical sibling with exact parity
2. keep as explicit unsupported and strengthen that demotion

## Concrete First Increment

Handle `Phrase` first.

Do not touch `Regex` until `Phrase` has an exact parity or explicit demotion rail.

## Red Rail First

- `./scripts/cargow test -p quanta-index-search-plane --lib -- --nocapture`
- `./scripts/cargow test -p quanta-index-searchd-runtime --test e2e_dual_syntax_lowering_parity -- --nocapture`

## DoD

- no ambiguous middle state remains for direct SG structural phrase/regex

## Not Done If

- docs say parity while code still rewrites to structural body semantics
