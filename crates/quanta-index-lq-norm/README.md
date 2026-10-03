# LQ syntax and normalization

Owns the canonical LQ parser, normalized AST, limits, and query hash. It
does not execute a query against an index.

Start with [parser](src/parser/mod.rs), [normalizer](src/normalizer/mod.rs),
[limits](src/limits.rs), and [hasher](src/hasher.rs). Product-level DSL rules
are in the [JUN-02-001 ADR](../../docs/adr/JUN-02-001-search-dsl-authority-and-runtime-contract.md).
