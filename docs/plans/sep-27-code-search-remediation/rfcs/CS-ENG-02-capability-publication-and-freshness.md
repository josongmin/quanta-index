# CS-ENG-02 — Total coverage pipeline cost qualification

Status: `ACTIVE_RESIDUAL` for total update work and physical heap.
Implemented immutable pages, row sharing, bounded decoding/cache and same-build
seal reuse are owned by
[SEP-27-003](../../../adr/SEP-27-003-code-search-source-and-preview-contract.md)
and [OCT-05-004](../../../adr/OCT-05-004-cost-capacity-and-qualification-boundaries.md).
[CS-INT-01](CS-INT-01-integration-and-qualification.md) owns combined-source
acceptance; [OCT-04 E4](../../oct-4-parallel-closure/tickets/INDEX.md#e4) owns
current measured optimization decisions. No stale-result defect is established.

## Remaining measurement and optimization boundary

- Measure increasing one-file replacement, deletion and mixed batches over the
  entire pipeline: index, ranked keys, coverage, text/file authority, catalog,
  semantic work, publication and independent open. Record actual bytes read/
  written, rows/pages/hash work, wall time and temporary/retained physical heap.
  Changed-page bytes and seal counters exclude other I/O.
- Attribute pre-intent preflight, lock-held preflight, build, seal and open
  separately. The normal new-delta path independently validates the base at both
  preflights and build; replay/repair paths differ. Cache row reuse retains page
  reads/hashes and directory checks. Same-build seal avoids a second coverage
  decode while still hashing the effective root/pages. These mechanisms do not
  establish sublinear total ingest.
- Only remove demonstrated repeated work through an authenticated immutable base
  capability that preserves pre-intent refusal, lock-time ownership and source
  lineage. A process-local operation lock cannot prevent external file mutation.
  Keep later rechecks until equivalent custody/refusal is proved; an unbound
  cache or a source high-water value cannot authorize an older physical base.
- Measure physical peak/resident allocation on the actual whole caller. Logical
  charges, fallible buffers, whole-process high-water RSS and a child process's
  startup peak do not establish delta-only allocator heap. Phase gaps, unavailable
  physical I/O and all sidecars remain explicit.
- Execute the selected public producer/SDK/daemon mutation, crash, restart,
  retention and activation paths on current matching binaries. Keep owner-local,
  installed, Linux, hosted and qualified performance outcomes separate.

## Required independent controls

Compare complete effective rows/identities and query outcomes with independently
constructed inputs and fresh rebuilds: fresh/no-op/one-file/mixed/scoped delete,
unchanged source survival, old readers, lineage/replay/conflict, retry and reopen.
Retain page inode inheritance, unchanged-row sharing and actual new-byte checks.

Corrupt cached or inherited pages between preflight/build/seal; change root,
inventory or source identity; forge semantic rows even after recomputing hashes.
Missing/tampered/symlink/orphan/oversized inputs refuse before target creation.
Refusal evicts cached authority; repaired retry can succeed. Staged root-rename
orphan recovery and sealed orphan rejection retain their distinct lifecycle rules.
Large roots exercise uncached decode admission, including definite/indefinite
CBOR cardinality and over-limit refusal. Conservative admission is not RSS proof.

Parsing stays producer-owned; optional symbol coverage does not independently
activate lexical publication. Use current source/seal decoders for format and
rebuild boundaries rather than old manifest-version labels.

## Execution and history

Use the existing lexical coverage owner diagnostics and registered
`runtime_extended_suite::e2e_coverage_pipeline_cost` public cases, selected through
`Justfile`/`scripts/cargow`. Run process cases separately when their fixture owns
shared state. [CS-BENCH-04](CS-BENCH-04-comparators-performance-and-incremental.md)
owns admitted cost measurements and [MISC](../../sep-27-misc/tickets/INDEX.md)
owns final-source/platform qualification. The source/input-specific older counts,
RSS values and commands are recoverable through
[the history index](../../ARCHIVE-INDEX.md#oct-05-residual-owner-clarification).
