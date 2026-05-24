//! Default backend: append-only WAL files with mmap-based subscriber tail.

pub mod codec;
pub mod cursor;
pub mod publisher;
pub mod segment;
pub mod subscriber;

use std::path::{Path, PathBuf};

use quanta_index_contract::{LexicalChannelOp, SemanticChannelOp};

use crate::api::error::ChannelError;

pub use codec::{LexicalCodec, OpCodec, SemanticCodec};
pub use publisher::WalPublisher;
pub use subscriber::WalSubscriber;

/// Subscriber event emitted by the lexical track.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LexicalChannelEvent {
    pub seq: quanta_index_contract::ChannelSeq,
    pub op: LexicalChannelOp,
}

/// Subscriber event emitted by the semantic track.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SemanticChannelEvent {
    pub seq: quanta_index_contract::ChannelSeq,
    pub op: SemanticChannelOp,
}

/// Concrete lexical publisher returned by `open_lexical_publisher`.
pub type LexicalWalPublisher = WalPublisher<LexicalCodec>;
/// Concrete lexical subscriber returned by `open_lexical_subscriber`.
pub type LexicalWalSubscriber = WalSubscriber<LexicalCodec>;
/// Concrete semantic publisher returned by `open_semantic_publisher`.
pub type SemanticWalPublisher = WalPublisher<SemanticCodec>;
/// Concrete semantic subscriber returned by `open_semantic_subscriber`.
pub type SemanticWalSubscriber = WalSubscriber<SemanticCodec>;

impl LexicalWalPublisher {
    pub fn open(state_root: &Path) -> Result<Self, ChannelError> {
        let track_root = lexical_track_root(state_root);
        WalPublisher::open_with_codec(track_root, LexicalCodec)
    }
}

impl LexicalWalSubscriber {
    pub fn open(state_root: &Path) -> Result<Self, ChannelError> {
        let track_root = lexical_track_root(state_root);
        WalSubscriber::open_with_codec(track_root, LexicalCodec)
    }
}

impl SemanticWalPublisher {
    pub fn open(state_root: &Path) -> Result<Self, ChannelError> {
        let track_root = semantic_track_root(state_root);
        WalPublisher::open_with_codec(track_root, SemanticCodec)
    }
}

impl SemanticWalSubscriber {
    pub fn open(state_root: &Path) -> Result<Self, ChannelError> {
        let track_root = semantic_track_root(state_root);
        WalSubscriber::open_with_codec(track_root, SemanticCodec)
    }
}

fn lexical_track_root(state_root: &Path) -> PathBuf {
    state_root.join("channel").join("lexical")
}

fn semantic_track_root(state_root: &Path) -> PathBuf {
    state_root.join("channel").join("semantic")
}
