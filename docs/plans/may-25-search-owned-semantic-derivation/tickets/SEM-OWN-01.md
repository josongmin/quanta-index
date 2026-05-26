# SEM-OWN-01 — Raw Ingest Contract and Chunk-text Authority

Status: `partial-execution-live`
Parent: [../README.md](../README.md)
Depends on: [SEM-OWN-00.md](SEM-OWN-00.md)

## 1. Purpose

Define the exact external payload that `quanta-index` receives for semantic
derivation. The worker cannot be correct until the raw input contract is stable.

## 1.1 Current tree truth

Already live on the current tree:

- `ChunkRecord.text` is the canonical lexical/semantic source field
- lexical indexing already consumes `text`
- public semantic/hybrid query callers no longer need producer-authored
  semantic vectors

Remaining work under this ticket is migration discipline and boundary cleanup:
producer-facing docs and low-level ingest assumptions still need to stop
treating raw semantic vectors as a stable external requirement.

## 2. Scope

### In

- replace the semantic source field on chunk payloads with canonical text
- make language typed and stable for rendering decisions
- keep symbol/parse-tree/history contracts unchanged unless they are required to
  compile against the new chunk shape

### Out

- embedder implementation
- query-time embedding
- readiness/activation logic

## 3. Required contract change

Adopt:

```rust
pub struct ChunkRecord {
    pub repo_relative_path: RepoRelativePath,
    pub language: LangId,
    pub start_line: u32,
    pub end_line: u32,
    pub text: Box<str>,
}
```

Breaking rules:

- remove `snippet`
- do not add a `text || snippet` dual-read shim
- do not make semantic derivation read source bytes from disk

## 4. Deliverables

- contract crate update for `ChunkRecord`
- lexical adapter update so lexical indexing uses `text`
- producer-facing migration note: semantic vectors are no longer required, chunk
  text is
- producer-facing chunk identity contract for semantic reuse
- test fixtures updated to publish chunk text in the new field

## 5. Stable chunk identity contract

`chunk_id` is not allowed to mean "current line range".

Required semantics:

- stable across whitespace-only edits
- stable across import reorder when the semantic region is unchanged
- stable across equivalent re-rendering of the same semantic region

Initial implementation escape hatch:

- if upstream cannot yet provide a stable `chunk_id`, semantic derivation may
  still ship, but it must treat semantic reuse as generation-full only.
  Incremental semantic reuse under unstable ids is forbidden.

## 6. Acceptance

- lexical indexing still works from the new `text` field
- no search-plane code path reads source bytes from the filesystem to recover
  missing semantic corpus text
- chunk delete semantics remain keyed by `chunk_id`
- a follow-on worker can derive embeddings without consulting symbol or parse
  tree payloads
- the ticket states whether semantic reuse is incremental-safe or
  full-generation-only under the available `chunk_id` contract

## 7. Risks to watch

- accidental rename-only patch that still treats `text` as a UI snippet instead
  of canonical retrieval text
- hidden dependencies on `String` language values instead of `LangId`
- silent assumption that `chunk_id` is already a semantic-stable identity when
  it is only a positional id
