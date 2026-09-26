# SGP-08 Shared Inventory And Parity Guard Followthrough

> Archive status: `Historical program record`. Current architecture: [JUN-06-001](../../../adr/JUN-06-001-sourcegraph-compatibility-boundary.md). Archive map: [Completed Plan Archive](../../ARCHIVE-INDEX.md).


Parent RFC: [../rfc.md](../rfc.md)

Status: `landed`

## Objective

Keep the shared inventory and guard aligned with the final supported + explicit
unsupported Sourcegraph verdict set.

## Current Source Truth

- current guard tracks:
  - accepted `SgFilter` keyword surface
  - required canonical surface ids
  - explicit unsupported structural demotion inventory
- structural demotion closeout from `SGP-06` / `SGP-07` is reflected
- `SGP-01`, `SGP-02`, `SGP-05` supported surfaces are reflected
- `file:has.owner(...)` is reflected as a canonical supported surface
- `file:has.contributor(...)` is reflected as a canonical supported surface
- `repo:has.topic(...)` is reflected as a canonical supported surface
- `select:file.owners` is reflected as a canonical supported select surface

## Files To Touch

- `tools/benchmark/sourcegraph_parity.py`
- `tools/benchmark/SOURCEGRAPH_PARITY.md`
- `docs/analysis/jun-4-dsl-capabilty.md`
- shared runtime/front-door inventories touched by landed tickets

## Final Increment

Current closed set:

- supported:
  - `SGP-01` canonical surface ids and runtime/front-door/parity evidence
  - `SGP-05` canonical surface id and targeted runtime/front-door evidence
  - `SGP-02` `repo.has.meta` / `repo.has.topic` canonical surface ids and runtime/front-door/parity/corpus evidence
  - `SGP-03` `file.has.owner` canonical surface id and runtime/front-door/parity/corpus evidence
  - `SGP-03` `select.file.owners` canonical surface id and runtime/front-door/parity/corpus evidence
  - `SGP-04` `file.has.contributor` canonical surface id and runtime/front-door/parity/corpus evidence
- explicit unsupported:
  - structural direct lexical `Phrase` / `Regex`
  - structural mixed `symbol.has.name(...)` predicate sibling

## Implementation Steps

1. keep the guard required-set aligned with supported canonical surfaces
2. keep the generated report explicit about supported select surfaces and unsupported structural comparison gaps
3. keep `--check` fail-closed on accepted-but-unverified comparison surfaces

## Red Rail First

- `python3 tools/benchmark/sourcegraph_parity.py --check`

## DoD

- current guard and analysis doc reflect the same Sourcegraph comparison baseline
- structural demotion surfaces are no longer accepted-but-unverified
- supported select surfaces and unsupported structural gaps are both explicit in the generated report

## Not Done If

- code gains a new surface but the parity guard comparison section stays stale
- unsupported comparison gaps are left implied instead of explicit
