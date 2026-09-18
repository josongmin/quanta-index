use std::collections::BTreeSet;

use quanta_index_contract::{
    BatchPublishReceipt, ClusterMembershipBatchReadRequestV1, ClusterMembershipBatchReadResponseV1,
    EmbeddingNormalization, LexicalCandidate, ManifestGeneration, OwnerDocKind,
    QueryConstraintSetV1, RepoId, RevisionId, SemanticCorpusKindV1,
};

use crate::domains::semantic::stream::{SemanticIngestHeaderV1, SemanticScopeSource};
use crate::error::CoreError;
use crate::request_budget::RequestBudgetV1;

/// Ingest one semantic batch into the direct authority path, streamed one
/// window of scopes at a time (QI-BB-021). QI-RT-01 counterpart to
/// [`crate::SearchCorpusIngestPort`].
///
/// `header` is everything of the batch but its replace scopes; `scopes`
/// issues those, embedded, in windows the port's build policy bounds. The
/// receipt acknowledges what the build appended.
pub trait SemanticIngestPort: Send + Sync {
    fn publish_stream(
        &self,
        header: &SemanticIngestHeaderV1,
        scopes: &mut dyn SemanticScopeSource,
    ) -> Result<BatchPublishReceipt, CoreError>;
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

pub trait SemanticIndexOpenPort: Send + Sync {
    fn open(
        &self,
        repo: &RepoId,
        revision: &RevisionId,
        generation: ManifestGeneration,
    ) -> Result<Box<dyn SemanticSearcher>, CoreError>;
}

/// How a sealed generation serves its dense lane (QI-BB-027).
///
/// The seal records which index the lane runs through and how much effort a
/// query spends in it; the open verifies the dataset against that record, so
/// a query never runs on an index topology nobody sealed. The searcher
/// reports the contract so explanations can name it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DenseLaneContractV1 {
    /// The index the lane runs through.
    pub index: DenseIndexV1,
    /// Whether the seal recorded this contract and the open proved it.
    pub attestation: DenseLaneAttestationV1,
}

/// The index a dense lane runs through.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DenseIndexV1 {
    /// Every row is scored; the result is exact.
    Exact,
    /// An approximate index: the bounded effort one query spends in it, and
    /// where its centroids came from.
    Approximate {
        effort: DenseIndexEffortV1,
        lineage: DenseIndexLineageV1,
    },
}

/// Where an approximate index's centroids came from, as the seal recorded
/// it and the open verified it (QI-BB-027 W3).
///
/// A delta seal may append its rows to the inherited index instead of
/// retraining it; the centroids then date from an earlier generation and a
/// growing share of the served rows was never part of their training set.
/// The trace names that share so a recall question can be answered from
/// the explanation alone.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DenseIndexLineageV1 {
    /// The seal recorded the training generation and every row assigned to
    /// or removed from those centroids since.
    Recorded(DenseIndexTrainingV1),
    /// The seal predates the lineage record, or there was no seal: nothing
    /// is known about how the index was trained.
    Unrecorded,
}

/// The training record of an approximate index.
///
/// The live coverage is `trained_rows + appended_rows - deleted_rows`; the
/// adapter refuses a seal whose record does not add up to what the index
/// covers.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DenseIndexTrainingV1 {
    /// The generation whose seal trained the centroids.
    pub trained_at_generation: u64,
    /// The rows the centroids were trained on: every row that seal covered.
    pub trained_rows: u64,
    /// Rows later seals assigned to those centroids without retraining,
    /// cumulative.
    pub appended_rows: u64,
    /// Rows removed from the index since training, cumulative.
    pub deleted_rows: u64,
}

/// The bounded effort one query spends in an approximate index.
///
/// The candidate list an index returns is `refine_factor` times the
/// requested top-k, re-ranked by exact distance; the graph search widens to
/// `max(ef_floor, ef_per_candidate * candidates)` entries and probes
/// `nprobes` of the `partitions` partitions.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DenseIndexEffortV1 {
    /// The algorithm family, e.g. `ivf_hnsw_sq`.
    pub index_kind: String,
    pub partitions: u32,
    pub nprobes: u32,
    pub ef_floor: u32,
    pub ef_per_candidate: u32,
    pub refine_factor: u32,
}

/// Whether a dense lane's contract was sealed and verified.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DenseLaneAttestationV1 {
    /// The seal recorded the contract, the open verified the index against
    /// it, and the library serving it is the one that built it.
    Sealed,
    /// As `Sealed`, but a different version of the library built the index
    /// than serves it: the metadata agrees, the recall was measured elsewhere.
    SealedByAnotherLibraryVersion,
    /// The generation predates the contract; the lane runs on what the
    /// dataset reports and nothing proved it.
    LegacyUnverified,
}

