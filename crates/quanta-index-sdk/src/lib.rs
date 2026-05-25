#![forbid(unsafe_code)]
#![deny(unused_must_use)]
#![deny(clippy::let_underscore_must_use)]
#![deny(clippy::map_err_ignore)]

mod batch;
mod client;
mod config;
mod error;
mod generations;
mod lexical;
mod repomap;
mod search;
mod semantic;
mod symbol;
mod transport;

pub use batch::{BatchMode, BatchReceipt};
pub use client::QuantaIndex;
pub use config::ConnectOptions;
pub use error::SdkError;
pub use generations::GenerationNamespace;
pub use lexical::{ChunkMutation, LexicalBatch, LexicalNamespace, LexicalQueryBuilder, SymbolMutation};
pub use repomap::RepoMapNamespace;
pub use search::{HybridQueryBuilder, SearchNamespace};
pub use semantic::{EmbeddingMutation, SemanticBatch, SemanticNamespace, SemanticQueryBuilder, SemanticVector};
pub use symbol::{SymbolNamespace, SymbolQueryBuilder};
pub use transport::{ControlTransport, QueryTransport, UdsControlTransport, UdsQueryTransport};

pub use quanta_index_contract::{
    ChannelSeq, ChunkId, ChunkRecord, EmbeddingId, EmbeddingRecord, GenerationPin,
    GenerationSelector, HybridQueryResponse, LexicalCandidate, ManifestGeneration,
    RepoId, RepoMapActivateGenerationRequestV1, RepoMapMutationAckV1, RepoMapQueryRequestV1,
    RepoMapQueryResponseV1, RepoMapSourceBundleV1, RepoRelativePath, RevisionId,
    SearchExplanation, SearchPlaneActivationAck, SearchPlaneExplainQueryResponse,
    SearchPlaneTrackKind, SemanticQueryResponse, SymbolId, SymbolQueryResponse,
    TextQueryResponse, TextQuerySyntax,
};
pub use quanta_index_contract::lex::{
    LangId, SymbolKind, SymbolRecord, SymbolRelationship, SymbolSpan, SymbolVisibility,
};

pub type CodeHit = LexicalCandidate;
pub type QuerySyntax = TextQuerySyntax;
pub type Track = SearchPlaneTrackKind;

#[cfg(test)]
mod tests;
