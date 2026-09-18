#![forbid(unsafe_code)]
#![deny(unused_must_use)]
#![deny(clippy::let_underscore_must_use)]
#![deny(clippy::map_err_ignore)]

//! Application core for the search-plane. Defines domain ports + policies. Driven
//! adapters (`quanta-index-lexical`, `quanta-index-semantic`, `quanta-index-ipc`,
//! repo-map/storage backends) implement these ports; the composition root in
//! `quanta-index-searchd` wires them together.

pub mod domains;
pub mod error;
pub mod ingest_resource;
pub mod request_budget;
pub mod timeref;

pub use error::{CoreError, validate_internal_fetch_size, validate_query_top_k};
pub use ingest_resource::{
    INGEST_RESOURCE_BUDGET_EXCEEDED_CODE, IngestBatchFootprint, IngestResourcePolicy,
    MAX_EMBEDDING_DIMENSION,
};
pub use request_budget::{
    BudgetInterruptionV1, CancelHandleV1, REQUEST_CANCELLED_CODE, REQUEST_DEADLINE_EXCEEDED_CODE,
    RequestBudgetV1,
};

pub use domains::auxiliary::{
    AUX_EPOCH_EXPIRED_CODE, AUX_EPOCH_RETAIN, AUX_EPOCH_RETAIN_FOR, AUX_EPOCH_UNKNOWN_CODE,
    AuxiliaryAuthorityCatalogPort, AuxiliaryDomainV1, AuxiliaryGenerationKeyV1,
    AuxiliaryMutationBatchV1, AuxiliaryMutationReceiptV1, AuxiliaryRowFamilyV1, AuxiliaryRowKeyV1,
    AuxiliaryRowMutationV1, AuxiliaryRowV1, AuxiliaryTrackRowV1,
};
pub use domains::generation::{
    GenerationIdentityValidatePort, GenerationQuarantineReasonV1, GenerationStorageKeyV1,
    IncompleteGenerationDiscardOutcomeV1, IncompleteGenerationDiscardPort,
    InventoriedSealedGenerationV1, PinnedGenerationReadinessV1,
    QUARANTINE_TARGET_NOT_QUARANTINED_CODE, QuarantineDiscardOutcomeV1,
    QuarantinedGenerationDiscardPort, QuarantinedGenerationV1, SealedArtifactCommitmentV1,
    SealedGenerationBytesV1, SealedGenerationInventoryV1, SealedGenerationReclaimOutcomeV1,
    SealedGenerationReclaimPort, SealedGenerationScanPort, TreeCommitmentMismatchV1,
    UNKNOWN_GENERATION_CODE, commit_tree_v1, sha256_of_file, unique_inode_tree_bytes,
    unknown_generation_error, validate_pinned_generation_v1, verify_tree_commitment_v1,
};
pub use domains::hybrid::{
    DenseAdmissionOutcomeV1, DenseLaneFilterClassV1, ExplainQueryPort, FusedKeyV1, FusedLaneRankV1,
    HYBRID_FILTER_UNSUPPORTED_CODE, HybridFilterPlanV1, HybridOrchestratorPolicy, HybridQueryPort,
    classify_hybrid_filter_v1, dense_admission_round_outcome_v1, hybrid_filter_name_v1,
};
pub use domains::idempotency::{
    BATCH_DIGEST_CONFLICT_CODE, BATCH_DIGEST_MISMATCH_CODE, CATALOG_BUSY_CODE,
    CATALOG_ROW_CORRUPT_CODE, IdempotencyBeginV1, IdempotencyCatalogPort, IdempotencyKeyV1,
};
pub use domains::ingest_body::IngestBatchBodyV1;
pub use domains::integrity::{
    IntegrityScrubPort, IntegrityScrubReportV1, IntegrityScrubStampV1, IntegrityScrubStatusV1,
};
pub use domains::lexical::{
    FileContributorIngestPort, FileOwnershipIngestPort, HISTORY_TEXT_INDEX_CORRUPT_CODE,
    HISTORY_TEXT_INDEX_NORMALIZER_UNSUPPORTED_CODE, HISTORY_TEXT_INDEX_NOT_READY_CODE,
    HISTORY_TEXT_QUERY_UNSCORABLE_CODE, HistoryTextAdmitFn, HistoryTextBuildV1,
    HistoryTextDiscardOutcomeV1, HistoryTextDocKeyV1, HistoryTextDocV1, HistoryTextEpochReceiptV1,
    HistoryTextEpochStatusV1, HistoryTextHitV1, HistoryTextIndexPort, HistoryTextKindV1,
    HistoryTextPageV1, HistoryTextQueryV1, HistoryTextSearcher,
    LEXICAL_EXAMINED_BUDGET_EXCEEDED_CODE, LEXICAL_WRITER_HEAP_BYTES_MAX,
    LEXICAL_WRITER_HEAP_BYTES_MIN, LexicalCandidateExplanationV1, LexicalExecutionBudgetV1,
    LexicalIndexBuildPort, LexicalIndexOpenPort, LexicalPolicy, LexicalQueryPort, LexicalReadiness,
    LexicalScoreEngineV1, LexicalScoreTraceV1, LexicalSearchPageV1, LexicalSearcher,
    LexicalWriterCacheStats, LexicalWriterPolicy, RegexMatchCachePolicy, RegexMatchCacheStats,
    RepoCommitRecencyIngestPort, RepoDescriptionIngestPort, RepoMetaIngestPort,
    RepoTopicIngestPort, SearchCorpusBatchBuildPort, SearchCorpusIngestPort,
    TextAuthorityUpdateStats,
};
pub use domains::observability::{
    MetricPointV1, MetricSourcePort, MetricValueV1, count_as_f64, count_from_usize,
};
pub use domains::read_view::{
    FILE_CONTRIBUTOR_UNAVAILABLE_CODE, FILE_OWNERSHIP_UNAVAILABLE_CODE, LexicalArtifactIdentityV1,
    LexicalPredicateAliasV1, LexicalPredicateFamilyV1, LexicalPredicateV1, QueryRouteV1,
    READ_VIEW_DOMAIN_UNDECLARED_CODE, READ_VIEW_GENERATION_MIX_CODE,
    REPO_COMMIT_RECENCY_UNAVAILABLE_CODE, REPO_DESCRIPTION_UNAVAILABLE_CODE,
    REPO_META_UNAVAILABLE_CODE, REPO_TOPIC_UNAVAILABLE_CODE, RUNTIME_NOT_READY_CODE, ReadDomainV1,
    ReadIdentityV1, ReadViewRefusedError, RepoMetadataAuthoritiesV1, RepoMetadataAuthorityV1,
    RequiredDomainsV1, SemanticProfileV1, TextNormalizerVersionV1, declare_required_domains_v1,
    lexical_predicate_v1,
};
pub use domains::repomap::{
    QuarantinedRepoMapFileV1, RepoMapBundleIngestPort, RepoMapGenerationActivatePort,
    RepoMapOpenReportV1, RepoMapPolicy, RepoMapQuarantinePort, RepoMapQueryPort, RepoMapService,
};
pub use domains::semantic::{
    DenseIndexEffortV1, DenseIndexLineageV1, DenseIndexTrainingV1, DenseIndexV1,
    DenseLaneAttestationV1, DenseLaneContractV1, L2_UNIT_NORM_TOLERANCE, L2UnitEmbeddingProvider,
    ResidentScopeSource, SEMANTIC_STREAM_OWNER_SCOPE_OVER_WINDOW_CODE,
    SEMANTIC_STREAM_WINDOW_EXCEEDED_CODE, SEMANTIC_STREAM_WINDOW_SCOPES,
    SEMANTIC_STREAM_WINDOW_STILL_RESIDENT_CODE, SEMANTIC_STREAM_WINDOW_VECTOR_BYTES,
    SemanticBatchIdentityV1, SemanticBatchMutationsV1, SemanticGenerationContractV1,
    SemanticIndexOpenPort, SemanticIngestHeaderV1, SemanticIngestPort, SemanticPolicy,
    SemanticQueryPort, SemanticReadiness, SemanticScopeSource, SemanticScopeStreamBuildPort,
    SemanticScopeWindowV1, SemanticSearchHitV1, SemanticSearcher, SemanticStreamTallyV1,
    SemanticStreamWindowPolicy, SemanticWindowFillV1, SemanticWindowIssuerV1,
    SemanticWindowLeaseV1, SemanticWindowPlacementV1, SemanticWindowResidencyV1,
    TextEmbeddingProvider, build_resident_semantic_batch_v1, owner_key_v1,
};
pub use domains::structural::{
    StructuralError, StructuralMatchBinding, StructuralMatchCandidate, StructuralPolicy,
    StructuralProducerPort, StructuralQueryRequest, StructuralQueryResponse, StructuralReadiness,
    StructuralService,
};
