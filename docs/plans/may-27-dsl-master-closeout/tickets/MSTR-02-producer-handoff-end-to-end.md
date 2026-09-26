# MSTR-02 Producer Handoff End-to-End

> Archive status: `Historical program record`. Current architecture: [JUN-02-001](../../../adr/JUN-02-001-search-dsl-authority-and-runtime-contract.md). Archive map: [Completed Plan Archive](../../ARCHIVE-INDEX.md).


Parent packet: [../README.md](../README.md)

## Objective

Build the remaining history/runtime executable query substrate on top of the
already-landed producer-side typed-batch handoff and reopen proof.

## Covered Semantics

- landed producer handoff:
  history `UpsertCommit`, `UpsertRef`, `UpsertTag`, `DeleteRef`, `DeleteTag`,
  `UpsertDiffHunk`
  runtime `UpsertDirty`, `EvictDirty`
  structural `UpsertParseTree`, `DeleteParseTree`
- remaining query substrate:
  history `before:`, `after:`, `since:`, `until:`, `diff.added:`,
  `diff.removed:`, `diff.touched:`
  runtime `changed:`, `stale:`, `affected:`, `invalidated_by:`, `snapshot:`,
  `meta.*`

## Defaults To Freeze

- commit ordering defaults to topological order
- diff hunks emit as `UpsertDiffHunk` separate ops
- dirty events use per-edit emission with a 100ms debounce ceiling
- parse trees publish only after the matching `UpsertChunk`

## Current Source Truth

- this historical ticket has been retired by `JFC-02` and `JFC-03`; do not use
  it as the active owner lane
- public ingest truth is `SearchPlaneIngestIpcRequest::{PublishHistoryBatch,
  PublishDirtyBatch, PublishStructuralBatch}`, not direct channel-op IPC
- history ref/tag deletes, runtime dirty evict, and structural tombstone
  semantics have repo-local end-to-end reopen proof through the public UDS
  ingest surface
- search-side readiness / authority persistence restores these auxiliary states
  across reopen
- query substrate on top of that authority is now materially wider:
  history executes `type:commit|diff`, `rev`, `author`, `committer`, `message`,
  `content`, `before:`, `after:`, `since:`, `until:`, qualified
  `since.time:` / `since.commit:`, and `diff.added:` / `diff.removed:` /
  `diff.touched:`
  runtime metadata executes `dirty:{yes|only|no}`, `changed:`, `stale:`,
  `snapshot:`, `meta.*`, `affected:`, and `invalidated_by:` with persisted
  generation-pinned authority

## Remaining Closeout

- none on this historical ticket
- active history/runtime substrate truth is owned by
  [../../jun-2-dsl-final-cut/tickets/JFC-02-history-date-and-diff-filters.md](../../jun-2-dsl-final-cut/tickets/JFC-02-history-date-and-diff-filters.md)
  and
  [../../jun-2-dsl-final-cut/tickets/JFC-03-runtime-catalog-authority.md](../../jun-2-dsl-final-cut/tickets/JFC-03-runtime-catalog-authority.md)

## Guardrails

- keep the landed op family; do not invent a parallel channel op shape
- keep the existing search-side readiness / ingest / query route
