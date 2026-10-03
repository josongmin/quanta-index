# Core contract

Shared producer/search-plane identities, generation pins, and query/result
primitives without IPC envelopes or socket behavior. Code that needs only
these types can depend on this smaller crate.

Start with [identifiers](src/ids.rs), [generation pins](src/query/pin.rs),
and [result types](src/results/mod.rs). Identity and digest rules are in the
[SEP-21-001 ADR](../../docs/adr/SEP-21-001-canonical-identity-and-digest-domains.md).
