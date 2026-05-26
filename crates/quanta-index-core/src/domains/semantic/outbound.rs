use std::collections::BTreeSet;

use quanta_index_contract::channel::SemanticChannelOp;
use quanta_index_contract::{
    BatchPublishReceipt, LexicalCandidate, ManifestGeneration, RepoId, RevisionId,
    SemanticIngestBatch,
};

use crate::error::CoreError;

/// Ingest a typed semantic batch into the direct authority path. QI-RT-01
/// counterpart to [`crate::LexicalIngestPort`].
pub trait SemanticIngestPort: Send + Sync {
    fn publish_batch(&self, batch: &SemanticIngestBatch) -> Result<BatchPublishReceipt, CoreError>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SemanticReadiness {
    pub manifest_generation: ManifestGeneration,
    pub materialized: bool,
}

pub trait SemanticIndexBuildPort: Send + Sync {
    fn build(
        &self,
        repo: &RepoId,
        revision: &RevisionId,
        generation: ManifestGeneration,
        ops: &[SemanticChannelOp],
    ) -> Result<(), CoreError>;
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

pub trait SemanticSearcher: Send + Sync {
    /// Embed the query externally and pass the dense vector to the searcher.
    /// Returning candidates as `LexicalCandidate` keeps the result shape uniform
    /// for the hybrid orchestrator's RRF fusion (`candidate_id`, score, snippet).
    fn search(&self, query_vector: &[f32], top_k: u32) -> Result<Vec<LexicalCandidate>, CoreError>;

    /// Search within a lexical allowlist. Callers rely on this for exact scope
    /// semantics rather than global-top-k followed by post-filtering.
    fn search_scoped(
        &self,
        query_vector: &[f32],
        allowed_ids: &BTreeSet<String>,
        top_k: u32,
    ) -> Result<Vec<LexicalCandidate>, CoreError>;

    /// Resolve a server-side semantic-vector handle to the concrete vector for
    /// the opened generation. Implementations must fail closed when the handle
    /// is absent rather than degrading to an empty result.
    fn resolve_handle(&self, handle: &str) -> Result<Vec<f32>, CoreError>;
}
