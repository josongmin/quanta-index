# Historical Map

Parent packet: [../README.md](../README.md)

This file is lineage-only. Execution order and first-PR boundaries live in
`../README.md` and `INDEX.md`.

## Source Packets

| historical source | prior role | successor tickets | note |
| --- | --- | --- | --- |
| [../../jun-2-dsl-final-cut/README.md](../../jun-2-dsl-final-cut/README.md) | whole-DSL closeout packet | `ADV-00` through `ADV-04` | advanced packet starts only after closeout |
| [../../may-25-lexical-enhancement/README.md](../../may-25-lexical-enhancement/README.md) | authoritative proof ledger and capability matrix | `ADV-00`, `ADV-04` | widening must keep proof inventory synchronized |

## Prior Tickets to Successor Lanes

| historical ticket | status now | successor | note |
| --- | --- | --- | --- |
| [JFC-01](../../jun-2-dsl-final-cut/tickets/JFC-01-predicate-oracle-and-surface-closure.md) | closed | `ADV-01` | widening starts after the shipped predicate subset and runtime proof closeout |
| [JFC-05](../../jun-2-dsl-final-cut/tickets/JFC-05-sourcegraph-bridge-and-carrier-parity.md) | closed | `ADV-02`, `ADV-03` | widening starts from the current SG narrow subset truth |
| [JFC-00](../../jun-2-dsl-final-cut/tickets/JFC-00-truth-freeze-and-scope-lock.md) | closed | `ADV-04` | generated truth replaces hand-maintained widening prose where possible |

## Execution Reference

For cold-start execution order, use:

1. `ADV-00`
2. `ADV-01` registry only
3. `ADV-01` first widened predicate
4. `ADV-02` `RawString` sibling
5. `ADV-02` `Predicate` sibling
6. `ADV-03`
7. `ADV-04`
