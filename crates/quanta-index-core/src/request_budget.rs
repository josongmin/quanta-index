//! The per-request execution budget every route runs under (QI-BB-002).
//!
//! A request carries one absolute deadline and one cancellation flag from
//! the transport down to the last collector. Native work cannot be
//! interrupted from outside (G0-R pinned that), so the budget is
//! cooperative: execution calls [`RequestBudgetV1::checkpoint`] at its own
//! boundaries — before a native search, between lanes, before encoding —
//! and, since W5 phase 2, the lexical adapter observes the budget inside
//! its native scans and candidate loops through a probe and reports what
//! it saw with [`RequestBudgetV1::interrupted_at`]; since W5 phase 3 the
//! semantic adapter does the same around its in-flight vector query and
//! the rows it reads back. Either way execution stops with a typed
//! interruption naming the checkpoint that observed it. A request that
//! "was cancelled" always names where, never merely that the client left.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crate::error::CoreError;

/// Wire code for a request that ran past its deadline.
pub const REQUEST_DEADLINE_EXCEEDED_CODE: &str = "REQUEST_DEADLINE_EXCEEDED";
/// Wire code for a request abandoned by its peer while it was running.
pub const REQUEST_CANCELLED_CODE: &str = "REQUEST_CANCELLED";

/// Why a checkpoint stopped a request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BudgetInterruptionV1 {
    /// The absolute deadline passed; `over_by` is how far past it the
    /// checkpoint ran.
    DeadlineExceeded { over_by: Duration },
    /// The peer disconnected (or the owner cancelled) before this checkpoint.
    Cancelled,
}

/// Cancels the budget it was taken from. Held by whoever watches the peer.
#[derive(Clone, Debug)]
pub struct CancelHandleV1 {
    cancelled: Arc<AtomicBool>,
}

impl CancelHandleV1 {
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }
}

/// One request's deadline and cancellation state.
#[derive(Clone, Debug)]
pub struct RequestBudgetV1 {
    deadline: Instant,
    cancelled: Arc<AtomicBool>,
}

impl RequestBudgetV1 {
    /// A budget that ends at `deadline`.
    #[must_use]
    pub fn until(deadline: Instant) -> Self {
        Self {
            deadline,
            cancelled: Arc::new(AtomicBool::new(false)),
        }
    }

    /// A budget of `budget` from now.
    #[must_use]
    pub fn for_duration(budget: Duration) -> Self {
        Self::until(
            Instant::now()
                .checked_add(budget)
                .unwrap_or_else(far_future),
        )
    }

    /// A budget that never interrupts. For callers with no transport peer
    /// and no deadline of their own (in-process tests, offline tools); a
    /// served request always has a real one.
    #[must_use]
    pub fn unbounded() -> Self {
        Self::until(far_future())
    }

    #[must_use]
    pub fn cancel_handle(&self) -> CancelHandleV1 {
        CancelHandleV1 {
            cancelled: Arc::clone(&self.cancelled),
        }
    }

    #[must_use]
    pub const fn deadline(&self) -> Instant {
        self.deadline
    }

