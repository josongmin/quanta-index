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

use quanta_index_core::{CoreError, RequestBudgetV1};

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

    /// Wake one waiter parked in [`Flight::await_outcome`]: the budget's
    /// `cancel` runs this after flipping the flag.
    ///
    /// The outcome mutex is locked before notifying so a cancel racing the
    /// waiter's flag check cannot slip between the check and the sleep:
    /// either the waiter still holds the lock (and the wake lands after it
    /// sleeps) or it already sleeps (and the wake lands at once).
    fn wake_waiter(flight: &Arc<Self>) {
        if let Ok(_outcome) = flight.outcome.lock() {
            flight.ready.notify_all();
        }
    }

    /// Wait for the flight under `budget`: the condvar wakes the waiter
    /// when the opener settles, the budget's cancellation wakes it the
    /// moment `cancel` runs, and the wait ends at the deadline — with no
    /// poll quantum in between. The interruption is named `checkpoint`.
    ///
    /// Takes the flight behind its registry [`Arc`] so the cancellation
    /// wake owns a reference: a `cancel` racing the waiter's return can
    /// still invoke a cloned wake after deregistration, and that wake must
    /// never dangle.
    pub(crate) fn await_outcome(
        flight: &Arc<Self>,
        budget: &RequestBudgetV1,
        checkpoint: &'static str,
    ) -> Result<Arc<H>, AwaitFlightFailure>
    where
        H: Send + Sync + 'static,
    {
        let woken = Arc::clone(flight);
        let wake: Arc<dyn Fn() + Send + Sync> = Arc::new(move || Self::wake_waiter(&woken));
        let _registered = budget.cancel_waiter(wake);
        let mut outcome = flight.lock_outcome().map_err(AwaitFlightFailure::Flight)?;
        loop {
            if let Some(settled) = outcome.as_ref() {
                return settled.clone().map_err(AwaitFlightFailure::Flight);
            }
            if let Some(interruption) = budget.interrupted_at(checkpoint) {
                return Err(AwaitFlightFailure::Interrupted(interruption));
            }
            // The only timed bound left: the wait ends exactly at the
            // deadline, where the next loop turn reports it typed.
            let slice = budget.remaining();
            let (guard, _timed_out) = flight.ready.wait_timeout(outcome, slice).map_err(|err| {
                AwaitFlightFailure::Flight(CoreError::Storage(format!(
                    "single-flight outcome poisoned: {err}"
                )))
            })?;
            outcome = guard;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Barrier};
    use std::time::Duration;

    use quanta_index_core::RequestBudgetV1;

    use super::{AwaitFlightFailure, Flight};

    struct Dummy(u64);

    fn budget() -> RequestBudgetV1 {
        RequestBudgetV1::for_duration(Duration::from_secs(60))
    }

    #[test]
    fn a_settled_flight_shares_its_handle_with_every_waiter() {
        let flight = Arc::new(Flight::<Dummy>::new());
        let start = Arc::new(Barrier::new(5));
        let waiters: Vec<_> = (0..4)
            .map(|_| {
                let (flight, start, budget) = (Arc::clone(&flight), Arc::clone(&start), budget());
                std::thread::spawn(move || {
                    let _arrived = start.wait();
                    Flight::await_outcome(&flight, &budget, "settle-broadcast")
                })
            })
            .collect();
        let _arrived = start.wait();
        flight
            .settle(Ok(Arc::new(Dummy(7))))
            .expect("settle succeeds");
        for waiter in waiters {
            match waiter.join().expect("waiter thread") {
                Ok(handle) => assert_eq!(handle.0, 7),
                other => panic!(
                    "every waiter shares the settled handle, got {}",
                    describe(&other)
                ),
            }
        }
    }

    #[test]
    fn a_cancelled_budget_interrupts_every_waiter_while_the_flight_lands() {
        // One budget shared by every waiter: a single cancel interrupts
        // all of them, parked or not, and the flight still lands for
        // whoever still waits.
        let flight = Arc::new(Flight::<Dummy>::new());
        let budget = budget();
        let cancel = budget.cancel_handle();
        let start = Arc::new(Barrier::new(5));
        let waiters: Vec<_> = (0..4)
            .map(|_| {
                let (flight, start, budget) =
                    (Arc::clone(&flight), Arc::clone(&start), budget.clone());
                std::thread::spawn(move || {
                    let _arrived = start.wait();
                    Flight::await_outcome(&flight, &budget, "cancel-broadcast")
                })
            })
            .collect();
        let _arrived = start.wait();
        cancel.cancel();
        for waiter in waiters {
            match waiter.join().expect("waiter thread") {
                Err(AwaitFlightFailure::Interrupted(_)) => {}
                other => panic!(
                    "a cancelled waiter is interrupted, got {}",
                    describe(&other)
                ),
            }
        }
        flight
            .settle(Ok(Arc::new(Dummy(9))))
            .expect("the flight lands after its waiters left");
        let late = RequestBudgetV1::for_duration(Duration::from_secs(60));
        match Flight::await_outcome(&flight, &late, "late-waiter") {
            Ok(handle) => assert_eq!(handle.0, 9),
            other => panic!(
                "a late waiter shares the landed outcome, got {}",
                describe(&other)
            ),
        }
    }

    fn describe(outcome: &Result<Arc<Dummy>, AwaitFlightFailure>) -> &'static str {
        match outcome {
            Ok(_) => "Ok",
            Err(AwaitFlightFailure::Flight(_)) => "Flight",
            Err(AwaitFlightFailure::Interrupted(_)) => "Interrupted",
        }
    }

    #[test]
    fn cancel_before_registration_is_observed_without_blocking() {
        let flight = Arc::new(Flight::<Dummy>::new());
        let budget = budget();
        budget.cancel_handle().cancel();
        match Flight::await_outcome(&flight, &budget, "pre-cancelled") {
            Err(AwaitFlightFailure::Interrupted(_)) => {}
            other => panic!(
                "a pre-cancelled wait is interrupted, got {}",
                describe(&other)
            ),
        }
    }

    #[test]
    fn a_deadline_ends_the_wait_typed_with_no_event() {
        use quanta_index_core::{CoreError, REQUEST_DEADLINE_EXCEEDED_CODE};

        let flight = Arc::new(Flight::<Dummy>::new());
        let budget = RequestBudgetV1::for_duration(Duration::from_millis(20));
        match Flight::await_outcome(&flight, &budget, "deadline") {
            Err(AwaitFlightFailure::Interrupted(CoreError::Typed { code, message })) => {
                assert_eq!(code, REQUEST_DEADLINE_EXCEEDED_CODE);
                assert!(message.contains("deadline"), "{message}");
            }
            other => panic!("an expired wait is interrupted, got {}", describe(&other)),
        }
    }

    #[test]
    fn settle_and_cancel_racing_give_each_waiter_exactly_one_terminal() {
        for _ in 0..200 {
            let flight = Arc::new(Flight::<Dummy>::new());
            let budget = budget();
            let race = Arc::new(Barrier::new(3));
            let waiter = {
                let (flight, race, budget) =
                    (Arc::clone(&flight), Arc::clone(&race), budget.clone());
                std::thread::spawn(move || {
                    let _arrived = race.wait();
                    Flight::await_outcome(&flight, &budget, "race")
                })
            };
            let settler = {
                let (flight, race) = (Arc::clone(&flight), Arc::clone(&race));
                std::thread::spawn(move || {
                    let _arrived = race.wait();
                    flight.settle(Ok(Arc::new(Dummy(1))))
                })
            };
            let canceller = {
                let (race, cancel) = (Arc::clone(&race), budget.cancel_handle());
                std::thread::spawn(move || {
                    let _arrived = race.wait();
                    cancel.cancel();
                })
            };
            // Every interleaving ends in exactly one terminal per waiter:
            // the shared handle, the opener's failure, or the interruption.
            // The match is exhaustive, so a fourth outcome fails to compile.
            match waiter.join().expect("waiter thread") {
                Ok(handle) => assert_eq!(handle.0, 1),
                Err(AwaitFlightFailure::Flight(_) | AwaitFlightFailure::Interrupted(_)) => {}
            }
            settler.join().expect("settler thread").expect("settle ok");
            canceller.join().expect("canceller thread");
        }
    }

    #[test]
    fn a_poisoned_outcome_is_a_flight_error_never_an_interruption() {
        let flight = Arc::new(Flight::<Dummy>::new());
        let poisoned = {
            let flight = Arc::clone(&flight);
            std::thread::spawn(move || {
                let _held = flight.lock_outcome().expect("lock held");
                panic!("poison the outcome mutex");
            })
            .join()
        };
        assert!(poisoned.is_err(), "the poisoning thread panicked");
        match Flight::await_outcome(&flight, &budget(), "poisoned") {
            Err(AwaitFlightFailure::Flight(_)) => {}
            other => panic!("a poisoned flight errors, got {}", describe(&other)),
        }
    }
}
