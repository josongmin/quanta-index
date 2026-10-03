# Repository map

Snapshot model, delta application, materialization, persistence, and pinned
read views for repository-map queries. Activation remains a search-plane
control operation.

Start with [model](src/model.rs), [delta](src/delta.rs),
[materializer](src/materializer.rs), [store](src/store.rs), and
[pinned query view](src/pinned.rs). See [engine status](../../docs/ssot/engine-status-v1.md)
for the live daemon path.
