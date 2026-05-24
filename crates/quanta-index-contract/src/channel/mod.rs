//! Channel transport ops.
//!
//! Producer code constructs [`LexicalChannelOp`] or [`SemanticChannelOp`] values and
//! calls `BundleChannelPublisher::publish(op)` from the `quanta-index-channel` crate.
//! Subscriber side receives the same values. Wire encoding lives in the channel
//! adapter's backend modules and is not exposed here.

mod ids;
mod ops;

pub use ids::{ChannelSeq, ChunkId, EmbeddingId, SymbolId};
pub use ops::{
    DeleteChunk, DeleteEmbedding, DeleteSymbol, LexicalChannelOp, LexicalFullBundle, LexicalSeal,
    SemanticChannelOp, SemanticFullBundle, SemanticSeal, UpsertChunk, UpsertEmbedding,
    UpsertSymbol,
};
