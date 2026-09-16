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
pub mod request_budget;
pub mod timeref;

pub use error::{CoreError, validate_internal_fetch_size, validate_query_top_k};
pub use request_budget::{
    BudgetInterruptionV1, CancelHandleV1, REQUEST_CANCELLED_CODE, REQUEST_DEADLINE_EXCEEDED_CODE,
    RequestBudgetV1,
};

pub use domains::generation::{
    GenerationIdentityValidatePort, GenerationQuarantineReasonV1, GenerationStorageKeyV1,
    IncompleteGenerationDiscardOutcomeV1, IncompleteGenerationDiscardPort, QuarantinedGenerationV1,
    SealedArtifactCommitmentV1, SealedGenerationInventoryV1, SealedGenerationReclaimOutcomeV1,
    SealedGenerationReclaimPort, SealedGenerationScanPort, TreeCommitmentMismatchV1,
    commit_tree_v1, sha256_of_file, verify_tree_commitment_v1,
};
pub use domains::hybrid::{ExplainQueryPort, HybridOrchestratorPolicy, HybridQueryPort};
pub use domains::idempotency::{
    BATCH_DIGEST_CONFLICT_CODE, CATALOG_BUSY_CODE, CATALOG_ROW_CORRUPT_CODE, IdempotencyBeginV1,
    IdempotencyCatalogPort, IdempotencyKeyV1, IngestOperationKindV1,
};
pub use domains::lexical::{
    FileContributorIngestPort, FileOwnershipIngestPort, LEXICAL_EXAMINED_BUDGET_EXCEEDED_CODE,
    LexicalExecutionBudgetV1, LexicalIndexBuildPort, LexicalIndexOpenPort, LexicalPolicy,
    LexicalQueryPort, LexicalReadiness, LexicalSearchPageV1, LexicalSearcher,
    RegexMatchCachePolicy, RegexMatchCacheStats, RepoCommitRecencyIngestPort,
    RepoDescriptionIngestPort, RepoMetaIngestPort, RepoTopicIngestPort, SearchCorpusBatchBuildPort,
    SearchCorpusIngestPort,
};
pub use domains::repomap::{
    RepoMapBundleIngestPort, RepoMapGenerationActivatePort, RepoMapPolicy, RepoMapQueryPort,
    RepoMapService,
};
pub use domains::semantic::{
    L2_UNIT_NORM_TOLERANCE, L2UnitEmbeddingProvider, SemanticBatchBuildPort, SemanticIndexOpenPort,
    SemanticIngestPort, SemanticPolicy, SemanticQueryPort, SemanticReadiness, SemanticSearchHitV1,
    SemanticSearcher, TextEmbeddingProvider,
};
pub use domains::structural::{
    StructuralError, StructuralMatchBinding, StructuralMatchCandidate, StructuralPolicy,
    StructuralProducerPort, StructuralQueryRequest, StructuralQueryResponse, StructuralReadiness,
    StructuralService,
};
