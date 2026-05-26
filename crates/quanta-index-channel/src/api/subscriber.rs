use quanta_index_contract::channel::ChannelSeq;

use super::error::ChannelError;

/// Consumer-facing channel surface.
///
/// Implementors must guarantee:
/// 1. events arrive in strict sequence order;
/// 2. `ack(seq)` durably advances the persistent cursor — restart after `ack(N)`
///    resumes from `N+1`;
/// 3. corruption is reported as [`ChannelError::Corrupted`] rather than silently
///    skipped.
pub trait BundleChannelSubscriber: Send {
    /// Event type observed on this channel (`LexicalChannelEvent` or
    /// `SemanticChannelEvent`).
    type Event;

    /// Read the next available event, or `None` if no event is currently
    /// queued. Non-blocking; transport implementors polling new data should
    /// return `None` rather than block.
    fn next_event(&mut self) -> Result<Option<Self::Event>, ChannelError>;

    /// Durably advance the consumed cursor to `up_to`. After this call returns,
    /// `cursor()` must return `up_to` and a restart will resume at `up_to.next()`.
    fn ack(&mut self, up_to: ChannelSeq) -> Result<(), ChannelError>;

    /// Last seq durably ack'd. Subscribers freshly opened with no prior cursor
    /// must return [`ChannelSeq::ZERO`].
    fn cursor(&self) -> ChannelSeq;
}
