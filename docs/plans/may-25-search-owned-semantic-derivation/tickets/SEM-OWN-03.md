# SEM-OWN-03 — Semantic Manifest, Readiness, and Seal Gating

Status: `proposed`
Parent: [../README.md](../README.md)
Depends on: [SEM-OWN-02.md](SEM-OWN-02.md)

## 1. Purpose

Make search-owned semantic derivation generation-safe. A semantic generation is
not queryable until its corpus embedding work is complete, dimension-consistent,
and bound to a manifest.

## 2. Required interfaces

```rust
pub struct SemanticBundleManifest {
    pub provider: EmbeddingProviderKind,
    pub model: String,
    pub embedding_dim: u32,
    pub render_policy_hash: [u8; 32],
    pub query_normalization_hash: [u8; 32],
}

pub trait SemanticManifestStore: Send + Sync {
    fn put(&self, pin: &GenerationPin, manifest: &SemanticBundleManifest) -> Result<(), CoreError>;
    fn get(&self, pin: &GenerationPin) -> Result<SemanticBundleManifest, CoreError>;
}
```

`SemanticFullBundle.payload` becomes the carrier for the internal semantic
manifest.

Seal must carry a completeness proof:

```rust
pub struct SemanticSealProof {
    pub expected_chunk_count: u64,
    pub embedded_chunk_count: u64,
    pub failed_chunk_count: u64,
    pub skipped_chunk_count: u64,
    pub manifest_hash: [u8; 32],
}
```

Blocked activation must be typed:

```rust
pub enum GenerationBlockedReason {
    SemanticProviderUnavailable,
    SemanticSealIncomplete,
    EmbeddingDimensionMismatch,
    ManifestDriftDetected,
    RetryBudgetExhausted,
}
```

## 3. Required runtime rules

1. one generation -> one manifest
2. one generation -> one embedding dimension
3. semantic `Seal(generation)` is emitted only after all chunk jobs for that
   generation are durably finished
4. activation remains fail-closed until lexical and semantic are both sealed
5. partial semantic derivation must not silently reuse a prior generation's
   ready bit
6. semantic readiness is provable from `SemanticSealProof`, not inferred from
   queue emptiness alone
7. blocked activation reasons are inspectable without log scraping

## 4. Deliverables

- manifest persistence
- pending-job accounting per `(repo, revision, generation)`
- restart recovery for in-flight semantic derivation
- readiness gating wired to semantic seal and manifest presence
- `SemanticSealProof`
- typed `GenerationBlockedReason`

## 5. Acceptance

- restart mid-generation does not produce a false semantic-ready state
- dim mismatch inside one generation is rejected before activation
- query against a generation with lexical seal but no semantic seal returns
  `NOT_READY`
- active generation flips only after both tracks are ready
- semantic seal cannot be emitted without a complete proof object
- blocked activation surfaces a typed reason instead of a generic failure

## 6. Risks to watch

- sealing semantic too early after the lexical seal event is observed
- leaking provider/model changes into an already materialized generation
- treating queue drain as proof of semantic completeness when failed or skipped
  work still exists
