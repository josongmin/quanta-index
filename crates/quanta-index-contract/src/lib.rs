#![forbid(unsafe_code)]
#![deny(unused_must_use)]
#![deny(clippy::let_underscore_must_use)]
#![deny(clippy::map_err_ignore)]

//! DTO surface shared between producer (`semantica-codegraph-v2`) and search-plane.
//!
//! Two surfaces:
//!
//! * Query path (`query`, `results`, `ipc` envelopes/split) — UDS frame payloads sent by query clients.
//! * Channel path (`channel`) — typed transport ops produced by producer and consumed
//!   by `searchd` modules.
//!
//! Core identifiers, generation pinning, and text-query syntax live in the
//! sibling `quanta-index-contract-base` crate. They are re-exported here so
//! existing imports keep working while downstream crates that only need the
//! core surface can migrate to depend on `quanta-index-contract-base` directly.

#[macro_use]
mod macros;

pub mod channel;
pub mod ipc;
pub mod query;
pub mod repomap;
pub mod results;

pub use channel::{
    ChannelSeq, ChunkId, ChunkRecord, DeleteChunk, DeleteEmbedding, DeleteParseTree, DeleteRef,
    DeleteSymbol, DeleteTag, EmbeddingId, EmbeddingRecord, EvictDirty, LexicalChannelOp,
    LexicalFullBundle, LexicalSeal, SemanticChannelOp, SemanticFullBundle, SemanticSeal, SymbolId,
    UpsertChunk, UpsertCommit, UpsertDiffHunk, UpsertDirty, UpsertEmbedding, UpsertParseTree,
    UpsertRef, UpsertSymbol, UpsertTag,
};
pub use ipc::*;
pub use quanta_index_contract_base::{
    FileId, GenerationId, ManifestDigest, ManifestGeneration, RepoId, RepoRelativePath, RevisionId,
};
pub use query::*;
pub use repomap::*;
pub use results::*;

// PRE-CONTRACT-EXT additive scaffold — canonical LQ-family wire types.
// New module, additive only; downstream LQ-crate migration is a separate
// ticket. Names live under `quanta_index_contract::lex::*` and are NOT
// re-exported at the crate root to avoid shadowing the existing
// `results::SearchExplanation` re-export above. See
// `crates/quanta-index-contract/src/lex/mod.rs` for the canonical surface.
pub mod lex;
