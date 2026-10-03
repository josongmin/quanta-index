# Daemon runtime

Concrete adapter wiring and the `quanta-index-searchd` executable. It
binds query, control, and ingest sockets and owns process-level startup.

Start with [runtime exports](src/lib.rs), [executable](src/bin/quanta-index-searchd.rs),
[signal handling](src/signal.rs), and [state migration](src/state_migration.rs).
Process and cutover rules are in the [SEP-21-004 ADR](../../docs/adr/SEP-21-004-process-supervision-state-cutover-and-proof.md).
