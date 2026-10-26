//! The per-request execution budget every route runs under (QI-BB-002).
//!
//! A request carries one absolute deadline from the transport down to the
//! last collector. Clones share its cancellation scope; optional child work
//! has a local scope that also observes cancellation of its parent. Native
//! work cannot be interrupted from outside (G0-R pinned that), so the budget is
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
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};

use crate::diagnostic_trace::DiagnosticContextV1;
use crate::error::CoreError;
use crate::{LexicalCollectionBudget, LexicalMemoryReservation};

// A served request may perform multiple refill/selection passes. Preallocate the
// bounded group carrier before optional work, so a sticky optional refusal does
// not prevent already-admitted output from retaining its reservations.
const LEXICAL_OUTPUT_GROUP_LIMIT: usize = 256;

#[derive(Debug)]
struct LexicalPreviewRetention {
    max_work: u64,
    max_bytes: u64,
    ledger: LexicalCollectionBudget,
    groups: Vec<Option<Vec<LexicalMemoryReservation>>>,
    _carrier: LexicalMemoryReservation,
}

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

/// Observe an existing execution owner's lifecycle without copying its
/// cancellation authority.
///
/// Implementations must be bounded and monotonic;
/// callers recheck during bounded waits. This port does not fire local waiters.
pub trait RequestExecutionObserverV1: std::fmt::Debug + Send + Sync {
    fn interruption(&self) -> Option<BudgetInterruptionV1>;
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
        let mut first_panic = None;
        for woken in wake {
            if let Err(panic) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| woken())) {
                let _retained_panic = first_panic.get_or_insert(panic);
            }
        }
        if let Some(panic) = first_panic {
            std::panic::resume_unwind(panic);
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

/// Keeps a child's parent cancellation registration and every ancestor relay
/// alive even when an intermediate budget is dropped before its child.
#[derive(Debug)]
struct CancellationRelayV1 {
    _guard: CancelWaiterGuardV1,
    _ancestor: Option<Arc<Self>>,
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

/// Provider boundary markers for diagnostics only.
///
/// Query markers carry the provider ledger's ticket ID; ingest markers carry a checked, per-request
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

/// One physical provider attempt's execution custody.
///
/// The implementation retains the original admission until the actual
/// attempt ends, including when its requester has already observed
/// cancellation or a deadline.
pub trait ProviderAttemptLeaseV1: Send {}

/// Finite attempt admission from the existing execution reservation.
///
/// Implementations must refuse an attempt when its reserved provider slots
/// are exhausted; the returned lease is moved into the physical attempt.
pub trait ProviderAttemptCustodyPortV1: std::fmt::Debug + Send + Sync {
    fn begin_attempt_v1(&self) -> Result<Box<dyn ProviderAttemptLeaseV1>, CoreError>;

    /// Charge the same original execution reservation before provider-side
    /// allocation. A default refusal keeps older off-native ports explicit.
    fn reserve_memory_v1(
        &self,
        _bytes: u64,
        _lifetime: crate::index::MemoryLifetime,
    ) -> Result<crate::index::MemoryLease, CoreError> {
        Err(CoreError::InvalidContract(
            "provider execution has no admitted memory scope".into(),
        ))
    }
}

/// One request's deadline and cancellation state.
#[derive(Clone, Debug)]
pub struct RequestBudgetV1 {
    deadline: Instant,
    shared: Arc<CancelSharedV1>,
    cancellation_relay: Option<Arc<CancellationRelayV1>>,
    external_cancellation: Option<Arc<AtomicBool>>,
    external_observer: Option<Arc<dyn RequestExecutionObserverV1>>,
    initial_backing: Option<crate::index::MemoryLease>,
    correlation: Option<RequestCorrelationV1>,
    diagnostics: Option<Arc<dyn RequestStageDiagnosticPortV1>>,
    diagnostic_context: Option<DiagnosticContextV1>,
    lexical_preview: Arc<std::sync::Mutex<Option<LexicalPreviewRetention>>>,
    semantic_work: Option<Arc<std::sync::Mutex<SemanticWorkLedgerV1>>>,
    /// Weak so a surviving request clone never prolongs CPU, IO, or provider
    /// admission. Only an actual physical attempt's lease can do that.
    provider_attempt_custody: Option<Weak<dyn ProviderAttemptCustodyPortV1>>,
}

#[derive(Debug)]
struct SemanticWorkLedgerV1 {
    limit: u64,
    consumed: u64,
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
            cancellation_relay: None,
            external_cancellation: None,
            external_observer: None,
            initial_backing: None,
            correlation: None,
            diagnostics: None,
            diagnostic_context: None,
            lexical_preview: Arc::new(std::sync::Mutex::new(None)),
            semantic_work: None,
            provider_attempt_custody: None,
        }
    }

    /// Observe an existing owner's cancellation bit and absolute deadline.
    /// The external owner must keep cancellation monotonic. Its atomic store
    /// is observed at checkpoints; it does not fire this budget's waiters.
    /// Blocking consumers must use bounded waits and recheck the budget.
    #[must_use]
    pub fn until_with_external_cancellation(
        deadline: Instant,
        cancellation: Arc<AtomicBool>,
    ) -> Self {
        let mut budget = Self::until(deadline);
        budget.external_cancellation = Some(cancellation);
        budget
    }

    /// Admit the two initial shared allocations before constructing a budget.
    /// Clones retain their backing charge. Registering waiters or constructing
    /// children requires separate admission; this constructor funds neither.
    pub fn until_with_external_cancellation_admitted(
        deadline: Instant,
        cancellation: Arc<AtomicBool>,
        memory: &crate::index::MemoryScope,
    ) -> Result<Self, crate::index::IndexError> {
        let backing = Self::reserve_initial_backing(memory)?;
        let mut budget = Self::until_with_external_cancellation(deadline, cancellation);
        budget.initial_backing = Some(backing);
        Ok(budget)
    }

    /// Observe a real execution lifecycle and admit the same initial backing
    /// as the atomic-owner constructor. The observer's own backing must already
    /// be admitted by its owner; this function only retains its existing Arc.
    pub fn until_with_external_observer_admitted(
        deadline: Instant,
        observer: Arc<dyn RequestExecutionObserverV1>,
        memory: &crate::index::MemoryScope,
    ) -> Result<Self, crate::index::IndexError> {
        if observer.interruption().is_some() {
            return Err(crate::index::IndexError::Cancelled);
        }
        let backing = Self::reserve_initial_backing(memory)?;
        let mut budget = Self::until(deadline);
        budget.external_observer = Some(observer);
        budget.initial_backing = Some(backing);
        Ok(budget)
    }

    /// Delegate lifecycle termination to an existing phase owner. Some owners
    /// deliberately change deadline policy after publication admission; copying
    /// their earlier deadline would revoke valid closeout work. Transports must
    /// independently cap each call to their live dispatch authority.
    pub fn observing_execution_admitted(
        observer: Arc<dyn RequestExecutionObserverV1>,
        memory: &crate::index::MemoryScope,
    ) -> Result<Self, crate::index::IndexError> {
        Self::until_with_external_observer_admitted(far_future(), observer, memory)
    }

    fn reserve_initial_backing(
        memory: &crate::index::MemoryScope,
    ) -> Result<crate::index::MemoryLease, crate::index::IndexError> {
        let counters = std::alloc::Layout::new::<[std::sync::atomic::AtomicUsize; 2]>();
        let mut bytes = 0u64;
        for payload in [
            std::alloc::Layout::new::<CancelSharedV1>(),
            std::alloc::Layout::new::<std::sync::Mutex<Option<LexicalPreviewRetention>>>(),
        ] {
            let (layout, _) = counters
                .extend(payload)
                .map_err(|_error| crate::index::IndexError::ResourceLimited)?;
            let allocation = u64::try_from(layout.pad_to_align().size())
                .map_err(|_error| crate::index::IndexError::ResourceLimited)?;
            bytes = bytes
                .checked_add(allocation)
                .ok_or(crate::index::IndexError::ResourceLimited)?;
        }
        memory.reserve(bytes, crate::index::MemoryLifetime::Retained)
    }

    /// Bind a dedicated semantic query to a finite work allowance. Clones
    /// share the same counter so embedding, dense search and projection cannot
    /// each spend the full allowance independently.
    pub fn with_semantic_work_limit_v1(mut self, limit: u64) -> Result<Self, CoreError> {
        if limit == 0 || self.semantic_work.is_some() {
            return Err(CoreError::InvalidContract(
                "semantic work allowance must be positive and set once".into(),
            ));
        }
        self.semantic_work = Some(Arc::new(std::sync::Mutex::new(SemanticWorkLedgerV1 {
            limit,
            consumed: 0,
        })));
        Ok(self)
    }

    /// Charge deterministic semantic work before performing it. Ordinary query
    /// routes have no semantic work ledger and cannot use this authority.
    pub fn consume_semantic_work_v1(&self, units: u64) -> Result<(), CoreError> {
        let work = self.semantic_work.as_ref().ok_or_else(|| {
            CoreError::InvalidContract("semantic work allowance is absent".into())
        })?;
        let mut work = work.lock().map_err(|error| {
            CoreError::Storage(format!("semantic work ledger poisoned: {error}"))
        })?;
        let next = work
            .consumed
            .checked_add(units)
            .ok_or_else(|| CoreError::Typed {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::SemanticWorkBudgetExceeded,
                message: "semantic work allowance overflowed".into(),
            })?;
        if next > work.limit {
            return Err(CoreError::Typed {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::SemanticWorkBudgetExceeded,
                message: "semantic work allowance exhausted".into(),
            });
        }
        work.consumed = next;
        drop(work);
        Ok(())
    }

    pub fn semantic_work_consumed_v1(&self) -> Result<Option<u64>, CoreError> {
        self.semantic_work
            .as_ref()
            .map(|work| {
                work.lock().map(|work| work.consumed).map_err(|error| {
                    CoreError::Storage(format!("semantic work ledger poisoned: {error}"))
                })
            })
            .transpose()
    }

    /// Attach a single stage's authority. A caller may bind a clone with the
    /// same original deadline/cancellation scope for one stage, then restore
    /// its original request budget. Escaped clones cannot acquire a later
    /// stage's grant, and no clone holds this grant strongly.
    pub fn bind_provider_attempt_custody_v1(
        &mut self,
        authority: &Arc<dyn ProviderAttemptCustodyPortV1>,
    ) -> Result<(), CoreError> {
        if self.provider_attempt_custody.is_some() {
            return Err(CoreError::InvalidContract(
                "provider attempt custody is already bound".into(),
            ));
        }
        self.provider_attempt_custody = Some(Arc::downgrade(authority));
        Ok(())
    }

    /// Admit a physical attempt before it is spawned. `None` is the explicit
    /// off-native provider contract; a previously bound but vanished native
    /// authority fails closed. This does not renew the request deadline.
    pub fn begin_provider_attempt_v1(
        &self,
    ) -> Result<Option<Box<dyn ProviderAttemptLeaseV1>>, CoreError> {
        self.checkpoint(crate::EMBED_CHECKPOINT)?;
        let Some(authority) = self.provider_attempt_custody.as_ref() else {
            return Ok(None);
        };
        let authority = authority.upgrade().ok_or_else(|| CoreError::Typed {
            code: quanta_index_contract::SearchPlaneErrorCodeV2::ServerOverloaded,
            message: "provider execution admission has ended".into(),
        })?;
        let lease = authority.begin_attempt_v1()?;
        self.checkpoint(crate::EMBED_CHECKPOINT)?;
        Ok(Some(lease))
    }

    /// Reserve provider-side bytes against this stage's original Native grant.
    /// Standalone requests return `None`; a stale Native binding refuses.
    pub fn reserve_provider_memory_v1(
        &self,
        bytes: u64,
        lifetime: crate::index::MemoryLifetime,
    ) -> Result<Option<crate::index::MemoryLease>, CoreError> {
        self.checkpoint(crate::EMBED_CHECKPOINT)?;
        let Some(authority) = self.provider_attempt_custody.as_ref() else {
            return Ok(None);
        };
        let authority = authority.upgrade().ok_or_else(|| CoreError::Typed {
            code: quanta_index_contract::SearchPlaneErrorCodeV2::ServerOverloaded,
            message: "provider execution admission has ended".into(),
        })?;
        let lease = authority.reserve_memory_v1(bytes, lifetime)?;
        self.checkpoint(crate::EMBED_CHECKPOINT)?;
        Ok(Some(lease))
    }

    /// Attach the admitted transport identity. The IPC server calls this
    /// once per request, right after the envelope validation refuses 0;
    /// every other constructor leaves the budget uncorrelated.
    #[must_use]
    pub fn with_correlation(mut self, correlation: RequestCorrelationV1) -> Self {
        self.correlation = Some(correlation);
        self
    }

    /// Attach the explicit request trace context. This is diagnostic-only and
    /// carries no deadline, cancellation, or usage authority.
    #[must_use]
    pub fn with_diagnostic_context(mut self, context: DiagnosticContextV1) -> Self {
        self.diagnostic_context = Some(context);
        self
    }

    /// Borrow the request trace context when one was explicitly injected.
    #[must_use]
    pub fn diagnostic_context(&self) -> Option<&DiagnosticContextV1> {
        self.diagnostic_context.as_ref()
    }

    /// One optional preview ledger across all selection/refill passes of this
    /// request. The first explicit policy is frozen; callers cannot reset a
    /// spent ledger or silently replace its limits.
    pub fn lexical_preview_budget(
        &self,
        max_work: u64,
        max_bytes: u64,
    ) -> Result<LexicalCollectionBudget, CoreError> {
        let mut state = self
            .lexical_preview
            .lock()
            .map_err(|error| CoreError::Storage(format!("preview retention poisoned: {error}")))?;
        if let Some(retained) = state.as_ref() {
            if (max_work, max_bytes) != (retained.max_work, retained.max_bytes) {
                return Err(CoreError::InvalidContract(
                    "request preview policy cannot change".into(),
                ));
            }
            return Ok(retained.ledger.clone());
        }
        let ledger = LexicalCollectionBudget::new(max_work, max_bytes)?;
        let bytes = u64::try_from(
            LEXICAL_OUTPUT_GROUP_LIMIT
                .checked_mul(std::mem::size_of::<Option<Vec<LexicalMemoryReservation>>>())
                .ok_or_else(|| {
                    CoreError::InvalidContract("preview carrier size overflow".into())
                })?,
        )
        .map_err(|_overflow| CoreError::InvalidContract("preview carrier size overflow".into()))?;
        let carrier = ledger.reserve_bytes(bytes)?;
        let mut groups = Vec::new();
        groups
            .try_reserve_exact(LEXICAL_OUTPUT_GROUP_LIMIT)
            .map_err(|error| CoreError::Storage(format!("preview carrier allocation: {error}")))?;
        *state = Some(LexicalPreviewRetention {
            max_work,
            max_bytes,
            ledger: ledger.clone(),
            groups,
            _carrier: carrier,
        });
        drop(state);
        Ok(ledger)
    }

    /// Transfer already-admitted output leases into the request's lifetime.
    /// IPC retains this budget through response encoding and socket write.
    /// In-process callers must retain their budget while retaining the returned
    /// output. Arbitrary later wire DTO clones are outside this accounting.
    pub fn reserve_lexical_output_group(&self) -> Result<Option<usize>, CoreError> {
        let mut state = self
            .lexical_preview
            .lock()
            .map_err(|error| CoreError::Storage(format!("preview retention poisoned: {error}")))?;
        let retained = state.as_mut().ok_or_else(|| {
            CoreError::InvalidContract("preview output lacks its request ledger".into())
        })?;
        if retained.groups.len() >= LEXICAL_OUTPUT_GROUP_LIMIT {
            return Ok(None);
        }
        let slot = retained.groups.len();
        retained.groups.push(None);
        drop(state);
        Ok(Some(slot))
    }

    /// Fill a previously reserved request slot with live allocation leases.
    /// This transfer performs no allocation or further budget admission.
    pub fn retain_lexical_output(
        &self,
        slot: usize,
        reservations: Vec<LexicalMemoryReservation>,
    ) -> Result<(), CoreError> {
        let mut state = self
            .lexical_preview
            .lock()
            .map_err(|error| CoreError::Storage(format!("preview retention poisoned: {error}")))?;
        let retained = state.as_mut().ok_or_else(|| {
            CoreError::InvalidContract("preview output lacks its request ledger".into())
        })?;
        let group = retained.groups.get_mut(slot).ok_or_else(|| {
            CoreError::InvalidContract("preview output group was not reserved".into())
        })?;
        if group.is_some() {
            return Err(CoreError::InvalidContract(
                "preview output group already filled".into(),
            ));
        }
        *group = Some(reservations);
        drop(state);
        Ok(())
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

    /// Limit a call to an authority's expiry without creating a new request
    /// scope or extending the admitted request's deadline. Cancellation,
    /// diagnostics, backing grants, and work ledgers remain shared.
    #[must_use]
    pub fn capped_at(&self, deadline: Instant) -> Self {
        let mut capped = self.clone();
        capped.deadline = self.deadline.min(deadline);
        capped
    }

    /// A preparation budget with the same absolute deadline and request
    /// ledgers, but its own cancellation flag. Cancelling the child does not
    /// interrupt the admitted parent work; cancelling the parent reaches the
    /// child, including after intermediate child budgets have been dropped.
    #[must_use]
    pub fn child(&self) -> Self {
        let mut child = Self {
            deadline: self.deadline,
            shared: Arc::new(CancelSharedV1 {
                cancelled: AtomicBool::new(false),
                waiters: std::sync::Mutex::new(CancelWaiterSetV1::default()),
            }),
            cancellation_relay: None,
            external_cancellation: self.external_cancellation.clone(),
            external_observer: self.external_observer.clone(),
            initial_backing: self.initial_backing.clone(),
            correlation: self.correlation,
            diagnostics: self.diagnostics.clone(),
            diagnostic_context: self.diagnostic_context.clone(),
            lexical_preview: Arc::clone(&self.lexical_preview),
            semantic_work: self.semantic_work.clone(),
            provider_attempt_custody: self.provider_attempt_custody.clone(),
        };
        let child_cancel = child.cancel_handle();
        let guard = self.cancel_waiter(Arc::new(move || child_cancel.cancel()));
        child.cancellation_relay = Some(Arc::new(CancellationRelayV1 {
            _guard: guard,
            _ancestor: self.cancellation_relay.clone(),
        }));
        // cancel_waiter does not invoke a newly registered callback after a
        // prior cancel. This check also closes the registration/cancel race.
        if self.is_cancelled() {
            child.cancel_handle().cancel();
        }
        child
    }

    /// Whether two budgets have the exact same cancellation scope. Clones
    /// match, while a child and its parent or sibling children do not. A
    /// caller binding policy must also compare the absolute deadline.
    #[must_use]
    pub fn same_request_as(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.shared, &other.shared)
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
        self.local_or_atomic_cancelled()
            || self.external_observer.as_ref().is_some_and(|observer| {
                matches!(
                    observer.interruption(),
                    Some(BudgetInterruptionV1::Cancelled)
                )
            })
    }

    fn local_or_atomic_cancelled(&self) -> bool {
        self.shared.cancelled.load(Ordering::Acquire)
            || self
                .external_cancellation
                .as_ref()
                .is_some_and(|cancelled| cancelled.load(Ordering::Acquire))
    }

    /// What, if anything, has interrupted this request by now.
    #[must_use]
    pub fn interruption(&self) -> Option<BudgetInterruptionV1> {
        if self.local_or_atomic_cancelled() {
            return Some(BudgetInterruptionV1::Cancelled);
        }
        if let Some(interruption) = self
            .external_observer
            .as_ref()
            .and_then(|observer| observer.interruption())
        {
            return Some(interruption);
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
    fn capped_budget_keeps_request_scope_and_never_extends_deadline() {
        let now = Instant::now();
        let parent = RequestBudgetV1::until(now + Duration::from_secs(10));
        let cap = now + Duration::from_secs(1);
        let capped = parent.capped_at(cap);
        assert_eq!(capped.deadline(), cap);
        assert_eq!(parent.deadline(), now + Duration::from_secs(10));
        assert!(parent.same_request_as(&capped));
        assert_eq!(
            parent.capped_at(now + Duration::from_secs(20)).deadline(),
            parent.deadline()
        );
        parent.cancel_handle().cancel();
        assert!(capped.is_cancelled());
    }

    #[test]
    fn capped_budget_observes_external_owner_cancellation() {
        let cancelled = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let parent = RequestBudgetV1::until_with_external_cancellation(
            Instant::now() + Duration::from_secs(10),
            std::sync::Arc::clone(&cancelled),
        );
        let capped = parent.capped_at(Instant::now() + Duration::from_secs(1));
        cancelled.store(true, std::sync::atomic::Ordering::Release);
        assert!(capped.is_cancelled());
        assert!(parent.same_request_as(&capped));
    }

    #[derive(Clone)]
    struct BudgetMemory {
        live: std::sync::Arc<std::sync::atomic::AtomicU64>,
        refuse: bool,
    }

    impl crate::index::MemoryLeaseOwner for BudgetMemory {
        fn release(&self, bytes: u64) {
            let previous = self
                .live
                .fetch_sub(bytes, std::sync::atomic::Ordering::AcqRel);
            assert!(previous >= bytes, "budget backing released twice");
        }
    }

    impl crate::index::MemoryScopePort for BudgetMemory {
        fn reserve(
            &self,
            bytes: u64,
            _lifetime: crate::index::MemoryLifetime,
        ) -> Result<crate::index::MemoryLease, crate::index::IndexError> {
            if self.refuse {
                return Err(crate::index::IndexError::ResourceLimited);
            }
            let _previous = self
                .live
                .fetch_add(bytes, std::sync::atomic::Ordering::AcqRel);
            Ok(crate::index::MemoryLease::new(
                bytes,
                std::sync::Arc::new(self.clone()),
            ))
        }
    }

    #[test]
    fn admitted_budget_initial_backing_lives_until_last_clone() {
        let live = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
        let memory = crate::index::MemoryScope::new(std::sync::Arc::new(BudgetMemory {
            live: live.clone(),
            refuse: false,
        }));
        let original = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let budget = RequestBudgetV1::until_with_external_cancellation_admitted(
            Instant::now() + Duration::from_secs(10),
            original,
            &memory,
        )
        .expect("original reservation admitted");
        let charged = live.load(std::sync::atomic::Ordering::Acquire);
        assert!(charged > 0);
        let clone = budget.clone();
        drop(budget);
        assert_eq!(live.load(std::sync::atomic::Ordering::Acquire), charged);
        drop(clone);
        assert_eq!(live.load(std::sync::atomic::Ordering::Acquire), 0);
    }

    #[test]
    fn admitted_budget_refuses_when_original_memory_is_unavailable() {
        let live = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
        let memory = crate::index::MemoryScope::new(std::sync::Arc::new(BudgetMemory {
            live: live.clone(),
            refuse: true,
        }));
        let original = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        assert!(matches!(
            RequestBudgetV1::until_with_external_cancellation_admitted(
                Instant::now() + Duration::from_secs(10),
                original,
                &memory,
            ),
            Err(crate::index::IndexError::ResourceLimited)
        ));
        assert_eq!(live.load(std::sync::atomic::Ordering::Acquire), 0);
    }

    #[derive(Debug)]
    struct ExecutionObserver(std::sync::atomic::AtomicU8);

    impl super::RequestExecutionObserverV1 for ExecutionObserver {
        fn interruption(&self) -> Option<BudgetInterruptionV1> {
            match self.0.load(std::sync::atomic::Ordering::Acquire) {
                0 => None,
                1 => Some(BudgetInterruptionV1::Cancelled),
                _ => Some(BudgetInterruptionV1::DeadlineExceeded {
                    over_by: Duration::from_millis(7),
                }),
            }
        }
    }

    #[test]
    fn external_observer_preserves_phase_cancellation_and_backing_lifetime() {
        let observer = std::sync::Arc::new(ExecutionObserver(std::sync::atomic::AtomicU8::new(0)));
        let live = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
        let memory = crate::index::MemoryScope::new(std::sync::Arc::new(BudgetMemory {
            live: live.clone(),
            refuse: false,
        }));
        let budget =
            RequestBudgetV1::observing_execution_admitted(observer.clone(), &memory).unwrap();
        let cap = Instant::now() + Duration::from_secs(1);
        let capped = budget.capped_at(cap);
        assert!(budget.same_request_as(&capped));
        assert_eq!(capped.deadline(), cap);
        assert!(budget.checkpoint("phase-live").is_ok());
        let charge = live.load(std::sync::atomic::Ordering::Acquire);
        assert!(charge > 0);
        observer.0.store(1, std::sync::atomic::Ordering::Release);
        assert!(capped.is_cancelled());
        assert_eq!(capped.interruption(), Some(BudgetInterruptionV1::Cancelled));
        assert!(capped.checkpoint("phase-cancelled").is_err());
        drop(budget);
        assert_eq!(live.load(std::sync::atomic::Ordering::Acquire), charge);
        drop(capped);
        assert_eq!(live.load(std::sync::atomic::Ordering::Acquire), 0);
    }

    #[test]
    fn external_observer_reports_deadline_without_relabelling_cancellation() {
        let observer = std::sync::Arc::new(ExecutionObserver(std::sync::atomic::AtomicU8::new(0)));
        let live = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
        let memory = crate::index::MemoryScope::new(std::sync::Arc::new(BudgetMemory {
            live: live.clone(),
            refuse: false,
        }));
        let budget =
            RequestBudgetV1::observing_execution_admitted(observer.clone(), &memory).unwrap();
        observer.0.store(2, std::sync::atomic::Ordering::Release);
        assert!(!budget.is_cancelled());
        assert_eq!(
            budget.interruption(),
            Some(BudgetInterruptionV1::DeadlineExceeded {
                over_by: Duration::from_millis(7),
            })
        );
        assert!(matches!(
            budget.checkpoint("phase-deadline"),
            Err(CoreError::Typed {
                code: REQUEST_DEADLINE_EXCEEDED_CODE,
                ..
            })
        ));
        drop(budget);
        assert_eq!(live.load(std::sync::atomic::Ordering::Acquire), 0);
        assert!(matches!(
            RequestBudgetV1::observing_execution_admitted(observer, &memory),
            Err(crate::index::IndexError::Cancelled)
        ));
        assert_eq!(live.load(std::sync::atomic::Ordering::Acquire), 0);
    }

    #[test]
    fn external_cancellation_preserves_owner_deadline_and_reaches_descendants() {
        let original = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let deadline = Instant::now() + Duration::from_secs(10);
        let budget = RequestBudgetV1::until_with_external_cancellation(
            deadline,
            std::sync::Arc::clone(&original),
        );
        let clone = budget.clone();
        let child = budget.child();
        let nested = child.child();
        assert_eq!(nested.deadline(), deadline);
        assert!(budget.checkpoint("original-live").is_ok());
        original.store(true, std::sync::atomic::Ordering::Release);
        for request in [&budget, &clone, &child, &nested] {
            assert_eq!(
                request.interruption(),
                Some(BudgetInterruptionV1::Cancelled)
            );
            assert!(request.checkpoint("original-cancelled").is_err());
        }
    }

    #[test]
    fn local_cancellation_does_not_cancel_external_owner() {
        let original = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let budget = RequestBudgetV1::until_with_external_cancellation(
            Instant::now() + Duration::from_secs(10),
            std::sync::Arc::clone(&original),
        );
        budget.child().cancel_handle().cancel();
        assert!(!budget.is_cancelled());
        budget.cancel_handle().cancel();
        assert!(budget.is_cancelled());
        assert!(!original.load(std::sync::atomic::Ordering::Acquire));
    }

    #[test]
    fn preview_ledger_is_shared_and_output_lives_until_last_request_owner() {
        let request = RequestBudgetV1::unbounded();
        let ledger = request
            .lexical_preview_budget(10, 100_000)
            .expect("valid policy");
        let clone = request.clone();
        let same = clone
            .lexical_preview_budget(10, 100_000)
            .expect("same policy");
        ledger.charge_work(3).expect("work admitted");
        assert_eq!(same.used_work(), 3);
        assert!(request.lexical_preview_budget(11, 100_000).is_err());
        let slot = request
            .reserve_lexical_output_group()
            .expect("carrier lock")
            .expect("space");
        let held = ledger.reserve_bytes(19).expect("output admitted");
        let before = ledger.resident_bytes();
        // A later optional refusal does not revoke already-admitted payloads.
        assert!(ledger.charge_work(100).is_err());
        request
            .retain_lexical_output(slot, vec![held])
            .expect("allocation-free transfer");
        assert_eq!(ledger.resident_bytes(), before);
        assert!(request.retain_lexical_output(slot, Vec::new()).is_err());
        drop(request);
        assert_eq!(ledger.resident_bytes(), before);
        drop(clone);
        assert_eq!(ledger.resident_bytes(), 0);
    }

    #[test]
    fn preview_group_exhaustion_is_explicit_without_poisoning_ledger() {
        let request = RequestBudgetV1::unbounded();
        let ledger = request
            .lexical_preview_budget(10, 100_000)
            .expect("valid policy");
        for expected in 0..super::LEXICAL_OUTPUT_GROUP_LIMIT {
            assert_eq!(
                request.reserve_lexical_output_group().expect("carrier"),
                Some(expected)
            );
        }
        assert_eq!(
            request.reserve_lexical_output_group().expect("carrier"),
            None
        );
        assert!(ledger.failure().is_none());
        assert!(
            request
                .retain_lexical_output(super::LEXICAL_OUTPUT_GROUP_LIMIT, Vec::new())
                .is_err()
        );
        request
            .retain_lexical_output(0, Vec::new())
            .expect("existing slot valid");
    }

    #[test]
    fn a_fresh_budget_passes_its_checkpoints() {
        let budget = RequestBudgetV1::for_duration(Duration::from_secs(60));
        assert!(budget.checkpoint("entry").is_ok());
        assert!(budget.interruption().is_none());
        assert!(budget.remaining() > Duration::from_secs(50));
    }

    #[derive(Debug)]
    struct TestProviderCustody;

    struct TestProviderLease;

    impl super::ProviderAttemptLeaseV1 for TestProviderLease {}

    impl super::ProviderAttemptCustodyPortV1 for TestProviderCustody {
        fn begin_attempt_v1(&self) -> Result<Box<dyn super::ProviderAttemptLeaseV1>, CoreError> {
            Ok(Box::new(TestProviderLease))
        }
    }

    #[test]
    fn old_stage_clone_cannot_borrow_later_stage_provider_admission() -> Result<(), CoreError> {
        use std::sync::Arc;

        macro_rules! check {
            ($condition:expr) => {
                if !$condition {
                    return Err(CoreError::InvalidContract(format!(
                        "assertion failed: {}",
                        stringify!($condition)
                    )));
                }
            };
        }

        let original = RequestBudgetV1::for_duration(Duration::from_secs(30));
        let mut first = original.clone();
        let first_owner: Arc<dyn super::ProviderAttemptCustodyPortV1> =
            Arc::new(TestProviderCustody);
        first.bind_provider_attempt_custody_v1(&first_owner)?;
        let escaped = first.clone();
        check!(first.begin_provider_attempt_v1()?.is_some());
        drop(first_owner);

        let mut second = original;
        let second_owner: Arc<dyn super::ProviderAttemptCustodyPortV1> =
            Arc::new(TestProviderCustody);
        second.bind_provider_attempt_custody_v1(&second_owner)?;
        check!(first.deadline() == second.deadline());
        check!(first.same_request_as(&second));
        check!(matches!(
            escaped.begin_provider_attempt_v1(),
            Err(CoreError::Typed {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::ServerOverloaded,
                ..
            })
        ));
        check!(matches!(
            escaped.reserve_provider_memory_v1(1, crate::index::MemoryLifetime::Temporary),
            Err(CoreError::Typed {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::ServerOverloaded,
                ..
            })
        ));
        check!(second.begin_provider_attempt_v1()?.is_some());
        check!(matches!(
            second.bind_provider_attempt_custody_v1(&second_owner),
            Err(CoreError::InvalidContract(_))
        ));
        Ok(())
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
    fn panicking_waiter_does_not_suppress_child_or_remaining_wakes() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering};

        let parent = RequestBudgetV1::unbounded();
        let _panicking = parent.cancel_waiter(Arc::new(|| panic!("first waiter")));
        let child = parent.child();
        let sibling = parent.child();
        let remaining_woke = Arc::new(AtomicBool::new(false));
        let _remaining = {
            let remaining_woke = Arc::clone(&remaining_woke);
            parent.cancel_waiter(Arc::new(move || {
                remaining_woke.store(true, Ordering::SeqCst);
            }))
        };
        let _later_panicking = parent.cancel_waiter(Arc::new(|| panic!("later waiter")));

        let cancelled = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            parent.cancel_handle().cancel();
        }));
        let panic = cancelled.expect_err("the first waiter panic must propagate");
        assert_eq!(panic.downcast_ref::<&str>(), Some(&"first waiter"));
        assert!(parent.is_cancelled());
        assert!(child.is_cancelled());
        assert!(sibling.is_cancelled());
        assert!(remaining_woke.load(Ordering::SeqCst));
        assert_eq!(parent.live_waiters(), 0);
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

    #[test]
    fn child_cancel_is_local_but_request_ledgers_and_deadline_are_shared() {
        let parent = RequestBudgetV1::for_duration(Duration::from_secs(60))
            .with_semantic_work_limit_v1(10)
            .expect("work allowance")
            .with_correlation(super::RequestCorrelationV1::from_raw(42).expect("request id"));
        let child = parent.child();
        let sibling = parent.child();
        assert_eq!(child.deadline(), parent.deadline());
        assert_eq!(child.correlation(), parent.correlation());
        assert!(!child.same_request_as(&parent));
        assert!(!child.same_request_as(&sibling));
        assert!(child.same_request_as(&child.clone()));
        let preview = parent
            .lexical_preview_budget(10, 100_000)
            .expect("preview allowance");
        child
            .lexical_preview_budget(10, 100_000)
            .expect("shared preview allowance")
            .charge_work(2)
            .expect("preview work");
        assert_eq!(preview.used_work(), 2);
        child.consume_semantic_work_v1(4).expect("child work");
        assert_eq!(parent.semantic_work_consumed_v1().expect("ledger"), Some(4));

        child.cancel_handle().cancel();
        assert!(child.is_cancelled());
        assert!(!parent.is_cancelled());
        assert!(!sibling.is_cancelled());
    }

    #[test]
    fn parent_cancel_reaches_nested_child_after_intermediate_drop() {
        let parent = RequestBudgetV1::for_duration(Duration::from_secs(60));
        let child = parent.child();
        let nested = child.child();
        assert_eq!(parent.live_waiters(), 1);
        assert_eq!(child.live_waiters(), 1);
        drop(child);
        assert_eq!(
            parent.live_waiters(),
            1,
            "nested child retains ancestor relay"
        );

        parent.cancel_handle().cancel();
        assert!(nested.is_cancelled());
        assert!(matches!(
            nested.checkpoint("nested preparation"),
            Err(CoreError::Typed {
                code: REQUEST_CANCELLED_CODE,
                ..
            })
        ));
        drop(nested);
        assert_eq!(parent.live_waiters(), 0);
    }

    #[test]
    fn child_observes_pre_cancel_and_absolute_deadline() {
        let parent = RequestBudgetV1::for_duration(Duration::from_secs(60));
        parent.cancel_handle().cancel();
        let child = parent.child();
        assert!(child.is_cancelled());
        assert_eq!(child.deadline(), parent.deadline());
        drop(child);
        assert_eq!(parent.live_waiters(), 0);

        let expired = just_expired();
        let expired_child = expired.child();
        assert_eq!(expired_child.deadline(), expired.deadline());
        assert!(matches!(
            expired_child.interruption(),
            Some(BudgetInterruptionV1::DeadlineExceeded { .. })
        ));
    }

    #[test]
    fn dropping_last_child_removes_all_ancestor_waiters() {
        let root = RequestBudgetV1::unbounded();
        let child = root.child();
        let nested = child.child();
        let nested_clone = nested.clone();
        drop(child);
        drop(nested);
        assert_eq!(root.live_waiters(), 1);
        drop(nested_clone);
        assert_eq!(root.live_waiters(), 0);
    }

    #[test]
    fn publication_owner_refuses_same_cancel_scope_with_changed_deadline() {
        let request = RequestBudgetV1::for_duration(Duration::from_secs(60));
        let mut changed = request.clone();
        changed.deadline += Duration::from_secs(1);
        assert!(request.same_request_as(&changed));
        let mut owner = crate::PublicationValidationOwner::default();
        owner
            .bind_prepare_request_budget(&request)
            .expect("first binding");
        assert!(matches!(
            owner.bind_prepare_request_budget(&changed),
            Err(CoreError::InvalidContract(message))
                if message.contains("preparation request budget changed")
        ));
        assert_eq!(
            owner
                .prepare_request_budget()
                .expect("original budget")
                .deadline(),
            request.deadline()
        );
    }

    #[test]
    fn semantic_work_allowance_is_shared_and_rejects_overrun_without_spending_it() {
        let budget = RequestBudgetV1::for_duration(Duration::from_secs(60))
            .with_semantic_work_limit_v1(10)
            .expect("finite semantic work budget");
        let sibling = budget.clone();
        budget.consume_semantic_work_v1(6).expect("first charge");
        assert!(matches!(
            sibling.consume_semantic_work_v1(5),
            Err(CoreError::Typed {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::SemanticWorkBudgetExceeded,
                ..
            })
        ));
        assert_eq!(
            budget.semantic_work_consumed_v1().expect("healthy ledger"),
            Some(6)
        );
        sibling
            .consume_semantic_work_v1(4)
            .expect("exact remaining charge");
        assert_eq!(
            budget.semantic_work_consumed_v1().expect("healthy ledger"),
            Some(10)
        );
    }
}
