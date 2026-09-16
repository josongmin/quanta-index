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
pub mod timeref;

pub use error::{CoreError, validate_internal_fetch_size, validate_query_top_k};

pub use domains::generation::{
    GenerationIdentityValidatePort, GenerationStorageKeyV1, IncompleteGenerationDiscardOutcomeV1,
    IncompleteGenerationDiscardPort, SealedGenerationScanPort,
};
pub use domains::hybrid::{ExplainQueryPort, HybridOrchestratorPolicy, HybridQueryPort};
pub use domains::lexical::{
    FileContributorIngestPort, FileOwnershipIngestPort, LEXICAL_EXAMINED_BUDGET_EXCEEDED_CODE,
    LexicalExecutionBudgetV1, LexicalIndexBuildPort, LexicalIndexOpenPort, LexicalPolicy,
    LexicalQueryPort, LexicalReadiness, LexicalSearchPageV1, LexicalSearcher,
    RepoCommitRecencyIngestPort, RepoDescriptionIngestPort, RepoMetaIngestPort,
    RepoTopicIngestPort, SearchCorpusBatchBuildPort, SearchCorpusIngestPort,
};
pub use domains::repomap::{
    RepoMapBundleIngestPort, RepoMapGenerationActivatePort, RepoMapPolicy, RepoMapQueryPort,
    RepoMapService,
};
pub use domains::semantic::{
    SemanticBatchBuildPort, SemanticIndexOpenPort, SemanticIngestPort, SemanticPolicy,
    SemanticQueryPort, SemanticReadiness, SemanticSearchHitV1, SemanticSearcher,
    TextEmbeddingProvider,
};
pub use domains::structural::{
    StructuralError, StructuralMatchBinding, StructuralMatchCandidate, StructuralPolicy,
    StructuralProducerPort, StructuralQueryRequest, StructuralQueryResponse, StructuralReadiness,
    StructuralService,
};
