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

use std::num::NonZeroU64;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crate::error::CoreError;

/// Wire code for a request that ran past its deadline.
pub const REQUEST_DEADLINE_EXCEEDED_CODE: quanta_index_contract::SearchPlaneErrorCodeV2 =
    quanta_index_contract::SearchPlaneErrorCodeV2::RequestDeadlineExceeded;
/// Wire code for a request abandoned by its peer while it was running.
pub const REQUEST_CANCELLED_CODE: quanta_index_contract::SearchPlaneErrorCodeV2 =
    quanta_index_contract::SearchPlaneErrorCodeV2::RequestCancelled;

/// Why a checkpoint stopped a request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BudgetInterruptionV1 {
    /// The absolute deadline passed; `over_by` is how far past it the
    /// checkpoint ran.
    DeadlineExceeded { over_by: Duration },
    /// The peer disconnected (or the owner cancelled) before this checkpoint.
    Cancelled,
}

/// Shared cancellation state: the flag every checkpoint reads, plus the
/// waiter wake-ups `cancel` fires exactly once (TOPT-02 / PO-4).
///
/// A waiter registers a `wake` closure, then checks the flag; `cancel`
/// invokes every registered closure after flipping the flag, so a waiter
/// blocked on its own condvar wakes the moment the budget is cancelled
/// instead of re-reading the flag on a poll quantum. The closures are
/// cloned out before invocation, so a `wake` may lock its own mutex
/// without nesting inside the registry lock.
struct CancelSharedV1 {
    cancelled: AtomicBool,
    waiters: std::sync::Mutex<CancelWaiterSetV1>,
}

impl CancelSharedV1 {
    fn lock_waiters(&self) -> std::sync::MutexGuard<'_, CancelWaiterSetV1> {
        // A poisoned registry must not suppress cancellation wake-ups.
        match self.waiters.lock() {
            Ok(waiters) => waiters,
            Err(poisoned) => poisoned.into_inner(),
        }
    }
}

impl std::fmt::Debug for CancelSharedV1 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let live = self.lock_waiters().wake.len();
        f.debug_struct("CancelSharedV1")
            .field("cancelled", &self.cancelled.load(Ordering::Acquire))
            .field("live_waiters", &live)
            .finish_non_exhaustive()
    }
}

#[derive(Default)]
struct CancelWaiterSetV1 {
    next_id: u64,
    wake: Vec<(u64, std::sync::Arc<dyn Fn() + Send + Sync>)>,
}

/// Cancels the budget it was taken from. Held by whoever watches the peer.
#[derive(Clone, Debug)]
pub struct CancelHandleV1 {
    shared: Arc<CancelSharedV1>,
}

impl CancelHandleV1 {
    pub fn cancel(&self) {
        if self.shared.cancelled.swap(true, Ordering::AcqRel) {
            return;
        }
        let wake = {
            let mut waiters = self.shared.lock_waiters();
            let wake = waiters
                .wake
                .iter()
                .map(|(_, woken)| Arc::clone(woken))
                .collect::<Vec<_>>();
            waiters.wake.clear();
            wake
        };
        for woken in wake {
            woken();
        }
    }
}

/// One live cancellation waiter: removed from the registry by id when
/// dropped, on every return and panic path; `cancel` clears fired
/// waiters. Removal is exact, so the registry holds live waiters only.
pub struct CancelWaiterGuardV1 {
    shared: Arc<CancelSharedV1>,
    id: u64,
}

impl std::fmt::Debug for CancelWaiterGuardV1 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CancelWaiterGuardV1")
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}

impl Drop for CancelWaiterGuardV1 {
    fn drop(&mut self) {
        self.shared
            .lock_waiters()
            .wake
            .retain(|(id, _)| *id != self.id);
    }
}

/// One request's transport identity (W10-R2).
///
/// The nonzero envelope id the server admitted, carried on the budget so
/// every stage — routes, typed responses, provider audit — correlates
/// without re-reading an envelope. The constructor is the only gate:
/// [`RequestCorrelationV1::from_raw`] refuses 0, so a value of this type
/// is proof a nonzero id was observed. There is deliberately no `Default`
/// and no 0-valued instance.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct RequestCorrelationV1(NonZeroU64);

