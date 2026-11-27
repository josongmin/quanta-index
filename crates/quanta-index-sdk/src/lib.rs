#![forbid(unsafe_code)]
#![deny(unused_must_use)]
#![deny(clippy::let_underscore_must_use)]
#![deny(clippy::map_err_ignore)]

mod batch;
pub mod binding;
pub(crate) mod client;
mod client_observation;
pub(crate) mod config;
mod diagnostic_attempt;
mod error;
mod generations;
mod history;
pub(crate) mod lexical;
mod namespace;
pub mod native_connect_v1;
pub mod native_index;
#[path = "client/native_code_search.rs"]
mod native_code_search;
pub use native_code_search::resolve_native_code_search_continuation_view_v1;
#[path = "client/native_query.rs"]
mod native_query;
#[path = "client/native_text_matches.rs"]
mod native_text_matches;
pub mod native_uds;
mod observability;
pub mod preparation;
mod quarantine;
mod runtime;
mod search;
pub(crate) mod semantic;
mod structural;
mod symbol;
pub(crate) mod text_query_builder;
mod text_query_route_shared;
mod transport;
mod unsupported;

pub use batch::{BatchMode, BatchReceipt};
pub use binding::{
    ExpectedQueryResponseV1, SDK_WIRE_ROUTE_EXCLUSIONS_V1, SDK_WIRE_ROUTES_V1, SdkWireRouteV1,
};
pub use client::QuantaIndexClientPayloadV1;
pub use client::{
    ClientLexicalQueryObservationV1, ClientQueryRpcKindV1, ClientQueryRpcObservationV1,
    ControlClient, ProducerClient, QuantaIndex, ReaderClient,
};
pub use config::{ClientProfile, ConnectOptions};
pub use diagnostic_attempt::{DiagnosticAttemptGuardV1, DiagnosticAttemptSourceV1};
pub use error::ResponseBindingAxis;
pub use error::SdkError;
pub use error::{PublishedBatchEvidence, PublishedBatchFailureStage};
pub use generations::GenerationNamespace;
pub use history::{
    AdmittedAuxiliaryBatchReceiptV1, AdmittedAuxiliaryRemoteErrorV1,
    AdmittedAuxiliaryResponseOwnerV1, AuxiliaryAdmittedPublishErrorV1, DiffHunkMutation,
    FileContributorBatch, FileContributorMutation, FileOwnershipBatch, FileOwnershipMutation,
    HistoryBatch, HistoryNamespace, HistoryQueryBuilder, RefMutation, RepoCommitRecencyBatch,
    RepoCommitRecencyMutation, RepoDescriptionBatch, RepoDescriptionMutation, RepoMetaBatch,
    RepoMetaMutation, RepoTopicBatch, RepoTopicMutation,
};
pub use lexical::{
    LexicalNamespace, LexicalQueryBuilder, SdkPublishActivateDurationsV1, SearchCorpusBatch,
    SearchCorpusNamespace,
};
pub(crate) use namespace::{NamespaceIngest, NamespaceQuery};
pub use native_query::{NativeQueryError, NativeQueryReader, NativeQuerySession};
pub use native_text_matches::NativeTextMatchesResponse;
pub use observability::ObservabilityNamespace;
pub use quanta_index_contract::{
    IngestObservationStatus, IngestStageDurations, IngestStageReport, SearchCorpusIngestObservation,
    SearchCorpusPublishOutcome,
};
/// The canonical batch digest (QI-BB-032): the SDK builders stamp it on
/// every batch they send; producers that assemble wire batches themselves
/// stamp them with the same function before publishing.
pub use quanta_index_ipc::{
    ClientIpcTimingV1, IngestBatchBodyV1, canonical_batch_digest_v1, stamp_batch_digest_v1,
};
pub use quarantine::QuarantineNamespace;
pub use runtime::{DirtyBatch, DirtyBatchMutation, RuntimeNamespace, RuntimeQueryBuilder};
pub use search::{HybridQueryBuilder, HybridSeedQueryBuilder, SearchNamespace};
pub use semantic::{SemanticNamespace, SemanticQueryBuilder};
pub use structural::{StructuralBatch, StructuralNamespace, StructuralQueryBuilder};
pub use symbol::{SymbolNamespace, SymbolQueryBuilder};
pub(crate) use transport::{
    ControlTransport, IngestTransport, QueryTransport, UdsControlTransport, UdsIngestTransport,
    UdsQueryTransport,
};
pub use unsupported::{LegacyQueryCapabilityV1, UNSUPPORTED_QUERY_DIAGNOSTIC_VERSION};

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
    QuarantinedGenerationEntryV1, RepoId, RepoRelativePath, RevisionId, RuntimeMetadataCursorV1,
    SearchCorpusActiveHeadV1, SearchCorpusGenerationIdentityV1, SearchCorpusReplaceScope,
    SearchCorpusTombstoneScope, SearchExplanation,
    SearchPlaneActivateSearchCorpusGenerationCasRequest, SearchPlaneExplainQueryResponse,
    SearchPlaneHistoryQueryResponse, SearchPlaneRollbackSearchCorpusGenerationCasRequest,
    SearchPlaneRuntimeMetadataQueryResponse, SearchPlaneSearchCorpusActivationCasAck,
    SearchPlaneSearchCorpusRollbackCasAck, SearchPlaneStructuralQueryResponse,
    SearchPlaneTrackKind, SearchScopeKey, SearchScopeSurface, SeedCandidate, SeedContribution,
    SeedLane, SemanticQueryResponse, SourceFileCoverage, SourceFileKey, SourceFileRevision,
    SourcePublicationEvent, StructuralCursorV1, StructuralReplaceScope, StructuralTombstoneScope,
    StructuralTreeRecord, SymbolCandidate, SymbolCoverage, SymbolId, SymbolQueryResponse,
    TextQueryResponse, TextQuerySyntax, TextRankUnit, TrackReadinessRecord,
};

pub type CodeHit = LexicalCandidate;
pub type QuerySyntax = TextQuerySyntax;
pub type Track = SearchPlaneTrackKind;

#[cfg(test)]
mod tests;
