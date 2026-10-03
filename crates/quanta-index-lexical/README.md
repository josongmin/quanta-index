# Lexical adapter

Tantivy-backed generation materialization and lexical query execution.
Source-backed code-search candidates and previews are owned here; daemon
activation and IPC remain outside this crate.

Start with [adapter exports](src/lib.rs), [searcher](src/searcher/mod.rs),
and [code search](src/searcher/code_search.rs). Code-search source and preview
rules are in the [SEP-27-003 ADR](../../docs/adr/SEP-27-003-code-search-source-and-preview-contract.md).
