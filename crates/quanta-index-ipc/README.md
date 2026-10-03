# IPC transport

CBOR framing, admission, and Unix-domain socket transport for typed
contract payloads. This crate transports requests; the search-plane crate
owns their state transitions.

Start with [public exports](src/lib.rs), [admission](src/admission.rs),
[codec](src/codec.rs), [server dispatch](src/server.rs), and the split
[peer watch](src/server/peer_watch.rs). Typed envelopes live in the
[contract crate](../quanta-index-contract/README.md); current system routing
is in the [living documentation index](../../docs/ssot/README.md).
