# MSTR-02 Producer Handoff End-to-End

Parent packet: [../README.md](../README.md)

## Objective

Promote producer-side history / runtime / structural emission from fixture-only
proof to real emitted-op proof on the existing channel op family.

## Covered Ops

- history: `UpsertCommit`, `UpsertRef`, `UpsertTag`, `DeleteRef`, `DeleteTag`,
  `UpsertDiffHunk`
- runtime: `UpsertDirty`, `EvictDirty`
- structural: `UpsertParseTree`, `DeleteParseTree`

## Defaults To Freeze

- commit ordering defaults to topological order
- diff hunks emit as `UpsertDiffHunk` separate ops
- dirty events use per-edit emission with a 100ms debounce ceiling
- parse trees publish only after the matching `UpsertChunk`

## Guardrails

- keep the landed op family; do not invent a parallel channel op shape
- keep the existing search-side readiness / ingest / query route
