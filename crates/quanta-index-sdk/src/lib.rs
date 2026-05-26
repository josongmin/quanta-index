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
mod text_query_builder;
mod transport;

pub use batch::{BatchMode, BatchReceipt};
pub use client::QuantaIndex;
pub use config::ConnectOptions;
pub use error::SdkError;
pub use generations::GenerationNamespace;
pub use history::{
    DiffHunkMutation, HistoryBatch, HistoryNamespace, HistoryNs, HistoryQueryBuilder, RefMutation,
};
pub use lexical::{LexicalBatch, LexicalNamespace, LexicalNs, LexicalQueryBuilder};
pub use namespace::{NamespaceHandle, NamespaceIngest, NamespaceQuery};
pub use repomap::{RepoMapNamespace, RepoMapNs};
pub use runtime::{
    DirtyBatch, DirtyBatchMutation, RuntimeNamespace, RuntimeNs, RuntimeQueryBuilder,
};
pub use search::{HybridQueryBuilder, SearchNamespace};
pub use semantic::{
    SemanticBatch, SemanticNamespace, SemanticNs, SemanticQueryBuilder, SemanticVector,
};
pub use structural::{StructuralBatch, StructuralNamespace, StructuralNs, StructuralQueryBuilder};
pub use symbol::{SymbolNamespace, SymbolQueryBuilder};
pub use transport::{
    ControlTransport, IngestTransport, QueryTransport, UdsControlTransport, UdsIngestTransport,
    UdsQueryTransport,
};

pub use quanta_index_contract::lex::{
    CommitRecord, CommitSha, DiffHunkRecord, DirtyRecord, LanguageCode, ParseNode, ParseRoleTag,
    ParseTreeRecord, SymbolKindCode, SymbolKindFamily, SymbolRecord, SymbolRelationship,
    SymbolSpan,
};
pub use quanta_index_contract::{
    ChannelSeq, ChunkId, ChunkRecord, ChunkStructuralMetadata, EmbeddingDistanceMetric,
    EmbeddingId, EmbeddingModelContract, EmbeddingNormalization, EmbeddingRecord, GenerationPin,
    GenerationSelector, GenerationSnapshot, GenerationStatusReport, HybridQueryResponse,
    LexicalCandidate, LexicalReplaceScope, LexicalTombstoneScope, ManifestGeneration, OwnerDocKind,
    RepoId, RepoMapActivateGenerationRequest, RepoMapCallEdge, RepoMapChunkNode,
    RepoMapContainsEdge, RepoMapDependsOnEdge, RepoMapEdge, RepoMapFileNode, RepoMapGraphCoverage,
    RepoMapImportEdge, RepoMapModuleId, RepoMapModuleNode, RepoMapMutationAck, RepoMapNode,
    RepoMapNodeRef, RepoMapOwnsChunkEdge, RepoMapQueryRequest, RepoMapQueryResponse,
    RepoMapSourceBundle, RepoMapSymbolNode, RepoRelativePath, RevisionId, SearchExplanation,
    SearchPlaneActivationAck, SearchPlaneExplainQueryResponse, SearchPlaneHistoryQueryResponse,
    SearchPlaneRuntimeMetadataQueryResponse, SearchPlaneStructuralQueryResponse,
    SearchPlaneTrackKind, SearchScopeKey, SearchScopeSurface, SemanticQueryResponse,
    SemanticReplaceScope, SemanticTombstoneScope, StructuralReplaceScope, StructuralTombstoneScope,
    StructuralTreeRecord, SymbolCandidate, SymbolId, SymbolQueryResponse, TextQueryResponse,
    TextQuerySyntax, TrackReadinessRecord,
};

pub type CodeHit = LexicalCandidate;
pub type QuerySyntax = TextQuerySyntax;
pub type Track = SearchPlaneTrackKind;

#[cfg(test)]
mod tests;
