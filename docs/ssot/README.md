# Source maps

The [documentation index](../README.md) routes ADRs, residual ledgers, usage and
history. This directory holds source maps rather than another execution ledger.

- [Engine source map and capability limits](engine-status-v1.md): ingest,
  activation, query/runtime owners, typed unsupported behavior and remaining scopes.
- [Crate ownership](crate-ownership.md): workspace ownership and source entry points.

Source behavior and actual verification outrank dated counts. For `.expect()`
reachability, inspect current callers/module boundaries; the retired snapshot is
recoverable through [the history index](../ARCHIVE-INDEX.md#historical-record-recovery).