impl DenseLaneContractV1 {
    /// One-line, key=value rendering for planner traces.
    #[must_use]
    pub fn trace_detail(&self) -> String {
        let attestation = match self.attestation {
            DenseLaneAttestationV1::Sealed => "sealed",
            DenseLaneAttestationV1::SealedByAnotherLibraryVersion => {
                "sealed_by_another_library_version"
            }
            DenseLaneAttestationV1::LegacyUnverified => "legacy_unverified",
        };
        match &self.index {
            DenseIndexV1::Exact => format!("dense.index=exact; dense.attestation={attestation}"),
            DenseIndexV1::Approximate { effort, lineage } => format!(
                "dense.index={}; dense.attestation={attestation}; dense.partitions={}; dense.nprobes={}; dense.ef=max({},{}*candidates); dense.refine_factor={}; {}",
                effort.index_kind,
                effort.partitions,
                effort.nprobes,
                effort.ef_floor,
                effort.ef_per_candidate,
                effort.refine_factor,
                lineage.trace_detail()
            ),
        }
    }
}

impl DenseIndexLineageV1 {
    /// The `ann.*` keys of the planner trace: the training generation and
    /// the rows appended to and deleted from it since, or that no record
    /// exists.
    #[must_use]
    pub fn trace_detail(&self) -> String {
        match self {
            Self::Recorded(training) => format!(
                "ann.trained_at=g{}; ann.appended={}/{}; ann.deleted={}",
                training.trained_at_generation,
                training.appended_rows,
                training.trained_rows,
                training.deleted_rows
            ),
            Self::Unrecorded => "ann.lineage=unrecorded".to_string(),
        }
    }
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

/// Searcher handle returned by [`SemanticIndexOpenPort::open`].
///
/// One per opened sealed generation, shareable across concurrent queries;
/// residency is the search plane's snapshot registry's job.
///
/// Every dense search takes the request's [`RequestBudgetV1`] (W5 phase 3):
/// an implementation observes it inside the lane — before the vector query
/// is issued, while it is in flight, and as its rows are read — and
/// answers the typed interruption naming the lane that looked
/// (`semantic:ann` for an approximate lane, `semantic:exact` for an exact
/// one), so a peer that left or a deadline that passed stops the work
/// there rather than at the next checkpoint outside the adapter.
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
    fn search(
        &self,
        query_vector: &[f32],
        top_k: u32,
        budget: &RequestBudgetV1,
    ) -> Result<Vec<LexicalCandidate>, CoreError>;

    fn search_constrained(
        &self,
        query_vector: &[f32],
        constraints: &QueryConstraintSetV1,
        top_k: u32,
        budget: &RequestBudgetV1,
    ) -> Result<Vec<LexicalCandidate>, CoreError> {
        if constraints.is_unconstrained() {
            self.search(query_vector, top_k, budget)
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
        budget: &RequestBudgetV1,
    ) -> Result<Vec<SemanticSearchHitV1>, CoreError>;

    fn search_hits_constrained(
        &self,
        query_vector: &[f32],
        constraints: &QueryConstraintSetV1,
        top_k: u32,
        budget: &RequestBudgetV1,
    ) -> Result<Vec<SemanticSearchHitV1>, CoreError> {
        if constraints.is_unconstrained() {
            self.search_hits(query_vector, top_k, budget)
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
        budget: &RequestBudgetV1,
    ) -> Result<Vec<SemanticSearchHitV1>, CoreError>;

    fn search_hits_for_corpus_constrained(
        &self,
        query_vector: &[f32],
        corpus_kind: SemanticCorpusKindV1,
        constraints: &QueryConstraintSetV1,
        top_k: u32,
        budget: &RequestBudgetV1,
    ) -> Result<Vec<SemanticSearchHitV1>, CoreError> {
        if constraints.is_unconstrained() {
            self.search_hits_for_corpus(query_vector, corpus_kind, top_k, budget)
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
        budget: &RequestBudgetV1,
    ) -> Result<Vec<LexicalCandidate>, CoreError>;

    fn search_scoped_constrained(
        &self,
        query_vector: &[f32],
        allowed_ids: &BTreeSet<String>,
        constraints: &QueryConstraintSetV1,
        top_k: u32,
        budget: &RequestBudgetV1,
    ) -> Result<Vec<LexicalCandidate>, CoreError> {
        if constraints.is_unconstrained() {
            self.search_scoped(query_vector, allowed_ids, top_k, budget)
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

    /// The index this searcher's dense lane runs through, and whether the
    /// seal recorded and the open verified it (QI-BB-027).
    fn dense_lane(&self) -> DenseLaneContractV1;
}
