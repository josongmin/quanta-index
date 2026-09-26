# MSTR-01 Surface and Matrix Closure

> Archive status: `Historical program record`. Current architecture: [JUN-02-001](../../../adr/JUN-02-001-search-dsl-authority-and-runtime-contract.md). Archive map: [Completed Plan Archive](../../ARCHIVE-INDEX.md).


Parent packet: [../README.md](../README.md)

## Objective

Close capability-matrix and route-surface drift against the current source and
owner proof, including predicate subset truth, oracle-limited rows, and
carrier-split surfaces.

## Required Rows

- predicate subset truth:
  `repo.has.file(...)`, `file.contains(...)`, `file.has.content(...)`
- runtime-metadata nuance:
  `dirty:{yes|only}` vs `dirty:no`
- oracle-limited runtime rows:
  `repo:` positive allow-list and `repo.has.file(...)` true-gate
- carrier split:
  `into:codeql`, `scope:results`, `with:lexical`, empty query
- route-specific typed failure normalization

## Current Source Truth

- this historical ticket has been retired by `JFC-01` and `JFC-03`; do not use
  it as the active owner lane
- owner proof already exists for `select:path`, `select:content.match`,
  phrase positive, structural subset, and timeout route truth
- `repo.has.file(...)` is executable for the current `path:` / `name:`
  argument subset and now has non-vacuous multi-repo true-gate proof
- `file.contains(...)` is runtime-proved on the native dotted predicate
  surface, with SG parity remaining companion coverage
- `file.has.content(...)` is directly proved on the SG alias rows plus the
  owner-local native predicate rail
- `dirty:` is a landed runtime-metadata family:
  `dirty:{yes|only|no}` executes with generation-pinned authority semantics
- bridge directives and empty query remain carrier-split surfaces, not missing
  runtime-row promotions

## Remaining Closeout

- none on this historical ticket
- active predicate/matrix truth is owned by
  [../../jun-2-dsl-final-cut/tickets/JFC-01-predicate-oracle-and-surface-closure.md](../../jun-2-dsl-final-cut/tickets/JFC-01-predicate-oracle-and-surface-closure.md)

## Guardrails

- no widen-first acceptance
- if proof is missing, the row moves here as open work
- current source beats prior packet prose
