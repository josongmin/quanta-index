use std::time::Instant;
use thiserror::Error;

#[derive(Clone, Debug, Eq, PartialEq, Error)]
pub enum IndexError {
    #[error("incremental input is invalid: {0}")]
    InvalidInput(String),
    #[error("incremental precondition conflict")]
    Conflict,
    #[error("operation digest conflicts with retained admission")]
    DigestConflict,
    #[error("attempt fence is no longer current")]
    FenceLost,
    #[error("owner work claim expired or was superseded")]
    WorkFenceLost,
    #[error("incremental work budget exhausted")]
    ResourceLimited,
    #[error("incremental execution cancelled; admitted operation custody is retained")]
    Cancelled,
    #[error("exact snapshot is unavailable")]
    SnapshotUnavailable,
    #[error("immutable object is corrupt: {0}")]
    CorruptObject(String),
    #[error("adapter violated its binding contract: {0}")]
    BackendViolation(String),
    #[error("storage failure: {0}")]
    Storage(String),
    #[error("explicit rebuild required")]
    RebuildRequired,
    #[error("bounded maintenance required")]
    MaintenanceRequired,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WorkLimits {
    pub reads: u64,
    pub writes: u64,
    pub bytes: u64,
    pub mutations: u64,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct WorkUsage {
    pub reads: u64,
    pub writes: u64,
    pub bytes: u64,
    pub mutations: u64,
}

#[derive(Debug)]
pub struct WorkBudget {
    limits: WorkLimits,
    usage: WorkUsage,
    request: crate::RequestBudgetV1,
}

impl WorkBudget {
    #[must_use]
    pub fn new(limits: WorkLimits, deadline: Instant) -> Self {
        Self::with_request(limits, crate::RequestBudgetV1::until(deadline))
    }

    #[must_use]
    pub fn with_request(limits: WorkLimits, request: crate::RequestBudgetV1) -> Self {
        Self {
            limits,
            usage: WorkUsage::default(),
            request,
        }
    }

    pub fn checkpoint(&self) -> Result<(), IndexError> {
        if self.request.is_cancelled() {
            return Err(IndexError::Cancelled);
        }
        if Instant::now() >= self.request.deadline() {
            Err(IndexError::ResourceLimited)
        } else {
            Ok(())
        }
    }

    pub fn charge(&mut self, delta: WorkUsage) -> Result<(), IndexError> {
        self.checkpoint()?;
        let add = |used: u64, extra: u64, limit: u64| {
            used.checked_add(extra)
                .filter(|next| *next <= limit)
                .ok_or(IndexError::ResourceLimited)
        };
        let next = WorkUsage {
            reads: add(self.usage.reads, delta.reads, self.limits.reads)?,
            writes: add(self.usage.writes, delta.writes, self.limits.writes)?,
            bytes: add(self.usage.bytes, delta.bytes, self.limits.bytes)?,
            mutations: add(self.usage.mutations, delta.mutations, self.limits.mutations)?,
        };
        self.usage = next;
        Ok(())
    }

    #[must_use]
    pub const fn usage(&self) -> WorkUsage {
        self.usage
    }

    /// The provider receives the same deadline and cancellation authority as
    /// admission. It must not create a fresh timeout when execution resumes.
    #[must_use]
    pub const fn request(&self) -> &crate::RequestBudgetV1 {
        &self.request
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    fn limits() -> WorkLimits {
        WorkLimits {
            reads: 3,
            writes: 2,
            bytes: 10,
            mutations: 4,
        }
    }

    #[test]
    fn rejected_composite_charge_preserves_all_usage_fields() -> Result<(), IndexError> {
        let deadline = Instant::now()
            .checked_add(Duration::from_secs(30))
            .ok_or(IndexError::ResourceLimited)?;
        let mut budget = WorkBudget::new(limits(), deadline);
        let accepted = WorkUsage {
            reads: 1,
            writes: 1,
            bytes: 4,
            mutations: 1,
        };
        budget.charge(accepted)?;
        assert_eq!(
            budget.charge(WorkUsage {
                reads: 1,
                writes: 1,
                bytes: 7,
                mutations: 1,
            }),
            Err(IndexError::ResourceLimited)
        );
        assert_eq!(budget.usage(), accepted);
        budget.charge(WorkUsage {
            reads: 2,
            writes: 1,
            bytes: 6,
            mutations: 3,
        })?;
        assert_eq!(
            budget.usage(),
            WorkUsage {
                reads: 3,
                writes: 2,
                bytes: 10,
                mutations: 4,
            }
        );
        Ok(())
    }

    #[test]
    fn expired_budget_rejects_before_accounting() {
        let mut budget = WorkBudget::new(limits(), Instant::now());
        assert_eq!(
            budget.charge(WorkUsage {
                reads: 1,
                ..WorkUsage::default()
            }),
            Err(IndexError::ResourceLimited)
        );
        assert_eq!(budget.usage(), WorkUsage::default());
    }

    #[test]
    fn provider_request_keeps_original_cancellation_authority() -> Result<(), IndexError> {
        let deadline = Instant::now()
            .checked_add(Duration::from_secs(30))
            .ok_or(IndexError::ResourceLimited)?;
        let request = crate::RequestBudgetV1::until(deadline);
        let mut budget = WorkBudget::with_request(limits(), request.clone());
        assert_eq!(budget.request().deadline(), deadline);
        request.cancel_handle().cancel();
        assert!(budget.request().is_cancelled());
        assert_eq!(
            budget.charge(WorkUsage {
                writes: 1,
                ..WorkUsage::default()
            }),
            Err(IndexError::Cancelled)
        );
        assert_eq!(budget.usage(), WorkUsage::default());
        Ok(())
    }
}
