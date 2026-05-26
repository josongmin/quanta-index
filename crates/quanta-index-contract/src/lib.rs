#![forbid(unsafe_code)]
#![deny(unused_must_use)]
#![deny(clippy::let_underscore_must_use)]
#![deny(clippy::map_err_ignore)]

//! DTO surface shared between producer (`semantica-codegraph-v2`) and search-plane.
//!
//! Two surfaces:
//!
//! * Query path (`query`, `results`, `ipc` envelopes/split) — UDS frame payloads sent by query clients.
//! * Channel path (`channel`) — legacy typed transport ops still consumed by
//!   `searchd` internals. New producer-facing publish authority lives under
//!   `ipc`.
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
    ChannelSeq, ChunkId, ChunkRecord, ChunkStructuralMetadata, DeleteChunk, DeleteEmbedding,
    DeleteParseTree, DeleteRef, DeleteSymbol, DeleteTag, EmbeddingId, EmbeddingRecord, EvictDirty,
    LexicalChannelOp, LexicalFullBundle, LexicalSeal, OwnerDocKind, ReplaceLexicalScope,
    ReplaceSemanticScope, ReplaceStructuralScope, SemanticChannelOp, SemanticFullBundle,
    SemanticSeal, SymbolId, TombstoneLexicalScope, TombstoneSemanticScope,
    TombstoneStructuralScope, UpsertChunk, UpsertCommit, UpsertDiffHunk, UpsertDirty,
    UpsertEmbedding, UpsertParseTree, UpsertRef, UpsertSymbol, UpsertTag,
};
pub use ipc::*;
pub use quanta_index_contract_base::{
    BridgeCandidatePacket, BridgeScope, BridgeTarget, DiffCandidate, DiffHunkSide, FileId,
    GenerationId, LexicalCandidate, ManifestDigest, ManifestGeneration, RepoId, RepoRelativePath,
    RevisionId, StructuralBinding, StructuralCandidate,
};
pub use query::*;
pub use repomap::*;
pub use results::*;

// PRE-CONTRACT-EXT canonical LQ-family wire types. `lex::*` is a thin
// re-export facade over `results::explanation::*` for the ranker-explanation
// surface (`ExplanationRow`, `SearchExplanation`, ...) plus the LQ-family
// records that live only under `lex` (`CommitRecord`, `SymbolRecord`, ...).
pub mod lex;
