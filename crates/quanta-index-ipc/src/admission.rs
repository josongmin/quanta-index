//! Connection and dispatch admission for the UDS servers (QI-BB-002).
//!
//! The phase-1 server handled every connection inline on the accept thread:
//! a peer that sent a length prefix and then stalled, or a long native
//! search, blocked every other client of that socket, and a client that
//! gave up left its query running to completion with nothing to tell the
//! dispatcher.
//!
//! Admission is ordered ingress → dispatch slot → execution. Decoding holds a
//! process-wide request-buffer budget and a request-count bound through the
//! response, including queued payloads. Each accepted
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
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use quanta_index_contract::{ERR_SERVER_OVERLOADED, SearchPlaneIpcError};

use crate::error::IpcError;
use crate::plane::IpcPlane;

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
    ingress: Arc<IngressBudget>,
}

/// Enough for one maximum 128 MiB multiframe request, its 16 MiB first
/// frame and one 16 MiB following frame, with room for smaller requests.
const DECODE_BYTE_CAPACITY: usize = 256 * 1024 * 1024;

/// Request-body admission shared by the query, control and ingest sockets of
/// one daemon.
///
/// The process budget reserves 32 MiB and two requests for the control socket
/// under data-plane saturation. Standalone servers have no control reserve.
#[derive(Debug)]
pub struct IngressBudget {
    decode_bytes_in_flight: AtomicUsize,
    decode_byte_capacity: usize,
    decode_requests_in_flight: AtomicUsize,
    decode_request_capacity: usize,
    data_bytes_in_flight: AtomicUsize,
    data_byte_capacity: usize,
    data_requests_in_flight: AtomicUsize,
    data_request_capacity: usize,
}

impl IngressBudget {
    /// One process-wide budget for all three daemon sockets. Query and ingest
    /// can use the full standalone 256 MiB bound; the additional 32 MiB and
    /// two request permits keep operator control reachable under saturation.
    /// The data count covers 64 connections on each of the query and ingest
    /// sockets, so ordinary concurrent retries keep their prior capacity.
    #[must_use]
    pub const fn for_process() -> Self {
        Self::new(288 * 1024 * 1024, 130, DECODE_BYTE_CAPACITY, 128)
    }

    pub(crate) fn for_server(policy: ServerAdmissionPolicy) -> Self {
        Self::with_request_capacity(policy.max_connections())
    }

    const fn with_request_capacity(requests: usize) -> Self {
        Self::new(
            DECODE_BYTE_CAPACITY,
            requests,
            DECODE_BYTE_CAPACITY,
            requests,
        )
    }

    const fn new(bytes: usize, requests: usize, data_bytes: usize, data_requests: usize) -> Self {
        Self {
            decode_bytes_in_flight: AtomicUsize::new(0),
            decode_byte_capacity: bytes,
            decode_requests_in_flight: AtomicUsize::new(0),
            decode_request_capacity: requests,
            data_bytes_in_flight: AtomicUsize::new(0),
            data_byte_capacity: data_bytes,
            data_requests_in_flight: AtomicUsize::new(0),
            data_request_capacity: data_requests,
        }
    }

    fn saturated(&self, data_plane: bool) -> IpcError {
        IpcError::IngressSaturated {
            bytes: if data_plane {
                self.data_byte_capacity
            } else {
                self.decode_byte_capacity
            },
            requests: if data_plane {
                self.data_request_capacity
            } else {
                self.decode_request_capacity
            },
        }
    }

    fn try_acquire(
        &self,
        body_bytes: usize,
        plane: IpcPlane,
    ) -> Result<DecodePermit<'_>, IpcError> {
        let data_plane = plane != IpcPlane::Control;
        if data_plane
            && !try_reserve_atomic(&self.data_requests_in_flight, 1, self.data_request_capacity)
        {
            return Err(self.saturated(true));
        }
        if !try_reserve_atomic(
            &self.decode_requests_in_flight,
            1,
            self.decode_request_capacity,
        ) {
            if data_plane {
                let _previous = self.data_requests_in_flight.fetch_sub(1, Ordering::Release);
            }
            return Err(self.saturated(data_plane));
        }
        if let Err(error) = self.reserve_bytes(body_bytes, data_plane) {
            let _previous = self
                .decode_requests_in_flight
                .fetch_sub(1, Ordering::Release);
            if data_plane {
                let _previous = self.data_requests_in_flight.fetch_sub(1, Ordering::Release);
            }
            return Err(error);
        }
        Ok(DecodePermit {
            budget: self,
            reserved_bytes: body_bytes,
            data_plane,
        })
    }

    fn reserve_bytes(&self, bytes: usize, data_plane: bool) -> Result<(), IpcError> {
        if data_plane
            && !try_reserve_atomic(&self.data_bytes_in_flight, bytes, self.data_byte_capacity)
        {
            return Err(self.saturated(true));
        }
        if !try_reserve_atomic(
            &self.decode_bytes_in_flight,
            bytes,
            self.decode_byte_capacity,
        ) {
            if data_plane {
                let _previous = self
                    .data_bytes_in_flight
                    .fetch_sub(bytes, Ordering::Release);
            }
            return Err(self.saturated(data_plane));
        }
        Ok(())
    }
}

