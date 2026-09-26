# Jun 2 DSL Hardening Ticket Index

> Archive status: `Historical program record`. Current architecture: [JUN-02-001](../../../adr/JUN-02-001-search-dsl-authority-and-runtime-contract.md), [JUN-08-001](../../../adr/JUN-08-001-verification-hellgate-and-benchmark-separation.md). Archive map: [Completed Plan Archive](../../ARCHIVE-INDEX.md).


Parent packet: [../README.md](../README.md)

## Recommended Start Order

1. [DH-00](DH-00-scope-lock-and-seam-map.md)
2. [DH-01](DH-01-runtime-catalog-integrity-and-replay-guard.md)
3. [DH-03](DH-03-predicate-proof-symmetry.md) in parallel with `DH-01`
4. [DH-02](DH-02-runtime-metadata-semantics-and-pushdown.md)

## Ticket Table

| ticket | status | first increment | red rail first | concrete deliverable focus |
| --- | --- | --- | --- | --- |
| [DH-00](DH-00-scope-lock-and-seam-map.md) | landed | freeze scope, seam map, and claim discipline | packet/doc truth spot-check | hardening-only ownership and acceptance bar |
| [DH-01](DH-01-runtime-catalog-integrity-and-replay-guard.md) | landed | replacement/replay owner-local red rail | readiness owner-local tests | authoritative catalog snapshot semantics |
| [DH-02](DH-02-runtime-metadata-semantics-and-pushdown.md) | landed | semantics rail before pushdown | query-dispatch owner-local tests | `stale:` / `dirty:` semantics plus set-driven execution |
| [DH-03](DH-03-predicate-proof-symmetry.md) | landed | missing sibling proof rows/tests only | `tantivy_smoke` | shipped predicate subset proof completion |
