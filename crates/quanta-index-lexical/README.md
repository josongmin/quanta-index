# Lexical adapter

Native serving uses the [Literal](src/native/literal.rs) and
[Text/BM25](src/native/text.rs) adapters under Kernel/Catalog selected-view
authority. Their storage and file-query contracts are in the
[Native ADR](../../docs/adr/NOV-12-001-native-query-and-semantic-engine.md).

The [historical data verifier](src/legacy/mod.rs) supports boot custody
checks and read-only Source export for existing sealed generations.
