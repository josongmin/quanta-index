# SGX-06 — SG Structural Mixed Non-Repo Predicate Support

Parent RFC: [../rfc.md](../rfc.md)

Status: `landed`

## Goal

Land SG structural mixed non-repo predicate siblings as real supported
surfaces.

## Current Code Fact

- current mixed non-repo siblings are executable on the live tree
- file-level siblings run on the current structural route
- `symbol.has.name(...)` projects same-path, line-overlapping symbol hits into
  all matching structural chunks with deterministic union

## Official Sourcegraph Baseline

- Sourcegraph users expect richer structural composition than our old
  repo-gate-only subset

## Owner Seam

- structural candidate execution path in `query_dispatcher.rs`
- structural lowering only as admission, not as proof

## Source Truth Anchors

- `/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-search-plane/src/lowering.rs`
- `/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-search-plane/src/query_dispatcher.rs`
- `/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-searchd-runtime/tests/common/frontdoor_scenarios.rs`
- `/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-searchd-runtime/tests/e2e_dual_syntax_lowering_parity.rs`
- `/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-searchd-runtime/tests/fixtures/lexical_corpus/runtime_rows.toml`

## Concrete Work Items

1. Admit SG structural lowering for:
   - `file.contains(path|file:...)`
   - `file.has.content(path|file:...)`
   - `symbol.has.name(...)`
2. Keep boolean context support exact:
   - `AND`
   - `OR`
   - `AND NOT`
3. Prove runtime/front-door/parity for all three families.
4. Keep direct lexical `Phrase` / `Regex` siblings explicit unsupported.

## First Increment

- keep all three mixed sibling families executable with exact proof
- do not regress SGX-06 back into demotion inventory

## Red Rail To Pin First

```bash
./scripts/cargow test -p quanta-index-search-plane --lib -- --nocapture
./scripts/cargow test -p quanta-index-searchd-runtime --test sdk_frontdoor -- --nocapture
./scripts/cargow test -p quanta-index-searchd-runtime --test e2e_dual_syntax_lowering_parity -- --nocapture
```

## Worker First Commands

```bash
rg -n "BridgeTranslateFail|file.contains\\(|file.has.content\\(|symbol.has.name\\(|candidate-level|structural" crates docs tools -S
sed -n '980,1080p' crates/quanta-index-search-plane/src/lowering.rs
sed -n '150,260p' crates/quanta-index-searchd-runtime/tests/common/frontdoor_scenarios.rs
```

## No-Go

- do not retry preserve-only widening
- do not widen all three families in one patch by default
- do not claim support from one boolean context only

## DoD

- support is live for all three mixed sibling families
- all boolean contexts are proven
- repo-gate subset remains correct

## Not Done If

- support is claimed from lowering preserve only
- only one boolean context is green
- runtime/front-door/parity disagree on the widened surface
- docs still describe this ticket as future backlog
