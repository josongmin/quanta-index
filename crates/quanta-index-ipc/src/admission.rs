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
//! dispatcher takes one of `dispatch_slots`, and at most
//! `max_in_flight_per_repo` of them may serve one repository at a time, so
//! one hot repository cannot hold every slot against the others; a request
//! that cannot get its slot within `queue_wait` is answered with a typed
//! overload refusal — naming the repository when that was the bound it hit
//! — instead of waiting forever. Every dispatch runs under a
//! [`RequestBudgetV1`](quanta_index_core::RequestBudgetV1) whose deadline is
//! `dispatch_budget` from admission and whose cancellation is armed when the
//! peer hangs up mid-dispatch.

use std::collections::BTreeMap;
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

use quanta_index_contract::{ERR_SERVER_OVERLOADED, SearchPlaneIpcError};

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
    max_in_flight_per_repo: usize,
    queue_wait: Duration,
    dispatch_budget: Duration,
    io_timeout: Duration,
}

impl ServerAdmissionPolicy {
    /// Deployment default for a query socket: 64 live connections, 4
    /// concurrent dispatches of which one repository may hold 3, a 2s wait
    /// for a slot, a 20s dispatch budget and a 30s per-operation I/O
    /// timeout.
    pub const DEFAULT: Self = Self {
        max_connections: 64,
        dispatch_slots: 4,
        max_in_flight_per_repo: 3,
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
        max_in_flight_per_repo: 1,
        queue_wait: Duration::from_secs(10),
        dispatch_budget: Duration::from_secs(120),
        io_timeout: Duration::from_secs(30),
    };

    /// A policy with explicit limits.
    ///
    /// Refused: a zero connection, slot or per-repository count, a zero
    /// dispatch budget or I/O timeout, more slots than connections, and a
    /// per-repository count above the slot count (it could never bind).
    pub fn new(
        max_connections: usize,
        dispatch_slots: usize,
        max_in_flight_per_repo: usize,
        queue_wait: Duration,
        dispatch_budget: Duration,
        io_timeout: Duration,
    ) -> Result<Self, IpcError> {
        if max_connections == 0
            || dispatch_slots == 0
            || max_in_flight_per_repo == 0
            || dispatch_budget.is_zero()
            || io_timeout.is_zero()
        {
            return Err(IpcError::InvalidAdmissionPolicy);
        }
        if dispatch_slots > max_connections || max_in_flight_per_repo > dispatch_slots {
            return Err(IpcError::InvalidAdmissionPolicy);
        }
        Ok(Self {
            max_connections,
            dispatch_slots,
            max_in_flight_per_repo,
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

    /// Most dispatch slots one repository may hold at once.
    #[must_use]
    pub const fn max_in_flight_per_repo(self) -> usize {
        self.max_in_flight_per_repo
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

/// What the slots hold right now: how many are free, and how many each
/// repository holds.
#[derive(Debug)]
struct SlotLedger {
    free: usize,
    per_repo: BTreeMap<String, usize>,
}

/// A counting semaphore over dispatch slots with a per-repository cap.
///
/// `std` has no semaphore; this one is a mutex-guarded ledger with a
/// condvar, which is all a handful of slots needs and keeps the wait
/// bounded by the caller's deadline rather than by a fairness scheme. The
/// per-repository cap is the fairness the ticket asked for: with it below
/// the slot count, a burst on one repository always leaves a slot for the
/// others.
#[derive(Debug)]
pub struct DispatchSlots {
    ledger: Mutex<SlotLedger>,
    released: Condvar,
    capacity: usize,
    per_repo_capacity: usize,
}

/// One held dispatch slot; dropping it releases the slot and, when the
/// dispatch was scoped to a repository, that repository's hold.
#[derive(Debug)]
pub struct DispatchPermit<'a> {
    slots: &'a DispatchSlots,
    repo: Option<String>,
    /// Whether the permit waited for a slot at all before taking one.
    waited: bool,
}

impl DispatchPermit<'_> {
    /// Whether this permit had to wait for a slot (any slot, or its
    /// repository's) before it was granted.
    #[must_use]
    pub const fn waited(&self) -> bool {
        self.waited
    }
}

impl Drop for DispatchPermit<'_> {
    fn drop(&mut self) {
        self.slots.release(self.repo.as_deref());
    }
}

/// Why a slot could not be taken.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SlotRefusal {
    /// `queue_wait` passed with every slot still busy.
    QueueWaitExceeded { waited: Duration, slots: usize },
    /// `queue_wait` passed with the request's repository holding its
    /// per-repository cap the whole time; other repositories may have had
    /// slots free.
    RepoInFlightExceeded {
        waited: Duration,
        repo: String,
        per_repo: usize,
    },
}

impl SlotRefusal {
    /// The typed wire refusal for this outcome: `SERVER_OVERLOADED` either
    /// way, the message naming which bound held.
    #[must_use]
    pub fn ipc_error(&self) -> SearchPlaneIpcError {
        match self {
            Self::QueueWaitExceeded { waited, slots } => {
                SearchPlaneIpcError::overloaded(*waited, *slots)
            }
            Self::RepoInFlightExceeded {
                waited,
                repo,
                per_repo,
            } => SearchPlaneIpcError {
                code: ERR_SERVER_OVERLOADED.to_string(),
                message: format!(
                    "repository `{repo}` held its {per_repo} in-flight dispatch slot(s) for {} ms; retry with backoff",
                    waited.as_millis()
                ),
                repair: None,
            },
        }
    }

