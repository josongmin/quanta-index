# MSTR-02 Producer Handoff End-to-End

Parent packet: [../README.md](../README.md)

## Objective

Promote producer-side history / runtime / structural ingest from fixture-only
proof to real public typed-batch UDS proof with persisted reopen behavior.

## Covered Semantics

- history: `UpsertCommit`, `UpsertRef`, `UpsertTag`, `DeleteRef`, `DeleteTag`,
  `UpsertDiffHunk`
- runtime: `UpsertDirty`, `EvictDirty`
- structural: `UpsertParseTree`, `DeleteParseTree`

## Defaults To Freeze

- commit ordering defaults to topological order
- diff hunks emit as `UpsertDiffHunk` separate ops
- dirty events use per-edit emission with a 100ms debounce ceiling
- parse trees publish only after the matching `UpsertChunk`

## Current Source Truth

- public ingest truth is `SearchPlaneIngestIpcRequest::{PublishHistoryBatch,
  PublishDirtyBatch, PublishStructuralBatch}`, not direct channel-op IPC
- history ref/tag deletes, runtime dirty evict, and structural tombstone
  semantics now have repo-local end-to-end reopen proof through the public UDS
  ingest surface
- search-side readiness / authority persistence already restores these
  auxiliary states across reopen

## Remaining Closeout

- external producer-repo handoff remains outside this repo-local packet
- do not reintroduce a second ingest truth beside the typed batch surface

## Guardrails

- keep the landed op family; do not invent a parallel channel op shape
- keep the existing search-side readiness / ingest / query route
