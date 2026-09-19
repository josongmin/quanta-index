//! One cold open that concurrent readers of the same key wait on.
//!
//! The sealed-generation snapshot registry (QI-BB-001) and the history
//! text epoch handles (QI-BB-020) both open a handle once per key and let
//! every other reader of that key share the outcome: the handle, or the
//! opener's own typed failure by value. A reader waits under its own
//! request budget, so a wait on someone else's cold open ends with the
//! reader's interruption while the open lands for whoever still waits.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::Duration;

use quanta_index_core::{CoreError, RequestBudgetV1};

/// How often a waiting reader wakes to observe its budget while the flight
/// is still open: cancellation does not signal the flight's condvar, so the
/// wait is sliced.
const AWAIT_FLIGHT_POLL: Duration = Duration::from_millis(20);

/// How a flight settled: the handle, or the opener's failure by value.
type FlightOutcome<H> = Option<Result<Arc<H>, CoreError>>;

/// One in-progress cold open that concurrent readers can wait on.
///
/// `fenced` is the owner's mark that the key was retired while the open
/// ran; the owner decides what a fenced landing means.
pub(crate) struct Flight<H: ?Sized> {
    outcome: Mutex<FlightOutcome<H>>,
    ready: Condvar,
    fenced: AtomicBool,
}

/// Why a wait on a flight ended without the flight's outcome.
pub(crate) enum AwaitFlightFailure {
    /// The flight settled with the opener's failure, shared by value.
    Flight(CoreError),
    /// The waiter's own budget interrupted it; the flight is still landing.
    Interrupted(CoreError),
}

impl<H: ?Sized> Flight<H> {
    pub(crate) fn new() -> Self {
        Self {
            outcome: Mutex::new(None),
            ready: Condvar::new(),
            fenced: AtomicBool::new(false),
        }
    }

    fn lock_outcome(&self) -> Result<MutexGuard<'_, FlightOutcome<H>>, CoreError> {
        self.outcome
            .lock()
            .map_err(|err| CoreError::Storage(format!("single-flight outcome poisoned: {err}")))
    }

    /// Deliver the opener's outcome to every waiter.
    pub(crate) fn settle(&self, outcome: Result<Arc<H>, CoreError>) -> Result<(), CoreError> {
        *self.lock_outcome()? = Some(outcome);
        self.ready.notify_all();
        Ok(())
    }

    /// Mark the flight's key retired while the open runs.
    pub(crate) fn fence(&self) {
        self.fenced.store(true, Ordering::Release);
    }

    pub(crate) fn is_fenced(&self) -> bool {
        self.fenced.load(Ordering::Acquire)
    }

    /// Block until the flight settles, with no budget: for a retirement,
    /// whose wait is bounded by the one open already running.
    pub(crate) fn wait_settled(&self) -> Result<(), CoreError> {
        let mut outcome = self.lock_outcome()?;
        while outcome.is_none() {
            outcome = self.ready.wait(outcome).map_err(|err| {
                CoreError::Storage(format!("single-flight outcome poisoned: {err}"))
            })?;
        }
        drop(outcome);
        Ok(())
    }

    /// Wait for the flight under `budget`: the condvar wakes the waiter
    /// when the opener settles, and the budget is observed on every wake
    /// and at least every [`AWAIT_FLIGHT_POLL`], its interruption named
    /// `checkpoint`.
    pub(crate) fn await_outcome(
        &self,
        budget: &RequestBudgetV1,
        checkpoint: &'static str,
    ) -> Result<Arc<H>, AwaitFlightFailure> {
        let mut outcome = self.lock_outcome().map_err(AwaitFlightFailure::Flight)?;
        loop {
            if let Some(settled) = outcome.as_ref() {
                return settled.clone().map_err(AwaitFlightFailure::Flight);
            }
            if let Some(interruption) = budget.interrupted_at(checkpoint) {
                return Err(AwaitFlightFailure::Interrupted(interruption));
            }
            let slice = budget.remaining().min(AWAIT_FLIGHT_POLL);
            let (guard, _timed_out) = self.ready.wait_timeout(outcome, slice).map_err(|err| {
                AwaitFlightFailure::Flight(CoreError::Storage(format!(
                    "single-flight outcome poisoned: {err}"
                )))
            })?;
            outcome = guard;
        }
    }
}
