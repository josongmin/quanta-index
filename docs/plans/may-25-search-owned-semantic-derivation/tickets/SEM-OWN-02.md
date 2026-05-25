# SEM-OWN-02 — Search-owned Corpus Embedding Derivation Worker

Status: `proposed`
Parent: [../README.md](../README.md)
Depends on: [SEM-OWN-01.md](SEM-OWN-01.md)

## 1. Purpose

Create the runtime component that turns lexical chunk ingress into internal
semantic vectors.

## 2. Required interfaces

```rust
pub trait EmbeddingClient: Send + Sync {
    fn embed_documents(&self, inputs: &[EmbeddingInputDoc]) -> Result<Vec<Vec<f32>>, EmbedError>;
}

pub trait EmbeddingRenderer: Send + Sync {
    fn render_chunk(&self, chunk_id: &str, chunk: &ChunkRecord) -> EmbeddingInputDoc;
    fn render_policy_hash(&self) -> [u8; 32];
}
```

`EmbeddingInputDoc` is canonicalized around chunk identity:

- `embedding_id`
- `repo_relative_path`
- `language`
- `start_line`
- `end_line`
- `text`

## 3. Worker behavior

New runtime component:

- own lexical subscriber cursor
- consume `UpsertChunk`, `DeleteChunk`, `Seal`
- on `UpsertChunk`, render text and call external embedding API
- publish internal `UpsertEmbedding { embedding_id == chunk_id }`
- on `DeleteChunk`, publish internal `DeleteEmbedding { embedding_id == chunk_id }`

Important constraint:

- do not run external API calls inline in the existing lexical channel-dispatcher
  ack loop

The derivation worker needs its own progress/accounting state, separate from the
lexical indexer.

Required persisted job lifecycle:

```rust
pub enum SemanticJobState {
    Pending,
    Rendered,
    Submitted,
    Embedded,
    Indexed,
    FailedRetryable,
    FailedTerminal,
}
```

This state machine is part of the minimal delivery. Without it, retry,
recovery, and seal correctness are underspecified.

## 4. Config surface

Add search-owned embedder config:

- `EMBEDDING_PROVIDER`
- `EMBEDDING_MODEL`
- `EMBEDDING_API_KEY`
- `EMBEDDING_BASE_URL`
- `EMBEDDING_REQUEST_TIMEOUT_MS`
- `EMBEDDING_BATCH_MAX_ITEMS`
- `EMBEDDING_MAX_IN_FLIGHT`

## 5. Acceptance

- fake-embedder e2e: lexical chunk ingress yields internal semantic upsert
- delete e2e: `DeleteChunk` yields `DeleteEmbedding`
- failure e2e: provider timeout or HTTP error does not mark semantic ready
- replay e2e: restart after partial work does not duplicate semantic rows under
  the same `chunk_id`
- render policy hash is persisted and attached to the derived semantic
  generation
- retryable and terminal failures are distinguishable in persisted job state

## 6. Non-goals

- query embedding
- public API cutover
- model/version manifest storage
