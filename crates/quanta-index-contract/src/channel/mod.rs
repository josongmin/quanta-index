//! Legacy channel/op transport shapes.
//!
//! These types remain the canonical wire records for persisted op streams and
//! op-oriented adapter tests. The typed ingest front door is defined under
//! [`crate::ipc`] (for example [`crate::SearchPlaneIngestIpcRequest`]);
//! producers are no longer expected to publish these ops directly.

mod ids;
mod ops;
mod records;

pub use ids::{ChannelSeq, ChunkId, EmbeddingId, SymbolId};
pub use ops::{
    DeleteChunk, DeleteEmbedding, DeleteParseTree, DeleteRef, DeleteSymbol, DeleteTag, EvictDirty,
    LexicalChannelOp, LexicalFullBundle, LexicalSeal, ReplaceLexicalScope, ReplaceSemanticScope,
    ReplaceStructuralScope, SemanticChannelOp, SemanticFullBundle, SemanticSeal,
    TombstoneLexicalScope, TombstoneSemanticScope, TombstoneStructuralScope, UpsertChunk,
    UpsertCommit, UpsertDiffHunk, UpsertDirty, UpsertEmbedding, UpsertParseTree, UpsertRef,
    UpsertSymbol, UpsertTag,
};
pub use records::{ChunkRecord, ChunkStructuralMetadata, EmbeddingRecord, OwnerDocKind};
