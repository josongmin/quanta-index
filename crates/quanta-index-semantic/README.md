# Semantic adapter

The historical Legacy semantic adapter uses a LanceDB-backed, persisted generation-scoped
vector index. The search plane decides its publication and active-generation routing.
Below the 256-row ANN threshold, the exact lane serves exhaustive ordered
top-k. From 256 rows, the approximate lane searches candidates and returns
exact cosine scores for those it admits; membership in the exhaustive top-k
is measured by recall. Its historical quality rail uses an aggregate recall@10
floor of 0.95 on its fixed fixture, not per-query exact top-k equality.

Start with [adapter exports](src/lib.rs), [build](src/build.rs),
[search](src/search.rs), and [manifest](src/manifest.rs). Storage-generation
authority is in the [MAY-31-001 ADR](../../docs/adr/MAY-31-001-lancedb-semantic-generation-authority.md).

The separate [Native adapter](src/native/mod.rs) uses exact vectors and
[immutable HNSW regions](src/native/embedding/native_graph.rs), joined through
[materialization](src/native/embedding/ann_materialization.rs) and
[selected-view queries](src/native/embedding/ann_query.rs). Catalog owns Native
publication and retention. File candidates authenticate their
[source-file identity](src/native/embedding/source_file.rs).
The [Native ADR](../../docs/adr/NOV-12-001-native-query-and-semantic-engine.md)
records these contracts; current full-product and filtered quality acceptance
remain in the [existing Native plan](../../docs/plans/nov-12-native-cutover/README.md).
