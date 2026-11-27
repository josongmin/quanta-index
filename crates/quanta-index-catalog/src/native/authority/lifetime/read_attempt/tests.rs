//! Real Catalog regressions.
//!
//! Expected floors and outcomes are fixed independently
//! from the batching algorithm. These tests do not qualify product latency.

use super::*;
use crate::connection::CatalogClockPort;
use quanta_index_core::index::{
    CatalogView, Digest, MapRoot, RegisteredObject, SourceCut, ViewRef, ports::CatalogGenesisPort,
};
use std::{
    error::Error,
    sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    time::Instant,
};

type TestResult = Result<(), Box<dyn Error>>;
macro_rules! check {
    ($condition:expr) => {
        if !$condition {
            return Err(format!("assertion failed: {}", stringify!($condition)).into());
        }
    };
}
macro_rules! check_eq {
    ($left:expr, $right:expr) => {{
        let left = &$left;
        let right = &$right;
        if left != right {
            return Err(format!(
                "assertion failed: {} == {}\nleft: {left:?}\nright: {right:?}",
                stringify!($left),
                stringify!($right)
            )
            .into());
        }
    }};
}
struct ManualClock(AtomicU64);
impl ManualClock {
    fn new(now: u64) -> Self {
        Self(AtomicU64::new(now))
    }
    fn set(&self, now: u64) {
        self.0.store(now, Ordering::SeqCst);
    }
}
impl CatalogClockPort for ManualClock {
    fn now_unix_ms(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }
}

fn pin_count(catalog: &SqliteCatalog) -> Result<i64, Box<dyn Error>> {
    Ok(catalog
        .lock()?
        .query_row("SELECT COUNT(*) FROM native_pin_v1", [], |row| row.get(0))?)
}

fn floor(catalog: &SqliteCatalog) -> Result<i64, Box<dyn Error>> {
    // Use a separately opened SQLite connection: this observes committed data.
    Ok(Connection::open(catalog.path())?.query_row(
        "SELECT last_ms FROM native_work_clock_v1 WHERE singleton=1",
        [],
        |row| row.get(0),
    )?)
}

fn ticket(
    catalog: &SqliteCatalog,
    session: &dyn ReadSessionLease,
    view: &CatalogView,
) -> Result<ReadTicketLease, IndexError> {
    Ok(catalog
        .acquire_ticket(
            session,
            ViewSelection::Exact(view.identity),
            Duration::from_millis(100),
            &admission_budget(),
        )?
        .lease)
}

fn concrete(session: &dyn ReadSessionLease) -> Result<&CatalogSessionLease, Box<dyn Error>> {
    let any: &dyn Any = session;
    any.downcast_ref::<CatalogSessionLease>()
        .ok_or_else(|| "not a real Catalog session".into())
}

