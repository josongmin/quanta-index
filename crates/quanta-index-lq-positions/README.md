# Phrase positions

Builds per-generation token-position postings and answers phrase or
adjacency queries over normalized terms. It is a query primitive, not the
search-plane generation authority.

Start with [builder](src/builder.rs), [position index](src/index.rs),
[phrase query](src/phrase_query.rs), and [adjacency query](src/adjacency_query.rs).
The LQ execution contract is in the [JUN-02-001 ADR](../../docs/adr/JUN-02-001-search-dsl-authority-and-runtime-contract.md).