    /// Whether the bound that held was the per-repository one.
    #[must_use]
    pub const fn is_repo_scoped(&self) -> bool {
        matches!(self, Self::RepoInFlightExceeded { .. })
    }

    /// How long the request waited before it was refused.
    #[must_use]
    pub const fn waited(&self) -> Duration {
        match self {
            Self::QueueWaitExceeded { waited, .. } | Self::RepoInFlightExceeded { waited, .. } => {
                *waited
            }
        }
    }
}

impl DispatchSlots {
    /// Slots for `policy`: its slot count, with its per-repository cap.
    #[must_use]
    pub fn for_policy(policy: ServerAdmissionPolicy) -> Self {
        Self::new(policy.dispatch_slots(), policy.max_in_flight_per_repo())
    }

    /// `capacity` slots, of which one repository may hold at most
    /// `per_repo_capacity` (clamped to `capacity`; zero means one).
    #[must_use]
    pub const fn new(capacity: usize, per_repo_capacity: usize) -> Self {
        let per_repo_capacity = if per_repo_capacity == 0 {
            1
        } else if per_repo_capacity > capacity {
            capacity
        } else {
            per_repo_capacity
        };
        Self {
            ledger: Mutex::new(SlotLedger {
                free: capacity,
                per_repo: BTreeMap::new(),
            }),
            released: Condvar::new(),
            capacity,
            per_repo_capacity,
        }
    }

    #[must_use]
    pub const fn capacity(&self) -> usize {
        self.capacity
    }

