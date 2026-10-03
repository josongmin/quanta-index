# Semantic adapter

LanceDB-backed, persisted generation-scoped vector index and ANN query
adapter. The search plane decides publication and active-generation routing.

Start with [adapter exports](src/lib.rs), [build](src/build.rs),
[search](src/search.rs), and [manifest](src/manifest.rs). Storage-generation
authority is in the [MAY-31-001 ADR](../../docs/adr/MAY-31-001-lancedb-semantic-generation-authority.md).
