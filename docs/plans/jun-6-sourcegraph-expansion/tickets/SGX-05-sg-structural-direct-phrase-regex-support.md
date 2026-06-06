# SGX-05 — SG Structural Direct Phrase And Regex Support

Parent RFC: [../rfc.md](../rfc.md)

Status: `landed`

## Goal

Close SG structural direct lexical `Phrase` / `Regex` siblings as permanent
explicit unsupported because the current SG route has no unambiguous sibling
surface.

## Current Code Fact

- direct SG structural lexical sibling support does not exist
- quoted phrase and `/.../` already participate in structural body syntax
- current verdict is permanent demotion

## Official Sourcegraph Baseline

- Sourcegraph structural search uses quoted and slash-delimited syntax inside
  structural patterns, but the current route has no distinct direct lexical
  sibling slot separate from structural body semantics
- Example:
  - `patterntype:structural "foo bar"` is a structural body pattern, not a
    lexical phrase sibling
  - `patterntype:structural /foo.*/` is lowered as a structural regex body, not
    a lexical regex sibling

## Owner Seam

- SG syntax parser
- bridge translator
- structural lowering
- structural dispatch semantics

## Source Truth Anchors

- `/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-lq-bridge/src/syntax.rs`
- `/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-lq-bridge/src/translator.rs`
- `/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-search-plane/src/lowering.rs`
- `/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-search-plane/src/query_dispatcher.rs`
- `/Users/songmin/Documents/code-new/quanta-index/tools/benchmark/sourcegraph_parity.py`

## Concrete Work Items

1. Freeze the current permanent demotion witness for direct `Phrase` and
   `Regex` siblings.
2. Reaffirm that no distinct SG-compatible sibling slot exists on the current
   route.
3. Keep guard inventory and docs on explicit unsupported verdicts.
4. State explicitly that this witness lives in lowering, not runtime hellgate,
   because there is no SG query surface that means “direct lexical phrase
   sibling” or “direct lexical regex sibling” on the current grammar.

## First Increment

- pin the current demotion witness
- keep the permanent unsupported verdict explicit in docs and guard inventory

## Red Rail To Pin First

```bash
./scripts/cargow test -p quanta-index-search-plane --lib -- --nocapture
python3 tools/benchmark/sourcegraph_parity.py --check
```

## Worker First Commands

```bash
rg -n "LowerPhraseBody|LowerRegexBody|patterntype:structural|Phrase|Regex" crates docs tools -S
sed -n '220,280p' crates/quanta-index-search-plane/src/lowering.rs
sed -n '780,835p' crates/quanta-index-lq-bridge/src/syntax.rs
```

## No-Go

- do not call a grammar collision “supported with caveats”
- do not ship a direct sibling claim if runtime still interprets the same syntax
  as structural body
- do not change only docs or translator names without a real route split

## DoD

- permanent unsupported is reaffirmed without ambiguity
- guard, capability docs, and packet docs no longer advertise this as open
  future widening work

## Not Done If

- docs still advertise this as planned support work
- guard stops tracking the explicit unsupported witness