    #[must_use]
    pub const fn per_repo_capacity(&self) -> usize {
        self.per_repo_capacity
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, SlotLedger> {
        match self.ledger.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    /// Take a slot for `repo` (or an unscoped one), waiting at most
    /// `queue_wait` for both a free slot and, when scoped, room under the
    /// repository's cap.
    pub fn acquire(
        &self,
        queue_wait: Duration,
        repo: Option<&str>,
    ) -> Result<DispatchPermit<'_>, SlotRefusal> {
        let started = Instant::now();
        let mut ledger = self.lock();
        let mut waited_at_all = false;
        loop {
            let repo_held = repo.map_or(0, |repo| ledger.per_repo.get(repo).copied().unwrap_or(0));
            let repo_blocked = repo.is_some() && repo_held >= self.per_repo_capacity;
            if ledger.free > 0 && !repo_blocked {
                ledger.free = ledger.free.saturating_sub(1);
                if let Some(repo) = repo {
                    let held = ledger.per_repo.entry(repo.to_string()).or_insert(0);
                    *held = held.saturating_add(1);
                }
                return Ok(DispatchPermit {
                    slots: self,
                    repo: repo.map(str::to_string),
                    waited: waited_at_all,
                });
            }
            waited_at_all = true;
            let waited = started.elapsed();
            let Some(left) = queue_wait.checked_sub(waited) else {
                return Err(self.refusal(repo, repo_blocked, waited));
            };
            let (guard, timeout) = match self.released.wait_timeout(ledger, left) {
                Ok(outcome) => outcome,
                Err(poisoned) => poisoned.into_inner(),
            };
            ledger = guard;
            if timeout.timed_out() {
                let repo_held =
                    repo.map_or(0, |repo| ledger.per_repo.get(repo).copied().unwrap_or(0));
                let repo_blocked = repo.is_some() && repo_held >= self.per_repo_capacity;
                if ledger.free == 0 || repo_blocked {
                    return Err(self.refusal(repo, repo_blocked, started.elapsed()));
                }
            }
        }
    }

    fn refusal(&self, repo: Option<&str>, repo_blocked: bool, waited: Duration) -> SlotRefusal {
        match repo {
            Some(repo) if repo_blocked => SlotRefusal::RepoInFlightExceeded {
                waited,
                repo: repo.to_string(),
                per_repo: self.per_repo_capacity,
            },
            Some(_) | None => SlotRefusal::QueueWaitExceeded {
                waited,
                slots: self.capacity,
            },
        }
    }

    fn release(&self, repo: Option<&str>) {
        let mut ledger = self.lock();
        ledger.free = ledger.free.saturating_add(1).min(self.capacity);
        if let Some(repo) = repo
            && let Some(held) = ledger.per_repo.get_mut(repo)
        {
            *held = held.saturating_sub(1);
            if *held == 0 {
                let _gone = ledger.per_repo.remove(repo);
            }
        }
        drop(ledger);
        // Every waiter re-checks its own repository's cap, so all of them
        // must wake: the one this release unblocks may not be the first.
        self.released.notify_all();
    }

    /// Slots not currently held.
    #[must_use]
    pub fn available(&self) -> usize {
        self.lock().free
    }

    /// Slots currently held.
    #[must_use]
    pub fn in_flight(&self) -> usize {
        self.capacity.saturating_sub(self.available())
    }

    /// Slots `repo` currently holds.
    #[must_use]
    pub fn in_flight_for_repo(&self, repo: &str) -> usize {
        self.lock().per_repo.get(repo).copied().unwrap_or(0)
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
        assert!(ServerAdmissionPolicy::new(0, 1, 1, second, second, second).is_err());
        assert!(ServerAdmissionPolicy::new(1, 0, 1, second, second, second).is_err());
        assert!(ServerAdmissionPolicy::new(1, 1, 0, second, second, second).is_err());
        assert!(ServerAdmissionPolicy::new(1, 1, 1, second, Duration::ZERO, second).is_err());
        assert!(ServerAdmissionPolicy::new(1, 1, 1, second, second, Duration::ZERO).is_err());
        assert!(ServerAdmissionPolicy::new(1, 2, 1, second, second, second).is_err());
        assert!(ServerAdmissionPolicy::new(4, 2, 3, second, second, second).is_err());
        assert!(ServerAdmissionPolicy::new(2, 2, 2, Duration::ZERO, second, second).is_ok());
        assert!(
            ServerAdmissionPolicy::DEFAULT.max_in_flight_per_repo()
                < ServerAdmissionPolicy::DEFAULT.dispatch_slots()
        );
        assert_eq!(
            ServerAdmissionPolicy::SERIAL_DISPATCH.max_in_flight_per_repo(),
            1
        );
    }

    #[test]
    fn slots_are_bounded_and_a_full_queue_is_refused_after_the_wait() {
        let slots = DispatchSlots::new(1, 1);
        let held = slots
            .acquire(Duration::ZERO, None)
            .expect("first slot is free");
        assert!(!held.waited());
        assert_eq!(slots.available(), 0);
        assert_eq!(slots.in_flight(), 1);
        match slots.acquire(Duration::from_millis(30), None) {
            Err(SlotRefusal::QueueWaitExceeded { waited, slots: 1 }) => {
                assert!(waited >= Duration::from_millis(30));
            }
            other => panic!("expected a queue-wait refusal, got {other:?}"),
        }
        drop(held);
        assert_eq!(slots.available(), 1);
        let again = slots
            .acquire(Duration::ZERO, None)
            .expect("released slot is free again");
        drop(again);
    }

    #[test]
    fn a_repository_at_its_cap_is_refused_by_name_while_another_is_served() {
        let slots = DispatchSlots::new(4, 2);
        let first = slots
            .acquire(Duration::ZERO, Some("hot"))
            .expect("first hot slot");
        let second = slots
            .acquire(Duration::ZERO, Some("hot"))
            .expect("second hot slot");
        assert_eq!(slots.in_flight_for_repo("hot"), 2);
        assert_eq!(slots.available(), 2);
        match slots.acquire(Duration::from_millis(20), Some("hot")) {
            Err(SlotRefusal::RepoInFlightExceeded {
                repo, per_repo: 2, ..
            }) => assert_eq!(repo, "hot"),
            other => panic!("expected a repo-scoped refusal, got {other:?}"),
        }
        // Two slots are free for everyone else.
        let other = slots
            .acquire(Duration::ZERO, Some("cold"))
            .expect("another repository takes a free slot");
        let unscoped = slots
            .acquire(Duration::ZERO, None)
            .expect("an unscoped request takes the last slot");
        assert_eq!(slots.available(), 0);
        // With every slot held, a hot request is refused by the global
        // bound, not the repository one, only if its repository has room.
        drop(first);
        match slots.acquire(Duration::from_millis(20), Some("hot")) {
            Ok(permit) => drop(permit),
            other => panic!("the released hot slot is free again, got {other:?}"),
        }
        drop(second);
        drop(other);
        drop(unscoped);
        assert_eq!(slots.available(), 4);
        assert_eq!(slots.in_flight_for_repo("hot"), 0);
    }

    #[test]
    fn a_waiter_gets_the_slot_when_the_holder_releases_within_the_wait() {
        let slots = Arc::new(DispatchSlots::new(1, 1));
        let held = slots
            .acquire(Duration::ZERO, Some("repo"))
            .expect("first slot is free");
        let waiter = {
            let slots = Arc::clone(&slots);
            thread::spawn(move || {
                slots
                    .acquire(Duration::from_secs(5), Some("repo"))
                    .map(|permit| {
                        let waited = permit.waited();
                        drop(permit);
                        waited
                    })
            })
        };
        thread::sleep(Duration::from_millis(50));
        drop(held);
        let outcome = waiter.join().expect("waiter thread");
        assert_eq!(outcome, Ok(true), "the waiter waited, then was served");
        assert_eq!(slots.available(), 1);
    }
}
