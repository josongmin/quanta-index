# Quanta Index contract

Typed, provider-independent identities, IPC requests/responses and result
shapes shared by the SDK and search plane. The crate defines wire and
validation rules; it does not open sockets or mutate index state.

The ingest contract is under [`src/ipc/ingest.rs`](src/ipc/ingest.rs): domain
DTOs retain their manual Serde implementations, while storage-independent
batch validation and the top-level envelope live in its submodules. Changing a
wire field or error shape requires contract golden/negative cases and SDK and
daemon consumer checks. A successful compile alone does not prove wire
compatibility.

For the public client and a compiling query example, see the
[`quanta-index-sdk` README](../quanta-index-sdk/README.md).
