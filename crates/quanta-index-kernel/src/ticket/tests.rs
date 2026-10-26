//! Read-ticket completion: first failure, unconditional cleanup and payload
//! lifetime.
//!
//! The catalog double injects cleanup failure/cancellation only;
//! these cases do not qualify durable Catalog or process recovery behavior.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use quanta_index_core::{
    CancelHandleV1, RequestBudgetV1,
    index::{
        CatalogView, CollectionId, Digest, MapRoot, ObjectRef, ReadSessionLease,
        ReadSessionRecoveryPage, ReadSessionRef, ReadTicketLease, SlotId, SourceCut, TenantId,
        TicketId, TicketRecord, ViewRef, ViewSelection, WorkBudget, WorkLimits, WorkspaceKey,
        WorktreeId,
    },
};

use super::{Arc, CatalogReadPort, Duration, IndexError, ReadTicket};

struct Session;
impl ReadSessionLease for Session {
    fn binding(&self) -> ReadSessionRef {
        ReadSessionRef { id: 1, fence: 1 }
    }
}

struct CleanupCatalog {
    refuse: bool,
    failures_remaining: AtomicUsize,
    cancel_on_release: Option<CancelHandleV1>,
    calls: AtomicUsize,
    payload_drops: Option<Arc<AtomicUsize>>,
    saw_early_payload_drop: AtomicBool,
    diagnostic_arguments: std::sync::Mutex<Vec<Option<quanta_index_core::DiagnosticCaptureIdV1>>>,
}
impl quanta_index_core::index::ports::CatalogDiagnosticPort for CleanupCatalog {
    fn enter_diagnostic_scope(
        &self,
        context: Option<&quanta_index_core::DiagnosticContextV1>,
    ) -> Option<Box<dyn quanta_index_core::index::ports::CatalogDiagnosticScope + '_>> {
        // This cleanup-only double has no native IO scope. Record consumer
        // arguments; the real VFS tests qualify propagation and isolation.
        if let Ok(mut seen) = self.diagnostic_arguments.lock() {
            seen.push(context.map(quanta_index_core::DiagnosticContextV1::capture_id));
        }
        None
    }
}

impl CatalogReadPort for CleanupCatalog {
    fn open_read_session(&self) -> Result<Arc<dyn ReadSessionLease>, IndexError> {
        Err(IndexError::BackendViolation(
            "not a session-open fixture".into(),
        ))
    }
    fn acquire_ticket(
        &self,
        _: &dyn ReadSessionLease,
        _: ViewSelection,
        _: Duration,
        _: &WorkBudget,
    ) -> Result<TicketRecord, IndexError> {
        Err(IndexError::BackendViolation(
            "not a ticket-acquire fixture".into(),
        ))
    }
    fn renew_ticket(
        &self,
        _: &dyn ReadSessionLease,
        _: ReadTicketLease,
        _: Duration,
    ) -> Result<ReadTicketLease, IndexError> {
        Err(IndexError::BackendViolation(
            "not a ticket-renew fixture".into(),
        ))
    }
    fn validate_ticket(
        &self,
        _: &dyn ReadSessionLease,
        _: ReadTicketLease,
    ) -> Result<(), IndexError> {
        Err(IndexError::BackendViolation(
            "not a ticket-validate fixture".into(),
        ))
    }
    fn release_ticket(
        &self,
        _: &dyn ReadSessionLease,
        _: ReadTicketLease,
    ) -> Result<(), IndexError> {
        let _prior = self.calls.fetch_add(1, Ordering::SeqCst);
        if self
            .payload_drops
            .as_ref()
            .is_some_and(|drops| drops.load(Ordering::SeqCst) != 0)
        {
            self.saw_early_payload_drop.store(true, Ordering::SeqCst);
        }
        if let Some(cancel) = &self.cancel_on_release {
            cancel.cancel();
        }
        let transient_failure = self
            .failures_remaining
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |remaining| {
                remaining.checked_sub(1)
            })
            .is_ok();
        if self.refuse || transient_failure {
            Err(IndexError::FenceLost)
        } else {
            Ok(())
        }
    }
    fn recover_read_sessions(
        &self,
        _: Option<ReadSessionRef>,
        _: u32,
        _: &mut WorkBudget,
    ) -> Result<ReadSessionRecoveryPage, IndexError> {
        Err(IndexError::BackendViolation(
            "not a session-recovery fixture".into(),
        ))
    }
}

