#![forbid(unsafe_code)]
#![deny(unused_must_use)]
#![deny(clippy::let_underscore_must_use)]
#![deny(clippy::map_err_ignore)]

//! Bundle / delta channel between the producer (e.g. `semantica-codegraph-v2`) and
//! the search-plane modules (`quanta-index-searchd`).
//!
//! The public surface is two trait families — [`api::publisher::BundleChannelPublisher`]
//! and [`api::subscriber::BundleChannelSubscriber`] — plus four factory functions
//! ([`open_lexical_publisher`], [`open_lexical_subscriber`], [`open_semantic_publisher`],
//! [`open_semantic_subscriber`]). Backends live under [`backends`] and never appear
//! in caller code.

pub mod api;
pub mod backends;

use std::path::Path;

pub use api::error::ChannelError;
pub use api::publisher::BundleChannelPublisher;
pub use api::subscriber::BundleChannelSubscriber;
pub use backends::wal_mmap::codec::{OpCodec, SemanticCodec};
pub use backends::wal_mmap::segment::{SegmentLayout, SegmentReader};
pub use backends::wal_mmap::{
    LexicalChannelEvent, LexicalWalPublisher, LexicalWalSubscriber, SemanticChannelEvent,
    SemanticWalPublisher, SemanticWalSubscriber,
};

/// Open the lexical-track publisher for the given `state_root`.
///
/// Creates `{state_root}/channel/lexical/` if it does not exist. At most one
/// publisher may hold the per-track lock at a time.
pub fn open_lexical_publisher(state_root: &Path) -> Result<LexicalWalPublisher, ChannelError> {
    LexicalWalPublisher::open(state_root)
}

/// Open the lexical-track subscriber for the given `state_root`.
///
/// Multiple subscribers may coexist; each tracks its own `cursor` file.
pub fn open_lexical_subscriber(state_root: &Path) -> Result<LexicalWalSubscriber, ChannelError> {
    LexicalWalSubscriber::open(state_root)
}

/// Open the semantic-track publisher for the given `state_root`.
pub fn open_semantic_publisher(state_root: &Path) -> Result<SemanticWalPublisher, ChannelError> {
    SemanticWalPublisher::open(state_root)
}

/// Open the semantic-track subscriber for the given `state_root`.
pub fn open_semantic_subscriber(state_root: &Path) -> Result<SemanticWalSubscriber, ChannelError> {
    SemanticWalSubscriber::open(state_root)
}
