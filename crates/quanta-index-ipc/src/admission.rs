//! Connection and dispatch admission for the UDS servers (QI-BB-002).
//!
//! The phase-1 server handled every connection inline on the accept thread:
//! a peer that sent a length prefix and then stalled, or a long native
//! search, blocked every other client of that socket, and a client that
//! gave up left its query running to completion with nothing to tell the
//! dispatcher.
//!
//! Admission is ordered ingress → dispatch slot → execution. Each accepted
//! connection gets its own thread (bounded by `max_connections`; past that
//! the accept loop closes the connection immediately rather than queue it
//! unbounded), so reading a request never waits on another peer. Running the
//! dispatcher takes one of `dispatch_slots`; a request that cannot get one
//! within `queue_wait` is answered with a typed overload refusal instead of
//! waiting forever. Every dispatch runs under a
//! [`RequestBudgetV1`](quanta_index_core::RequestBudgetV1) whose deadline is
//! `dispatch_budget` from admission and whose cancellation is armed when the
//! peer hangs up mid-dispatch.

use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

use crate::codec::IpcError;

/// Ingress and dispatch limits for one server.
///
/// Fields are private so every policy in existence is a valid one: zero
/// slots or zero connections would refuse every request and is a
/// configuration defect, not a request for a disabled server.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ServerAdmissionPolicy {
    max_connections: usize,
    dispatch_slots: usize,
    queue_wait: Duration,
    dispatch_budget: Duration,
    io_timeout: Duration,
}

impl ServerAdmissionPolicy {
    /// Deployment default for a query socket: 64 live connections, 4
    /// concurrent dispatches, a 2s wait for a slot, a 20s dispatch budget
    /// and a 30s per-operation I/O timeout.
    pub const DEFAULT: Self = Self {
        max_connections: 64,
        dispatch_slots: 4,
        queue_wait: Duration::from_secs(2),
        dispatch_budget: Duration::from_secs(20),
        io_timeout: Duration::from_secs(30),
    };

    /// A policy for sockets whose mutations must not interleave: one
    /// dispatch at a time, but connections still read independently so a
    /// stalled peer cannot block the socket.
    pub const SERIAL_DISPATCH: Self = Self {
        max_connections: 64,
        dispatch_slots: 1,
        queue_wait: Duration::from_secs(10),
        dispatch_budget: Duration::from_secs(120),
        io_timeout: Duration::from_secs(30),
    };

    pub fn new(
        max_connections: usize,
        dispatch_slots: usize,
        queue_wait: Duration,
        dispatch_budget: Duration,
        io_timeout: Duration,
    ) -> Result<Self, IpcError> {
        if max_connections == 0
            || dispatch_slots == 0
            || dispatch_budget.is_zero()
            || io_timeout.is_zero()
        {
            return Err(IpcError::InvalidAdmissionPolicy);
        }
        if dispatch_slots > max_connections {
            return Err(IpcError::InvalidAdmissionPolicy);
        }
        Ok(Self {
            max_connections,
            dispatch_slots,
            queue_wait,
            dispatch_budget,
            io_timeout,
        })
    }

    #[must_use]
    pub const fn max_connections(self) -> usize {
        self.max_connections
    }

    #[must_use]
    pub const fn dispatch_slots(self) -> usize {
        self.dispatch_slots
    }

    #[must_use]
    pub const fn queue_wait(self) -> Duration {
        self.queue_wait
    }

    #[must_use]
    pub const fn dispatch_budget(self) -> Duration {
        self.dispatch_budget
    }

    #[must_use]
    pub const fn io_timeout(self) -> Duration {
        self.io_timeout
    }
}

/// A counting semaphore over dispatch slots.
///
/// `std` has no semaphore; this one is a mutex-guarded count with a condvar,
/// which is all a handful of slots needs and keeps the wait bounded by the
/// caller's deadline rather than by a fairness scheme.
#[derive(Debug)]
pub struct DispatchSlots {
    free: Mutex<usize>,
    released: Condvar,
    capacity: usize,
}

/// One held dispatch slot; dropping it releases the slot.
#[derive(Debug)]
pub struct DispatchPermit<'a> {
    slots: &'a DispatchSlots,
}

