# MSTR-04 Sourcegraph and Bridge V2

> Archive status: `Historical program record`. Current architecture: [JUN-02-001](../../../adr/JUN-02-001-search-dsl-authority-and-runtime-contract.md). Archive map: [Completed Plan Archive](../../ARCHIVE-INDEX.md).


Parent packet: [../README.md](../README.md)

## Objective

Widen the Sourcegraph structural frontdoor and bridge surface only after native
structural semantics already exist.

## Scope

- Sourcegraph structural boolean composition
- Sourcegraph structural richer body lowering where native semantics exist
- structural bridge / CodeQL follow-on on the existing bridge path

## Current Source Truth

- SG structural boolean composition is live.
- SG structural typed-hole surface now lowers into native structural execution
  and preserves fail-closed behavior for unsupported hole kinds.
- SG structural regex bodies now lower into a native structural block over
  authoritative node text rather than translator-side lexical fallback.
- structural bridge packets preserve `BridgeScope::Structural` and structural
  candidates for native and SG structural queries.
- bridge directives remain intentional bridge-packet carrier surfaces rather
  than runtime search-result rows.

## Remaining Closeout

- widen no further than native executable semantics
- keep non-executable SG structural shapes typed
- keep bridge directives as intentional bridge-packet carrier surfaces rather
  than forcing them into runtime search-result inventory
- add any missing matrix/doc proof rows instead of widening acceptance

## Guardrails

- translator never falls back to lexical
- unsupported shapes remain typed failures
- bridge export preserves structural result truth rather than down-casting to
  lexical candidates
