# Search-owned Semantic Derivation

Status: `proposed`
Date: `2026-05-25`
Scope: breaking-first semantic ownership inversion for `quanta-index`

---

## 1. Final decision

`quanta-index` owns both corpus embedding and query embedding.

The external producer no longer authors semantic vectors for the search plane.
The external producer authors raw/searchable records only. Semantic vectors are
derived inside `quanta-index` from chunk text and are published on an
internal-only semantic track.

This decision intentionally keeps:

- `git` history authorship in the producer
- `tree-sitter` / parse-tree authorship in the producer
- symbol extraction authorship in the producer

But it inverts semantic ownership only.

## 2. External interfaces

### 2.1 Producer -> `quanta-index`

Accepted external inputs:

- `UpsertChunk { chunk_id, payload: ChunkRecord }`
- `DeleteChunk { chunk_id }`
- `UpsertSymbol { symbol_id, payload: SymbolRecord }`
- `DeleteSymbol { symbol_id }`
- `UpsertParseTree` / `DeleteParseTree`
- `UpsertCommit` / `UpsertRef` / `UpsertTag` / `DeleteRef` / `DeleteTag`
- `UpsertDirty` / `EvictDirty`
- `Seal`

Forbidden external inputs:

- producer-authored `UpsertEmbedding`
- producer-authored `DeleteEmbedding`
- producer-side corpus embedding model selection
- producer-side semantic manifest authorship

### 2.2 Search query clients -> `quanta-index`

Public query surface becomes text-only for semantic paths.

- `semantic(query_text, top_k, generation, lexical_scope?)`
- `hybrid(lexical_query, semantic_query_text, top_k, generation)`

Public vector/handle query surfaces are removed from the main request schema.
If a debug/admin vector surface is kept, it is not part of the stable user
query contract.

## 3. Canonical data model

### 3.1 Corpus unit

Semantic retrieval identity is `chunk_id`.

Reasons:

- lexical main search already runs on the text-doc / chunk plane
- hybrid lexical-universe pushdown scopes semantic by lexical `candidate_id`
- delete cascade already assumes `embedding_id == chunk_id`

`symbol` stays an auxiliary index, not the primary semantic corpus identity.

### 3.2 External chunk payload

The canonical producer payload is:

```rust
pub struct ChunkRecord {
    pub repo_relative_path: RepoRelativePath,
    pub language: LangId,
    pub start_line: u32,
    pub end_line: u32,
    pub text: Box<str>,
}
```

`text` is the canonical source for both lexical indexing and semantic
derivation. The old `snippet` field is removed rather than shimmed.

### 3.3 Internal semantic manifest

Each semantic generation carries an internal manifest:

```rust
pub struct SemanticBundleManifest {
    pub provider: EmbeddingProviderKind,
    pub model: String,
    pub embedding_dim: u32,
    pub render_policy_hash: [u8; 32],
    pub query_normalization_hash: [u8; 32],
}
```

The manifest is authored by `quanta-index`, not by the external producer.

`render_policy_hash` is the stable authority, not a single renderer version
integer. It must fingerprint every embedding-affecting rendering decision:

- chunk template
- normalization rules
- context injection policy
- truncation policy
- language-specific rendering policy

### 3.4 Stable chunk identity contract

`chunk_id` is a stable semantic-region identity, not a line-range identity.

Required properties:

- whitespace-only edits should not force identity churn
- import reorder should not force identity churn when the semantic region is
  otherwise unchanged
- semantically identical regenerated chunks should preserve identity

Initial delivery rule:

- if the producer cannot yet guarantee stable `chunk_id`, semantic derivation is
  still allowed to ship, but semantic reuse must stay generation-full rather
  than incremental. Full-generation rebuild is acceptable. False incremental
  reuse is not.

## 4. Internal components

Required internal surfaces:

- `SemanticDerivationWorker`
- `EmbeddingClient`
- `EmbeddingRenderer`
- `SemanticManifestStore`
- `QueryTextEmbedderPort`
- `QueryNormalizer`
- `QueryEmbeddingCache`
- `SemanticJobStore`

Canonical worker flow:

1. subscribe to lexical channel
2. consume `UpsertChunk` / `DeleteChunk` / `Seal`
3. render chunk text into embedding input
4. call external embedding API
5. publish internal `UpsertEmbedding` / `DeleteEmbedding`
6. write semantic manifest + semantic seal after all chunk jobs complete

## 5. Invariants

- `embedding_id == chunk_id`
- one semantic generation uses exactly one `(provider, model, dim, render_policy_hash, query_normalization_hash)`
- semantic seal is emitted only after all chunk-derived embedding jobs for that
  generation finish durably
- query embedding uses the same manifest selected for the corpus generation
- query embedding cache keys are derived from normalized query text plus the
  selected manifest
- generation activation remains fail-closed: lexical and semantic must both be
  ready before the generation is active
- model change is generation-scoped, never mixed within one generation
- semantic seal carries an explicit completeness proof
- blocked activation reasons are typed, not free-form log strings

## 6. Current delivery boundary

The first delivery is intentionally narrower than the full long-term semantic
platform.

Shipped now:

- chunk-owned semantic corpus
- search-owned corpus embedding
- search-owned query embedding
- manifest-guided generation freeze
- fail-closed readiness and activation
- query embedding cache contract
- semantic job lifecycle persistence

Deferred but structurally prepared:

- symbol-projection semantic lane
- more advanced semantic-region identity generation
- learned hybrid weighting
- multi-tier query embedding caches
- richer rendering-policy introspection beyond a stable hash

## 7. Why the previous shape is being replaced

The older semantic design optimized for a pure "producer-authored everything"
split. That is still reasonable for parse trees, history records, and symbol
records, but it overfits semantic vectors.

Semantic vectors are tightly coupled to search-owned concerns:

- retrieval unit identity
- lexical-universe scoped hybrid search
- model/provider rollout
- embedding dimension compatibility
- query-time embedding generation

Those concerns belong inside `quanta-index`.

## 8. Execution units

Ticket pack:

- [tickets/INDEX.md](tickets/INDEX.md)
- [tickets/SEM-OWN-00.md](tickets/SEM-OWN-00.md)
- [tickets/SEM-OWN-01.md](tickets/SEM-OWN-01.md)
- [tickets/SEM-OWN-02.md](tickets/SEM-OWN-02.md)
- [tickets/SEM-OWN-03.md](tickets/SEM-OWN-03.md)
- [tickets/SEM-OWN-04.md](tickets/SEM-OWN-04.md)
- [tickets/SEM-OWN-05.md](tickets/SEM-OWN-05.md)

Wave order:

1. boundary freeze
2. ingest contract cutover
3. corpus embedding derivation worker
4. semantic manifest + seal/readiness
5. query text embedding cutover
6. cleanup + proof

## 9. Non-goals

- symbol-owned semantic corpus as the primary design
- per-request model override
- mixed-model semantic generations
- producer-authored semantic vectors as a stable compatibility mode
- heuristic fallback from failed semantic derivation to silent lexical-only
  success