fn wait_pending(session: &dyn ReadSessionLease, count: usize) -> TestResult {
    let deadline = Instant::now()
        .checked_add(Duration::from_secs(3))
        .ok_or("queue deadline overflow")?;
    loop {
        let slot = concrete(session)?
            .read_observations
            .lock()
            .map_err(|_error| "group slot poisoned")?;
        let observed = if let Some(group) = slot.as_ref() {
            group
                .pending
                .lock()
                .map_err(|_error| "group poisoned")?
                .entries
                .iter()
                .flatten()
                .count()
        } else {
            0
        };
        drop(slot);
        if observed == count {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err("observations did not queue".into());
        }
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[test]
fn read_attempt_intermediate_floor_survives_abort_drop_failure_and_reopen() -> TestResult {
    let root = tempfile::tempdir()?;
    let clock = Arc::new(ManualClock::new(1_000));
    let (catalog, view) = setup_with_clock(root.path(), clock.clone())?;
    let session = catalog.open_read_session()?;
    let ticket = ticket(&catalog, session.as_ref(), &view)?;
    let budget = admission_budget();
    clock.set(1_001);
    let mut attempt = start(&catalog, session.as_ref(), ticket, &budget)?;
    check_eq!(attempt.ticket(), ticket);
    check_eq!(floor(&catalog)?, 1_001);
    // A public call in the middle proves no connection is retained by attempt.
    catalog.validate_ticket(session.as_ref(), ticket)?;
    clock.set(1_002);
    attempt.observe()?;
    check_eq!(floor(&catalog)?, 1_002);
    attempt.abort();
    check_eq!(pin_count(&catalog)?, 1);
    clock.set(1_003);
    let attempt = start(&catalog, session.as_ref(), ticket, &budget)?;
    drop(attempt);
    check_eq!(floor(&catalog)?, 1_003);
    clock.set(1_004);
    let mut attempt = start(&catalog, session.as_ref(), ticket, &budget)?;
    clock.set(1_003);
    check!(
        matches!(attempt.observe(), Err(IndexError::Storage(message)) if message == "catalog clock regressed")
    );
    clock.set(1_005);
    check!(attempt.finalize().is_err());
    check_eq!(floor(&catalog)?, 1_004);
    drop(catalog);
    clock.set(1_003);
    let reopened = SqliteCatalog::open_with_clock(root.path(), Duration::from_secs(1), clock)?;
    check!(
        matches!(start(&reopened, session.as_ref(), ticket, &budget), Err(IndexError::Storage(message)) if message == "catalog clock regressed")
    );
    reopened.release_ticket(session.as_ref(), ticket)?;
    check_eq!(pin_count(&reopened)?, 0);
    Ok(())
}

#[test]
fn read_attempt_expiry_and_renewal_fence_are_checked_on_every_observation() -> TestResult {
    let root = tempfile::tempdir()?;
    let clock = Arc::new(ManualClock::new(1_000));
    let (catalog, view) = setup_with_clock(root.path(), clock.clone())?;
    let session = catalog.open_read_session()?;
    let ticket = ticket(&catalog, session.as_ref(), &view)?;
    let budget = admission_budget();
    let mut attempt = start(&catalog, session.as_ref(), ticket, &budget)?;
    clock.set(ticket.expires_at_ms);
    check_eq!(attempt.observe(), Err(IndexError::SnapshotUnavailable));
    check_eq!(attempt.finalize(), Err(IndexError::SnapshotUnavailable));
    check_eq!(floor(&catalog)?, 1_000);
    check_eq!(pin_count(&catalog)?, 1);
    clock.set(1_001);
    let mut attempt = start(&catalog, session.as_ref(), ticket, &budget)?;
    let next = catalog.renew_ticket(session.as_ref(), ticket, Duration::from_millis(100))?;
    check_eq!(attempt.observe(), Err(IndexError::SnapshotUnavailable));
    attempt.abort();
    let attempt = start(&catalog, session.as_ref(), next, &budget)?;
    attempt.finalize()?;
    catalog.release_ticket(session.as_ref(), next)?;
    check_eq!(pin_count(&catalog)?, 0);
    Ok(())
}

#[test]
fn read_attempt_final_commit_failure_cannot_return_success_or_erase_prior_floor() -> TestResult {
    let root = tempfile::tempdir()?;
    let clock = Arc::new(ManualClock::new(1_000));
    let (catalog, view) = setup_with_clock(root.path(), clock.clone())?;
    let session = catalog.open_read_session()?;
    let ticket = ticket(&catalog, session.as_ref(), &view)?;
    let budget = admission_budget();
    clock.set(1_001);
    let attempt = start(&catalog, session.as_ref(), ticket, &budget)?;
    clock.set(1_002);
    catalog.lock()?.commit_hook(Some(|| true))?;
    check!(matches!(attempt.finalize(), Err(IndexError::Storage(_))));
    catalog.lock()?.commit_hook(None::<fn() -> bool>)?;
    check_eq!(floor(&catalog)?, 1_001);
    check_eq!(pin_count(&catalog)?, 1);
    let attempt = start(&catalog, session.as_ref(), ticket, &budget)?;
    clock.set(1_003);
    attempt.finalize()?;
    check_eq!(floor(&catalog)?, 1_003);
    catalog.release_ticket(session.as_ref(), ticket)?;
    Ok(())
}

#[test]
fn read_attempt_cancel_during_final_commit_keeps_durable_floor_and_refuses_success() -> TestResult {
    let root = tempfile::tempdir()?;
    let clock = Arc::new(ManualClock::new(1_000));
    let (catalog, view) = setup_with_clock(root.path(), clock.clone())?;
    let session = catalog.open_read_session()?;
    let ticket = ticket(&catalog, session.as_ref(), &view)?;
    let budget = admission_budget();
    let attempt = start(&catalog, session.as_ref(), ticket, &budget)?;
    let cancel = budget.request().cancel_handle();
    clock.set(1_001);
    catalog.lock()?.commit_hook(Some(move || {
        cancel.cancel();
        false
    }))?;
    check_eq!(attempt.finalize(), Err(IndexError::Cancelled));
    catalog.lock()?.commit_hook(None::<fn() -> bool>)?;
    check_eq!(floor(&catalog)?, 1_001);
    check_eq!(pin_count(&catalog)?, 1);
    catalog.release_ticket(session.as_ref(), ticket)?;
    Ok(())
}

#[test]
fn read_attempt_rechecks_inode_os_lock_and_corrupt_clock() -> TestResult {
    for fault in [0, 1, 2] {
        let root = tempfile::tempdir()?;
        let clock = Arc::new(ManualClock::new(1_000));
        let (catalog, view) = setup_with_clock(root.path(), clock.clone())?;
        let session = catalog.open_read_session()?;
        let ticket = ticket(&catalog, session.as_ref(), &view)?;
        let budget = admission_budget();
        let mut attempt = start(&catalog, session.as_ref(), ticket, &budget)?;
        let directory = session_directory(&catalog)?;
        let original = lock_path(&directory, session.binding().id);
        let moved = directory.join("test-held-lock");
        match fault {
            0 => {
                fs::rename(&original, &moved)?;
                drop(open_lock(&catalog, &original, true)?);
            }
            1 => concrete(session.as_ref())?.file.file()?.unlock()?,
            _ => {
                let changed = catalog.lock()?.execute(
                    "UPDATE native_work_clock_v1 SET row_digest=zeroblob(32) WHERE singleton=1",
                    [],
                )?;
                check_eq!(changed, 1);
            }
        }
        check!(matches!(attempt.observe(), Err(IndexError::Storage(_))));
        check!(attempt.finalize().is_err());
        check_eq!(pin_count(&catalog)?, 1);
        if fault == 2 {
            match SqliteCatalog::open_with_clock(root.path(), Duration::from_secs(1), clock) {
                Ok(reopened) => check!(matches!(
                    start(&reopened, session.as_ref(), ticket, &budget),
                    Err(IndexError::Storage(message)) if message == "native work clock digest mismatch"
                )),
                Err(error) => check!(error.to_string().contains("clock")),
            }
        }
        match fault {
            0 => {
                fs::remove_file(&original)?;
                fs::rename(&moved, &original)?;
            }
            1 => concrete(session.as_ref())?.file.file()?.lock()?,
            _ => {
                let digest = fields_digest(b"qidx.native.workclock.v1", &(1_i64, 1_000_i64))?;
                let changed = catalog.lock()?.execute(
                    "UPDATE native_work_clock_v1 SET row_digest=?1 WHERE singleton=1",
                    params![digest],
                )?;
                check_eq!(changed, 1);
            }
        }
        catalog.release_ticket(session.as_ref(), ticket)?;
    }
    Ok(())
}

struct AdvancingClock {
    enabled: AtomicBool,
    next: AtomicU64,
}

impl CatalogClockPort for AdvancingClock {
    fn now_unix_ms(&self) -> u64 {
        if self.enabled.load(Ordering::SeqCst) {
            self.next.fetch_add(1, Ordering::SeqCst)
        } else {
            1_000
        }
    }
}

#[test]
fn read_attempt_concurrent_successes_share_one_real_commit_and_clock_write() -> TestResult {
    // Both paths start at floor 1000 and observe exactly 1001..=1008. This
    // isolates batching from the existing equal-clock UPDATE suppression.
    for grouped in [false, true] {
        let root = tempfile::tempdir()?;
        let clock = Arc::new(AdvancingClock {
            enabled: AtomicBool::new(false),
            next: AtomicU64::new(1_001),
        });
        let (catalog, view) = setup_with_clock(root.path(), clock.clone())?;
        let session = catalog.open_read_session()?;
        let ticket = ticket(&catalog, session.as_ref(), &view)?;
        let commits = Arc::new(AtomicUsize::new(0));
        {
            let mut connection = catalog.lock()?;
            // SQLite update_hook omits WITHOUT ROWID tables. A TEMP trigger
            // counts actual clock UPDATEs without changing the durable format.
            connection.execute_batch("CREATE TEMP TABLE read_attempt_clock_updates(sample INTEGER NOT NULL);
                CREATE TEMP TRIGGER read_attempt_clock_update AFTER UPDATE ON main.native_work_clock_v1
                BEGIN INSERT INTO read_attempt_clock_updates VALUES (NEW.last_ms); END;")?;
            let count = Arc::clone(&commits);
            connection.commit_hook(Some(move || {
                let _previous = count.fetch_add(1, Ordering::SeqCst);
                false
            }))?;
        }
        clock.enabled.store(true, Ordering::SeqCst);
        if grouped {
            std::thread::scope(|scope| -> TestResult {
                let held = catalog.lock()?;
                let mut workers = Vec::new();
                for _ in 0..8 {
                    let catalog = &catalog;
                    let session = session.as_ref();
                    workers.push(scope.spawn(move || -> Result<(), IndexError> {
                        let budget = admission_budget();
                        start(catalog, session, ticket, &budget)?.abort();
                        Ok(())
                    }));
                }
                let queued = wait_pending(session.as_ref(), 8);
                drop(held);
                queued?;
                for worker in workers {
                    worker.join().map_err(|_panic| "group worker panicked")??;
                }
                Ok(())
            })?;
        } else {
            for _ in 0..8 {
                catalog.validate_ticket(session.as_ref(), ticket)?;
            }
        }
        check_eq!(clock.next.load(Ordering::SeqCst), 1_009);
        check_eq!(floor(&catalog)?, 1_008);
        let expected = if grouped { 1 } else { 8 };
        check_eq!(commits.load(Ordering::SeqCst), expected);
        let writes: i64 = catalog.lock()?.query_row(
            "SELECT COUNT(*) FROM read_attempt_clock_updates",
            [],
            |row| row.get(0),
        )?;
        check_eq!(writes, i64::try_from(expected)?);
        check_eq!(pin_count(&catalog)?, 1);
        wait_pending(session.as_ref(), 0)?;
        catalog.release_ticket(session.as_ref(), ticket)?;
        check_eq!(pin_count(&catalog)?, 0);
    }
    Ok(())
}

struct RegressingBatchClock {
    armed: AtomicBool,
    samples: AtomicUsize,
}

impl CatalogClockPort for RegressingBatchClock {
    fn now_unix_ms(&self) -> u64 {
        if self.armed.load(Ordering::SeqCst) {
            match self.samples.fetch_add(1, Ordering::SeqCst) {
                1 => 1_001,
                _ => 1_002,
            }
        } else {
            1_000
        }
    }
}

#[test]
fn read_attempt_in_batch_clock_regression_refuses_later_sample_and_preserves_restart_floor()
-> TestResult {
    let root = tempfile::tempdir()?;
    let clock = Arc::new(RegressingBatchClock {
        armed: AtomicBool::new(false),
        samples: AtomicUsize::new(0),
    });
    let (catalog, view) = setup_with_clock(root.path(), clock.clone())?;
    let session = catalog.open_read_session()?;
    let ticket = ticket(&catalog, session.as_ref(), &view)?;
    let bytes = Arc::new(AtomicU64::new(0));
    let memory = quanta_index_core::index::MemoryScope::new(Arc::new(MemoryAudit {
        bytes: Arc::clone(&bytes),
        retained: Arc::new(AtomicUsize::new(0)),
        refuse: false,
    }));
    let outcomes = std::thread::scope(
        |scope| -> Result<Vec<Result<(), IndexError>>, Box<dyn Error>> {
            let held = catalog.lock()?;
            let mut workers = Vec::new();
            for _ in 0..2 {
                let catalog = &catalog;
                let session = session.as_ref();
                let memory = memory.clone();
                workers.push(scope.spawn(move || {
                    let mut budget = admission_budget();
                    budget.bind_memory_scope(Some(memory));
                    catalog
                        .begin_read_attempt(session, ticket, &budget)
                        .map(CatalogReadAttempt::abort)
                }));
            }
            let queued = wait_pending(session.as_ref(), 2);
            clock.armed.store(true, Ordering::SeqCst);
            drop(held);
            queued?;
            workers
                .into_iter()
                .map(|worker| {
                    worker
                        .join()
                        .map_err(|_panic| "clock regression worker panicked".into())
                })
                .collect()
        },
    )?;
    // Both samples exceed the original durable floor (1000). Only enforcing
    // ordering inside the batch can reject the second sample (1001).
    check_eq!(clock.samples.load(Ordering::SeqCst), 2);
    check_eq!(outcomes.iter().filter(|outcome| outcome.is_ok()).count(), 1);
    check_eq!(
        outcomes
            .iter()
            .filter(
                |outcome| **outcome == Err(IndexError::Storage("catalog clock regressed".into()))
            )
            .count(),
        1
    );
    check_eq!(floor(&catalog)?, 1_002);
    check_eq!(pin_count(&catalog)?, 1);
    wait_pending(session.as_ref(), 0)?;
    drop(catalog);
    let reopened = SqliteCatalog::open_with_clock(
        root.path(),
        Duration::from_secs(1),
        Arc::new(ManualClock::new(1_001)),
    )?;
    check!(
        matches!(reopened.begin_read_attempt(session.as_ref(), ticket, &admission_budget()), Err(IndexError::Storage(message)) if message == "catalog clock regressed")
    );
    check_eq!(floor(&reopened)?, 1_002);
    reopened.release_ticket(session.as_ref(), ticket)?;
    check_eq!(pin_count(&reopened)?, 0);
    drop(session);
    check_eq!(bytes.load(Ordering::SeqCst), 0);
    Ok(())
}

#[test]
fn read_attempt_cancelled_leader_does_not_cancel_live_follower() -> TestResult {
    let root = tempfile::tempdir()?;
    let clock = Arc::new(ManualClock::new(1_000));
    let (catalog, view) = setup_with_clock(root.path(), clock.clone())?;
    let session = catalog.open_read_session()?;
    let ticket = ticket(&catalog, session.as_ref(), &view)?;
    let held = catalog.lock()?;
    let cancelled = admission_budget();
    clock.set(1_001);
    std::thread::scope(|scope| -> TestResult {
        let (send, receive) = std::sync::mpsc::channel();
        let catalog = &catalog;
        let session_ref = session.as_ref();
        let cancelled = &cancelled;
        let first = scope.spawn(move || {
            let result =
                start(catalog, session_ref, ticket, cancelled).map(CatalogReadAttempt::abort);
            let _sent = send.send(result);
        });
        let queued_first = wait_pending(session_ref, 1);
        if queued_first.is_err() {
            drop(held);
            return queued_first;
        }
        let second = scope.spawn(move || -> Result<(), IndexError> {
            let budget = admission_budget();
            start(catalog, session_ref, ticket, &budget)?.abort();
            Ok(())
        });
        let queued_second = wait_pending(session_ref, 2);
        if queued_second.is_err() {
            drop(held);
            return queued_second;
        }
        cancelled.request().cancel_handle().cancel();
        let result = receive.recv_timeout(Duration::from_secs(2));
        drop(held);
        check_eq!(result?, Err(IndexError::Cancelled));
        first.join().map_err(|_panic| "cancelled worker panicked")?;
        second.join().map_err(|_panic| "live worker panicked")??;
        Ok(())
    })?;
    wait_pending(session.as_ref(), 0)?;
    check_eq!(floor(&catalog)?, 1_001);
    check_eq!(pin_count(&catalog)?, 1);
    catalog.release_ticket(session.as_ref(), ticket)?;
    Ok(())
}

#[test]
fn read_attempt_queued_cancellation_and_deadline_remove_uncommitted_observation() -> TestResult {
    for cancel in [false, true] {
        let root = tempfile::tempdir()?;
        let clock = Arc::new(ManualClock::new(1_000));
        let (catalog, view) = setup_with_clock(root.path(), clock.clone())?;
        let session = catalog.open_read_session()?;
        let ticket = ticket(&catalog, session.as_ref(), &view)?;
        let request = quanta_index_core::RequestBudgetV1::for_duration(if cancel {
            Duration::from_secs(5)
        } else {
            Duration::from_millis(100)
        });
        let budget = WorkBudget::with_request(
            WorkLimits {
                reads: 1,
                writes: 1,
                bytes: 1,
                mutations: 1,
            },
            request,
        );
        let held = catalog.lock()?;
        clock.set(1_001);
        std::thread::scope(|scope| -> TestResult {
            let (send, receive) = std::sync::mpsc::channel();
            let catalog = &catalog;
            let session_ref = session.as_ref();
            let budget_ref = &budget;
            let worker = scope.spawn(move || {
                let result =
                    start(catalog, session_ref, ticket, budget_ref).map(CatalogReadAttempt::abort);
                let _sent = send.send(result);
            });
            let queued = wait_pending(session_ref, 1);
            if queued.is_err() {
                drop(held);
                return queued;
            }
            if cancel {
                budget.request().cancel_handle().cancel();
            }
            let result = receive.recv_timeout(Duration::from_secs(2));
            drop(held);
            check_eq!(
                result?,
                Err(if cancel {
                    IndexError::Cancelled
                } else {
                    IndexError::ResourceLimited
                })
            );
            worker
                .join()
                .map_err(|_panic| "interrupted worker panicked")?;
            Ok(())
        })?;
        wait_pending(session.as_ref(), 0)?;
        check_eq!(floor(&catalog)?, 1_000);
        check_eq!(pin_count(&catalog)?, 1);
        catalog.release_ticket(session.as_ref(), ticket)?;
    }
    Ok(())
}

#[derive(Clone, Copy)]
enum QueuePressure {
    Deadline,
    Cancellation,
    CapacityAvailable,
}

fn assert_full_pending_queue_keeps_request_authority(pressure: QueuePressure) -> TestResult {
    use quanta_index_core::index::WorkBudgetDenial;

    let root = tempfile::tempdir()?;
    let clock = Arc::new(ManualClock::new(1_000));
    let (catalog, view) = setup_with_clock(root.path(), clock.clone())?;
    let session = catalog.open_read_session()?;
    let ticket = ticket(&catalog, session.as_ref(), &view)?;
    clock.set(1_001);
    std::thread::scope(|scope| -> TestResult {
        let held = catalog.lock()?;
        let mut workers = Vec::new();
        for _ in 0..64 {
            let catalog = &catalog;
            let session = session.as_ref();
            workers.push(scope.spawn(move || -> Result<(), IndexError> {
                let budget = admission_budget();
                start(catalog, session, ticket, &budget)?.abort();
                Ok(())
            }));
        }
        let queued = wait_pending(session.as_ref(), 64);
        if queued.is_err() {
            drop(held);
            return queued;
        }
        let (send, receive) = std::sync::mpsc::channel();
        let (started_send, started_receive) = std::sync::mpsc::channel();
        let catalog_ref = &catalog;
        let session_ref = session.as_ref();
        let extra = scope.spawn(move || {
            let budget = WorkBudget::with_request(
                WorkLimits {
                    reads: 100,
                    writes: 100,
                    bytes: 4096,
                    mutations: 100,
                },
                quanta_index_core::RequestBudgetV1::for_duration(match pressure {
                    QueuePressure::Deadline => Duration::from_millis(500),
                    QueuePressure::Cancellation | QueuePressure::CapacityAvailable => {
                        Duration::from_secs(5)
                    }
                }),
            );
            let _sent = started_send.send(budget.request().cancel_handle());
            let result =
                start(catalog_ref, session_ref, ticket, &budget).map(CatalogReadAttempt::abort);
            let _sent = send.send((result, budget.first_denial()));
        });
        let cancel = match started_receive.recv_timeout(Duration::from_secs(1)) {
            Ok(cancel) => cancel,
            Err(_error) => {
                drop(held);
                return Err("extra request did not start".into());
            }
        };
        // A full queue must wait rather than synthesize an authority failure.
        let early = receive.recv_timeout(Duration::from_millis(30));
        let result = if matches!(pressure, QueuePressure::CapacityAvailable) {
            drop(held);
            receive.recv_timeout(Duration::from_secs(2))
        } else {
            if matches!(pressure, QueuePressure::Cancellation) {
                cancel.cancel();
            }
            let result = receive.recv_timeout(Duration::from_secs(2));
            drop(held);
            result
        };
        if result.is_err() {
            cancel.cancel();
        }
        extra
            .join()
            .map_err(|_panic| "extra queue worker panicked")?;
        for worker in workers {
            worker
                .join()
                .map_err(|_panic| "bounded worker panicked")??;
        }
        check!(matches!(
            early,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout)
        ));
        let (result, denial) = result?;
        match pressure {
            QueuePressure::Deadline => {
                check_eq!(result, Err(IndexError::ResourceLimited));
                check_eq!(denial, Some(WorkBudgetDenial::Deadline));
            }
            QueuePressure::Cancellation => {
                check_eq!(result, Err(IndexError::Cancelled));
                check_eq!(denial, Some(WorkBudgetDenial::Cancelled));
            }
            QueuePressure::CapacityAvailable => {
                result?;
                check_eq!(denial, None);
            }
        }
        Ok(())
    })?;
    wait_pending(session.as_ref(), 0)?;
    check_eq!(pin_count(&catalog)?, 1);
    start(&catalog, session.as_ref(), ticket, &admission_budget())?.finalize()?;
    check_eq!(floor(&catalog)?, 1_001);
    catalog.release_ticket(session.as_ref(), ticket)?;
    check_eq!(pin_count(&catalog)?, 0);
    Ok(())
}

#[test]
fn read_attempt_full_pending_queue_waits_under_original_deadline() -> TestResult {
    assert_full_pending_queue_keeps_request_authority(QueuePressure::Deadline)
}

#[test]
fn read_attempt_full_pending_queue_cancellation_preserves_pin_and_denial() -> TestResult {
    assert_full_pending_queue_keeps_request_authority(QueuePressure::Cancellation)
}

#[test]
fn read_attempt_full_pending_queue_capacity_opening_completes() -> TestResult {
    assert_full_pending_queue_keeps_request_authority(QueuePressure::CapacityAvailable)
}

#[test]
fn read_attempt_authenticates_catalog_and_session_before_queuing() -> TestResult {
    let root = tempfile::tempdir()?;
    let foreign_root = tempfile::tempdir()?;
    let clock = Arc::new(ManualClock::new(1_000));
    let (catalog, view) = setup_with_clock(root.path(), clock.clone())?;
    let (foreign, _) = setup_with_clock(foreign_root.path(), clock)?;
    let session = catalog.open_read_session()?;
    let other_session = catalog.open_read_session()?;
    let ticket = ticket(&catalog, session.as_ref(), &view)?;
    let budget = admission_budget();
    check!(matches!(
        start(&foreign, session.as_ref(), ticket, &budget),
        Err(IndexError::InvalidInput(_))
    ));
    check!(matches!(
        start(&catalog, other_session.as_ref(), ticket, &budget),
        Err(IndexError::SnapshotUnavailable)
    ));
    wait_pending(session.as_ref(), 0)?;
    wait_pending(other_session.as_ref(), 0)?;
    catalog.release_ticket(session.as_ref(), ticket)?;
    Ok(())
}

#[test]
fn read_attempt_group_keeps_expired_pin_failure_separate_from_live_pin_success() -> TestResult {
    let root = tempfile::tempdir()?;
    let clock = Arc::new(ManualClock::new(1_000));
    let (catalog, view) = setup_with_clock(root.path(), clock.clone())?;
    let session = catalog.open_read_session()?;
    let live = ticket(&catalog, session.as_ref(), &view)?;
    let expired = catalog
        .acquire_ticket(
            session.as_ref(),
            ViewSelection::Exact(view.identity),
            Duration::from_millis(1),
            &admission_budget(),
        )?
        .lease;
    clock.set(1_001);
    let held = catalog.lock()?;
    std::thread::scope(|scope| -> TestResult {
        let catalog = &catalog;
        let session_ref = session.as_ref();
        let failed = scope.spawn(move || {
            let budget = admission_budget();
            start(catalog, session_ref, expired, &budget).map(CatalogReadAttempt::abort)
        });
        let succeeded = scope.spawn(move || {
            let budget = admission_budget();
            start(catalog, session_ref, live, &budget).map(CatalogReadAttempt::abort)
        });
        let queued = wait_pending(session_ref, 2);
        drop(held);
        queued?;
        check_eq!(
            failed
                .join()
                .map_err(|_panic| "expired pin worker panicked")?,
            Err(IndexError::SnapshotUnavailable)
        );
        succeeded
            .join()
            .map_err(|_panic| "live pin worker panicked")??;
        Ok(())
    })?;
    check_eq!(floor(&catalog)?, 1_001);
    check_eq!(pin_count(&catalog)?, 2);
    catalog.release_ticket(session.as_ref(), expired)?;
    catalog.release_ticket(session.as_ref(), live)?;
    check_eq!(pin_count(&catalog)?, 0);
    Ok(())
}

struct MemoryAudit {
    bytes: Arc<AtomicU64>,
    retained: Arc<AtomicUsize>,
    refuse: bool,
}

struct DelayedGroupMemory {
    first: AtomicBool,
    entered: std::sync::mpsc::Sender<()>,
    proceed: Mutex<std::sync::mpsc::Receiver<()>>,
    audit: MemoryAudit,
}

struct CappedMemoryAudit {
    gate: Mutex<()>,
    limit: Arc<AtomicU64>,
    audit: MemoryAudit,
}

impl quanta_index_core::index::MemoryScopePort for CappedMemoryAudit {
    fn reserve(&self, bytes: u64, lifetime: MemoryLifetime) -> Result<MemoryLease, IndexError> {
        let guard = self
            .gate
            .lock()
            .map_err(|error| IndexError::Storage(error.to_string()))?;
        let next = self
            .audit
            .bytes
            .load(Ordering::SeqCst)
            .checked_add(bytes)
            .ok_or(IndexError::ResourceLimited)?;
        if next > self.limit.load(Ordering::SeqCst) {
            return Err(IndexError::ResourceLimited);
        }
        let lease =
            quanta_index_core::index::MemoryScopePort::reserve(&self.audit, bytes, lifetime)?;
        drop(guard);
        Ok(lease)
    }
}

impl quanta_index_core::index::MemoryScopePort for DelayedGroupMemory {
    fn reserve(&self, bytes: u64, lifetime: MemoryLifetime) -> Result<MemoryLease, IndexError> {
        if self.first.swap(false, Ordering::SeqCst) {
            self.entered
                .send(())
                .map_err(|error| IndexError::Storage(error.to_string()))?;
            self.proceed
                .lock()
                .map_err(|error| IndexError::Storage(error.to_string()))?
                .recv_timeout(Duration::from_secs(5))
                .map_err(|error| IndexError::Storage(error.to_string()))?;
        }
        quanta_index_core::index::MemoryScopePort::reserve(&self.audit, bytes, lifetime)
    }
}

fn assert_group_initialization_wait_keeps_original_request(cancelled: bool) -> TestResult {
    use quanta_index_core::index::WorkBudgetDenial;

    let root = tempfile::tempdir()?;
    let clock = Arc::new(ManualClock::new(1_000));
    let (catalog, view) = setup_with_clock(root.path(), clock)?;
    let session = catalog.open_read_session()?;
    let ticket = ticket(&catalog, session.as_ref(), &view)?;
    let bytes = Arc::new(AtomicU64::new(0));
    let (entered_send, entered_receive) = std::sync::mpsc::channel();
    let (proceed_send, proceed_receive) = std::sync::mpsc::channel();
    let memory = quanta_index_core::index::MemoryScope::new(Arc::new(DelayedGroupMemory {
        first: AtomicBool::new(true),
        entered: entered_send,
        proceed: Mutex::new(proceed_receive),
        audit: MemoryAudit {
            bytes: Arc::clone(&bytes),
            retained: Arc::new(AtomicUsize::new(0)),
            refuse: false,
        },
    }));
    std::thread::scope(|scope| -> TestResult {
        let catalog_ref = &catalog;
        let session_ref = session.as_ref();
        let initializer = scope.spawn(move || -> Result<(), IndexError> {
            let mut budget = admission_budget();
            budget.bind_memory_scope(Some(memory));
            start(catalog_ref, session_ref, ticket, &budget)?.abort();
            Ok(())
        });
        let entered = entered_receive.recv_timeout(Duration::from_secs(1));
        if entered.is_err() {
            let _released = proceed_send.send(());
            return Err("group initializer did not reach memory admission".into());
        }
        let (started_send, started_receive) = std::sync::mpsc::channel();
        let (result_send, result_receive) = std::sync::mpsc::channel();
        let waiter = scope.spawn(move || {
            let budget = WorkBudget::with_request(
                WorkLimits {
                    reads: 100,
                    writes: 100,
                    bytes: 4096,
                    mutations: 100,
                },
                quanta_index_core::RequestBudgetV1::for_duration(if cancelled {
                    Duration::from_secs(5)
                } else {
                    Duration::from_millis(100)
                }),
            );
            let _started = started_send.send(budget.request().cancel_handle());
            let result =
                start(catalog_ref, session_ref, ticket, &budget).map(CatalogReadAttempt::abort);
            let _sent = result_send.send((result, budget.first_denial()));
        });
        let cancel = started_receive.recv_timeout(Duration::from_secs(1));
        if let Ok(cancel) = &cancel
            && cancelled
        {
            cancel.cancel();
        }
        // This result must arrive while the other request still owns the
        // initialization mutex inside a live memory-admission call.
        let result = result_receive.recv_timeout(Duration::from_secs(1));
        let _released = proceed_send.send(());
        if let Ok(cancel) = cancel {
            cancel.cancel();
        }
        initializer
            .join()
            .map_err(|_panic| "group initializer panicked")??;
        waiter.join().map_err(|_panic| "group waiter panicked")?;
        let (result, denial) = result.map_err(|error| {
            format!("request did not finish while group initialization was held: {error}")
        })?;
        check_eq!(
            result,
            Err(if cancelled {
                IndexError::Cancelled
            } else {
                IndexError::ResourceLimited
            })
        );
        check_eq!(
            denial,
            Some(if cancelled {
                WorkBudgetDenial::Cancelled
            } else {
                WorkBudgetDenial::Deadline
            })
        );
        Ok(())
    })?;
    wait_pending(session.as_ref(), 0)?;
    start(&catalog, session.as_ref(), ticket, &admission_budget())?.finalize()?;
    catalog.release_ticket(session.as_ref(), ticket)?;
    check_eq!(pin_count(&catalog)?, 0);
    drop(session);
    check_eq!(bytes.load(Ordering::SeqCst), 0);
    Ok(())
}

#[test]
fn read_attempt_group_initialization_mutex_wait_respects_original_deadline() -> TestResult {
    assert_group_initialization_wait_keeps_original_request(false)
}

#[test]
fn read_attempt_group_initialization_mutex_wait_respects_original_cancellation() -> TestResult {
    assert_group_initialization_wait_keeps_original_request(true)
}

fn assert_observation_metadata_wait_respects_budget(queued: bool, cancelled: bool) -> TestResult {
    use quanta_index_core::index::WorkBudgetDenial;

    let root = tempfile::tempdir()?;
    let clock = Arc::new(ManualClock::new(1_000));
    let (catalog, view) = setup_with_clock(root.path(), clock.clone())?;
    let session = catalog.open_read_session()?;
    let ticket = ticket(&catalog, session.as_ref(), &view)?;
    let bytes = Arc::new(AtomicU64::new(0));
    let memory = quanta_index_core::index::MemoryScope::new(Arc::new(MemoryAudit {
        bytes: Arc::clone(&bytes),
        retained: Arc::new(AtomicUsize::new(0)),
        refuse: false,
    }));
    let mut initial = admission_budget();
    initial.bind_memory_scope(Some(memory.clone()));
    start(&catalog, session.as_ref(), ticket, &initial)?.abort();
    let baseline_bytes = bytes.load(Ordering::SeqCst);
    let group = concrete(session.as_ref())?
        .read_observations
        .lock()
        .map_err(|_error| "group slot poisoned")?
        .as_ref()
        .cloned()
        .ok_or("group missing")?;
    clock.set(1_001);
    std::thread::scope(|scope| -> TestResult {
        let writer = if queued { Some(catalog.lock()?) } else { None };
        let catalog_ref = &catalog;
        let session_ref = session.as_ref();
        let leader_memory = memory.clone();
        let leader = queued.then(|| {
            scope.spawn(move || -> Result<(), IndexError> {
                let mut budget = admission_budget();
                budget.bind_memory_scope(Some(leader_memory));
                start(catalog_ref, session_ref, ticket, &budget)?.abort();
                Ok(())
            })
        });
        if queued {
            let entered = wait_pending(session_ref, 1);
            if entered.is_err() {
                drop(writer);
                return entered;
            }
        }
        let mut held = if queued {
            None
        } else {
            Some(group.pending.lock().map_err(|_error| "group poisoned")?)
        };
        let (started_send, started_receive) = std::sync::mpsc::channel();
        let (send, receive) = std::sync::mpsc::channel();
        let waiter_memory = memory.clone();
        let waiter = scope.spawn(move || {
            let mut budget = WorkBudget::with_request(
                WorkLimits {
                    reads: 100,
                    writes: 100,
                    bytes: 4096,
                    mutations: 100,
                },
                quanta_index_core::RequestBudgetV1::for_duration(if cancelled {
                    Duration::from_secs(10)
                } else if queued {
                    Duration::from_secs(2)
                } else {
                    Duration::from_millis(100)
                }),
            );
            budget.bind_memory_scope(Some(waiter_memory));
            let _started = started_send.send(budget.request().cancel_handle());
            let result =
                start(catalog_ref, session_ref, ticket, &budget).map(CatalogReadAttempt::abort);
            let _sent = send.send((result, budget.first_denial()));
        });
        let cancel = match started_receive.recv_timeout(Duration::from_secs(1)) {
            Ok(cancel) => cancel,
            Err(_error) => {
                drop(held);
                drop(writer);
                return Err("metadata waiter did not start".into());
            }
        };
        if queued {
            let entered = wait_pending(session_ref, 2);
            if entered.is_err() {
                cancel.cancel();
                drop(writer);
                return entered;
            }
            held = Some(group.pending.lock().map_err(|_error| "group poisoned")?);
        }
        if cancelled {
            cancel.cancel();
        }
        // A paused coordinator cannot extend the original request's budget.
        // In the queued case this also covers condition-variable reacquisition.
        let reply = receive.recv_timeout(Duration::from_secs(3));
        drop(held);
        drop(writer);
        cancel.cancel();
        waiter.join().map_err(|_panic| "metadata waiter panicked")?;
        if let Some(leader) = leader {
            leader
                .join()
                .map_err(|_panic| "metadata leader panicked")??;
        }
        let (result, denial) = reply
            .map_err(|error| format!("request stayed blocked on coordinator metadata: {error}"))?;
        check_eq!(
            result,
            Err(if cancelled {
                IndexError::Cancelled
            } else {
                IndexError::ResourceLimited
            })
        );
        check_eq!(
            denial,
            Some(if cancelled {
                WorkBudgetDenial::Cancelled
            } else {
                WorkBudgetDenial::Deadline
            })
        );
        Ok(())
    })?;
    wait_pending(session.as_ref(), 0)?;
    check_eq!(bytes.load(Ordering::SeqCst), baseline_bytes);
    check_eq!(floor(&catalog)?, if queued { 1_001 } else { 1_000 });
    start(&catalog, session.as_ref(), ticket, &initial)?.finalize()?;
    check_eq!(pin_count(&catalog)?, 1);
    catalog.release_ticket(session.as_ref(), ticket)?;
    drop(group);
    drop(session);
    check_eq!(bytes.load(Ordering::SeqCst), 0);
    Ok(())
}

#[test]
fn read_attempt_observation_metadata_admission_respects_deadline() -> TestResult {
    assert_observation_metadata_wait_respects_budget(false, false)
}

#[test]
fn read_attempt_observation_metadata_queued_wait_respects_deadline() -> TestResult {
    assert_observation_metadata_wait_respects_budget(true, false)
}

#[test]
fn read_attempt_observation_metadata_queued_wait_respects_cancellation() -> TestResult {
    assert_observation_metadata_wait_respects_budget(true, true)
}

#[test]
fn read_attempt_abandoned_full_queue_is_reclaimed_for_reopened_catalog() -> TestResult {
    use quanta_index_core::index::WorkBudgetDenial;

    let root = tempfile::tempdir()?;
    let clock = Arc::new(ManualClock::new(1_000));
    let (catalog, view) = setup_with_clock(root.path(), clock.clone())?;
    let session = catalog.open_read_session()?;
    let ticket = ticket(&catalog, session.as_ref(), &view)?;
    let bytes = Arc::new(AtomicU64::new(0));
    let limit = Arc::new(AtomicU64::new(u64::MAX));
    let memory = quanta_index_core::index::MemoryScope::new(Arc::new(CappedMemoryAudit {
        gate: Mutex::new(()),
        limit: Arc::clone(&limit),
        audit: MemoryAudit {
            bytes: Arc::clone(&bytes),
            retained: Arc::new(AtomicUsize::new(0)),
            refuse: false,
        },
    }));
    let mut initial = admission_budget();
    initial.bind_memory_scope(Some(memory.clone()));
    start(&catalog, session.as_ref(), ticket, &initial)?.abort();
    let baseline_bytes = bytes.load(Ordering::SeqCst);
    let group = concrete(session.as_ref())?
        .read_observations
        .lock()
        .map_err(|_error| "group slot poisoned")?
        .as_ref()
        .cloned()
        .ok_or("group missing")?;
    clock.set(1_001);
    std::thread::scope(|scope| -> TestResult {
        let writer = catalog.lock()?;
        let (send, receive) = std::sync::mpsc::channel();
        let mut workers = Vec::new();
        for _ in 0..64 {
            let catalog = &catalog;
            let session = session.as_ref();
            let memory = memory.clone();
            let send = send.clone();
            workers.push(scope.spawn(move || {
                let mut budget = WorkBudget::with_request(
                    WorkLimits {
                        reads: 100,
                        writes: 100,
                        bytes: 4096,
                        mutations: 100,
                    },
                    quanta_index_core::RequestBudgetV1::for_duration(Duration::from_secs(3)),
                );
                budget.bind_memory_scope(Some(memory));
                let result =
                    start(catalog, session, ticket, &budget).map(CatalogReadAttempt::abort);
                let _sent = send.send((result, budget.first_denial()));
            }));
        }
        drop(send);
        let queued = wait_pending(session.as_ref(), 64);
        if queued.is_err() {
            drop(writer);
            return queued;
        }
        let held = group.pending.lock().map_err(|_error| "group poisoned")?;
        let mut replies = Vec::new();
        for _ in 0..64 {
            match receive.recv_timeout(Duration::from_secs(5)) {
                Ok(reply) => replies.push(reply),
                Err(_error) => break,
            }
        }
        drop(held);
        drop(writer);
        for worker in workers {
            worker
                .join()
                .map_err(|_panic| "abandoned waiter panicked")?;
        }
        check_eq!(replies.len(), 64);
        for (result, denial) in replies {
            check_eq!(result, Err(IndexError::ResourceLimited));
            check_eq!(denial, Some(WorkBudgetDenial::Deadline));
        }
        Ok(())
    })?;
    check!(!group.active.load(Ordering::Acquire));
    wait_pending(session.as_ref(), 64)?;
    check!(bytes.load(Ordering::SeqCst) > baseline_bytes);
    // No new bytes fit until abandoned request data is actually reclaimed.
    // All worker admissions have ended before this cap is lowered.
    limit.store(bytes.load(Ordering::SeqCst), Ordering::SeqCst);
    check_eq!(floor(&catalog)?, 1_000);
    // The expired queue entries belong to the old Catalog object. They may be
    // retired as request data, but cannot validate time under the new object's
    // clock. The new request must obtain its own live durable observation.
    let reopened = SqliteCatalog::open_with_clock(root.path(), Duration::from_secs(1), clock)?;
    let mut next = WorkBudget::with_request(
        WorkLimits {
            reads: 100,
            writes: 100,
            bytes: 4096,
            mutations: 100,
        },
        quanta_index_core::RequestBudgetV1::for_duration(Duration::from_secs(2)),
    );
    next.bind_memory_scope(Some(memory));
    start(&reopened, session.as_ref(), ticket, &next)?.finalize()?;
    wait_pending(session.as_ref(), 0)?;
    check_eq!(floor(&reopened)?, 1_001);
    check_eq!(bytes.load(Ordering::SeqCst), baseline_bytes);
    reopened.release_ticket(session.as_ref(), ticket)?;
    check_eq!(pin_count(&reopened)?, 0);
    drop(group);
    drop(session);
    check_eq!(bytes.load(Ordering::SeqCst), 0);
    Ok(())
}

impl quanta_index_core::index::MemoryLeaseOwner for MemoryAudit {
    fn release(&self, bytes: u64) {
        let _previous = self.bytes.fetch_sub(bytes, Ordering::SeqCst);
    }
}

impl quanta_index_core::index::MemoryScopePort for MemoryAudit {
    fn reserve(&self, bytes: u64, lifetime: MemoryLifetime) -> Result<MemoryLease, IndexError> {
        if self.refuse {
            return Err(IndexError::ResourceLimited);
        }
        let _previous = self.bytes.fetch_add(bytes, Ordering::SeqCst);
        if lifetime == MemoryLifetime::Retained {
            let _previous = self.retained.fetch_add(1, Ordering::SeqCst);
        }
        Ok(MemoryLease::new(
            bytes,
            Arc::new(Self {
                bytes: Arc::clone(&self.bytes),
                retained: Arc::clone(&self.retained),
                refuse: false,
            }),
        ))
    }
}

#[derive(Clone, Copy)]
enum BatchFailure {
    Begin,
    ClockRead,
    ClockWrite,
    Commit,
    CancelAtCommit,
}

fn assert_failed_batch_settles_followers_and_recovers(fault: BatchFailure) -> TestResult {
    use rusqlite::hooks::{AuthAction, AuthContext, Authorization, TransactionOperation};

    let root = tempfile::tempdir()?;
    let clock = Arc::new(ManualClock::new(1_000));
    let (catalog, view) = setup_with_clock(root.path(), clock.clone())?;
    let session = catalog.open_read_session()?;
    let ticket = ticket(&catalog, session.as_ref(), &view)?;
    let bytes = Arc::new(AtomicU64::new(0));
    let memory = quanta_index_core::index::MemoryScope::new(Arc::new(MemoryAudit {
        bytes: Arc::clone(&bytes),
        retained: Arc::new(AtomicUsize::new(0)),
        refuse: false,
    }));
    let mut initial_budget = admission_budget();
    initial_budget.bind_memory_scope(Some(memory.clone()));
    start(&catalog, session.as_ref(), ticket, &initial_budget)?.abort();
    // Only explicitly funded per-session metadata remains between requests.
    let metadata_bytes = bytes.load(Ordering::SeqCst);
    check!(metadata_bytes > 0);
    let requests = [
        quanta_index_core::RequestBudgetV1::for_duration(Duration::from_secs(30)),
        quanta_index_core::RequestBudgetV1::for_duration(Duration::from_secs(30)),
    ];
    let injected = Arc::new(AtomicUsize::new(0));
    clock.set(1_001);
    std::thread::scope(|scope| -> TestResult {
        let mut held = catalog.lock()?;
        match fault {
            BatchFailure::Begin => {
                let injected = Arc::clone(&injected);
                held.authorizer(Some(move |context: AuthContext<'_>| {
                    if matches!(
                        context.action,
                        AuthAction::Transaction {
                            operation: TransactionOperation::Begin
                        }
                    ) {
                        let _previous = injected.fetch_add(1, Ordering::SeqCst);
                        Authorization::Deny
                    } else {
                        Authorization::Allow
                    }
                }))?;
            }
            BatchFailure::ClockRead => {
                let changed = held.execute(
                    "UPDATE native_work_clock_v1 SET row_digest=zeroblob(32) WHERE singleton=1",
                    [],
                )?;
                check_eq!(changed, 1);
            }
            BatchFailure::ClockWrite => {
                held.execute_batch("CREATE TEMP TRIGGER refuse_read_attempt_clock BEFORE UPDATE ON main.native_work_clock_v1
                    BEGIN SELECT RAISE(ABORT, 'injected read attempt clock write failure'); END;")?;
            }
            BatchFailure::Commit => {
                let injected = Arc::clone(&injected);
                held.commit_hook(Some(move || {
                    let _previous = injected.fetch_add(1, Ordering::SeqCst);
                    true
                }))?;
            }
            BatchFailure::CancelAtCommit => {
                let handles = requests
                    .each_ref()
                    .map(quanta_index_core::RequestBudgetV1::cancel_handle);
                let injected = Arc::clone(&injected);
                held.commit_hook(Some(move || {
                    let _previous = injected.fetch_add(1, Ordering::SeqCst);
                    for handle in &handles {
                        handle.cancel();
                    }
                    false
                }))?;
            }
        }
        let (send, receive) = std::sync::mpsc::channel();
        let mut workers = Vec::new();
        for request in &requests {
            let catalog = &catalog;
            let session = session.as_ref();
            let request = request.clone();
            let memory = memory.clone();
            let send = send.clone();
            workers.push(scope.spawn(move || {
                let mut budget = WorkBudget::with_request(
                    WorkLimits {
                        reads: 100,
                        writes: 100,
                        bytes: 4096,
                        mutations: 100,
                    },
                    request,
                );
                budget.bind_memory_scope(Some(memory));
                let result =
                    start(catalog, session, ticket, &budget).map(CatalogReadAttempt::abort);
                let _sent = send.send(result);
            }));
        }
        drop(send);
        let queued = wait_pending(session.as_ref(), 2);
        drop(held);
        if queued.is_err() {
            for request in &requests {
                request.cancel_handle().cancel();
            }
            return queued;
        }
        // Both must settle well before their 30-second request deadlines. If
        // not, cancel them before scoped join so the regression cannot hang.
        let first = receive.recv_timeout(Duration::from_secs(2));
        let second = receive.recv_timeout(Duration::from_secs(2));
        if first.is_err() || second.is_err() {
            for request in &requests {
                request.cancel_handle().cancel();
            }
        }
        for worker in workers {
            worker
                .join()
                .map_err(|_panic| "failed batch worker panicked")?;
        }
        for result in [first?, second?] {
            match fault {
                BatchFailure::CancelAtCommit => check_eq!(result, Err(IndexError::Cancelled)),
                BatchFailure::ClockRead => check!(
                    matches!(result, Err(IndexError::Storage(message)) if message == "native work clock digest mismatch")
                ),
                BatchFailure::ClockWrite => check!(
                    matches!(result, Err(IndexError::Storage(message)) if message.contains("injected read attempt clock write failure"))
                ),
                BatchFailure::Begin | BatchFailure::Commit => {
                    check!(matches!(result, Err(IndexError::Storage(_))));
                }
            }
        }
        Ok(())
    })?;
    wait_pending(session.as_ref(), 0)?;
    {
        let slot = concrete(session.as_ref())?
            .read_observations
            .lock()
            .map_err(|_error| "group slot poisoned")?;
        let group = slot.as_ref().ok_or("group missing after failure")?;
        check!(!group.active.load(Ordering::Acquire));
        drop(slot);
    }
    check_eq!(bytes.load(Ordering::SeqCst), metadata_bytes);
    check_eq!(pin_count(&catalog)?, 1);
    check_eq!(
        floor(&catalog)?,
        if matches!(fault, BatchFailure::CancelAtCommit) {
            1_001
        } else {
            1_000
        }
    );
    {
        let mut connection = catalog.lock()?;
        match fault {
            BatchFailure::Begin => {
                check_eq!(injected.load(Ordering::SeqCst), 1);
                connection.authorizer(None::<fn(AuthContext<'_>) -> Authorization>)?;
            }
            BatchFailure::ClockRead => {
                let checksum = fields_digest(b"qidx.native.workclock.v1", &(1_i64, 1_000_i64))?;
                let changed = connection.execute(
                    "UPDATE native_work_clock_v1 SET row_digest=?1 WHERE singleton=1",
                    params![checksum],
                )?;
                check_eq!(changed, 1);
            }
            BatchFailure::ClockWrite => {
                connection.execute_batch("DROP TRIGGER refuse_read_attempt_clock")?;
            }
            BatchFailure::Commit | BatchFailure::CancelAtCommit => {
                check_eq!(injected.load(Ordering::SeqCst), 1);
                connection.commit_hook(None::<fn() -> bool>)?;
            }
        }
    }
    clock.set(1_002);
    let mut retry_budget = admission_budget();
    retry_budget.bind_memory_scope(Some(memory));
    start(&catalog, session.as_ref(), ticket, &retry_budget)?.finalize()?;
    check_eq!(floor(&catalog)?, 1_002);
    check_eq!(bytes.load(Ordering::SeqCst), metadata_bytes);
    catalog.release_ticket(session.as_ref(), ticket)?;
    drop(session);
    check_eq!(bytes.load(Ordering::SeqCst), 0);
    Ok(())
}

#[test]
fn read_attempt_begin_failure_settles_follower_and_allows_next_attempt() -> TestResult {
    assert_failed_batch_settles_followers_and_recovers(BatchFailure::Begin)
}

#[test]
fn read_attempt_clock_read_failure_settles_follower_and_allows_next_attempt() -> TestResult {
    assert_failed_batch_settles_followers_and_recovers(BatchFailure::ClockRead)
}

#[test]
fn read_attempt_clock_write_failure_settles_follower_and_allows_next_attempt() -> TestResult {
    assert_failed_batch_settles_followers_and_recovers(BatchFailure::ClockWrite)
}

#[test]
fn read_attempt_commit_failure_settles_follower_and_allows_next_attempt() -> TestResult {
    assert_failed_batch_settles_followers_and_recovers(BatchFailure::Commit)
}

#[test]
fn read_attempt_commit_cancellation_settles_follower_and_allows_next_attempt() -> TestResult {
    assert_failed_batch_settles_followers_and_recovers(BatchFailure::CancelAtCommit)
}

#[test]
fn read_observation_refuses_unfunded_pin_scratch_before_advancing_clock() -> TestResult {
    let root = tempfile::tempdir()?;
    let clock = Arc::new(ManualClock::new(1_000));
    let (catalog, view) = setup_with_clock(root.path(), clock.clone())?;
    let session = catalog.open_read_session()?;
    let ticket = ticket(&catalog, session.as_ref(), &view)?;
    start(&catalog, session.as_ref(), ticket, &admission_budget())?.abort();
    clock.set(1_001);

    // The group already exists. Admit the attempt and the observation entry,
    // leaving no bytes for its copy of the persisted pin.
    let entry_bytes = std::mem::size_of::<Observation>() + std::mem::size_of::<[usize; 2]>() + 16;
    let limit = u64::try_from(std::mem::size_of::<ReadAttempt<'_>>() + entry_bytes)?;
    let bytes = Arc::new(AtomicU64::new(0));
    let limit = Arc::new(AtomicU64::new(limit));
    let memory = quanta_index_core::index::MemoryScope::new(Arc::new(CappedMemoryAudit {
        gate: Mutex::new(()),
        limit: Arc::clone(&limit),
        audit: MemoryAudit {
            bytes: Arc::clone(&bytes),
            retained: Arc::new(AtomicUsize::new(0)),
            refuse: false,
        },
    }));
    let mut work = admission_budget();
    work.bind_memory_scope(Some(memory.clone()));
    check!(matches!(
        start(&catalog, session.as_ref(), ticket, &work),
        Err(IndexError::ResourceLimited)
    ));
    check_eq!(
        work.first_denial(),
        Some(quanta_index_core::index::WorkBudgetDenial::Memory)
    );
    check_eq!(bytes.load(Ordering::SeqCst), 0);
    check_eq!(floor(&catalog)?, 1_000);
    check_eq!(pin_count(&catalog)?, 1);

    limit.store(u64::MAX, Ordering::SeqCst);
    let mut retry = admission_budget();
    retry.bind_memory_scope(Some(memory));
    start(&catalog, session.as_ref(), ticket, &retry)?.finalize()?;
    check_eq!(bytes.load(Ordering::SeqCst), 0);
    check_eq!(floor(&catalog)?, 1_001);
    catalog.release_ticket(session.as_ref(), ticket)?;
    Ok(())
}

#[test]
fn read_attempt_memory_admission_is_required_and_every_terminal_path_releases_charges() -> TestResult
{
    let root = tempfile::tempdir()?;
    let clock = Arc::new(ManualClock::new(1_000));
    let (catalog, view) = setup_with_clock(root.path(), clock.clone())?;
    for terminal in 0..5 {
        let session = catalog.open_read_session()?;
        let ticket = ticket(&catalog, session.as_ref(), &view)?;
        clock.set(1_000);
        let bytes = Arc::new(AtomicU64::new(0));
        let retained = Arc::new(AtomicUsize::new(0));
        let mut budget = admission_budget();
        budget.bind_memory_scope(Some(quanta_index_core::index::MemoryScope::new(Arc::new(
            MemoryAudit {
                bytes: Arc::clone(&bytes),
                retained: Arc::clone(&retained),
                refuse: terminal == 4,
            },
        ))));
        if terminal == 4 {
            check!(matches!(
                start(&catalog, session.as_ref(), ticket, &budget),
                Err(IndexError::ResourceLimited)
            ));
            check_eq!(retained.load(Ordering::SeqCst), 0);
        } else {
            let attempt = start(&catalog, session.as_ref(), ticket, &budget)?;
            check!(bytes.load(Ordering::SeqCst) > 0);
            check!(retained.load(Ordering::SeqCst) > 0);
            match terminal {
                0 => attempt.finalize()?,
                1 => attempt.abort(),
                2 => drop(attempt),
                _ => {
                    budget.request().cancel_handle().cancel();
                    check_eq!(attempt.finalize(), Err(IndexError::Cancelled));
                }
            }
        }
        wait_pending(session.as_ref(), 0)?;
        check_eq!(pin_count(&catalog)?, 1);
        catalog.release_ticket(session.as_ref(), ticket)?;
        drop(session);
        check_eq!(bytes.load(Ordering::SeqCst), 0);
    }
    Ok(())
}
fn id(value: u8) -> [u8; 16] {
    [value; 16]
}
fn digest_id(value: u8) -> Digest {
    Digest([value; 32])
}
fn object(value: u8, children: Vec<ObjectRef>) -> RegisteredObject {
    RegisteredObject::unscoped(ObjectRef(digest_id(value)), 1, children)
}
fn workspace() -> WorkspaceKey {
    WorkspaceKey {
        tenant: TenantId(id(1)),
        collection: CollectionId(id(2)),
        worktree: WorktreeId(id(3)),
        slot: SlotId(id(4)),
    }
}
fn view(
    sequence: u64,
    profile: ObjectRef,
    member: Option<ObjectRef>,
) -> Result<CatalogView, Box<dyn Error>> {
    let mut view = CatalogView {
        identity: ViewRef {
            workspace: workspace(),
            epoch: 7,
            sequence,
            digest: digest_id(0),
        },
        profile,
        source: SourceCut {
            owners: MapRoot(None),
            paths: MapRoot(None),
            frontiers: MapRoot(None),
        },
        projections: MapRoot(None),
        members: MapRoot(member),
        coverage: MapRoot(None),
    };
    view.identity.digest = view.identity_digest()?;
    Ok(view)
}

struct GenesisIdentity(RegisteredObject);

impl ImmutableObjectPort for GenesisIdentity {
    fn identify(
        &self,
        payload: &[u8],
        children: &[ObjectRef],
        budget: &mut WorkBudget,
    ) -> Result<RegisteredObject, IndexError> {
        budget.checkpoint()?;
        let expected = encode(&quanta_index_core::index::IndexProfile {
            instances: Vec::new(),
        })?;
        if payload != expected || !children.is_empty() {
            return Err(IndexError::InvalidInput(
                "genesis identity input differs from empty profile".into(),
            ));
        }
        Ok(self.0.clone())
    }

    fn put(
        &self,
        _payload: &[u8],
        _children: &[ObjectRef],
        _budget: &mut WorkBudget,
    ) -> Result<RegisteredObject, IndexError> {
        Err(IndexError::BackendViolation("identity-only fixture".into()))
    }

    fn install(
        &self,
        _payload: &[u8],
        _children: &[ObjectRef],
        _sink: &dyn quanta_index_core::index::ports::ObjectWriteSink,
        _budget: &mut WorkBudget,
    ) -> Result<RegisteredObject, IndexError> {
        Err(IndexError::BackendViolation("identity-only fixture".into()))
    }

    fn install_batch(
        &self,
        _inputs: &[quanta_index_core::index::ObjectBatchInput],
        _sink: &dyn quanta_index_core::index::ports::ObjectWriteSink,
        _budget: &mut WorkBudget,
    ) -> Result<quanta_index_core::index::RegisteredObjectBatch, IndexError> {
        Err(IndexError::BackendViolation("identity-only fixture".into()))
    }

    fn get(
        &self,
        _reference: ObjectRef,
        _budget: &mut WorkBudget,
    ) -> Result<quanta_index_core::index::StoredObject, IndexError> {
        Err(IndexError::BackendViolation("identity-only fixture".into()))
    }
}

fn setup_with_clock(
    root: &Path,
    clock: Arc<dyn CatalogClockPort>,
) -> Result<(SqliteCatalog, CatalogView), Box<dyn Error>> {
    let catalog = SqliteCatalog::open_with_clock(root, Duration::from_secs(1), clock)?;
    let profile = object(30, vec![]);
    let genesis = view(0, profile.reference, None)?;
    let stage = catalog.begin_bootstrap(
        genesis.identity.workspace,
        genesis.identity.epoch,
        &GenesisIdentity(profile.clone()),
        &mut admission_budget(),
    )?;
    catalog.reserve_bootstrap(stage, &profile)?;
    catalog.confirm_bootstrap(stage, &profile)?;
    let created = catalog.publish_bootstrap(stage, genesis.clone(), &mut admission_budget())?;
    check_eq!(created, genesis.identity);
    Ok((catalog, genesis))
}

fn admission_budget() -> WorkBudget {
    WorkBudget::with_request(
        WorkLimits {
            reads: 100_000,
            writes: 100_000,
            bytes: 32 * 1024 * 1024,
            mutations: 100_000,
        },
        quanta_index_core::RequestBudgetV1::for_duration(Duration::from_secs(30)),
    )
}
