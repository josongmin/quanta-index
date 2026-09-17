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
    AuxiliaryAuthorityCatalogPort, AuxiliaryDomainV1, AuxiliaryGenerationKeyV1,
    AuxiliaryMutationBatchV1, AuxiliaryMutationReceiptV1, AuxiliaryRowFamilyV1, AuxiliaryRowKeyV1,
    AuxiliaryRowMutationV1, AuxiliaryRowV1, AuxiliaryTrackRowV1,
};
pub use domains::generation::{
    GenerationIdentityValidatePort, GenerationQuarantineReasonV1, GenerationStorageKeyV1,
    IncompleteGenerationDiscardOutcomeV1, IncompleteGenerationDiscardPort,
    QUARANTINE_TARGET_NOT_QUARANTINED_CODE, QuarantineDiscardOutcomeV1,
    QuarantinedGenerationDiscardPort, QuarantinedGenerationV1, SealedArtifactCommitmentV1,
    SealedGenerationInventoryV1, SealedGenerationReclaimOutcomeV1, SealedGenerationReclaimPort,
    SealedGenerationScanPort, TreeCommitmentMismatchV1, commit_tree_v1, sha256_of_file,
    verify_tree_commitment_v1,
};
pub use domains::hybrid::{ExplainQueryPort, HybridOrchestratorPolicy, HybridQueryPort};
pub use domains::idempotency::{
    BATCH_DIGEST_CONFLICT_CODE, CATALOG_BUSY_CODE, CATALOG_ROW_CORRUPT_CODE, IdempotencyBeginV1,
    IdempotencyCatalogPort, IdempotencyKeyV1, IngestOperationKindV1,
};
pub use domains::lexical::{
    FileContributorIngestPort, FileOwnershipIngestPort, LEXICAL_EXAMINED_BUDGET_EXCEEDED_CODE,
    LEXICAL_WRITER_HEAP_BYTES_MAX, LEXICAL_WRITER_HEAP_BYTES_MIN, LexicalCandidateExplanationV1,
    LexicalExecutionBudgetV1, LexicalIndexBuildPort, LexicalIndexOpenPort, LexicalPolicy,
    LexicalQueryPort, LexicalReadiness, LexicalScoreEngineV1, LexicalScoreTraceV1,
    LexicalSearchPageV1, LexicalSearcher, LexicalWriterCacheStats, LexicalWriterPolicy,
    RegexMatchCachePolicy, RegexMatchCacheStats, RepoCommitRecencyIngestPort,
    RepoDescriptionIngestPort, RepoMetaIngestPort, RepoTopicIngestPort, SearchCorpusBatchBuildPort,
    SearchCorpusIngestPort, TextAuthorityUpdateStats,
};
pub use domains::observability::{
    MetricPointV1, MetricSourcePort, MetricValueV1, count_as_f64, count_from_usize,
};
pub use domains::repomap::{
    QuarantinedRepoMapFileV1, RepoMapBundleIngestPort, RepoMapGenerationActivatePort,
    RepoMapOpenReportV1, RepoMapPolicy, RepoMapQuarantinePort, RepoMapQueryPort, RepoMapService,
};
pub use domains::semantic::{
    DenseIndexEffortV1, DenseIndexLineageV1, DenseIndexTrainingV1, DenseIndexV1,
    DenseLaneAttestationV1, DenseLaneContractV1, L2_UNIT_NORM_TOLERANCE, L2UnitEmbeddingProvider,
    SemanticBatchBuildPort, SemanticIndexOpenPort, SemanticIngestPort, SemanticPolicy,
    SemanticQueryPort, SemanticReadiness, SemanticSearchHitV1, SemanticSearcher,
    TextEmbeddingProvider,
};
pub use domains::structural::{
    StructuralError, StructuralMatchBinding, StructuralMatchCandidate, StructuralPolicy,
    StructuralProducerPort, StructuralQueryRequest, StructuralQueryResponse, StructuralReadiness,
    StructuralService,
};
