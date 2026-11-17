# Application core

Domain ports and policies consumed by the search plane and implemented by
storage, lexical, semantic, and IPC adapters. This crate does not compose a
daemon or choose a provider.

[Native index ports and budgets](src/index/mod.rs) are also consumed by the
[Kernel](../quanta-index-kernel/src/lib.rs); the Legacy search plane is not the
only core consumer.

Start with [domain modules](src/domains/mod.rs), [request budgets](src/request_budget.rs),
and [errors](src/error.rs). See the [living documentation index](../../docs/ssot/README.md)
for the current implementation and qualification owners.
