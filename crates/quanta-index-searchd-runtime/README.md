# Daemon runtime

Concrete adapter wiring and the `quanta-index-searchd` executable. It
binds query, control, and ingest sockets and owns process-level startup.

`serve-native` is available in the default build and uses the
[Native product configuration](src/native_product.rs) and
[dispatcher](src/native_dispatch.rs), with Kernel/Catalog selected views.
`serve` composes the Legacy generation service when built with
`--features legacy-tantivy`.
Current caller migration and installed/default acceptance are in the
[Native cutover plan](../../docs/plans/nov-12-native-cutover/README.md).

Start with [runtime exports](src/lib.rs), [executable](src/bin/quanta-index-searchd.rs),
[signal handling](src/signal.rs), and [state migration](src/state_migration.rs).
Process and cutover rules are in the [SEP-21-004 ADR](../../docs/adr/SEP-21-004-process-supervision-state-cutover-and-proof.md).