/// Held from the first request-body allocation until the corresponding
/// response is written or the connection closes.
#[derive(Debug)]
pub(crate) struct DecodePermit<'a> {
    budget: &'a IngressBudget,
    reserved_bytes: usize,
    data_plane: bool,
}

impl DecodePermit<'_> {
    pub(crate) fn reserve(&mut self, additional_bytes: usize) -> Result<(), IpcError> {
        let new_reserved = self
            .reserved_bytes
            .checked_add(additional_bytes)
            .ok_or_else(|| self.budget.saturated(self.data_plane))?;
        self.budget
            .reserve_bytes(additional_bytes, self.data_plane)?;
        self.reserved_bytes = new_reserved;
        Ok(())
    }
}

impl Drop for DecodePermit<'_> {
    fn drop(&mut self) {
        let _previous = self
            .budget
            .decode_bytes_in_flight
            .fetch_sub(self.reserved_bytes, Ordering::Release);
        let _previous = self
            .budget
            .decode_requests_in_flight
            .fetch_sub(1, Ordering::Release);
        if self.data_plane {
            let _previous = self
                .budget
                .data_bytes_in_flight
                .fetch_sub(self.reserved_bytes, Ordering::Release);
            let _previous = self
                .budget
                .data_requests_in_flight
                .fetch_sub(1, Ordering::Release);
        }
    }
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
                code: ERR_SERVER_OVERLOADED,
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
        // Keep room for a half frame and queued requests while all dispatch
        // slots are occupied. A decode cap equal to dispatch slots breaks the
        // socket's slow-peer isolation and typed queue refusal contract.
        Self::with_shared_ingress(policy, Arc::new(IngressBudget::for_server(policy)))
    }

    /// The dispatch limits remain socket-local while all sockets can share
    /// one ingress byte and request-count budget.
    #[must_use]
    pub(crate) fn with_shared_ingress(
        policy: ServerAdmissionPolicy,
        ingress: Arc<IngressBudget>,
    ) -> Self {
        let mut slots = Self::new(policy.dispatch_slots(), policy.max_in_flight_per_repo());
        slots.ingress = ingress;
        slots
    }

    /// `capacity` slots, of which one repository may hold at most
    /// `per_repo_capacity` (clamped to `capacity`; zero means one).
    #[must_use]
    pub fn new(capacity: usize, per_repo_capacity: usize) -> Self {
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
            ingress: Arc::new(IngressBudget::with_request_capacity(
                capacity.saturating_add(4).max(8),
            )),
        }
    }

    /// Reject immediately instead of waiting while retaining a partially
    /// decoded request. Such waits could deadlock the shared byte budget.
    pub(crate) fn try_acquire_decode(
        &self,
        body_bytes: usize,
        plane: IpcPlane,
    ) -> Result<DecodePermit<'_>, IpcError> {
        self.ingress.try_acquire(body_bytes, plane)
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

