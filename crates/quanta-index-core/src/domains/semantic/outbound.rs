use std::collections::BTreeSet;

use quanta_index_contract::{
    BatchPublishReceipt, ClusterMembershipBatchReadRequestV1, ClusterMembershipBatchReadResponseV1,
    EmbeddingNormalization, GenerationSnapshot, LexicalCandidate, ManifestGeneration, OwnerDocKind,
    QueryConstraintSetV1, RepoId, RevisionId, SemanticContentRootsV1, SemanticCorpusKindV1,
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
/// receipt acknowledges what the build appended, independently from the
/// transient report returned alongside it.
pub trait SemanticIngestPort: Send + Sync {
    fn publish_stream(
        &self,
        header: &SemanticIngestHeaderV1,
        scopes: &mut dyn SemanticScopeSource,
    ) -> Result<
        (
            BatchPublishReceipt,
            quanta_index_contract::IngestStageReport,
        ),
        CoreError,
    >;
}

/// The checkpoint every budget-observing embedder names when the budget
/// interrupts it (QI-BB-002).
pub const EMBED_CHECKPOINT: &str = "semantic:embed";

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

    /// Embed `texts` under a request budget (QI-BB-002).
    ///
    /// A provider whose embedding is outbound I/O observes the budget
    /// inside that I/O — a deadline caps every attempt, a cancellation
    /// abandons the attempt in flight — and answers with the typed
    /// interruption at the `semantic:embed` checkpoint. The provided body
    /// is the whole contract for a provider with nothing to interrupt: one
    /// checkpoint, then the batch.
    fn embed_batch_within(
        &self,
        texts: &[&str],
        budget: &RequestBudgetV1,
    ) -> Result<Vec<Vec<f32>>, CoreError> {
        budget.checkpoint(EMBED_CHECKPOINT)?;
        self.embed_batch(texts)
    }

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

/// Wire code for an activation that names semantic content roots the
/// physical generation does not carry (QI-BB-028).
pub const SEMANTIC_ROW_ROOT_MISMATCH_CODE: quanta_index_contract::SearchPlaneErrorCodeV2 =
    quanta_index_contract::SearchPlaneErrorCodeV2::SemanticRowRootMismatch;

/// The content roots a sealed semantic generation carries (QI-BB-028).
///
/// The sealed scope manifest records the row root over every sealed row
/// (vectors included) and the cluster-membership root; this port reads
/// them back for the identity a producer names, so a seal receipt can
/// attest them and an activation can be refused when the physical
/// generation's roots are not the ones named
/// ([`SEMANTIC_ROW_ROOT_MISMATCH_CODE`]). Implementations read the sealed
/// manifest only — no rows, no dataset bytes — and refuse an unsealed or
/// digest-mismatched generation typed.
pub trait SemanticContentRootsPort: Send + Sync {
    fn sealed_content_roots(
        &self,
        sealed: &GenerationSnapshot,
    ) -> Result<SemanticContentRootsV1, CoreError>;
}

pub trait SemanticIndexOpenPort: Send + Sync {
    fn open(
        &self,
        repo: &RepoId,
        revision: &RevisionId,
        generation: ManifestGeneration,
    ) -> Result<Box<dyn SemanticSearcher>, CoreError>;
    /// Prove `candidate` and return the handle the proof opened.
    ///
    /// The proof is the activation validator's: the sealed identity on
    /// disk is exactly `candidate` (digest included), every committed file
    /// is what the seal committed, and the generation directory is durable.
    /// Activation, rollback and restart call this once per track, so the
    /// proof and the handle the first query is served from are one open
    /// (QI-BB-017 보완 #4, QI-BB-030 완료 기준 #2).
    fn open_proven(
        &self,
        candidate: &GenerationSnapshot,
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
    /// An approximate index: the bounded effort one query spends in it,
    /// where its centroids came from, and how each of its segments was
    /// built.
    Approximate {
        effort: DenseIndexEffortV1,
        lineage: DenseIndexTrainingV1,
        build: DenseIndexBuildV1,
    },
}

/// The training record of an approximate index, as the seal recorded it
/// and the open verified it (QI-BB-027 W3).
///
/// A delta seal may append its rows to the inherited index instead of
/// retraining it; the centroids then date from an earlier generation and a
/// growing share of the served rows was never part of their training set.
/// The trace names that share so a recall question can be answered from
/// the explanation alone. The live coverage is
/// `trained_rows + appended_rows - deleted_rows`; the adapter refuses a
/// seal whose record does not add up to what the index covers.
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

/// How every segment of an approximate index was built, as the library
/// reported it back at seal and again at open (QI-BB-027).
///
/// The trained segment carries the policy's graph recipe; segments a delta
/// appended were built by the library's incremental builder under its own
/// parameters, which the seal records verbatim rather than claiming the
/// recipe for them.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DenseIndexBuildV1 {
    /// Graph neighbours per node and construction beam width of the
    /// trained segment: the policy's recipe.
    pub hnsw_m: u32,
    pub hnsw_ef_construction: u32,
    /// Each appended segment's `(m, ef_construction)`, in segment order.
    pub appended_segments: Vec<DenseIndexSegmentBuildV1>,
}

/// The graph parameters one appended segment was actually built with.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DenseIndexSegmentBuildV1 {
    pub hnsw_m: u32,
    pub hnsw_ef_construction: u32,
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
///
/// Every served generation carries a seal (a generation without one is
/// refused at open, never served on what its dataset happens to report),
/// so the only question left is which library version built the index.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DenseLaneAttestationV1 {
    /// The seal recorded the contract, the open verified the index against
    /// it, and the library serving it is the one that built it.
    Sealed,
    /// As `Sealed`, but a different version of the library built the index
    /// than serves it: the metadata agrees, the recall was measured elsewhere.
    SealedByAnotherLibraryVersion,
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
        };
        match &self.index {
            DenseIndexV1::Exact => format!("dense.index=exact; dense.attestation={attestation}"),
            DenseIndexV1::Approximate {
                effort,
                lineage,
                build,
            } => format!(
                "dense.index={}; dense.attestation={attestation}; dense.partitions={}; dense.nprobes={}; dense.ef=max({},{}*candidates); dense.refine_factor={}; {}; {}",
                effort.index_kind,
                effort.partitions,
                effort.nprobes,
                effort.ef_floor,
                effort.ef_per_candidate,
                effort.refine_factor,
                lineage.trace_detail(),
                build.trace_detail()
            ),
        }
    }
}

