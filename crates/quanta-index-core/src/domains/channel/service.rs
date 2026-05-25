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

#[cfg(test)]
mod tests {
    //! Kills the three `cargo mutants` survivors on line 17:
    //! `<=` → `>`, `&&` → `||`, `!=` → `==`.

    use super::*;

    fn seq(n: u64) -> ChannelSeq {
        ChannelSeq::new(n)
    }

    #[test]
    fn regression_below_last_rejected_kills_le_to_gt_mutation() {
        let err = ChannelDispatchPolicy::validate_monotonic_seq(seq(5), seq(10))
            .err()
            .expect("regression below last must be rejected");
        let CoreError::InvalidContract(msg) = err else {
            panic!("expected InvalidContract");
        };
        assert!(msg.contains("regression"), "msg={msg}");
    }

    #[test]
    fn equal_to_last_rejected_kills_le_boundary() {
        assert!(ChannelDispatchPolicy::validate_monotonic_seq(seq(10), seq(10)).is_err());
    }

    #[test]
    fn strictly_above_last_accepted() {
        assert!(ChannelDispatchPolicy::validate_monotonic_seq(seq(11), seq(10)).is_ok());
    }

    #[test]
    fn initial_zero_seed_accepted_kills_ne_to_eq_and_and_to_or_mutations() {
        assert!(ChannelDispatchPolicy::validate_monotonic_seq(seq(0), seq(0)).is_ok());
        assert!(ChannelDispatchPolicy::validate_monotonic_seq(seq(1), seq(0)).is_ok());
    }
}
