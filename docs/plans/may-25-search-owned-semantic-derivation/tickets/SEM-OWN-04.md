# SEM-OWN-04 — Query Text Embedding and Public Search-surface Cutover

Status: `proposed`
Parent: [../README.md](../README.md)
Depends on: [SEM-OWN-03.md](SEM-OWN-03.md)

## 1. Purpose

Align the public query API with search-owned embedding. Users send text; the
search plane embeds it internally against the selected generation manifest.

## 2. Required interface decision

Stable public surface:

```rust
pub struct SearchPlaneSemanticQueryRequest {
    pub query_text: String,
    pub generation: Option<GenerationPin>,
    pub generation_selector: Option<GenerationSelector>,
    pub lexical_scope: Option<SearchPlaneLexicalTextQueryRequest>,
    pub top_k: u32,
}

pub struct SearchPlaneHybridQueryRequest {
    pub lexical: SearchPlaneLexicalTextQueryRequest,
    pub semantic_query_text: String,
    pub generation: Option<GenerationPin>,
    pub generation_selector: Option<GenerationSelector>,
    pub top_k: u32,
}
```

Remove from the stable public request contract:

- `query_vector`
- `query_vector_ref`
- `semantic_vector`
- `semantic_vector_ref`

## 3. Required runtime interface

```rust
pub struct NormalizedSemanticQuery {
    pub text: String,
    pub normalized_query_hash: [u8; 32],
}

pub struct QueryEmbeddingCacheKey {
    pub normalized_query_hash: [u8; 32],
    pub manifest_hash: [u8; 32],
}

pub trait QueryNormalizer: Send + Sync {
    fn normalize(&self, text: &str, manifest: &SemanticBundleManifest) -> Result<NormalizedSemanticQuery, CoreError>;
}

pub trait QueryTextEmbedderPort: Send + Sync {
    fn embed_query(&self, query: &NormalizedSemanticQuery, manifest: &SemanticBundleManifest) -> Result<Vec<f32>, CoreError>;
}
```

The dispatcher must resolve the active or pinned semantic manifest first, then
normalize and embed the query text using that exact provider/model/dim contract.

## 4. Deliverables

- query dispatcher cutover to internal query embedding
- `searchctl` cutover to text-only semantic/hybrid public CLI
- removal of numeric-vector-text fallback from the public path
- explicit typed error for embedder failures on query time
- normalized-query cache contract keyed by `QueryEmbeddingCacheKey`

## 5. Acceptance

- semantic query e2e succeeds from plain text only
- hybrid query e2e succeeds from plain text only
- old public vector flags or wire fields fail closed instead of silently
  behaving as the main API
- query-time dim drift cannot happen without a typed failure because the
  generation manifest is authoritative
- `FooBar`, `foobar`, `foo_bar`, and `foo bar` have a defined normalization and
  cache-key story rather than ad hoc misses

## 6. Non-goals

- a stable public "vector search" API
- caller-provided semantic handles as a primary query mechanism
