//! Lexical build ops, channel identities, and chunk / embedding records.
//!
//! Nothing here is a wire shape: `LexicalChannelOp` has no serde and no
//! persisted op stream exists. The ops are the lexical adapter's in-process
//! build vocabulary, lowered from the typed ingest front door under
//! [`crate::ipc`] (for example [`crate::SearchPlaneIngestIpcRequest`]). The
//! identities and records are shared by the ingest DTOs.

mod ids;
mod ops;
mod records;

pub use ids::{ChannelSeq, ChunkId, EmbeddingId, SymbolId};
pub use ops::{
    ClearLexicalSurface, LexicalChannelOp, LexicalFullBundle, LexicalSeal, ReplaceLexicalScope,
    ReplaceStructuralScope, TombstoneLexicalScope, UpsertChunk, UpsertCommit, UpsertParseTree,
    UpsertRef, UpsertSymbol,
};
pub use records::{ChunkRecord, ChunkStructuralMetadata, EmbeddingRecord, OwnerDocKind};
