# SGP-08 Shared Inventory And Parity Guard Followthrough

Parent RFC: [../rfc.md](../rfc.md)

Status: `planned`

## Objective

After any new Sourcegraph parity work lands, refresh the shared inventory and
guard so the comparison baseline stays fail-closed.

## Current Source Truth

- current guard tracks:
  - accepted `SgFilter` keyword surface
  - required canonical surface ids
- this packet introduces new comparison surfaces that the current required-set
  does not track yet

## Files To Touch

- `tools/benchmark/sourcegraph_parity.py`
- `tools/benchmark/SOURCEGRAPH_PARITY.md`
- `docs/analysis/jun-4-dsl-capabilty.md`
- shared runtime/front-door inventories touched by landed tickets

## Concrete First Increment

When the first new parity surface lands, add it to the guard immediately.

## Implementation Steps

1. extend the guard required-set or comparison section
2. add shared runtime/front-door inventory rows
3. keep `--check` fail-closed on accepted-but-unverified comparison surfaces

## Red Rail First

- `python3 tools/benchmark/sourcegraph_parity.py --check`

## DoD

- guard and analysis doc reflect the same Sourcegraph comparison baseline

## Not Done If

- code gains a new surface but the parity guard comparison section stays stale
