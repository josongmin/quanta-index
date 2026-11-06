//! Admission-independent cadence for optional native Catalog WAL reclaim.
//!
//! The caller owns the existing maintenance reservation and work budget. This
//! schedule neither observes `SQLite` nor grants authority to run a checkpoint.

use std::time::{Duration, Instant};

use quanta_index_core::index::IndexError;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NativeWalReclaimPolicy {
    pub trigger_bytes: u64,
    pub min_interval: Duration,
}

impl NativeWalReclaimPolicy {
    pub fn validate(&self, max_io_bytes: u64) -> Result<(), IndexError> {
        if self.trigger_bytes == 0
            || self.trigger_bytes
                > max_io_bytes
                    .checked_div(4)
                    .ok_or(IndexError::ResourceLimited)?
        {
            return Err(IndexError::InvalidInput(
                "native WAL reclaim trigger must be positive and fit the existing IO allowance"
                    .into(),
            ));
        }
        if self.min_interval.is_zero() {
            return Err(IndexError::InvalidInput(
                "native WAL reclaim interval must be positive".into(),
            ));
        }
        if Instant::now().checked_add(self.min_interval).is_none() {
            return Err(IndexError::InvalidInput(
                "native WAL reclaim interval exceeds the supported clock range".into(),
            ));
        }
        Ok(())
    }
}

/// One pump-owned timer; a due attempt consumes its cadence slot before IO.
pub(crate) struct NativeWalReclaimSchedule {
    policy: NativeWalReclaimPolicy,
    next_due: Instant,
}

impl NativeWalReclaimSchedule {
    pub(crate) fn new(policy: NativeWalReclaimPolicy, now: Instant) -> Result<Self, IndexError> {
        if policy.min_interval.is_zero() {
            return Err(IndexError::InvalidInput(
                "native WAL reclaim interval must be positive".into(),
            ));
        }
        let next_due = now
            .checked_add(policy.min_interval)
            .ok_or(IndexError::ResourceLimited)?;
        Ok(Self { policy, next_due })
    }

    /// A failed or busy checkpoint still consumes this slot. A long prior
    /// round schedules from its actual observation time, never catches up.
    pub(crate) fn try_begin(&mut self, now: Instant) -> Result<bool, IndexError> {
        if now < self.next_due {
            return Ok(false);
        }
        let next_due = now
            .checked_add(self.policy.min_interval)
            .ok_or(IndexError::ResourceLimited)?;
        self.next_due = next_due;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy() -> NativeWalReclaimPolicy {
        NativeWalReclaimPolicy {
            trigger_bytes: 1024,
            min_interval: Duration::from_secs(2),
        }
    }

    fn require(condition: bool, detail: &'static str) -> Result<(), IndexError> {
        if condition {
            Ok(())
        } else {
            Err(IndexError::BackendViolation(detail.into()))
        }
    }

    #[test]
    fn policy_fits_original_io_envelope() {
        assert!(policy().validate(4096).is_ok());
        assert!(policy().validate(4095).is_err());
        assert!(
            NativeWalReclaimPolicy {
                trigger_bytes: 0,
                ..policy()
            }
            .validate(4096)
            .is_err()
        );
        assert!(
            NativeWalReclaimPolicy {
                min_interval: Duration::ZERO,
                ..policy()
            }
            .validate(4096)
            .is_err()
        );
        assert!(
            NativeWalReclaimPolicy {
                min_interval: Duration::MAX,
                ..policy()
            }
            .validate(4096)
            .is_err()
        );
    }

    #[test]
    fn first_due_and_long_round_use_observed_time_without_catch_up() -> Result<(), IndexError> {
        let start = Instant::now();
        let mut schedule = NativeWalReclaimSchedule::new(policy(), start)?;
        let first_due = start
            .checked_add(Duration::from_secs(2))
            .ok_or(IndexError::ResourceLimited)?;
        let before_due = first_due
            .checked_sub(Duration::from_nanos(1))
            .ok_or(IndexError::ResourceLimited)?;
        require(
            !schedule.try_begin(start)?,
            "WAL reclaim started immediately",
        )?;
        require(
            !schedule.try_begin(before_due)?,
            "WAL reclaim started early",
        )?;
        require(
            schedule.try_begin(first_due)?,
            "first WAL reclaim was not due",
        )?;
        require(
            !schedule.try_begin(first_due)?,
            "WAL reclaim repeated at due",
        )?;
        let late = start
            .checked_add(Duration::from_secs(10))
            .ok_or(IndexError::ResourceLimited)?;
        let after_late = late
            .checked_add(Duration::from_secs(1))
            .ok_or(IndexError::ResourceLimited)?;
        let next_due = late
            .checked_add(Duration::from_secs(2))
            .ok_or(IndexError::ResourceLimited)?;
        require(schedule.try_begin(late)?, "long round skipped due reclaim")?;
        require(
            !schedule.try_begin(after_late)?,
            "WAL reclaim caught up after a long round",
        )?;
        require(
            schedule.try_begin(next_due)?,
            "next WAL reclaim was not due",
        )?;
        Ok(())
    }

    #[test]
    fn clock_overflow_does_not_advance_schedule() {
        let now = Instant::now();
        let oversized = NativeWalReclaimPolicy {
            min_interval: Duration::MAX,
            ..policy()
        };
        assert!(matches!(
            NativeWalReclaimSchedule::new(oversized, now),
            Err(IndexError::ResourceLimited)
        ));
        let mut schedule = NativeWalReclaimSchedule {
            policy: oversized,
            next_due: now,
        };
        assert!(matches!(
            schedule.try_begin(now),
            Err(IndexError::ResourceLimited)
        ));
        assert_eq!(schedule.next_due, now);
    }
}
