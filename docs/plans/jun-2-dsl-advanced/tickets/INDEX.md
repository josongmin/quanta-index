# Jun 2 DSL Advanced Ticket Index

Parent packet: [../README.md](../README.md)

## Recommended Start Order

1. [ADV-00](ADV-00-scope-lock-and-admission-bar.md)
2. [ADV-01](ADV-01-predicate-capability-registry.md) registry only
3. [ADV-01](ADV-01-predicate-capability-registry.md) first widened predicate
4. [ADV-02](ADV-02-sourcegraph-structural-mixed-domain-widening.md) `RawString` sibling
5. [ADV-02](ADV-02-sourcegraph-structural-mixed-domain-widening.md) `Predicate` sibling
6. [ADV-03](ADV-03-sourcegraph-scoped-filter-or-widening.md)
7. [ADV-04](ADV-04-generated-proof-truth-and-benchmark-bar.md)

## Ticket Table

| ticket | status | first increment | red rail first | concrete deliverable focus |
| --- | --- | --- | --- | --- |
| [ADV-00](ADV-00-scope-lock-and-admission-bar.md) | landed | admission table frozen in [../README.md](../README.md) §9 | packet/doc truth spot-check | packet-level admission table + benchmark/shadow bar |
| [ADV-01](ADV-01-predicate-capability-registry.md) | landed | `predicate_registry.rs` without widening | `tantivy_smoke` + planner unit | registry SSOT + one widened predicate family |
| [ADV-02](ADV-02-sourcegraph-structural-mixed-domain-widening.md) | landed | legality matrix freeze, then `RawString` | lowering owner rail + parity rail | one sibling family per PR with explicit typed-fail remainder |
| [ADV-03](ADV-03-sourcegraph-scoped-filter-or-widening.md) | landed | legality verdict frozen | rejected scoped-`OR` owner rail | legality verdict per family: accept / rewrite / reject |
| [ADV-04](ADV-04-generated-proof-truth-and-benchmark-bar.md) | active | checker landed; warm benchmark artifact refresh remains | docs-vs-code drift checker | generator/checker + benchmark/shadow gate |
| [HISTORICAL-MAP](HISTORICAL-MAP.md) | reference | lineage lookup | n/a | predecessor-to-successor map only |
