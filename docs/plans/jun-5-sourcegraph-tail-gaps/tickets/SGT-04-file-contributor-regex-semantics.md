# SGT-04 File Contributor Regex Semantics

> Archive status: `Historical program record`. Current architecture: [JUN-06-001](../../../adr/JUN-06-001-sourcegraph-compatibility-boundary.md). Archive map: [Completed Plan Archive](../../ARCHIVE-INDEX.md).


Parent RFC: [../rfc.md](../rfc.md)

Status: `done`

## Goal

Close the gap between current exact contributor matching and Sourcegraph's documented name-or-email regex semantics for `file:has.contributor(...)`.

## Current Code Fact

- current contributor authority is source-repo/path keyed
- current admitted arg is one textual contributor scalar
- current semantics are exact lowercase contributor-identity matching

## Official Sourcegraph Baseline

- Sourcegraph docs describe contributor matching as name or email regex pattern matching

## Owner Seam

- contributor authority schema
- lexical executor contributor matching seam
- runtime/front-door/parity proof rails

## Source Truth Anchors

- `/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-lexical/src/predicate_registry.rs`
- `/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-lexical/src/lib.rs`
- `/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-core/src/domains/lexical/outbound.rs`
- `/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-searchd-runtime/tests/e2e_filter_execution.rs`
- `/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-searchd-runtime/tests/e2e_dual_syntax_lowering_parity.rs`

## First Increment

Pin the current exact-string semantics as the baseline and add explicit negative truth for regex/name-email cells before any widening.

## Red Rail To Pin First

- exact current positive/miss rail
- negative rails for regex-shaped contributor inputs that Sourcegraph documents but current tree does not actually implement

## Worker First Commands

```bash
rg -n "file:has\\.contributor|parse_file_contributor_arg|FileContributor|contributor" crates docs tools -S
```

## No-Go

- do not relabel exact lowercase contributor matching as regex support
- do not fall back to repo-level author search

## DoD

- current exact contributor matching remains green
- regex/name-email semantics are either implemented with real authority changes or closed as explicit unsupported
- no repo-level author fallback appears

## Not Done If

- regex contributor semantics are described while still using exact lowercase string compare
- contributor widening degrades into repo-level author search
- current exact behavior is not separately pinned from future regex behavior