impl Drop for DispatchPermit<'_> {
    fn drop(&mut self) {
        self.slots.release();
    }
}

/// Why a slot could not be taken.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SlotRefusal {
    /// `queue_wait` passed with every slot still busy.
    QueueWaitExceeded { waited: Duration, slots: usize },
}

impl DispatchSlots {
    #[must_use]
    pub const fn new(capacity: usize) -> Self {
        Self {
            free: Mutex::new(capacity),
            released: Condvar::new(),
            capacity,
        }
    }

    #[must_use]
    pub const fn capacity(&self) -> usize {
        self.capacity
    }

    /// Take a slot, waiting at most `queue_wait`.
    pub fn acquire(&self, queue_wait: Duration) -> Result<DispatchPermit<'_>, SlotRefusal> {
        let started = Instant::now();
        let mut free = match self.free.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        loop {
            if *free > 0 {
                *free = free.saturating_sub(1);
                return Ok(DispatchPermit { slots: self });
            }
            let waited = started.elapsed();
            let Some(left) = queue_wait.checked_sub(waited) else {
                return Err(SlotRefusal::QueueWaitExceeded {
                    waited,
                    slots: self.capacity,
                });
            };
            let (guard, timeout) = match self.released.wait_timeout(free, left) {
                Ok(outcome) => outcome,
                Err(poisoned) => poisoned.into_inner(),
            };
            free = guard;
            if timeout.timed_out() && *free == 0 {
                return Err(SlotRefusal::QueueWaitExceeded {
                    waited: started.elapsed(),
                    slots: self.capacity,
                });
            }
        }
    }

    fn release(&self) {
        let mut free = match self.free.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        *free = free.saturating_add(1).min(self.capacity);
        drop(free);
        self.released.notify_one();
    }

    /// Slots not currently held.
    #[must_use]
    pub fn available(&self) -> usize {
        match self.free.lock() {
            Ok(guard) => *guard,
            Err(poisoned) => *poisoned.into_inner(),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::thread;
    use std::time::Duration;

    use super::{DispatchSlots, ServerAdmissionPolicy, SlotRefusal};

    #[test]
    fn a_policy_refuses_zero_limits_and_more_slots_than_connections() {
        let second = Duration::from_secs(1);
        assert!(ServerAdmissionPolicy::new(0, 1, second, second, second).is_err());
        assert!(ServerAdmissionPolicy::new(1, 0, second, second, second).is_err());
        assert!(ServerAdmissionPolicy::new(1, 1, second, Duration::ZERO, second).is_err());
        assert!(ServerAdmissionPolicy::new(1, 1, second, second, Duration::ZERO).is_err());
        assert!(ServerAdmissionPolicy::new(1, 2, second, second, second).is_err());
        assert!(ServerAdmissionPolicy::new(2, 2, Duration::ZERO, second, second).is_ok());
    }

    #[test]
    fn slots_are_bounded_and_a_full_queue_is_refused_after_the_wait() {
        let slots = DispatchSlots::new(1);
        let held = slots.acquire(Duration::ZERO).expect("first slot is free");
        assert_eq!(slots.available(), 0);
        match slots.acquire(Duration::from_millis(30)) {
            Err(SlotRefusal::QueueWaitExceeded { waited, slots: 1 }) => {
                assert!(waited >= Duration::from_millis(30));
            }
            other => panic!("expected a queue-wait refusal, got {other:?}"),
        }
        drop(held);
        assert_eq!(slots.available(), 1);
        let again = slots
            .acquire(Duration::ZERO)
            .expect("released slot is free again");
        drop(again);
    }

    #[test]
    fn a_waiter_gets_the_slot_when_the_holder_releases_within_the_wait() {
        let slots = Arc::new(DispatchSlots::new(1));
        let held = slots.acquire(Duration::ZERO).expect("first slot is free");
        let waiter = {
            let slots = Arc::clone(&slots);
            thread::spawn(move || {
                slots.acquire(Duration::from_secs(5)).map(|permit| {
                    drop(permit);
                })
            })
        };
        thread::sleep(Duration::from_millis(50));
        drop(held);
        let outcome = waiter.join().expect("waiter thread");
        assert!(outcome.is_ok(), "{outcome:?}");
        assert_eq!(slots.available(), 1);
    }
}
