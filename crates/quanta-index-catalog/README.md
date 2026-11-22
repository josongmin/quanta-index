# Durable catalog

The historical generation catalog stores idempotency records and durable state
in SQLite. The search plane uses it through core ports; this crate owns
persistence and recovery, not query planning.

The [Native authority](src/native/authority.rs) owns current operation/view
publication, retained lifetime and reclaim. Its format and namespace differ
from the historical generation catalog.

Opening a v20 catalog installs only the base native namespace. Quarantine,
body-incident and Source-stage replay tables are optional extensions installed
atomically by their first writer. A root that has not used an extension keeps
the base v20 namespace for pinned offline readers; once an extension is used,
older readers must refuse that root instead of silently ignoring its authority.

Start with [connection/open](src/open.rs), [idempotency](src/idempotency.rs)
and [backup](src/backup.rs). The durable lifecycle contract is in the
[SEP-21-002 ADR](../../docs/adr/SEP-21-002-durable-authority-and-operation-lifecycle.md).