fn fixture(
    refuse: bool,
    cancel_on_release: bool,
    payload_drops: Option<Arc<AtomicUsize>>,
) -> (ReadTicket, Arc<CleanupCatalog>, WorkBudget) {
    let budget = WorkBudget::with_request(
        WorkLimits {
            reads: 1,
            writes: 1,
            bytes: 1,
            mutations: 1,
        },
        RequestBudgetV1::for_duration(Duration::from_secs(30)),
    );
    let catalog = Arc::new(CleanupCatalog {
        refuse,
        failures_remaining: AtomicUsize::new(0),
        cancel_on_release: cancel_on_release.then(|| budget.request().cancel_handle()),
        calls: AtomicUsize::new(0),
        payload_drops,
        saw_early_payload_drop: AtomicBool::new(false),
        diagnostic_arguments: std::sync::Mutex::new(Vec::new()),
    });
    let lease = ReadTicketLease {
        id: TicketId([1; 16]),
        session: Session.binding(),
        fence: 1,
        expires_at_ms: 1000,
    };
    let ticket = ReadTicket::new(
        TicketRecord {
            lease,
            view: CatalogView {
                identity: ViewRef {
                    workspace: WorkspaceKey {
                        tenant: TenantId([1; 16]),
                        collection: CollectionId([2; 16]),
                        worktree: WorktreeId([3; 16]),
                        slot: SlotId([4; 16]),
                    },
                    epoch: 1,
                    sequence: 1,
                    digest: Digest([5; 32]),
                },
                profile: ObjectRef(Digest([6; 32])),
                source: SourceCut {
                    owners: MapRoot(None),
                    paths: MapRoot(None),
                    frontiers: MapRoot(None),
                },
                projections: MapRoot(None),
                members: MapRoot(None),
                coverage: MapRoot(None),
            },
        },
        catalog.clone(),
        Arc::new(Session),
        Duration::from_secs(1),
    );
    (ticket, catalog, budget)
}

#[test]
fn first_typed_failure_survives_release_refusal_and_late_cancellation() {
    for primary in [
        IndexError::ResourceLimited,
        IndexError::WorkLimitExceeded,
        IndexError::Cancelled,
        IndexError::CorruptObject("original read corruption".into()),
        IndexError::SnapshotUnavailable,
    ] {
        for (refuse, cancel) in [(false, false), (false, true), (true, false), (true, true)] {
            let (ticket, catalog, budget) = fixture(refuse, cancel, None);
            let result: Result<(), IndexError> = ticket.finish(Err(primary.clone()), &budget);
            assert_eq!(result, Err(primary.clone()));
            // A failed explicit release retains the existing guard's retry.
            assert_eq!(
                catalog.calls.load(Ordering::SeqCst),
                if refuse { 2 } else { 1 }
            );
        }
    }
}

#[derive(Debug)]
struct Payload(Arc<AtomicUsize>);
impl Drop for Payload {
    fn drop(&mut self) {
        let _prior = self.0.fetch_add(1, Ordering::SeqCst);
    }
}

#[test]
fn cleanup_and_post_cleanup_budget_refuse_success_without_early_payload_drop() {
    for (refuse, cancel) in [(false, false), (false, true), (true, false), (true, true)] {
        let drops = Arc::new(AtomicUsize::new(0));
        let (ticket, catalog, budget) = fixture(refuse, cancel, Some(Arc::clone(&drops)));
        let result = ticket.finish(Ok(Payload(Arc::clone(&drops))), &budget);
        let expected = if refuse {
            Some(IndexError::FenceLost)
        } else if cancel {
            Some(IndexError::Cancelled)
        } else {
            None
        };
        match result {
            Ok(value) => {
                assert_eq!(expected, None);
                assert_eq!(drops.load(Ordering::SeqCst), 0);
                drop(value);
            }
            Err(error) => assert_eq!(Some(error), expected),
        }
        assert_eq!(drops.load(Ordering::SeqCst), 1);
        assert!(!catalog.saw_early_payload_drop.load(Ordering::SeqCst));
        assert_eq!(
            catalog.calls.load(Ordering::SeqCst),
            if refuse { 2 } else { 1 }
        );
    }
    // Already-cancelled work must still release its read ticket.
    let (ticket, catalog, budget) = fixture(false, false, None);
    budget.request().cancel_handle().cancel();
    assert_eq!(ticket.finish(Ok(()), &budget), Err(IndexError::Cancelled));
    assert_eq!(catalog.calls.load(Ordering::SeqCst), 1);
}

