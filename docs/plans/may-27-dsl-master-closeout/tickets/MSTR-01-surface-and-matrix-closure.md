# MSTR-01 Surface and Matrix Closure

Parent packet: [../README.md](../README.md)

## Objective

Close capability-matrix and route-surface drift against the current source and
owner proof.

## Required Rows

- `select:path`
- `select:content.match`
- route-specific typed failure normalization
- bridge / explain / runtime rows re-anchored to current source

## Current Source Truth

- owner proof already exists for `select:path` and `select:content.match` on both
  Sourcegraph and native front doors
- structural / bridge / runtime typed-failure behavior already has route-local
  proof; the remaining work is packet-level matrix alignment rather than
  executor bring-up
- current source already ships structural-only boolean, typed holes, structural
  bridge packets, and Sourcegraph structural regex lowering; the matrix must not
  regress to older single-leaf or no-regex claims

## Remaining Closeout

- repo-local residue is closed on the current tree
- active packet prose and the legacy capability matrix are now aligned with the
  current owner proof
- any further row work would be new surface, not unclosed matrix drift

## Guardrails

- no widen-first acceptance
- if proof is missing, the row moves here as open work
- current source beats prior packet prose