impl DenseIndexTrainingV1 {
    /// The `ann.*` keys of the planner trace: the training generation and
    /// the rows appended to and deleted from it since.
    #[must_use]
    pub fn trace_detail(&self) -> String {
        format!(
            "ann.trained_at=g{}; ann.appended={}/{}; ann.deleted={}",
            self.trained_at_generation, self.appended_rows, self.trained_rows, self.deleted_rows
        )
    }
}

impl DenseIndexBuildV1 {
    /// The `ann.hnsw_*` keys of the planner trace: the trained segment's
    /// recipe, then each appended segment's actual construction beam width
    /// so a recall question about appended rows is answerable from the
    /// explanation alone.
    #[must_use]
    pub fn trace_detail(&self) -> String {
        let appended = if self.appended_segments.is_empty() {
            "none".to_string()
        } else {
            self.appended_segments
                .iter()
                .map(|segment| format!("{}/{}", segment.hnsw_m, segment.hnsw_ef_construction))
                .collect::<Vec<_>>()
                .join(",")
        };
        format!(
            "ann.hnsw_m={}; ann.hnsw_ef_construction={}; ann.appended_segments={}; ann.appended_segments_m/ef_construction={appended}",
            self.hnsw_m,
            self.hnsw_ef_construction,
            self.appended_segments.len()
        )
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

    /// Validate the query vector against this opened generation without
    /// issuing a search. Routes use this before an empty scope can return,
    /// so a dimension or vector-contract error cannot become a false empty.
    fn validate_query_vector(&self, query_vector: &[f32]) -> Result<(), CoreError>;

    /// The exact cosine similarity between `query_vector` and the vector
    /// this generation stores for `candidate_id`, or `None` when the
    /// generation holds no vector for it (QI-BB-022).
    ///
    /// A direct lookup of one stored row: it bypasses any approximate
    /// index, so an explain reconciles a carried dense score against the
    /// stored vector itself and never against a neighbour search's recall.
    /// Observes `budget` like every dense read.
    fn score_candidate(
        &self,
        candidate_id: &str,
        query_vector: &[f32],
        budget: &RequestBudgetV1,
    ) -> Result<Option<f32>, CoreError>;

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

    /// The sealed manifest digest this open proved: what an activation
    /// compares with its candidate before promoting the handle into the
    /// snapshot registry, so a promoted handle is exactly the identity
    /// the durable activation names.
    fn manifest_digest(&self) -> &str;

    /// The index this searcher's dense lane runs through, and whether the
    /// seal recorded and the open verified it (QI-BB-027).
    fn dense_lane(&self) -> DenseLaneContractV1;
}
