# Quanta Index SDK

The public Rust entry point for producers and readers. `QuantaIndex` binds
query, control and ingest responses to their requests. Use the query-only
profile when the caller needs no mutation authority.

## Query an active generation

Start `quanta-index-searchd` against a current-format state root and publish
and activate a generation first. Then run the compiling
[`query_active` example](examples/query_active.rs) as the Unix user allowed by
the daemon socket policy:

```sh
./scripts/cargow run -p quanta-index-sdk --example query_active --locked -- \
  /absolute/state-root repo-id revision-id 'search terms'
```

The example calls `QuantaIndex::connect_query_only`, then the lexical builder
with an explicit active repository/revision and result cap. Transport refusal,
missing generation and remote query errors are returned as `SdkError`; an empty
successful result is printed as a response with zero rows.

## Producer path

Build a typed `SearchCorpusBatch`, publish through `search_corpus()`, inspect
the receipt, and activate by compare-and-swap. Publication and activation are
separate operations; callers must preserve the batch digest, source event and
generation identity across retries. The end-to-end reference fixture is
[`l2_daemon_publication.rs`](tests/l2_daemon_publication.rs); it requires its
declared fresh-daemon test environment and is not a standalone example.

The wire DTO and validation authority live in
[`quanta-index-contract`](../quanta-index-contract/README.md). Server-side
ingest and query ownership live in
[`quanta-index-search-plane`](../quanta-index-search-plane/README.md).
