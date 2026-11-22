# Quanta Index SDK

The public Rust entry point for producers and readers. `QuantaIndex` binds
query, control and ingest responses to their requests. Use the query-only
profile when the caller needs no mutation authority.

The ordinary `QuantaIndex` query example uses a Native selected view.
[NativeIndexClient](src/native_index.rs) handles durable submission and
[NativeUdsClient](src/native_uds.rs) handles selected-view operations, including
[retained reads](src/native_uds/read_session.rs). The UDS client is blocking;
async callers supply a bounded blocking executor. Installed acceptance remains in the
[cutover plan](../../docs/plans/nov-12-native-cutover/README.md).

## Query a selected Native view

Start `quanta-index-searchd serve-native` with an admitted configuration and
publish a selected view first. Provide `binding.cbor` with the caller's
`(ViewSelection, InstanceSpec, [u8; 32])` and run the compiling
[`query_active` example](examples/query_active.rs) as the Unix user allowed by
the daemon socket policy:

```sh
./scripts/cargow --lane dev-lane run -p quanta-index-sdk --example query_active --locked -- \
  /absolute/native.sock /absolute/binding.cbor 'search terms'
```

The example opens a retained Native read session, loads its profile, runs a
bounded text query, and explicitly releases the session. Its binding must come
from the admitted producer and selected view. A release error preserves the
session identity for reconciliation.

## Historical generation producer path

The Legacy comparison rail builds a typed `SearchCorpusBatch`, publishes through
`search_corpus()`, inspects the receipt, and activates by compare-and-swap.
Publication and activation are
separate operations; callers must preserve the batch digest, source event and
generation identity across retries. The end-to-end reference fixture is
[`l2_daemon_publication.rs`](tests/l2_daemon_publication.rs); it requires its
declared pinned old-Q executable and fresh-daemon test environment. It is not
a current release daemon example.

The wire DTO and validation authority live in
[`quanta-index-contract`](../quanta-index-contract/README.md). Server-side
ingest and query ownership live in
[`quanta-index-search-plane`](../quanta-index-search-plane/README.md).