#[test]
fn unwind_releases_ticket_without_explicit_completion() {
    let (ticket, catalog, _budget) = fixture(false, false, None);
    let unwound = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ticket = ticket;
        std::panic::resume_unwind(Box::new("read aborted"));
    }));
    assert!(unwound.is_err());
    assert_eq!(catalog.calls.load(Ordering::SeqCst), 1);
}

#[test]
fn retained_release_retries_the_same_pin_and_acknowledges_only_once() {
    let (mut ticket, catalog, _budget) = fixture(false, false, None);
    catalog.failures_remaining.store(1, Ordering::SeqCst);
    let original = ticket.record().lease;
    assert_eq!(ticket.try_release(), Err(IndexError::FenceLost));
    assert_eq!(catalog.calls.load(Ordering::SeqCst), 1);
    assert_eq!(ticket.record().lease, original);
    assert!(!ticket.released);

    assert_eq!(ticket.try_release(), Ok(()));
    assert!(ticket.released);
    assert_eq!(ticket.try_release(), Ok(()));
    assert_eq!(ticket.validate(), Err(IndexError::SnapshotUnavailable));
    assert_eq!(ticket.renew(), Err(IndexError::SnapshotUnavailable));
    drop(ticket);
    assert_eq!(catalog.calls.load(Ordering::SeqCst), 2);
}

#[test]
fn failed_retained_release_keeps_the_drop_retry() {
    let (mut ticket, catalog, _budget) = fixture(false, false, None);
    catalog.failures_remaining.store(1, Ordering::SeqCst);
    assert_eq!(ticket.try_release(), Err(IndexError::FenceLost));
    assert!(!ticket.released);
    assert_eq!(catalog.calls.load(Ordering::SeqCst), 1);
    drop(ticket);
    assert_eq!(catalog.calls.load(Ordering::SeqCst), 2);
}

#[derive(Debug)]
struct UnusedDiagnosticClock;
impl quanta_index_core::DiagnosticClockV1 for UnusedDiagnosticClock {
    fn now_ns(&self) -> u64 {
        0
    }
}
#[derive(Debug)]
struct UnusedDiagnosticSink;
impl quanta_index_core::DiagnosticEventSinkV1 for UnusedDiagnosticSink {
    fn try_emit(&self, _event: quanta_index_core::DiagnosticEmitV1) -> bool {
        true
    }
}

#[test]
fn original_diagnostic_context_reaches_release_retry_and_drop_after_cancellation() {
    let (ticket, catalog, budget) = fixture(false, true, None);
    catalog.failures_remaining.store(1, Ordering::SeqCst);
    let id = quanta_index_core::DiagnosticCaptureIdV1([19; 16]);
    let context = quanta_index_core::DiagnosticContextV1::new(
        id,
        Arc::new(UnusedDiagnosticClock),
        Arc::new(UnusedDiagnosticSink),
    );
    let request = budget.request().clone().with_diagnostic_context(context);
    let observed = WorkBudget::with_request(
        WorkLimits {
            reads: 1,
            writes: 1,
            bytes: 1,
            mutations: 1,
        },
        request,
    );
    let mut ticket = ticket.with_diagnostics(&observed);
    drop(observed);
    assert_eq!(ticket.try_release(), Err(IndexError::FenceLost));
    assert_eq!(budget.checkpoint(), Err(IndexError::Cancelled));
    drop(ticket);
    assert_eq!(catalog.calls.load(Ordering::SeqCst), 2);
    let seen = match catalog.diagnostic_arguments.lock() {
        Ok(seen) => seen,
        Err(error) => panic!("diagnostic argument fixture poisoned: {error}"),
    };
    assert_eq!(&*seen, &[Some(id), Some(id)]);
    drop(seen);
}

#[test]
fn composite_observer_preserves_exact_catalog_and_session_identity() {
    let (ticket, catalog, _budget) = fixture(false, false, None);
    let same: Arc<dyn CatalogReadPort> = catalog;
    assert!(ticket.belongs_to(&same, &ticket.session));
    let (_foreign_ticket, foreign, _foreign_budget) = fixture(false, false, None);
    let foreign: Arc<dyn CatalogReadPort> = foreign;
    assert!(!ticket.belongs_to(&foreign, &ticket.session));
    let same_binding_other_owner: Arc<dyn ReadSessionLease> = Arc::new(Session);
    assert!(!ticket.belongs_to(&same, &same_binding_other_owner));
}
