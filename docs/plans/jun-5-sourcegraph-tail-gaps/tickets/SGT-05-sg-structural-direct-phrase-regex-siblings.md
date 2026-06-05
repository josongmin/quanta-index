# SGT-05 SG Structural Direct Phrase Regex Siblings

Parent RFC: [../rfc.md](../rfc.md)

Status: `done`

## Goal

Close SG structural direct lexical `Phrase` and `Regex` sibling gaps with a real grammar decision.

## Current Code Fact

- direct SG structural lexical `Phrase` sibling does not exist as a shipped surface
- direct SG structural lexical `Regex` sibling does not exist as a shipped surface
- quoted phrase and `/.../` are already structural body syntax on the SG route

## Official Sourcegraph Baseline

- Sourcegraph structural search is a distinct route, but current quanta-index SG structural route does not expose direct lexical sibling forms for these cells

## Owner Seam

- SG grammar / bridge / lowering boundary
- not just runtime widening

## Source Truth Anchors

- `/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-search-plane/src/lowering.rs`
- `/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-lq-bridge/src/translator.rs`
- `/Users/songmin/Documents/code-new/quanta-index/docs/analysis/jun-4-dsl-capabilty.md`

## First Increment

Document the current direct-surface absence and decide whether the packet targets:

1. new explicit escape surface, or
2. permanent explicit unsupported verdict

## Red Rail To Pin First

- current SG structural typed-fail / demotion witness for direct lexical phrase/regex intent

## Worker First Commands

```bash
rg -n "LowerPhraseBody|LowerRegexBody|PreserveLexical|structural.*Phrase|structural.*Regex" crates docs tools -S
```

## No-Go

- do not frame this as a simple lowering-only widen
- do not create a hidden escape surface without documenting the grammar decision

## DoD

- there is one unambiguous grammar-level verdict for both cells
- preserve-only widening without surface disambiguation is prohibited

## Not Done If

- the ticket implies simple lowering widening is enough
- phrase/regex direct cells remain ambiguous between lexical sibling and structural body semantics
- the packet lacks a decision between new explicit surface and permanent demotion