    /// Time left before the deadline; zero once it has passed.
    #[must_use]
    pub fn remaining(&self) -> Duration {
        self.deadline.saturating_duration_since(Instant::now())
    }

    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }

    /// What, if anything, has interrupted this request by now.
    #[must_use]
    pub fn interruption(&self) -> Option<BudgetInterruptionV1> {
        if self.is_cancelled() {
            return Some(BudgetInterruptionV1::Cancelled);
        }
        let now = Instant::now();
        if now > self.deadline {
            return Some(BudgetInterruptionV1::DeadlineExceeded {
                over_by: now.saturating_duration_since(self.deadline),
            });
        }
        None
    }

    /// Stop here if the request is over budget, naming the checkpoint.
    ///
    /// Execution places these at every boundary it owns; a native call
    /// between two checkpoints observes the budget through its own probe
    /// where it can (W5 phase 2) and runs to completion where it cannot.
    pub fn checkpoint(&self, stage: &'static str) -> Result<(), CoreError> {
        self.interrupted_at(stage).map_or(Ok(()), Err)
    }

    /// The typed interruption a checkpoint at `stage` would raise now, if
    /// any.
    ///
    /// For code that observed the budget elsewhere (inside a native scan)
    /// and reports the interruption after the scan unwound.
    #[must_use]
    pub fn interrupted_at(&self, stage: &'static str) -> Option<CoreError> {
        match self.interruption()? {
            BudgetInterruptionV1::Cancelled => Some(CoreError::Typed {
                code: REQUEST_CANCELLED_CODE.to_string(),
                message: format!("request cancelled by its peer; observed at checkpoint `{stage}`"),
            }),
            BudgetInterruptionV1::DeadlineExceeded { over_by } => Some(CoreError::Typed {
                code: REQUEST_DEADLINE_EXCEEDED_CODE.to_string(),
                message: format!(
                    "request deadline exceeded by {}ms; observed at checkpoint `{stage}`",
                    over_by.as_millis()
                ),
            }),
        }
    }
}

/// An instant so far ahead that no request reaches it; `Instant` has no
/// `MAX`, so this is the largest offset the platform clock accepts.
fn far_future() -> Instant {
    let now = Instant::now();
    // Halve the range until the addition is representable.
    let mut offset = Duration::from_secs(u64::MAX.div_euclid(4));
    loop {
        if let Some(instant) = now.checked_add(offset) {
            return instant;
        }
        offset = offset.div_f64(2.0);
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::{
        BudgetInterruptionV1, REQUEST_CANCELLED_CODE, REQUEST_DEADLINE_EXCEEDED_CODE,
        RequestBudgetV1,
    };
    use crate::CoreError;

    #[test]
    fn a_fresh_budget_passes_its_checkpoints() {
        let budget = RequestBudgetV1::for_duration(Duration::from_secs(60));
        assert!(budget.checkpoint("entry").is_ok());
        assert!(budget.interruption().is_none());
        assert!(budget.remaining() > Duration::from_secs(50));
    }

    fn just_expired() -> RequestBudgetV1 {
        let deadline = Instant::now()
            .checked_sub(Duration::from_millis(5))
            .expect("the clock is more than five milliseconds old");
        RequestBudgetV1::until(deadline)
    }

    #[test]
    fn an_expired_budget_names_the_checkpoint_and_the_overrun() {
        let budget = just_expired();
        match budget.checkpoint("after-lexical") {
            Err(CoreError::Typed { code, message }) => {
                assert_eq!(code, REQUEST_DEADLINE_EXCEEDED_CODE);
                assert!(message.contains("after-lexical"), "{message}");
            }
            other => panic!("expected a typed deadline interruption, got {other:?}"),
        }
        assert!(matches!(
            budget.interruption(),
            Some(BudgetInterruptionV1::DeadlineExceeded { .. })
        ));
        assert_eq!(budget.remaining(), Duration::ZERO);
    }

    #[test]
    fn cancellation_wins_over_the_deadline_and_names_the_checkpoint() {
        let budget = just_expired();
        budget.cancel_handle().cancel();
        match budget.checkpoint("before-encode") {
            Err(CoreError::Typed { code, message }) => {
                assert_eq!(code, REQUEST_CANCELLED_CODE);
                assert!(message.contains("before-encode"), "{message}");
            }
            other => panic!("expected a typed cancellation, got {other:?}"),
        }
        assert!(budget.is_cancelled());
    }

    #[test]
    fn an_unbounded_budget_never_interrupts() {
        let budget = RequestBudgetV1::unbounded();
        assert!(budget.checkpoint("anywhere").is_ok());
        assert!(budget.remaining() > Duration::from_secs(365 * 24 * 3600));
    }
}
