# Durable catalog

SQLite-backed catalog for idempotency records and durable generation state.
The search plane uses it through core ports; this crate owns persistence and
recovery, not query planning.

Start with [connection/open](src/open.rs), [idempotency](src/idempotency.rs)
and [backup](src/backup.rs). The durable lifecycle contract is in the
[SEP-21-002 ADR](../../docs/adr/SEP-21-002-durable-authority-and-operation-lifecycle.md).
