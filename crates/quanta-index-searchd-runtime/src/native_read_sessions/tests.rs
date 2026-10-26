use std::{
    error::Error,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

use quanta_index_catalog::SqliteCatalog;
use quanta_index_contract::index::{
    CollectionId, Digest, SlotId, TenantId, ViewRef, WorkspaceKey, WorktreeId,
};
use quanta_index_contract::native_protocol::{
    NativeReadSessionId, NativeReadSessionState, NativeReadSessionStatus,
};
use quanta_index_core::{
    RequestBudgetV1,
    index::{NativeReadSettlementScope, WorkBudget, WorkLimits},
};
use quanta_index_kernel::{AdapterRegistry, Kernel, KernelServices};
use quanta_index_storage::{FileObjectStore, FileScratchSpace};

use super::{
    CLOSING_BIT, Entry, EntryState, IndexError, NativeReadSessionConfig, NativeReadSessionSweep,
    NativeReadSessions, PublicReleaseAttempt, RELEASE_WAITER, ViewSelection, keep_first,
    record_failure,
};

type TestResult<T = ()> = Result<T, Box<dyn Error>>;

macro_rules! check {
    ($condition:expr $(,)?) => {{
        if !($condition) {
            return Err(format!("condition failed: {}", stringify!($condition)).into());
        }
    }};
}

macro_rules! check_eq {
    ($left:expr, $right:expr $(,)?) => {{
        match (&$left, &$right) {
            (left, right) if !(*left == *right) => {
                return Err(format!(
                    "values differ: {} and {}; left: {left:?}, right: {right:?}",
                    stringify!($left),
                    stringify!($right)
                )
                .into());
            }
            _ => {}
        }
    }};
}

fn selection(epoch: u64) -> ViewSelection {
    ViewSelection::Latest {
        workspace: WorkspaceKey {
            tenant: TenantId([1; 16]),
            collection: CollectionId([2; 16]),
            worktree: WorktreeId([3; 16]),
            slot: SlotId([4; 16]),
        },
        epoch,
    }
}

fn budget() -> WorkBudget {
    WorkBudget::with_request(
        WorkLimits {
            reads: 1,
            writes: 1,
            bytes: 1,
            mutations: 1,
        },
        RequestBudgetV1::for_duration(Duration::from_secs(5)),
    )
}

fn registered_entry(
    registry: &NativeReadSessions,
    state: NativeReadSessionState,
) -> TestResult<Arc<Entry>> {
    let id = NativeReadSessionId([9; 32]);
    let selection = selection(1);
    let ViewSelection::Latest { workspace, epoch } = selection else {
        return Err("read-session fixture selection differs".into());
    };
    let entry = Arc::new(Entry {
        id,
        uid: 42,
        open_key: [7; 32],
        selection,
        lifecycle: AtomicUsize::new(0),
        inner: Mutex::new(EntryState {
            status: Some(NativeReadSessionStatus {
                session: id,
                view: ViewRef {
                    workspace,
                    epoch,
                    sequence: 1,
                    digest: Digest([5; 32]),
                },
                expires_at_ms: 1_000,
                revision: 1,
                state,
            }),
            ..EntryState::default()
        }),
    });
    let mut table = registry
        .table
        .lock()
        .map_err(|error| std::io::Error::other(format!("read-session table poisoned: {error}")))?;
    check!(table.by_id.insert(id, Arc::clone(&entry)).is_none());
    check!(table.by_open.insert((42, [7; 32]), id).is_none());
    drop(table);
    Ok(entry)
}

#[test]
fn config_rejects_unbounded_registry() -> TestResult {
    let default = NativeReadSessionConfig::default();
    check_eq!(default.global_limit, 64);
    check_eq!(default.per_uid_limit, 16);
    check_eq!(default.terminal_ttl, Duration::from_secs(300));
    check_eq!(default.sweep_limit, 8);
    check_eq!(NativeReadSessions::required_memory(&default)?, 266_240);

    for invalid in [
        NativeReadSessionConfig {
            global_limit: 0,
            ..default
        },
        NativeReadSessionConfig {
            per_uid_limit: 65,
            ..default
        },
        NativeReadSessionConfig {
            terminal_ttl: Duration::ZERO,
            ..default
        },
        NativeReadSessionConfig {
            sweep_limit: 65,
            ..default
        },
    ] {
        check!(matches!(
            NativeReadSessions::required_memory(&invalid),
            Err(IndexError::InvalidInput(_))
        ));
    }
    Ok(())
}

#[test]
fn failed_pin_does_not_leak_an_open_key_or_slot() -> TestResult {
    let registry = NativeReadSessions::new_unfunded_for_test(NativeReadSessionConfig {
        global_limit: 1,
        per_uid_limit: 1,
        sweep_limit: 1,
        ..NativeReadSessionConfig::default()
    })?;
    let key = [7; 32];
    let first = registry.open(42, key, selection(1), &budget(), |_selection| {
        Err(IndexError::Storage("first known no-ticket failure".into()))
    });
    check_eq!(
        first,
        Err(IndexError::Storage("first known no-ticket failure".into()))
    );
    let second = registry.open(42, key, selection(1), &budget(), |_selection| {
        Err(IndexError::Storage("second known no-ticket failure".into()))
    });
    check_eq!(
        second,
        Err(IndexError::Storage("second known no-ticket failure".into()))
    );
    Ok(())
}

#[test]
fn cancelled_open_after_real_pin_replays_one_ticket_and_releases_it() -> TestResult {
    let root = tempfile::tempdir()?;
    let objects = Arc::new(FileObjectStore::open(root.path().join("native-objects"))?);
    let catalog = Arc::new(SqliteCatalog::open(root.path(), Duration::from_secs(2))?);
    let kernel = Kernel::new(
        KernelServices::new(
            objects.clone(),
            objects,
            Arc::clone(&catalog),
            Arc::new(FileScratchSpace::open_standalone(
                root.path().join("scratch"),
                16 * 1024 * 1024 * 1024,
            )?),
            Duration::from_secs(30),
        )?,
        AdapterRegistry::new(),
    );
    let ViewSelection::Latest { workspace, epoch } = selection(1) else {
        return Err("read-session fixture selection differs".into());
    };
    let limits = WorkLimits {
        reads: 1_000_000,
        writes: 1_000_000,
        bytes: 512 * 1024 * 1024,
        mutations: 1_000_000,
    };
    let genesis = kernel.create_workspace(
        workspace,
        epoch,
        &mut WorkBudget::with_request(
            limits,
            RequestBudgetV1::for_duration(Duration::from_secs(30)),
        ),
    )?;
    let pin_count = || -> TestResult<i64> {
        Ok(rusqlite::Connection::open(catalog.path())?.query_row(
            "SELECT COUNT(*) FROM native_pin_v1",
            [],
            |row| row.get(0),
        )?)
    };
    check_eq!(pin_count()?, 0);
    let registry = NativeReadSessions::new_unfunded_for_test(NativeReadSessionConfig::default())?;
    let original = WorkBudget::with_request(
        limits,
        RequestBudgetV1::for_duration(Duration::from_secs(30)),
    );
    let pin_calls = AtomicUsize::new(0);
    let first = registry.open(42, [7; 32], selection(1), &original, |selected| {
        let _prior = pin_calls.fetch_add(1, Ordering::Relaxed);
        let ticket = kernel.pin_view(selected, &original)?;
        original.request().cancel_handle().cancel();
        Ok(ticket)
    });
    check!(matches!(first, Err(IndexError::Cancelled)));

    check_eq!(pin_count()?, 1);
    let retained_id = *registry
        .table
        .lock()
        .map_err(|error| std::io::Error::other(format!("read-session table poisoned: {error}")))?
        .by_open
        .get(&(42, [7; 32]))
        .ok_or("cancelled Open lost its idempotency key")?;
    let retained = registry.status(42, retained_id, &budget())?;
    check_eq!(retained.state, NativeReadSessionState::Active);
    let replay = registry.open(42, [7; 32], selection(1), &budget(), |_selected| {
        let _prior = pin_calls.fetch_add(1, Ordering::Relaxed);
        Err(IndexError::BackendViolation(
            "replay attempted a second Catalog pin".into(),
        ))
    })?;
    check_eq!(replay.view, genesis);
    check_eq!(replay, retained);
    check_eq!(replay.state, NativeReadSessionState::Active);
    check_eq!(pin_calls.load(Ordering::Relaxed), 1);
    check_eq!(pin_count()?, 1);

    let mut release_work = WorkBudget::with_request(
        limits,
        RequestBudgetV1::for_duration(Duration::from_secs(30)),
    );
    let (released, remaining) = registry.release(42, replay.session, &budget(), |ticket| {
        let target = ticket.view().identity;
        kernel
            .release_read_ticket(
                ticket,
                NativeReadSettlementScope {
                    workspace: target.workspace,
                    epoch: target.epoch,
                    target,
                },
                &mut release_work,
            )
            .map(|observation| observation.has_remaining_target_pins)
    })?;
    check_eq!(released.state, NativeReadSessionState::Released);
    check!(!remaining);
    check_eq!(pin_count()?, 0);
    Ok(())
}

#[test]
fn opening_reservation_rejects_other_work_without_creating_a_second_pin() -> TestResult {
    let registry = Arc::new(NativeReadSessions::new_unfunded_for_test(
        NativeReadSessionConfig {
            global_limit: 1,
            per_uid_limit: 1,
            sweep_limit: 1,
            ..NativeReadSessionConfig::default()
        },
    )?);
    let (entered_tx, entered_rx) = mpsc::channel();
    let (continue_tx, continue_rx) = mpsc::channel();
    let first_registry = Arc::clone(&registry);
    let worker = thread::spawn(move || {
        first_registry.open(42, [7; 32], selection(1), &budget(), |_selection| {
            entered_tx
                .send(())
                .map_err(|error| IndexError::Storage(error.to_string()))?;
            continue_rx
                .recv_timeout(Duration::from_secs(3))
                .map_err(|error| IndexError::Storage(error.to_string()))?;
            Err(IndexError::Storage("known no-ticket failure".into()))
        })
    });
    entered_rx.recv_timeout(Duration::from_secs(3))?;
    let mismatched = registry.open(42, [7; 32], selection(2), &budget(), |_selection| {
        Err(IndexError::BackendViolation(
            "mismatched selection reached pin".into(),
        ))
    });
    check_eq!(mismatched, Err(IndexError::Conflict));
    let extra = registry.open(43, [8; 32], selection(1), &budget(), |_selection| {
        Err(IndexError::BackendViolation(
            "full table reached pin".into(),
        ))
    });
    check_eq!(extra, Err(IndexError::ResourceLimited));
    continue_tx.send(())?;
    let first = worker
        .join()
        .map_err(|_panic| std::io::Error::other("open worker panicked"))?;
    check_eq!(
        first,
        Err(IndexError::Storage("known no-ticket failure".into()))
    );
    Ok(())
}

#[test]
fn first_release_failure_survives_later_request_cancellation() {
    let mut report = NativeReadSessionSweep::default();
    let first = IndexError::Storage("original Catalog release failed".into());
    record_failure(&mut report, first.clone());
    let interrupted = budget();
    interrupted.request().cancel_handle().cancel();
    assert_eq!(interrupted.checkpoint(), Err(IndexError::Cancelled));
    assert_eq!(keep_first(&report, interrupted.checkpoint()), Err(first));
}

#[test]
fn release_intent_blocks_new_query_before_waiting_and_survives_cancellation() -> TestResult {
    let registry = Arc::new(NativeReadSessions::new_unfunded_for_test(
        NativeReadSessionConfig::default(),
    )?);
    let entry = registered_entry(&registry, NativeReadSessionState::Active)?;
    let id = entry.id;

    let cancelled = budget();
    cancelled.request().cancel_handle().cancel();
    check_eq!(
        registry.release(42, id, &cancelled, |_ticket| Ok(false)),
        Err(IndexError::Cancelled)
    );
    check!(!entry.is_closing());

    // Holding the ticket mutex models an in-flight query. Release must
    // publish Closing without waiting for that query to finish.
    let guard = entry
        .inner
        .lock()
        .map_err(|error| std::io::Error::other(format!("read-session entry poisoned: {error}")))?;
    check_eq!(entry.require_active_query(&guard), Ok(()));
    let release_budget = budget();
    let cancel_release = release_budget.request().cancel_handle();
    let worker_registry = Arc::clone(&registry);
    let worker = thread::spawn(move || {
        worker_registry.release(42, id, &release_budget, |_ticket| {
            Err(IndexError::BackendViolation(
                "cancelled release reached Catalog".into(),
            ))
        })
    });
    let deadline = Instant::now()
        .checked_add(Duration::from_secs(3))
        .ok_or("read-session fixture deadline overflow")?;
    while !entry.is_closing() {
        if Instant::now() >= deadline {
            cancel_release.cancel();
            drop(guard);
            let _joined = worker.join();
            return Err("Release did not publish Closing before the query finished".into());
        }
        thread::sleep(Duration::from_millis(1));
    }
    let lifecycle_while_query_held = entry.lifecycle.load(Ordering::Acquire);
    let status_while_query_held = entry.status(&guard).map(|status| status.state);
    // The same post-lock predicate is used by with_ticket, closing the race
    // where a query passed its first check before Release published intent.
    let post_lock_query = entry.require_active_query(&guard);
    let new_query: Result<(), IndexError> = registry.with_ticket(42, id, &budget(), |_ticket| {
        Err(IndexError::BackendViolation(
            "new query reached the held ticket".into(),
        ))
    });
    cancel_release.cancel();
    let timed_out = worker.join().map_err(|_panic| "Release worker panicked")?;
    drop(guard);
    check_eq!(lifecycle_while_query_held, CLOSING_BIT | RELEASE_WAITER);
    check_eq!(status_while_query_held, Ok(NativeReadSessionState::Closing));
    check_eq!(post_lock_query, Err(IndexError::SnapshotUnavailable));
    check_eq!(new_query, Err(IndexError::SnapshotUnavailable));
    check_eq!(timed_out, Err(IndexError::Cancelled));
    check_eq!(entry.lifecycle.load(Ordering::Acquire), CLOSING_BIT);

    check_eq!(
        registry.status(42, id, &budget())?.state,
        NativeReadSessionState::Closing
    );
    check_eq!(
        registry.renew(42, id, 1, &budget()),
        Err(IndexError::SnapshotUnavailable)
    );
    let replay = registry.open(42, [7; 32], selection(1), &budget(), |_selection| {
        Err(IndexError::BackendViolation(
            "replay created a second Catalog ticket".into(),
        ))
    })?;
    check_eq!(replay.state, NativeReadSessionState::Closing);
    Ok(())
}

#[test]
fn maintenance_defers_closing_entry_while_public_release_owns_the_ack() -> TestResult {
    let registry = NativeReadSessions::new_unfunded_for_test(NativeReadSessionConfig::default())?;
    let entry = registered_entry(&registry, NativeReadSessionState::Active)?;
    // Repeat after Closing is already retained from a cancelled attempt.
    // A split Closing/waiter read could otherwise miss the second attempt.
    for _attempt in 0..2 {
        let public = PublicReleaseAttempt::begin(&entry)?;
        check_eq!(
            entry.lifecycle.load(Ordering::Acquire),
            CLOSING_BIT | RELEASE_WAITER
        );
        let round = registry.sweep(&budget(), |_ticket| {
            Err(IndexError::BackendViolation(
                "maintenance stole a live public Release".into(),
            ))
        })?;
        check_eq!(round.examined, 1);
        check_eq!(round.released, 0);
        check_eq!(round.failures, 0);
        drop(public);
        check_eq!(entry.lifecycle.load(Ordering::Acquire), CLOSING_BIT);
    }
    let guard = entry
        .inner
        .lock()
        .map_err(|error| std::io::Error::other(format!("entry poisoned: {error}")))?;
    check_eq!(entry.status(&guard)?.state, NativeReadSessionState::Closing);
    Ok(())
}

#[test]
fn maintenance_released_tombstone_cannot_replay_public_release_ack_or_settlement() -> TestResult {
    let registry = NativeReadSessions::new_unfunded_for_test(NativeReadSessionConfig::default())?;
    let entry = registered_entry(&registry, NativeReadSessionState::Released)?;
    let id = entry.id;
    {
        let mut guard = entry.inner.lock().map_err(|error| {
            std::io::Error::other(format!("read-session entry poisoned: {error}"))
        })?;
        guard.has_remaining_target_pins = Some(false);
        guard.public_release_ack = false;
    }
    check_eq!(
        registry.status(42, id, &budget())?.state,
        NativeReadSessionState::Released
    );
    check_eq!(
        registry.release(42, id, &budget(), |_ticket| {
            Err(IndexError::BackendViolation(
                "maintenance tombstone reached a new Catalog release".into(),
            ))
        }),
        Err(IndexError::SnapshotUnavailable)
    );
    let settlement: Result<(), IndexError> =
        registry.with_released_ticket(42, id, &budget(), |_ticket| {
            Err(IndexError::BackendViolation(
                "maintenance tombstone reached settlement".into(),
            ))
        });
    check_eq!(settlement, Err(IndexError::SnapshotUnavailable));
    {
        let mut guard = entry.inner.lock().map_err(|error| {
            std::io::Error::other(format!("read-session entry poisoned: {error}"))
        })?;
        guard.public_release_ack = true;
    }
    let committed = registry.release(42, id, &budget(), |_ticket| {
        Err(IndexError::BackendViolation(
            "committed ACK retried Catalog deletion".into(),
        ))
    })?;
    check_eq!(committed.0.state, NativeReadSessionState::Released);
    check!(!committed.1);
    Ok(())
}
