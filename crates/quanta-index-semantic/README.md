# Semantic adapter

LanceDB-backed, persisted generation-scoped vector index and ANN query
adapter. The search plane decides publication and active-generation routing.
Below the 256-row ANN threshold, the exact lane serves exhaustive ordered
top-k. From 256 rows, the approximate lane searches candidates and returns
exact cosine scores for those it admits; membership in the exhaustive top-k
is measured by recall. The current quality rail uses an aggregate recall@10
floor of 0.95 on its fixed fixture, not per-query exact top-k equality.

Start with [adapter exports](src/lib.rs), [build](src/build.rs),
[search](src/search.rs), and [manifest](src/manifest.rs). Storage-generation
authority is in the [MAY-31-001 ADR](../../docs/adr/MAY-31-001-lancedb-semantic-generation-authority.md).