impl RequestCorrelationV1 {
    /// Admit a raw wire id. `None` (for 0) means the caller never had a
    /// transport identity and must stay uncorrelated — never a fallback id.
    #[must_use]
    pub fn from_raw(raw: u64) -> Option<Self> {
        NonZeroU64::new(raw).map(Self::from_admitted)
    }

    /// Wrap an id the transport gate already admitted. Infallible by
    /// construction — only call sites holding a validated id use this.
    #[must_use]
    pub fn from_admitted(admitted: NonZeroU64) -> Self {
        Self(admitted)
    }

    /// The wire id to echo in typed responses and audit events.
    #[must_use]
    pub fn get(self) -> u64 {
        self.0.get()
    }
}

/// Provider boundary markers for diagnostics only. Query markers carry the
/// provider ledger's ticket ID; ingest markers carry a checked, per-request
/// window ordinal because ingest has no provider-ledger ticket. Neither
/// marker owns reservation, settlement, usage or terminal outcome.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RequestProviderStageV1 {
    Started { ticket_id: u64 },
    Returned { ticket_id: u64 },
    IngestWindowStarted { window_ordinal: u64 },
    IngestWindowReturned { window_ordinal: u64 },
}

/// A per-request bridge to the transport's existing bounded event sink.
/// This port does not allocate an ID, settle usage or own another ring.
pub trait RequestStageDiagnosticPortV1: std::fmt::Debug + Send + Sync {
    fn record_provider_stage_v1(&self, stage: RequestProviderStageV1);
}

/// One request's deadline and cancellation state.
#[derive(Clone, Debug)]
pub struct RequestBudgetV1 {
    deadline: Instant,
    shared: Arc<CancelSharedV1>,
    correlation: Option<RequestCorrelationV1>,
    diagnostics: Option<Arc<dyn RequestStageDiagnosticPortV1>>,
}

impl RequestBudgetV1 {
    /// A budget that ends at `deadline`.
    #[must_use]
    pub fn until(deadline: Instant) -> Self {
        Self {
            deadline,
            shared: Arc::new(CancelSharedV1 {
                cancelled: AtomicBool::new(false),
                waiters: std::sync::Mutex::new(CancelWaiterSetV1::default()),
            }),
            correlation: None,
            diagnostics: None,
        }
    }

    /// Attach the admitted transport identity. The IPC server calls this
    /// once per request, right after the envelope validation refuses 0;
    /// every other constructor leaves the budget uncorrelated.
    #[must_use]
    pub fn with_correlation(mut self, correlation: RequestCorrelationV1) -> Self {
        self.correlation = Some(correlation);
        self
    }

    /// The admitted transport identity, if this budget runs under one.
    #[must_use]
    pub fn correlation(&self) -> Option<RequestCorrelationV1> {
        self.correlation
    }

    /// Attach the transport-owned diagnostic bridge to an admitted request.
    /// Off-transport budgets intentionally have no diagnostic sink.
    #[must_use]
    pub fn with_diagnostics(mut self, diagnostics: Arc<dyn RequestStageDiagnosticPortV1>) -> Self {
        self.diagnostics = Some(diagnostics);
        self
    }

    /// Record a provider boundary marker when this budget came from IPC.
    /// Diagnostic loss is accounted by the transport sink and never changes
    /// the provider's usage/settlement result.
    pub fn record_provider_stage_v1(&self, stage: RequestProviderStageV1) {
        if let Some(diagnostics) = &self.diagnostics {
            diagnostics.record_provider_stage_v1(stage);
        }
    }

