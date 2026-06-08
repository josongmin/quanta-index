#![forbid(unsafe_code)]
#![deny(unused_must_use)]
#![deny(clippy::let_underscore_must_use)]
#![deny(clippy::map_err_ignore)]

mod batch;
pub(crate) mod client;
pub(crate) mod config;
mod error;
mod generations;
mod history;
pub(crate) mod lexical;
mod namespace;
mod repomap;
mod runtime;
mod search;
pub(crate) mod semantic;
mod structural;
mod symbol;
pub(crate) mod text_query_builder;
mod transport;

pub use batch::{BatchMode, BatchReceipt};
pub use client::{ControlClient, ProducerClient, QuantaIndex, ReaderClient};
pub use config::ConnectOptions;
pub use error::SdkError;
pub use generations::{ActivationBuilder, GenerationNamespace};
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
pub use repomap::RepoMapNamespace;
pub use runtime::{DirtyBatch, DirtyBatchMutation, RuntimeNamespace, RuntimeQueryBuilder};
pub use search::{HybridSeedQueryBuilder, SearchNamespace};
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
    ChunkId, ChunkRecord, ChunkStructuralMetadata, EmbeddingDistanceMetric, EmbeddingId,
    EmbeddingModelContract, EmbeddingNormalization, EmbeddingRecord, GenerationPin,
    GenerationSelector, GenerationSnapshot, GenerationStatusReport, HybridSeedCandidate,
    HybridSeedLane, HybridSeedQueryRequest, HybridSeedQueryResponse, LexicalCandidate,
    ManifestGeneration, OwnerDocKind, RepoId, RepoMapActivateGenerationRequest, RepoMapCallEdge,
    RepoMapChunkNode, RepoMapContainsEdge, RepoMapDependsOnEdge, RepoMapEdge, RepoMapFileNode,
    RepoMapGraphCoverage, RepoMapImportEdge, RepoMapModuleId, RepoMapModuleNode,
    RepoMapMutationAck, RepoMapNode, RepoMapNodeRef, RepoMapOwnsChunkEdge, RepoMapQueryRequest,
    RepoMapQueryResponse, RepoMapSourceBundle, RepoMapSymbolNode, RepoRelativePath, RevisionId,
    SearchCorpusReplaceScope, SearchCorpusTombstoneScope, SearchExplanation,
    SearchPlaneActivationAck, SearchPlaneExplainQueryResponse, SearchPlaneHistoryQueryResponse,
    SearchPlaneRuntimeMetadataQueryResponse, SearchPlaneStructuralQueryResponse,
    SearchPlaneTrackKind, SearchScopeKey, SearchScopeSurface, SemanticQueryResponse,
    StructuralReplaceScope, StructuralTombstoneScope, StructuralTreeRecord, SymbolCandidate,
    SymbolId, SymbolQueryResponse, TextQueryResponse, TextQuerySyntax, TrackReadinessRecord,
};

pub type CodeHit = LexicalCandidate;
pub type QuerySyntax = TextQuerySyntax;
pub type Track = SearchPlaneTrackKind;

#[cfg(test)]
mod tests;
