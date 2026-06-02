# Jun 2 DSL Hardening Ticket Index

Parent packet: [../README.md](../README.md)

## Recommended Start Order

1. [DH-00](DH-00-scope-lock-and-seam-map.md)
2. [DH-01](DH-01-runtime-catalog-integrity-and-replay-guard.md)
3. [DH-03](DH-03-predicate-proof-symmetry.md) in parallel with `DH-01`
4. [DH-02](DH-02-runtime-metadata-semantics-and-pushdown.md)

## Ticket Table

| ticket | status | first increment | red rail first | concrete deliverable focus |
| --- | --- | --- | --- | --- |
| [DH-00](DH-00-scope-lock-and-seam-map.md) | planned | freeze scope, seam map, and claim discipline | packet/doc truth spot-check | hardening-only ownership and acceptance bar |
| [DH-01](DH-01-runtime-catalog-integrity-and-replay-guard.md) | planned | replacement/replay owner-local red rail | readiness owner-local tests | authoritative catalog snapshot semantics |
| [DH-02](DH-02-runtime-metadata-semantics-and-pushdown.md) | planned | semantics rail before pushdown | query-dispatch owner-local tests | `stale:` / `dirty:` semantics plus set-driven execution |
| [DH-03](DH-03-predicate-proof-symmetry.md) | planned | missing sibling proof rows/tests only | `tantivy_smoke` | shipped predicate subset proof completion |
