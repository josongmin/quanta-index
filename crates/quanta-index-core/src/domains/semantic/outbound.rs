use std::collections::BTreeSet;

use quanta_index_contract::{
    BatchPublishReceipt, ClusterMembershipBatchReadRequestV1, ClusterMembershipBatchReadResponseV1,
    EmbeddingNormalization, LexicalCandidate, ManifestGeneration, OwnerDocKind,
    QueryConstraintSetV1, RepoId, RevisionId, SemanticCorpusKindV1, SemanticIngestBatch,
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
/// model-identity gate compares [`Self::model_id`]/[`Self::model_revision`]
/// against the indexed generation's).
///
/// `embed_batch` returns exactly one vector per input, in input order, and
/// fails the whole batch closed on any error — a partial/misaligned batch
/// must never reach the index. What the vectors' bytes promise is stated by
/// [`Self::normalization`]: a raw provider says [`EmbeddingNormalization::None`]
/// and the composition root wraps it in
/// [`L2UnitEmbeddingProvider`](crate::L2UnitEmbeddingProvider) before either
/// path sees it, so every served or sealed vector is unit-normalized by the
/// same code (QI-BB-031).
pub trait TextEmbeddingProvider: Send + Sync {
    /// Embed `texts` into one vector each, in input order. Errors fail closed.
    fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, CoreError>;

    /// Stable identity of the model these vectors come from.
    fn model_id(&self) -> &str;

    /// The immutable revision of that model these vectors come from
    /// (QI-BB-028). Two providers with the same `model_id` and different
    /// revisions produce vectors that are not comparable and must not share
    /// a cache namespace, a sealed generation, or a query gate; a provider
    /// that cannot name its revision cannot be composed into a runtime.
    fn model_revision(&self) -> &str;

    /// Output vector dimension every returned vector must have.
    fn dimension(&self) -> usize;

    /// What every returned vector's bytes promise.
    fn normalization(&self) -> EmbeddingNormalization;
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
    pub owner_kind: OwnerDocKind,
    pub corpus_kind: Option<SemanticCorpusKindV1>,
    pub authority_digest: String,
}

pub trait SemanticSearcher: Send + Sync {
    /// Bytes this handle keeps resident while open (the dataset files it
    /// maps plus decoded manifests). An open-time estimate consumed by the
    /// search plane's snapshot registry byte budget; see the lexical
    /// counterpart for the monotonicity requirement.
    fn resident_bytes_estimate(&self) -> u64;

    /// Read a bounded set of structured `ClusterCard` memberships from this
    /// exact sealed generation through one storage operation.
    fn cluster_membership_batch_read(
        &self,
        request: &ClusterMembershipBatchReadRequestV1,
    ) -> Result<ClusterMembershipBatchReadResponseV1, CoreError>;

    /// Embed the query externally and pass the dense vector to the searcher.
    /// Returning candidates as `LexicalCandidate` keeps the result shape uniform
    /// for the hybrid orchestrator's RRF fusion (`candidate_id`, score, snippet).
    fn search(&self, query_vector: &[f32], top_k: u32) -> Result<Vec<LexicalCandidate>, CoreError>;

    fn search_constrained(
        &self,
        query_vector: &[f32],
        constraints: &QueryConstraintSetV1,
        top_k: u32,
    ) -> Result<Vec<LexicalCandidate>, CoreError> {
        if constraints.is_unconstrained() {
            self.search(query_vector, top_k)
        } else {
            Err(CoreError::NotImplemented(
                "semantic searcher does not provide native query-constraint pushdown".to_string(),
            ))
        }
    }

    /// Search globally and preserve stable record/owner identity for seed
    /// assembly paths that must fuse on entity rather than lexical candidate ID.
    fn search_hits(
        &self,
        query_vector: &[f32],
        top_k: u32,
    ) -> Result<Vec<SemanticSearchHitV1>, CoreError>;

    fn search_hits_constrained(
        &self,
        query_vector: &[f32],
        constraints: &QueryConstraintSetV1,
        top_k: u32,
    ) -> Result<Vec<SemanticSearchHitV1>, CoreError> {
        if constraints.is_unconstrained() {
            self.search_hits(query_vector, top_k)
        } else {
            Err(CoreError::NotImplemented(
                "semantic hit searcher does not provide native query-constraint pushdown"
                    .to_string(),
            ))
        }
    }

    /// Search one logical corpus with a storage-level prefilter. Implementors
    /// must not emulate this as global top-k followed by in-memory filtering,
    /// because that loses corpus-local recall before ranking.
    fn search_hits_for_corpus(
        &self,
        query_vector: &[f32],
        corpus_kind: SemanticCorpusKindV1,
        top_k: u32,
    ) -> Result<Vec<SemanticSearchHitV1>, CoreError>;

    fn search_hits_for_corpus_constrained(
        &self,
        query_vector: &[f32],
        corpus_kind: SemanticCorpusKindV1,
        constraints: &QueryConstraintSetV1,
        top_k: u32,
    ) -> Result<Vec<SemanticSearchHitV1>, CoreError> {
        if constraints.is_unconstrained() {
            self.search_hits_for_corpus(query_vector, corpus_kind, top_k)
        } else {
            Err(CoreError::NotImplemented(
                "semantic corpus searcher does not provide native query-constraint pushdown"
                    .to_string(),
            ))
        }
    }

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

    fn search_scoped_constrained(
        &self,
        query_vector: &[f32],
        allowed_ids: &BTreeSet<String>,
        constraints: &QueryConstraintSetV1,
        top_k: u32,
    ) -> Result<Vec<LexicalCandidate>, CoreError> {
        if constraints.is_unconstrained() {
            self.search_scoped(query_vector, allowed_ids, top_k)
        } else {
            Err(CoreError::NotImplemented(
                "scoped semantic searcher does not provide native query-constraint pushdown"
                    .to_string(),
            ))
        }
    }

    /// Model identity the indexed vectors were built with (from the generation's
    /// persisted manifest). The query path compares this against the query
    /// embedder's model identity and fails closed if they differ — a query
    /// vector from a different model is not cosine-comparable even at equal
    /// dimension.
    fn index_model_id(&self) -> &str;

    /// The model revision the sealed generation recorded, paired with
    /// [`Self::index_model_id`]. `None` means the generation was sealed
    /// before revisions were required (QI-BB-028); the query gate refuses
    /// it rather than guessing.
    fn index_model_revision(&self) -> Option<&str>;
}
