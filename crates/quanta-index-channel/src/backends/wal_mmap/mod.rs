//! Default backend: append-only WAL files with mmap-based subscriber tail.

pub mod codec;
pub mod cursor;
pub mod events;
pub mod publisher;
pub mod segment;
pub mod subscriber;
pub mod tracks;

pub use codec::{LexicalCodec, OpCodec, SemanticCodec};
pub use events::{LexicalChannelEvent, SemanticChannelEvent};
pub use publisher::WalPublisher;
pub use subscriber::WalSubscriber;
pub use tracks::{
    LexicalWalPublisher, LexicalWalSubscriber, SemanticWalPublisher, SemanticWalSubscriber,
};
