use quanta_index_contract::ChannelSeq;

use crate::error::CoreError;

/// Stateless validators for the channel dispatcher. The dispatcher itself owns
/// per-track in-memory state; this policy only encodes invariants that hold for
/// every event independent of prior observations.
#[derive(Debug, Default, Clone, Copy)]
pub struct ChannelDispatchPolicy;

impl ChannelDispatchPolicy {
    /// Reject regression in event sequence numbers.
    pub fn validate_monotonic_seq(
        observed: ChannelSeq,
        last_emitted: ChannelSeq,
    ) -> Result<(), CoreError> {
        if observed.get() <= last_emitted.get() && last_emitted.get() != 0 {
            return Err(CoreError::InvalidContract(format!(
                "channel seq regression: observed {} <= last_emitted {}",
                observed.get(),
                last_emitted.get()
            )));
        }
        Ok(())
    }
}
