# Quanta Index contract

Typed, provider-independent identities, IPC requests/responses and result
shapes shared by the SDK and search plane. The crate defines wire and
validation rules; it does not open sockets or mutate index state.

The ingest contract is routed through [`src/ipc/ingest.rs`](src/ipc/ingest.rs).
Corpus, semantic, history/metadata, and runtime/structural wire DTOs retain
their manual Serde implementations in separate submodules. Storage-independent
batch validation, payload digest, and the top-level envelope have their own
modules. Changing a wire field or error shape requires contract golden/negative
cases and SDK and daemon consumer checks. A successful compile alone does not
prove wire compatibility.

The separate [Native protocol](src/native_protocol.rs) and
[read-session contract](src/native_protocol/read_session.rs) bind operations,
selected views, file results and retained pages. They are not Legacy generation DTOs.

For the public client and a compiling query example, see the
[`quanta-index-sdk` README](../quanta-index-sdk/README.md).
