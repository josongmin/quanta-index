//! Concrete per-track publisher / subscriber type aliases and their
//! `open(state_root)` constructors.
//!
//! Each track resolves to its own on-disk directory under `<state_root>/channel/`
//! so the lexical and semantic streams never share storage.

use std::path::{Path, PathBuf};

use crate::api::error::ChannelError;

use super::codec::{LexicalCodec, SemanticCodec};
use super::publisher::WalPublisher;
use super::subscriber::WalSubscriber;

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
