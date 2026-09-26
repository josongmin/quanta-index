# SGT-06 SG Structural Mixed Non-Repo Predicate Siblings

> Archive status: `Historical program record`. Current architecture: [JUN-06-001](../../../adr/JUN-06-001-sourcegraph-compatibility-boundary.md). Archive map: [Completed Plan Archive](../../ARCHIVE-INDEX.md).


Parent RFC: [../rfc.md](../rfc.md)

Status: `done`

## Goal

Close the SG structural mixed non-repo predicate sibling gap:

- `file.contains(path|file:...)`
- `file.has.content(path|file:...)`
- `symbol.has.name(...)`

## Current Code Fact

- preserve-only widening was attempted and rolled back
- current state is explicit demotion
- runtime/front-door/parity did not close from lowering preserve alone

## Official Sourcegraph Baseline

- current official docs do not grant the repo-gate-only subset as proof for these non-repo sibling cells

## Owner Seam

- structural route candidate-level lexical evaluation seam
- not a lowering-only patch

## Source Truth Anchors

- `/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-search-plane/src/lowering.rs`
- `/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-search-plane/src/query_dispatcher.rs`
- `/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-searchd-runtime/tests/common/frontdoor_scenarios.rs`
- `/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-searchd-runtime/tests/e2e_dual_syntax_lowering_parity.rs`
- `/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-searchd-runtime/tests/fixtures/lexical_corpus/runtime_rows.toml`

## First Increment

Freeze the rolled-back truth:

1. current demotion is intentional
2. repo-gate shipped subset must not be generalized
3. candidate-level evaluation seam is the minimum real support owner

## Red Rail To Pin First

- existing owner-local/runtime/front-door demotion rails for the three families

## Worker First Commands

```bash
rg -n "BridgeTranslateFail|file.contains\\(|file.has.content\\(|symbol.has.name\\(|structural" crates docs tools -S
```

## No-Go

- do not reuse repo-gate proof as evidence for non-repo siblings
- do not try preserve-only widening again without a new candidate-level evaluation seam

## DoD

- the ticket makes explicit that support requires a new structural execution seam
- preserve-only widening is ruled out
- repo-gate subset proof is not reused as evidence for these families

## Not Done If

- the ticket still frames this as a translator tweak
- support is claimed without candidate-level lexical evaluation proof
- the rollback history is not preserved in the ticket
