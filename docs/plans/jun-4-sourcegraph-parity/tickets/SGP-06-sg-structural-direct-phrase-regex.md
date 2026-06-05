# SGP-06 SG Structural Direct Phrase Regex

Parent RFC: [../rfc.md](../rfc.md)

Status: `landed`

## Objective

Close the direct SG structural lexical sibling gap for:

- `Phrase`
- `Regex`

## Current Source Truth

- current SG structural legality:
  - `Phrase -> LowerPhraseBody`
  - `Regex -> LowerRegexBody`
- local analysis currently classifies both as explicit unsupported direct surfaces
- the same Sourcegraph tokens are already the pure structural body syntax:
  - quoted `"..."`
  - regex `/.../`
- therefore a global `PreserveLexical` flip would change existing pure structural meaning, not just widen a mixed-domain subset

## Files Touched

- `crates/quanta-index-search-plane/src/lowering.rs`
- `tools/benchmark/sourcegraph_parity.py`
- `tools/benchmark/SOURCEGRAPH_PARITY.md`
- `docs/analysis/jun-4-dsl-capabilty.md`

## Final Verdict

- kept as explicit unsupported direct surfaces
- no new escape syntax in this packet
- quoted `"...“` and `/.../` remain structural body syntax on the SG route

## DoD

- no ambiguous middle state remains for direct SG structural phrase/regex
- parity guard carries explicit unsupported direct-surface inventory
- capability doc says unsupported, not partial/parity

## Landed Evidence

- owner-local:
  - `sourcegraph_structural_route_rewrites_single_pattern_body_into_structural_leaf`
  - `sourcegraph_structural_route_rewrites_regex_body_into_structural_leaf`
- guard/docs:
  - `tools/benchmark/sourcegraph_parity.py --check`
  - `docs/analysis/jun-4-dsl-capabilty.md`
