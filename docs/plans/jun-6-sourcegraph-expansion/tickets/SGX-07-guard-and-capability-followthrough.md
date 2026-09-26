# SGX-07 — Guard And Capability Followthrough

> Archive status: `Historical program record`. Current architecture: [JUN-06-001](../../../adr/JUN-06-001-sourcegraph-compatibility-boundary.md). Archive map: [Completed Plan Archive](../../ARCHIVE-INDEX.md).


Parent RFC: [../rfc.md](../rfc.md)

Status: `landed`

## Goal

Keep guard, capability inventory, and packet docs aligned with the current
`jun-6` widening verdicts.

## Current Code Fact

- `sourcegraph_parity.py --check` machine-checks supported surfaces and current
  explicit unsupported inventory
- `jun-4` analysis doc is the human-readable verdict table
- `file.has.contributor.regex.name` and `.email` are now part of the required
  supported surface inventory
- semantica ingress contributor live roundtrip proof is green and packet docs
  now reflect final landed closeout

## Official Sourcegraph Baseline

- any promoted Sourcegraph-facing surface must remain visible in both the guard
  inventory and the capability analysis

## Owner Seam

- `tools/benchmark/sourcegraph_parity.py`
- `tools/benchmark/SOURCEGRAPH_PARITY.md`
- `docs/analysis/jun-4-dsl-capabilty.md`
- `docs/plans/jun-6-sourcegraph-expansion/**`

## Source Truth Anchors

- `/Users/songmin/Documents/code-new/quanta-index/tools/benchmark/sourcegraph_parity.py`
- `/Users/songmin/Documents/code-new/quanta-index/tools/benchmark/SOURCEGRAPH_PARITY.md`
- `/Users/songmin/Documents/code-new/quanta-index/docs/analysis/jun-4-dsl-capabilty.md`
- `/Users/songmin/Documents/code-new/quanta-index/docs/plans/jun-6-sourcegraph-expansion/rfc.md`
- `/Users/songmin/Documents/code-new/quanta-index/docs/plans/jun-6-sourcegraph-expansion/tickets/INDEX.md`

## Concrete Work Items

1. Keep only the truly unsupported cells in demotion inventory without leaving
   already-landed widening cells in backlog prose.
2. Require every promoted cell to land with machine-checkable supported or
   unsupported inventory changes.
3. Refuse support claims that only updated docs or report markdown.
4. Keep `jun-4` capability analysis and `jun-6` packet status in lockstep.

## First Increment

- classify every landed or permanently unsupported `jun-6` cell correctly
- require machine-check coverage for any promotion or reaffirmed unsupported shape

## Red Rail To Pin First

```bash
python3 tools/benchmark/sourcegraph_parity.py --check
python3 tools/ci/lint/check-dsl-capability-truth.py
```

## Worker First Commands

```bash
rg -n "EXPLICIT_UNSUPPORTED_COMPARISON_GAPS|REQUIRED_SURFACES|repo.has.meta.regex|file.has.contributor.regex|mixed non-repo predicate sibling" tools/benchmark/sourcegraph_parity.py tools/benchmark/SOURCEGRAPH_PARITY.md docs/analysis/jun-4-dsl-capabilty.md -S
```

## No-Go

- do not add a supported verdict that the guard cannot classify
- do not remove a current unsupported inventory row before the widening lands
- do not leave already-supported cells in `jun-6` backlog prose
- do not let docs say “planned backlog” while guard already classifies the cell
  as supported

## DoD

- widened cells are reflected in guard and docs
- non-widened cells remain explicit unsupported
- no already-supported cell remains in the packet scope as backlog
- packet docs no longer carry stale external proof residue after green

## Not Done If

- docs say a cell is backlog but guard cannot classify it
- docs still say `file:has.contributor(<name-or-email regex>)` is a reopened widening cell
- guard says a cell is supported or unsupported but docs still lag
- a widened shape lands without machine-checked coverage
- packet docs still claim external producer proof is `unverified` after it is green
