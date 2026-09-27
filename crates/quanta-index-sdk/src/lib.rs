#![forbid(unsafe_code)]
#![deny(unused_must_use)]
#![deny(clippy::let_underscore_must_use)]
#![deny(clippy::map_err_ignore)]

mod batch;
pub mod binding;
pub(crate) mod client;
pub(crate) mod config;
mod error;
mod generations;
mod history;
pub(crate) mod lexical;
mod namespace;
mod observability;
mod quarantine;
mod repomap;
mod runtime;
mod search;
pub(crate) mod semantic;
mod structural;
mod symbol;
pub(crate) mod text_query_builder;
mod transport;

pub use batch::{BatchMode, BatchReceipt};
pub use binding::{
    ExpectedQueryResponseV1, SDK_WIRE_ROUTE_EXCLUSIONS_V1, SDK_WIRE_ROUTES_V1, SdkWireRouteV1,
};
pub use client::{ControlClient, ProducerClient, QuantaIndex, ReaderClient};
pub use config::{ClientProfile, ConnectOptions};
pub use error::ResponseBindingAxis;
pub use error::SdkError;
pub use generations::GenerationNamespace;
pub use history::{
    DiffHunkMutation, FileContributorBatch, FileContributorMutation, FileOwnershipBatch,
    FileOwnershipMutation, HistoryBatch, HistoryNamespace, HistoryQueryBuilder, RefMutation,
    RepoCommitRecencyBatch, RepoCommitRecencyMutation, RepoDescriptionBatch,
    RepoDescriptionMutation, RepoMetaBatch, RepoMetaMutation, RepoTopicBatch, RepoTopicMutation,
};
pub use lexical::{
    LexicalNamespace, LexicalQueryBuilder, SearchCorpusBatch, SearchCorpusNamespace,
};
pub(crate) use namespace::{NamespaceIngest, NamespaceQuery};
pub use observability::ObservabilityNamespace;
pub use quanta_index_contract::{
    IngestObservationStatus, IngestStageDurations, IngestStageReport,
    SearchCorpusIngestObservation, SearchCorpusPublishOutcome,
};
/// The canonical batch digest (QI-BB-032): the SDK builders stamp it on
/// every batch they send; producers that assemble wire batches themselves
/// stamp them with the same function before publishing.
pub use quanta_index_ipc::{IngestBatchBodyV1, canonical_batch_digest_v1, stamp_batch_digest_v1};
pub use quarantine::QuarantineNamespace;
pub use repomap::RepoMapNamespace;
pub use runtime::{DirtyBatch, DirtyBatchMutation, RuntimeNamespace, RuntimeQueryBuilder};
pub use search::{HybridQueryBuilder, HybridSeedQueryBuilder, SearchNamespace};
pub use semantic::{SemanticNamespace, SemanticQueryBuilder};
pub use structural::{StructuralBatch, StructuralNamespace, StructuralQueryBuilder};
pub use symbol::{SymbolNamespace, SymbolQueryBuilder};
pub(crate) use transport::{
    ControlTransport, IngestTransport, QueryTransport, UdsControlTransport, UdsIngestTransport,
    UdsQueryTransport,
};

pub use quanta_index_contract::lex::{
    CommitRecord, CommitSha, DiffHunkRecord, DirtyRecord, LanguageCode, ParseNode, ParseRoleTag,
    ParseTreeRecord, SymbolKindCode, SymbolKindFamily, SymbolRecord, SymbolRelationship,
    SymbolSpan,
};
pub use quanta_index_contract::{
    AuxEpochV1, ChunkId, ChunkRecord, ChunkStructuralMetadata, EmbeddingDistanceMetric,
    EmbeddingId, EmbeddingModelContract, EmbeddingNormalization, EmbeddingRecord,
    ExactRepoRelativePathV1, ExplainCandidateV1, GenerationPin, GenerationSelector,
    GenerationSnapshot, GenerationStatusReport, HistoryCursor, HistoryCursorOrderV1,
    HistoryOrderV1, HistoryScoreV1, HybridCandidateV1, HybridLaneContributionV1, HybridLaneV1,
    HybridQueryRequest, HybridQueryResponse, HybridSeedQueryRequest, HybridSeedQueryResponse,
    IngestOperationKindV1, LexicalCandidate, ManifestGeneration, MetricBucketV1, MetricCounterV1,
    MetricGaugeV1, MetricHistogramV1, MetricsDiagnosticsV1, MetricsSnapshotV1, OwnerDocKind,
    QuarantineDiscardAck, QuarantineDiscardOutcomeDtoV1, QuarantineInventoryV1, QuarantineTargetV1,
    QuarantinedGenerationEntryV1, QuarantinedRepoMapFileEntryV1, RepoId, RepoMapCallEdge,
    RepoMapChunkNode, RepoMapContainsEdge, RepoMapDependsOnEdge, RepoMapEdge, RepoMapFileNode,
    RepoMapGraphCoverage, RepoMapImportEdge, RepoMapModuleId, RepoMapModuleNode,
    RepoMapMutationAck, RepoMapNode, RepoMapNodeRef, RepoMapOwnsChunkEdge, RepoMapQueryRequest,
    RepoMapQueryResponse, RepoMapSourceBundle, RepoMapSymbolNode, RepoRelativePath, RevisionId,
    RuntimeMetadataCursorV1, SearchCorpusGenerationIdentityV1, SearchCorpusReplaceScope,
    SearchCorpusTombstoneScope, SearchExplanation,
    SearchPlaneActivateSearchCorpusGenerationCasRequest, SearchPlaneExplainQueryResponse,
    SearchPlaneHistoryQueryResponse, SearchPlaneRollbackSearchCorpusGenerationCasRequest,
    SearchPlaneRuntimeMetadataQueryResponse, SearchPlaneSearchCorpusActivationCasAck,
    SearchPlaneSearchCorpusRollbackCasAck, SearchPlaneStructuralQueryResponse,
    SearchPlaneTrackKind, SearchScopeKey, SearchScopeSurface, SeedCandidate, SeedContribution,
    SeedLane, SemanticQueryResponse, SourceFileCoverage, SourceFileKey, SourceFileRevision,
    SourcePublicationEvent, StructuralCursorV1, StructuralReplaceScope, StructuralTombstoneScope,
    StructuralTreeRecord, SymbolCandidate, SymbolCoverage, SymbolId, SymbolQueryResponse,
    TextQueryResponse, TextQuerySyntax, TrackReadinessRecord,
};

pub type CodeHit = LexicalCandidate;
pub type QuerySyntax = TextQuerySyntax;
pub type Track = SearchPlaneTrackKind;

#[cfg(test)]
mod tests;
