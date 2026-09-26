# SGT-02 Repo Description Predicate

> Archive status: `Historical program record`. Current architecture: [JUN-06-001](../../../adr/JUN-06-001-sourcegraph-compatibility-boundary.md). Archive map: [Completed Plan Archive](../../ARCHIVE-INDEX.md).


Parent RFC: [../rfc.md](../rfc.md)

Status: `done`

## Goal

Decide and close `repo:has.description(regexp)`.

## Current Code Fact

- current tree has no repo-description predicate seam
- current shipped repo authorities are separate:
  - repo commit recency
  - repo metadata
  - repo topic
- description is not present as a shipped lexical predicate family

## Official Sourcegraph Baseline

- Sourcegraph docs list `repo:has.description(...)` as a built-in repo predicate
- semantics are regexp-based, not exact-string metadata reuse

## Owner Seam

- distinct repo-description authority or explicit unsupported closeout
- must not be piggybacked onto `repo:has.meta(...)`

## Source Truth Anchors

- `/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-lexical/src/predicate_registry.rs`
- `/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-lq-bridge/src/translator.rs`
- `/Users/songmin/Documents/code-new/quanta-index/docs/analysis/jun-4-dsl-capabilty.md`

## First Increment

Pin current behavior as unsupported and decide whether a dedicated authority exists or must be introduced.

## Red Rail To Pin First

- parser / bridge / runtime current-cell behavior for `repo:has.description(...)`

## Worker First Commands

```bash
rg -n "has\\.description|description predicate|RepoDescription|repo_description" crates docs tools -S
```

## No-Go

- do not route description through `repo:has.meta(...)`
- do not claim regexp semantics from exact key/value metadata substrate

## DoD

- description is either backed by a distinct authority seam or explicitly unsupported
- metadata/topic authorities are not reused by implication

## Not Done If

- description is folded into `repo:has.meta(...)`
- regexp semantics are claimed on top of an exact-string substrate without proof
- current absence of a description seam is not explicitly recorded
