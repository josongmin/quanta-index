# LQ observability contract

Typed metric, span, audit, and cardinality surfaces for query observation.
This crate defines emission data; runtime transport/export lives elsewhere.

Start with [metric types](src/metric.rs), [spans](src/span.rs), and
[cardinality guard](src/cardinality_guard.rs). The DSL observation contract
is in the [JUN-02-001 ADR](../../docs/adr/JUN-02-001-search-dsl-authority-and-runtime-contract.md).
