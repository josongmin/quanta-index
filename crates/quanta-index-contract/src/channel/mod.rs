//! Channel transport ops.
//!
//! Producer code constructs [`LexicalChannelOp`] or [`SemanticChannelOp`] values and
//! calls `BundleChannelPublisher::publish(op)` from the `quanta-index-channel` crate.
//! Subscriber side receives the same values. Wire encoding lives in the channel
//! adapter's backend modules and is not exposed here.

mod ids;
mod ops;
mod records;

pub use ids::{ChannelSeq, ChunkId, EmbeddingId, SymbolId};
pub use ops::{
    DeleteChunk, DeleteEmbedding, DeleteParseTree, DeleteRef, DeleteSymbol, DeleteTag, EvictDirty,
    LexicalChannelOp, LexicalFullBundle, LexicalSeal, SemanticChannelOp, SemanticFullBundle,
    SemanticSeal, UpsertChunk, UpsertCommit, UpsertDiffHunk, UpsertDirty, UpsertEmbedding,
    UpsertParseTree, UpsertRef, UpsertSymbol, UpsertTag,
};
pub use records::{ChunkRecord, EmbeddingRecord};
