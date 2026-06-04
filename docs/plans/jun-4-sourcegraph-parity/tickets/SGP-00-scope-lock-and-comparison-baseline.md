# SGP-00 Scope Lock And Comparison Baseline

Parent RFC: [../rfc.md](../rfc.md)

Status: `landed`

## Objective

Freeze:

- the exact Sourcegraph comparison baseline
- the exact gap list this packet owns
- which gaps are implementation targets vs explicit demotion candidates

## Current Source Truth

- local inventory:
  - `docs/analysis/jun-4-dsl-capabilty.md`
  - `tools/benchmark/SOURCEGRAPH_PARITY.md`
- external baseline:
  - Sourcegraph search query syntax/reference/ownership/structural docs

## Files To Touch

- `docs/plans/jun-4-sourcegraph-parity/rfc.md`
- `docs/plans/jun-4-sourcegraph-parity/tickets/INDEX.md`
- `docs/analysis/jun-4-dsl-capabilty.md`
- `tools/benchmark/SOURCEGRAPH_PARITY.md`

## Implementation Steps

1. freeze the baseline surface list once
2. map each surface to one ticket and one owner seam
3. mark any non-goal explicitly

## Red Rail First

- `rg -n "has.commit.after|has.meta|has.topic|has.owner|has.contributor|rev:at.time|file.owners" docs/analysis/jun-4-dsl-capabilty.md tools/benchmark/SOURCEGRAPH_PARITY.md docs/plans/jun-4-sourcegraph-parity`

## DoD

- no Sourcegraph gap is tracked only in prose
- each gap has one owner ticket
- blocked gaps are marked with the real missing seam, not a hopeful shortcut label

## Not Done If

- the same gap is split across multiple tickets without an owner seam reason