    /// The `request_id` typed responses and audit events must carry: the
    /// admitted id on a served request, 0 off-transport. This is the only
    /// place `None` becomes 0 — callers never invent the mapping.
    #[must_use]
    pub fn response_request_id(&self) -> u64 {
        self.correlation.map_or(0, RequestCorrelationV1::get)
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
            shared: Arc::clone(&self.shared),
        }
    }

    /// Register `wake` to run once when this budget is cancelled.
    ///
    /// The caller must check [`Self::interrupted_at`] after registering:
    /// a budget cancelled before registration never fires its waiters, so
    /// the flag check is what observes a pre-registration cancel. The
    /// registration lives until the returned guard drops.
    pub fn cancel_waiter(&self, wake: Arc<dyn Fn() + Send + Sync>) -> CancelWaiterGuardV1 {
        let id = {
            let mut set = self.shared.lock_waiters();
            let id = set.next_id;
            set.next_id = set.next_id.wrapping_add(1);
            set.wake.push((id, wake));
            id
        };
        CancelWaiterGuardV1 {
            shared: Arc::clone(&self.shared),
            id,
        }
    }

    /// Live waiter registrations. Test-only census for the RAII proof.
    #[cfg(test)]
    fn live_waiters(&self) -> usize {
        self.shared.lock_waiters().wake.len()
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
        self.shared.cancelled.load(Ordering::Acquire)
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
                code: REQUEST_CANCELLED_CODE,
                message: format!("request cancelled by its peer; observed at checkpoint `{stage}`"),
            }),
            BudgetInterruptionV1::DeadlineExceeded { over_by } => Some(CoreError::Typed {
                code: REQUEST_DEADLINE_EXCEEDED_CODE,
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

    #[test]
    fn cancel_fires_each_waiter_exactly_once() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};

        let budget = RequestBudgetV1::for_duration(Duration::from_secs(60));
        let fired = Arc::new(AtomicUsize::new(0));
        let first = {
            let fired = Arc::clone(&fired);
            budget.cancel_waiter(Arc::new(move || {
                let _prior = fired.fetch_add(1, Ordering::SeqCst);
            }))
        };
        let second = {
            let fired = Arc::clone(&fired);
            budget.cancel_waiter(Arc::new(move || {
                let _prior = fired.fetch_add(10, Ordering::SeqCst);
            }))
        };
        assert_eq!(budget.live_waiters(), 2);
        budget.cancel_handle().cancel();
        assert_eq!(fired.load(Ordering::SeqCst), 11);
        assert_eq!(budget.live_waiters(), 0, "fired waiters are cleared");
        budget.cancel_handle().cancel();
        assert_eq!(
            fired.load(Ordering::SeqCst),
            11,
            "a second cancel fires nothing"
        );
        drop(first);
        drop(second);
    }

    #[test]
    fn a_dropped_waiter_is_removed_without_cancel() {
        use std::sync::Arc;

        let budget = RequestBudgetV1::for_duration(Duration::from_secs(60));
        let guard = budget.cancel_waiter(Arc::new(|| {}));
        assert_eq!(budget.live_waiters(), 1);
        drop(guard);
        assert_eq!(budget.live_waiters(), 0);
    }

    #[test]
    fn poisoned_waiter_registry_still_wakes_and_clears_on_cancel() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering};

        let budget = RequestBudgetV1::for_duration(Duration::from_secs(60));
        let fired = Arc::new(AtomicBool::new(false));
        let waiter = {
            let fired = Arc::clone(&fired);
            budget.cancel_waiter(Arc::new(move || fired.store(true, Ordering::SeqCst)))
        };
        let shared = Arc::clone(&budget.shared);
        let poisoned = std::thread::spawn(move || {
            let _held = shared.waiters.lock().expect("fresh waiter registry");
            panic!("poison the waiter registry");
        })
        .join();
        assert!(poisoned.is_err());

        budget.cancel_handle().cancel();
        assert!(fired.load(Ordering::SeqCst));
        assert_eq!(budget.live_waiters(), 0);
        drop(waiter);
    }

    #[test]
    fn a_waiter_registered_after_cancel_never_fires() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering};

        let budget = RequestBudgetV1::for_duration(Duration::from_secs(60));
        budget.cancel_handle().cancel();
        let fired = Arc::new(AtomicBool::new(false));
        let waiter = {
            let fired = Arc::clone(&fired);
            budget.cancel_waiter(Arc::new(move || {
                fired.store(true, Ordering::SeqCst);
            }))
        };
        // The flag check after registration is what observes a
        // pre-registration cancel; the waiter itself stays silent.
        assert!(budget.interrupted_at("late").is_some());
        assert!(!fired.load(Ordering::SeqCst));
        drop(waiter);
        assert_eq!(budget.live_waiters(), 0);
    }
}
