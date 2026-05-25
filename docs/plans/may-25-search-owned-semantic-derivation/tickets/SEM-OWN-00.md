# SEM-OWN-00 — Semantic Ownership Inversion and Boundary Freeze

Status: `proposed`
Parent: [../README.md](../README.md)
Depends on: none

## 1. Purpose

Freeze one canonical ownership model before code changes start:

- parse tree / symbol / history remain producer-authored
- semantic vectors become search-authored
- external producer sends raw records only
- public semantic query APIs become text-first

Without this freeze, contract and runtime work will drift in opposite
directions.

## 2. Required decisions

Lock the following as normative:

1. semantic corpus identity is `chunk_id`
2. `embedding_id == chunk_id`
3. external semantic publishing is not part of the stable producer contract
4. `ChunkRecord.text` is the canonical semantic source text
5. public semantic/hybrid query requests are text-only
6. semantic model/provider selection is generation-scoped and owned by
   `quanta-index`

## 3. Deliverables

- update SSOT and plan docs so semantic is no longer grouped with the
  producer-authored domains
- mark older producer-authored semantic statements as superseded
- document internal-vs-external boundary for the semantic channel
- document that this is a breaking change, not a compatibility shim

## 4. Acceptance

- no active architecture doc claims that the external producer authors semantic
  vectors for the target end-state
- no active architecture doc claims that public semantic query callers must
  embed query text themselves
- the canonical identity rule `embedding_id == chunk_id` is stated once and
  reused everywhere else by reference

## 5. Non-goals

- implementing any runtime code
- finalizing provider-specific API behavior
- designing a symbol-first semantic corpus
