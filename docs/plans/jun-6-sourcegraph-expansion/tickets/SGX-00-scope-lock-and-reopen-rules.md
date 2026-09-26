# SGX-00 — Scope Lock And Reopen Rules

> Archive status: `Historical program record`. Current architecture: [JUN-06-001](../../../adr/JUN-06-001-sourcegraph-compatibility-boundary.md). Archive map: [Completed Plan Archive](../../ARCHIVE-INDEX.md).


Parent RFC: [../rfc.md](../rfc.md)

Status: `landed`

## Goal

Freeze which cells are legitimately reopened in `jun-6` and which surfaces stay
landed truth from `jun-4` and `jun-5`.

## Current Code Fact

- `jun-4-sourcegraph-parity` and `jun-5-sourcegraph-tail-gaps` are both landed
- `jun-6` reopened list is narrower than the original packet draft
- current explicit unsupported cells are machine-checked in
  `sourcegraph_parity.py --check`

## Official Sourcegraph Baseline

- current Sourcegraph docs still describe these surfaces as query language
  features, but our tree keeps several of them explicit unsupported

## Owner Seam

- packet docs
- capability analysis
- parity guard inventory

## Source Truth Anchors

- `/Users/songmin/Documents/code-new/quanta-index/docs/analysis/jun-4-dsl-capabilty.md`
- `/Users/songmin/Documents/code-new/quanta-index/docs/plans/jun-5-sourcegraph-tail-gaps/rfc.md`
- `/Users/songmin/Documents/code-new/quanta-index/docs/plans/jun-5-sourcegraph-tail-gaps/tickets/INDEX.md`
- `/Users/songmin/Documents/code-new/quanta-index/tools/benchmark/SOURCEGRAPH_PARITY.md`
- `/Users/songmin/Documents/code-new/quanta-index/tools/benchmark/sourcegraph_parity.py`

## Concrete Work Items

1. Freeze the reopen list in one packet instead of scattering future work across
   `jun-4` and `jun-5`.
2. Remove cells that are already supported on the live tree from the reopened
   backlog.
3. Keep “future widening only” language aligned with current code truth so
   low-context readers do not reopen landed work.
4. Keep preflight residue explicit in the worker docs.

## First Increment

- trim the packet to only still-open widening cells
- keep backlinks from the landed backlog packet and capability analysis

## Red Rail To Pin First

```bash
rg -n "jun-6-sourcegraph-expansion|repo:has.meta\\(/key/:/value/\\)|file:has.contributor\\(<name-or-email regex>\\)|structural direct lexical|mixed non-repo predicate sibling" docs/analysis docs/plans
```

## Worker First Commands

```bash
sed -n '176,240p' docs/analysis/jun-4-dsl-capabilty.md
sed -n '1,220p' docs/plans/jun-5-sourcegraph-tail-gaps/rfc.md
sed -n '1,160p' docs/plans/jun-5-sourcegraph-tail-gaps/tickets/INDEX.md
```

## No-Go

- do not rewrite `jun-5` as if it still owns open work
- do not add future backlog cells that are already supported on the current tree
- do not leave already-supported cells in the reopened backlog just because an
  older ticket draft listed them

## DoD

- reopened cells are explicitly listed and only still-open cells remain
- landed packets are not rephrased as incomplete
- capability analysis links to this packet as future widening only

## Not Done If

- a landed packet is described as if it still owns the work
- a supported cell still appears in the reopened list
- current unsupported truth is weakened without code proof
