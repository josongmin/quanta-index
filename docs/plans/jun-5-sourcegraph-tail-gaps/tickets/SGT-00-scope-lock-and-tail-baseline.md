# SGT-00 Scope Lock And Tail Baseline

> Archive status: `Historical program record`. Current architecture: [JUN-06-001](../../../adr/JUN-06-001-sourcegraph-compatibility-boundary.md). Archive map: [Completed Plan Archive](../../ARCHIVE-INDEX.md).


Parent RFC: [../rfc.md](../rfc.md)

Status: `done`

## Goal

Freeze the exact post-`jun-4` backlog so the next packet does not accidentally reopen landed cells.

## Current Code Fact

- `jun-4-sourcegraph-parity` is landed
- current tree already executes:
  - `repo:has.topic(...)`
  - `file:has.owner(...)`
  - `file:has.contributor(...)`
  - `select:file.owners`
  - `rev:at.time(...)`
- current intentional unsupported set still includes SG structural direct lexical `Phrase` / `Regex` sibling and SG structural mixed non-repo predicate sibling

## Official Sourcegraph Baseline

- Sourcegraph docs still advertise built-in repo/file predicate surfaces beyond the current shipped subset
- this ticket owns the comparison cut, not implementation

## Owner Seam

- `docs/analysis/jun-4-dsl-capabilty.md`
- `docs/plans/jun-4-sourcegraph-parity/`
- `docs/plans/jun-5-sourcegraph-tail-gaps/`

## Source Truth Anchors

- `/Users/songmin/Documents/code-new/quanta-index/docs/analysis/jun-4-dsl-capabilty.md`
- `/Users/songmin/Documents/code-new/quanta-index/docs/plans/jun-4-sourcegraph-parity/rfc.md`
- `/Users/songmin/Documents/code-new/quanta-index/docs/plans/jun-4-sourcegraph-parity/tickets/INDEX.md`

## First Increment

Define three buckets only:

1. done, not backlog
2. explicit unsupported
3. real tail gap

## Red Rail To Pin First

- `rg -n "repo:has.topic|file:has.owner|file:has.contributor|select:file.owners|rev:at.time" docs/analysis docs/plans`

## Worker First Commands

```bash
rg -n "repo:has.topic|file:has.owner|file:has.contributor|select:file.owners|rev:at.time" docs/analysis docs/plans
rg -n "repo:contains.file|repo:contains.path|repo:has.description|repo:has.meta\\(key\\)|repo:has.meta\\(tag:|file:has.contributor|structural direct lexical|mixed non-repo predicate sibling" docs/analysis docs/plans/jun-5-sourcegraph-tail-gaps
```

## DoD

- done surfaces are not listed as backlog
- unsupported structural cells stay explicit
- every remaining backlog cell is owned by exactly one `SGT-*` ticket

## Not Done If

- a landed `jun-4` surface is still described as an open parity gap
- the same remaining gap appears in multiple tickets with conflicting scope
- the packet lacks a single worker entrypoint
