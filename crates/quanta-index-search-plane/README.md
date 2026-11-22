# Quanta Index search plane

The historical Legacy daemon's ingest, query and control authority. It coordinates durable
generation state, adapter ports, activation, retention and readiness. The
runtime composes concrete lexical and semantic adapters; this crate has no
normal dependency on either adapter. The SDK and IPC crates own client and
transport behavior; this crate owns the state transitions after typed
admission.

The current `serve-native` command uses [Kernel](../quanta-index-kernel/src/lib.rs),
[Catalog](../quanta-index-catalog/src/native/authority.rs) and the
[Native runtime](../quanta-index-searchd-runtime/src/native_product.rs)
for publication and selected-view query coordination.

Start at [`src/lib.rs`](src/lib.rs) for the exported dispatcher surfaces.
`ingest_dispatcher` applies typed producer batches, `query_dispatcher`
serves generation-pinned requests, and `control_dispatcher` owns activation
and lifecycle operations. Behavioral qualification must reach the daemon and
SDK front door for claims about publication, visibility or recovery; unit
tests of one dispatcher have a narrower scope.

See the [repository verification profiles](../../README.md#development-and-verification)
and [release evidence contract](../../docs/adr/OCT-05-004-cost-capacity-and-qualification-boundaries.md#installed-paired-and-operational-acceptance).
