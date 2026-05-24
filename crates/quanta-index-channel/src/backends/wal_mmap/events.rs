//! Subscriber event types emitted by the WAL mmap backend.
//!
//! Each track exposes its own event shape so consumers do not have to match
//! on a generic `Either<LexicalChannelOp, SemanticChannelOp>`.

use quanta_index_contract::{ChannelSeq, LexicalChannelOp, SemanticChannelOp};

/// Subscriber event emitted by the lexical track.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LexicalChannelEvent {
    pub seq: ChannelSeq,
    pub op: LexicalChannelOp,
}

/// Subscriber event emitted by the semantic track.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SemanticChannelEvent {
    pub seq: ChannelSeq,
    pub op: SemanticChannelOp,
}
