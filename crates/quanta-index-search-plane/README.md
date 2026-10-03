# Quanta Index search plane

The daemon's ingest, query and control authority. It coordinates durable
generation state, lexical and semantic adapters, activation, retention and
readiness. The SDK and IPC crates own client and transport behavior; this
crate owns the state transitions after typed admission.

Start at [`src/lib.rs`](src/lib.rs) for the exported dispatcher surfaces.
`ingest_dispatcher` applies typed producer batches, `query_dispatcher`
serves generation-pinned requests, and `control_dispatcher` owns activation
and lifecycle operations. Behavioral qualification must reach the daemon and
SDK front door for claims about publication, visibility or recovery; unit
tests of one dispatcher have a narrower scope.

See the [repository verification profiles](../../README.md#build-and-verification)
and [release evidence contract](../../docs/plans/sep-21-search-plane-sota-hardening/tickets/S21-13-release-evidence-and-sota-qualification.md).