fn try_reserve_atomic(counter: &AtomicUsize, additional: usize, capacity: usize) -> bool {
    let mut held = counter.load(Ordering::Acquire);
    loop {
        let Some(next) = held
            .checked_add(additional)
            .filter(|total| *total <= capacity)
        else {
            return false;
        };
        match counter.compare_exchange_weak(held, next, Ordering::AcqRel, Ordering::Acquire) {
            Ok(_previous) => return true,
            Err(actual) => held = actual,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::thread;
    use std::time::Duration;

    use super::{
        DECODE_BYTE_CAPACITY, DispatchSlots, IngressBudget, ServerAdmissionPolicy, SlotRefusal,
    };
    use crate::plane::IpcPlane;

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
    fn decode_admission_counts_bytes_and_releases_upgrades_on_drop() {
        let slots = DispatchSlots::for_policy(ServerAdmissionPolicy::DEFAULT);
        let mut first = slots
            .try_acquire_decode(16, IpcPlane::Query)
            .expect("first decode admitted");
        let second = slots
            .try_acquire_decode(32, IpcPlane::Query)
            .expect("second decode admitted");
        first
            .reserve(DECODE_BYTE_CAPACITY - 48)
            .expect("upgrade fits");
        assert!(matches!(
            slots.try_acquire_decode(1, IpcPlane::Query),
            Err(crate::error::IpcError::IngressSaturated {
                bytes: DECODE_BYTE_CAPACITY,
                requests: 64,
            })
        ));
        assert!(matches!(
            first.reserve(1),
            Err(crate::error::IpcError::IngressSaturated {
                bytes: DECODE_BYTE_CAPACITY,
                requests: 64,
            })
        ));
        let dispatch = slots
            .acquire(Duration::ZERO, None)
            .expect("dispatch slots remain independent");
        drop(dispatch);
        drop(first);
        let third = slots
            .try_acquire_decode(16, IpcPlane::Query)
            .expect("drop releases upgrade");
        drop(second);
        drop(third);
        assert!(
            slots
                .try_acquire_decode(DECODE_BYTE_CAPACITY, IpcPlane::Query)
                .is_ok()
        );
    }

    #[test]
    fn decode_admission_also_bounds_tiny_decoded_envelopes() {
        let slots = DispatchSlots::for_policy(ServerAdmissionPolicy::DEFAULT);
        let permits: Vec<_> = (0..64)
            .map(|_| {
                slots
                    .try_acquire_decode(1, IpcPlane::Query)
                    .expect("small request admitted")
            })
            .collect();
        assert!(matches!(
            slots.try_acquire_decode(1, IpcPlane::Query),
            Err(crate::error::IpcError::IngressSaturated { requests: 64, .. })
        ));
        drop(permits);
        assert!(slots.try_acquire_decode(1, IpcPlane::Query).is_ok());
    }

    #[test]
    fn two_socket_dispatch_ledgers_share_one_ingress_byte_budget() {
        let ingress = Arc::new(IngressBudget::for_process());
        let query = DispatchSlots::with_shared_ingress(
            ServerAdmissionPolicy::DEFAULT,
            Arc::clone(&ingress),
        );
        let ingest = DispatchSlots::with_shared_ingress(
            ServerAdmissionPolicy::SERIAL_DISPATCH,
            Arc::clone(&ingress),
        );
        let held = query
            .try_acquire_decode(DECODE_BYTE_CAPACITY - 1, IpcPlane::Query)
            .expect("query fills shared budget");
        assert!(matches!(
            ingest.try_acquire_decode(2, IpcPlane::Ingest),
            Err(crate::error::IpcError::IngressSaturated { requests: 128, .. })
        ));
        let control =
            DispatchSlots::with_shared_ingress(ServerAdmissionPolicy::SERIAL_DISPATCH, ingress);
        let control_permit = control
            .try_acquire_decode(32 * 1024 * 1024, IpcPlane::Control)
            .expect("control reserve survives saturated data plane");
        drop(control_permit);
        drop(held);
        assert!(ingest.try_acquire_decode(2, IpcPlane::Ingest).is_ok());
    }

    #[test]
    fn control_request_permits_survive_data_plane_count_saturation() {
        let ingress = Arc::new(IngressBudget::for_process());
        let query = DispatchSlots::with_shared_ingress(
            ServerAdmissionPolicy::DEFAULT,
            Arc::clone(&ingress),
        );
        let ingest = DispatchSlots::with_shared_ingress(
            ServerAdmissionPolicy::SERIAL_DISPATCH,
            Arc::clone(&ingress),
        );
        let control =
            DispatchSlots::with_shared_ingress(ServerAdmissionPolicy::SERIAL_DISPATCH, ingress);
        let query_permits: Vec<_> = (0..64)
            .map(|_| {
                query
                    .try_acquire_decode(1, IpcPlane::Query)
                    .expect("data permit")
            })
            .collect();
        let ingest_permits: Vec<_> = (0..64)
            .map(|_| {
                ingest
                    .try_acquire_decode(1, IpcPlane::Ingest)
                    .expect("ingest permit")
            })
            .collect();
        assert!(query.try_acquire_decode(1, IpcPlane::Query).is_err());
        let control_permits: Vec<_> = (0..2)
            .map(|_| {
                control
                    .try_acquire_decode(1, IpcPlane::Control)
                    .expect("control permit")
            })
            .collect();
        assert!(control.try_acquire_decode(1, IpcPlane::Control).is_err());
        drop(query_permits);
        drop(ingest_permits);
        drop(control_permits);
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
