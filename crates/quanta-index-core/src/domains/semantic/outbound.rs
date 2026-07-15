use std::collections::BTreeSet;

use quanta_index_contract::{
    BatchPublishReceipt, LexicalCandidate, ManifestGeneration, RepoId, RevisionId,
    SemanticCorpusKindV1, SemanticIngestBatch,
};

use crate::error::CoreError;

/// Ingest a typed semantic batch into the direct authority path. QI-RT-01
/// counterpart to [`crate::SearchCorpusIngestPort`].
pub trait SemanticIngestPort: Send + Sync {
    fn publish_batch(&self, batch: &SemanticIngestBatch) -> Result<BatchPublishReceipt, CoreError>;
}

/// Produces embedding vectors for text.
///
/// This is the single embedder seam shared by BOTH the query path and corpus
/// derivation, so the two can never disagree on model identity (the query-time
/// model-identity gate compares [`Self::model_id`]/[`Self::model_version`]
/// against the indexed generation's).
///
/// Vectors are unit-normalized (cosine-comparable). `embed_batch` returns exactly
/// one vector per input, in input order, and fails the whole batch closed on any
/// error — a partial/misaligned batch must never reach the index.
pub trait TextEmbeddingProvider: Send + Sync {
    /// Embed `texts` into one vector each, in input order. Errors fail closed.
    fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, CoreError>;

    /// Stable identity of the model these vectors come from.
    fn model_id(&self) -> &str;

    /// Optional model version/snapshot paired with [`Self::model_id`].
    fn model_version(&self) -> Option<&str>;

    /// Output vector dimension every returned vector must have.
    fn dimension(&self) -> usize;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SemanticReadiness {
    pub manifest_generation: ManifestGeneration,
    pub materialized: bool,
}

/// Build / replay a typed semantic ingest batch into a semantic index for a
/// given generation.
///
/// This is the batch-native authority surface used by the direct ingest path.
/// Implementations may internally lower into legacy op handlers, but callers
/// do not construct or route channel ops on the hot path.
pub trait SemanticBatchBuildPort: Send + Sync {
    fn build_batch(&self, batch: &SemanticIngestBatch) -> Result<(), CoreError>;
}

pub trait SemanticIndexOpenPort: Send + Sync {
    fn open(
        &self,
        repo: &RepoId,
        revision: &RevisionId,
        generation: ManifestGeneration,
    ) -> Result<Box<dyn SemanticSearcher>, CoreError>;
}

#[derive(Clone, Debug, PartialEq)]
pub struct SemanticSearchHitV1 {
    pub candidate: LexicalCandidate,
    pub record_id: String,
    pub owner_id: String,
    pub corpus_kind: Option<SemanticCorpusKindV1>,
}

pub trait SemanticSearcher: Send + Sync {
    /// Embed the query externally and pass the dense vector to the searcher.
    /// Returning candidates as `LexicalCandidate` keeps the result shape uniform
    /// for the hybrid orchestrator's RRF fusion (`candidate_id`, score, snippet).
    fn search(&self, query_vector: &[f32], top_k: u32) -> Result<Vec<LexicalCandidate>, CoreError>;

    /// Search globally and preserve stable record/owner identity for seed
    /// assembly paths that must fuse on entity rather than lexical candidate ID.
    fn search_hits(
        &self,
        query_vector: &[f32],
        top_k: u32,
    ) -> Result<Vec<SemanticSearchHitV1>, CoreError>;

    /// Search within a lexical allowlist. Callers rely on this for exact scope
    /// semantics rather than global-top-k followed by post-filtering.
    ///
    /// An empty `allowed_ids` yields an empty result (`Ok(vec![])`): the scope
    /// admits nothing, so there is nothing to rank. Implementors must honor this
    /// rather than treating an empty scope as "unscoped"/global search.
    fn search_scoped(
        &self,
        query_vector: &[f32],
        allowed_ids: &BTreeSet<String>,
        top_k: u32,
    ) -> Result<Vec<LexicalCandidate>, CoreError>;

    /// Model identity the indexed vectors were built with (from the generation's
    /// persisted manifest). The query path compares this against the query
    /// embedder's model identity and fails closed if they differ — a query
    /// vector from a different model is not cosine-comparable even at equal
    /// dimension.
    fn index_model_id(&self) -> &str;

    /// Optional model version paired with [`Self::index_model_id`].
    fn index_model_version(&self) -> Option<&str>;
}
