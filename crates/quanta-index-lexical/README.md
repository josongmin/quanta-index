# Lexical adapter

The default build provides the Native adapters. Explicit compatibility builds
enable the Legacy adapter with `legacy-tantivy`.

Legacy serving uses Tantivy-backed generation materialization and lexical query execution.
Source-backed code-search candidates and previews are owned here; daemon
activation and IPC remain outside this crate.

Start with [adapter exports](src/lib.rs), [searcher](src/searcher/mod.rs),
and [code search](src/searcher/code_search.rs). Code-search source and preview
rules are in the [SEP-27-003 ADR](../../docs/adr/SEP-27-003-code-search-source-and-preview-contract.md).

Native serving uses the separate [Literal](src/native/literal.rs) and
[Text/BM25](src/native/text.rs) adapters under Kernel/Catalog selected-view
authority. Their storage and file-query contracts are in the
[Native ADR](../../docs/adr/NOV-12-001-native-query-and-semantic-engine.md).
