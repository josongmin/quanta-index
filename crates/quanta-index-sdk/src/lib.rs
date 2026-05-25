#![forbid(unsafe_code)]
#![deny(unused_must_use)]
#![deny(clippy::let_underscore_must_use)]
#![deny(clippy::map_err_ignore)]

mod batch;
pub(crate) mod client;
pub(crate) mod config;
mod error;
mod generations;
pub(crate) mod lexical;
mod repomap;
mod search;
pub(crate) mod semantic;
mod sourcegraph;
mod symbol;
mod transport;

pub use batch::{BatchMode, BatchReceipt};
pub use client::QuantaIndex;
pub use config::ConnectOptions;
pub use error::SdkError;
pub use generations::GenerationNamespace;
pub use lexical::{
    ChunkMutation, LexicalBatch, LexicalNamespace, LexicalQueryBuilder, SymbolMutation,
};
pub use repomap::RepoMapNamespace;
pub use search::{HybridQueryBuilder, SearchNamespace};
pub use semantic::{
    EmbeddingMutation, SemanticBatch, SemanticNamespace, SemanticQueryBuilder, SemanticVector,
};
pub use sourcegraph::{SourcegraphNamespace, SourcegraphQueryBuilder};
pub use symbol::{SymbolNamespace, SymbolQueryBuilder};
pub use transport::{
    ControlTransport, IngestTransport, QueryTransport, UdsControlTransport, UdsIngestTransport,
    UdsQueryTransport,
};

pub use quanta_index_contract::lex::{
    LangId, SymbolKind, SymbolRecord, SymbolRelationship, SymbolSpan,
};
pub use quanta_index_contract::{
    ChannelSeq, ChunkId, ChunkRecord, EmbeddingId, EmbeddingRecord, GenerationPin,
    GenerationSelector, GenerationSnapshot, GenerationStatusReport, HybridQueryResponse,
    LexicalCandidate, ManifestGeneration, RepoId, RepoMapActivateGenerationRequest,
    RepoMapMutationAck, RepoMapQueryRequest, RepoMapQueryResponse, RepoMapSourceBundle,
    RepoRelativePath, RevisionId, SearchExplanation, SearchPlaneActivationAck,
    SearchPlaneExplainQueryResponse, SearchPlaneSourcegraphQueryResponse, SearchPlaneTrackKind,
    SemanticQueryResponse, SymbolId, SymbolQueryResponse, TextQueryResponse, TextQuerySyntax,
    TrackReadinessRecord,
};

pub type CodeHit = LexicalCandidate;
pub type QuerySyntax = TextQuerySyntax;
pub type Track = SearchPlaneTrackKind;

#[cfg(test)]
mod tests;
