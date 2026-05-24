#![forbid(unsafe_code)]
#![deny(unused_must_use)]
#![deny(clippy::let_underscore_must_use)]
#![deny(clippy::map_err_ignore)]

//! DTO surface shared between producer (`semantica-codegraph-v2`) and search-plane.
//!
//! Two surfaces:
//!
//! * Query path (`ipc`, `query`, `results`) — UDS frame payloads sent by query clients.
//! * Channel path (`channel`) — typed transport ops produced by producer and consumed
//!   by `searchd` modules.

pub mod channel;
pub mod ids;
pub mod ipc;
pub mod query;
pub mod repomap;
pub mod results;

pub use channel::{
    ChannelSeq, ChunkId, DeleteChunk, DeleteEmbedding, DeleteSymbol, EmbeddingId, LexicalChannelOp,
    LexicalFullBundle, LexicalSeal, SemanticChannelOp, SemanticFullBundle, SemanticSeal, SymbolId,
    UpsertChunk, UpsertEmbedding, UpsertSymbol,
};
pub use ids::*;
pub use ipc::*;
pub use query::*;
pub use repomap::*;
pub use results::*;
