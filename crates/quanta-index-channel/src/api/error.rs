use core::fmt;
use std::io;

use quanta_index_contract::ChannelSeq;

/// All channel surface errors.
///
/// Backend-specific details are deliberately quarantined inside
/// [`ChannelError::State`] and [`ChannelError::Io`] — no `SegmentFull`,
/// `MmapRemap`, or other backend-leaking variant lives here.
#[derive(Debug)]
pub enum ChannelError {
    /// The channel was closed (publisher dropped or storage offline).
    Closed,
    /// A persisted entry failed integrity check at `at_seq`. The track must be
    /// treated as degraded; do not advance the cursor past this point.
    Corrupted { at_seq: ChannelSeq, reason: String },
    /// The transport refused a publish because backpressure has built up
    /// (e.g. unconsumed segments past the retention limit).
    BackpressureFull,
    /// Subscriber polled before any entries were ready.
    NotReady,
    /// Underlying I/O failure.
    Io(io::Error),
    /// Encoded payload was rejected by codec validation (length, version, etc.).
    Encoding(String),
    /// Higher-level invariant violation (sequence regression, generation
    /// monotonicity, double-publisher lock, etc.).
    State(String),
}

impl fmt::Display for ChannelError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Closed => f.write_str("channel closed"),
            Self::Corrupted { at_seq, reason } => {
                write!(f, "channel corrupted at seq {}: {}", at_seq.get(), reason)
            }
            Self::BackpressureFull => f.write_str("channel backpressure full"),
            Self::NotReady => f.write_str("channel not ready"),
            Self::Io(err) => write!(f, "channel io: {err}"),
            Self::Encoding(msg) => write!(f, "channel encoding: {msg}"),
            Self::State(msg) => write!(f, "channel state: {msg}"),
        }
    }
}

impl std::error::Error for ChannelError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(err) => Some(err),
            Self::Closed
            | Self::Corrupted { .. }
            | Self::BackpressureFull
            | Self::NotReady
            | Self::Encoding(_)
            | Self::State(_) => None,
        }
    }
}

impl From<io::Error> for ChannelError {
    fn from(err: io::Error) -> Self {
        Self::Io(err)
    }
}
