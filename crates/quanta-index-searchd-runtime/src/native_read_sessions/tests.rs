use std::{
    error::Error,
    sync::{
        Arc, Mutex, RwLock,
        atomic::{AtomicBool, AtomicUsize, Ordering},
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

use quanta_index_lexical::native::TextRankEntry;

use super::{
    CLOSING_BIT, Entry, EntryState, IndexError, NativeReadSessionConfig, NativeReadSessionSweep,
    NativeReadSessions, PublicReleaseAttempt, RELEASE_WAITER, TextOrderShare, ViewSelection,
    keep_first, record_failure,
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

#[path = "tests/read_attempt.rs"]
mod read_attempt;

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
    uid: u32,
    key: u8,
    state: NativeReadSessionState,
) -> TestResult<Arc<Entry>> {
    let id = NativeReadSessionId([key; 32]);
    let open_key = [key; 32];
    let selection = selection(1);
    let ViewSelection::Latest { workspace, epoch } = selection else {
        return Err("read-session fixture selection differs".into());
    };
    let entry = Arc::new(Entry {
        id,
        uid,
        open_key,
        selection,
        lifecycle: AtomicUsize::new(0),
        renew_waiters: AtomicUsize::new(0),
        uncertain: AtomicBool::new(false),
        terminal_since: std::sync::OnceLock::new(),
        inner: RwLock::new(EntryState {
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
        text_order: Mutex::new(None),
    });
    if state == NativeReadSessionState::Released {
        entry.mark_released();
    }
    let mut table = registry
        .table
        .lock()
        .map_err(|error| std::io::Error::other(format!("read-session table poisoned: {error}")))?;
    check!(table.by_id.insert(id, Arc::clone(&entry)).is_none());
    check!(table.by_open.insert((uid, open_key), id).is_none());
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
    // Every default session may keep the largest captured order: 4,096 rows
    // of one ranked entry each.
    let largest_order = u64::try_from(std::mem::size_of::<TextRankEntry>())?
        .checked_mul(4_096)
        .ok_or("order bytes overflow")?;
    let full_share = largest_order
        .checked_mul(64)
        .ok_or("order share overflow")?;
    check_eq!(default.text_order_memory, full_share);
    // The slots and table, plus the declared order share in the same grant.
    check_eq!(
        NativeReadSessions::required_memory(&default)?,
        2_101_248_u64
            .checked_add(full_share)
            .ok_or("registry grant overflow")?
    );
    check_eq!(
        NativeReadSessions::required_memory(&NativeReadSessionConfig {
            text_order_memory: 0,
            ..default
        })?,
        2_101_248
    );
    check_eq!(
        NativeReadSessions::required_memory(&NativeReadSessionConfig {
            text_order_memory: 1,
            ..default
        })?,
        2_101_249
    );

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
        // More than every session's largest order can never be used.
        NativeReadSessionConfig {
            text_order_memory: full_share.checked_add(1).ok_or("order share overflow")?,
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

/// Cached orders are charged to the declared share only: a charge the share
/// cannot fund is refused, and a dropped charge returns its bytes.
#[test]
fn text_order_share_funds_orders_up_to_its_declared_bytes() -> TestResult {
    let share = TextOrderShare::new(100);
    let first = share.charge(60).ok_or("60 of 100 bytes must be funded")?;
    check!(share.charge(41).is_none());
    let second = share.charge(40).ok_or("the last 40 bytes must be funded")?;
    check_eq!(share.used(), 100);
    check!(share.charge(1).is_none());
    drop(first);
    check_eq!(share.used(), 40);
    check!(share.charge(61).is_none());
    let third = share.charge(60).ok_or("returned bytes must fund again")?;
    drop((second, third));
    check_eq!(share.used(), 0);
    Ok(())
}

#[test]
fn failed_pin_does_not_leak_an_open_key_or_slot() -> TestResult {
    let registry = NativeReadSessions::new_unfunded_for_test(NativeReadSessionConfig {
        global_limit: 1,
        per_uid_limit: 1,
        sweep_limit: 1,
        text_order_memory: NativeReadSessionConfig::full_text_order_memory(1)?,
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
    let registry = NativeReadSessions::new_unfunded_for_test(NativeReadSessionConfig {
        global_limit: 1,
        per_uid_limit: 1,
        sweep_limit: 1,
        text_order_memory: 0,
        ..NativeReadSessionConfig::default()
    })?;
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
    let released_replay = registry.open(42, [7; 32], selection(1), &budget(), |_selected| {
        let _prior = pin_calls.fetch_add(1, Ordering::Relaxed);
        Err(IndexError::BackendViolation(
            "released replay attempted a second Catalog pin".into(),
        ))
    })?;
    check_eq!(released_replay, released);
    check_eq!(pin_calls.load(Ordering::Relaxed), 1);
    check_eq!(pin_count()?, 0);

    // A full replay table cannot discard the original ticket or Release ACK
    // before TTL. A fresh Open is refused, while settlement still observes
    // the same ticket without a new Catalog pin.
    for key in 20..27 {
        let fixture = registered_entry(&registry, 42, key, NativeReadSessionState::Released)?;
        drop(fixture);
    }
    check_eq!(
        registry.open(42, [30; 32], selection(1), &budget(), |_selected| {
            Err(IndexError::BackendViolation(
                "full replay table reached Catalog pin".into(),
            ))
        }),
        Err(IndexError::ResourceLimited)
    );
    check_eq!(
        registry.open(42, [7; 32], selection(1), &budget(), |_selected| {
            Err(IndexError::BackendViolation(
                "full replay table re-pinned original key".into(),
            ))
        })?,
        released
    );
    check_eq!(
        registry.release(42, replay.session, &budget(), |_ticket| {
            Err(IndexError::BackendViolation(
                "full replay table re-released Catalog ticket".into(),
            ))
        })?,
        (released, remaining)
    );
    check_eq!(
        registry.with_released_ticket(42, replay.session, &budget(), |ticket| {
            Ok(ticket.view().identity)
        })?,
        genesis
    );
    check_eq!(pin_calls.load(Ordering::Relaxed), 1);
    check_eq!(pin_count()?, 0);
    {
        let mut table = registry.table.lock().map_err(|error| {
            std::io::Error::other(format!("read-session table poisoned: {error}"))
        })?;
        for key in 20..27 {
            check!(table.remove(NativeReadSessionId([key; 32])));
        }
    }

    let query_ticket = registry.open(42, [8; 32], selection(1), &budget(), |selected| {
        kernel.pin_view(selected, &budget())
    })?;
    check_eq!(pin_count()?, 1);
    check_eq!(
        registry.release(42, replay.session, &budget(), |_ticket| {
            Err(IndexError::BackendViolation(
                "replay reached Catalog release".into(),
            ))
        })?,
        (released, remaining)
    );
    check_eq!(
        registry.with_released_ticket(42, replay.session, &budget(), |ticket| {
            Ok(ticket.view().identity)
        })?,
        genesis
    );
    let query_id = query_ticket.session;
    let release_queries = AtomicBool::new(false);
    thread::scope(|scope| -> TestResult {
        let (entered_tx, entered_rx) = mpsc::channel();
        let mut workers = Vec::new();
        for _ in 0..2 {
            let entered = entered_tx.clone();
            let release = &release_queries;
            let registry_ref = &registry;
            workers.push(scope.spawn(move || {
                registry_ref.with_ticket(
                    42,
                    query_id,
                    &budget(),
                    || Ok(()),
                    |_ticket, _attempt, ()| {
                        entered.send(()).map_err(|error| {
                            IndexError::BackendViolation(format!(
                                "concurrent read observation failed: {error}"
                            ))
                        })?;
                        let deadline = Instant::now()
                            .checked_add(Duration::from_secs(4))
                            .ok_or(IndexError::ResourceLimited)?;
                        while !release.load(Ordering::Acquire) {
                            if Instant::now() >= deadline {
                                return Err(IndexError::ResourceLimited);
                            }
                            thread::sleep(Duration::from_millis(1));
                        }
                        Ok(())
                    },
                )
            }));
        }
        drop(entered_tx);
        let both_entered = entered_rx.recv_timeout(Duration::from_secs(3)).is_ok()
            && entered_rx.recv_timeout(Duration::from_secs(3)).is_ok();
        release_queries.store(true, Ordering::Release);
        for worker in workers {
            worker
                .join()
                .map_err(|_panic| "concurrent query worker panicked")??;
        }
        check!(both_entered);
        Ok(())
    })?;
    let renewed = registry.renew(42, query_ticket.session, query_ticket.revision, &budget())?;
    check_eq!(renewed.revision, query_ticket.revision + 1);
    check_eq!(renewed.state, NativeReadSessionState::Active);
    check_eq!(pin_count()?, 1);
    check_eq!(
        registry.renew(42, query_ticket.session, query_ticket.revision, &budget())?,
        renewed
    );
    let interrupted = budget();
    check_eq!(
        registry.with_ticket(
            42,
            query_ticket.session,
            &interrupted,
            || Ok(()),
            |_ticket, _attempt, ()| {
                interrupted.request().cancel_handle().cancel();
                Ok(())
            }
        ),
        Err(IndexError::Cancelled)
    );
    check_eq!(
        registry.status(42, query_ticket.session, &budget())?,
        renewed
    );
    check_eq!(pin_count()?, 1);
    let unavailable: Result<(), IndexError> = registry.with_ticket(
        42,
        query_ticket.session,
        &budget(),
        || Ok(()),
        |_ticket, _attempt, ()| Err(IndexError::SnapshotUnavailable),
    );
    check_eq!(unavailable, Err(IndexError::SnapshotUnavailable));
    check_eq!(
        registry.status(42, query_ticket.session, &budget())?,
        renewed
    );
    check_eq!(pin_count()?, 1);
    let query_error = IndexError::InvalidInput("reader refused the query".into());
    let outcome: Result<(), IndexError> = registry.with_ticket(
        42,
        query_ticket.session,
        &budget(),
        || Ok(()),
        |_ticket, _attempt, ()| Err(query_error.clone()),
    );
    check_eq!(outcome, Err(query_error));
    check_eq!(
        registry.status(42, query_ticket.session, &budget())?,
        renewed
    );
    // A reader failure remains primary if final observation sees cancellation.
    // Cancellation must not turn a live pin into Unknown or require a repin.
    let interrupted = budget();
    let primary = IndexError::CorruptObject("reader primary before cancellation".into());
    let outcome: Result<(), IndexError> = registry.with_ticket(
        42,
        query_ticket.session,
        &interrupted,
        || Ok(()),
        |_ticket, _attempt, ()| {
            interrupted.request().cancel_handle().cancel();
            Err(primary.clone())
        },
    );
    check_eq!(outcome, Err(primary));
    check_eq!(
        registry.status(42, query_ticket.session, &budget())?,
        renewed
    );
    let same_pin = registry.with_ticket(
        42,
        query_ticket.session,
        &budget(),
        || Ok(()),
        |ticket, _attempt, ()| Ok(ticket.record().lease.id),
    )?;
    let repeated_pin = registry.with_ticket(
        42,
        query_ticket.session,
        &budget(),
        || Ok(()),
        |ticket, _attempt, ()| Ok(ticket.record().lease.id),
    )?;
    check_eq!(same_pin, repeated_pin);
    check_eq!(pin_count()?, 1);
    let entry = registry.lookup(42, query_ticket.session, &budget())?;
    {
        let mut guard = entry.inner.write().map_err(|error| {
            std::io::Error::other(format!("read-session entry poisoned: {error}"))
        })?;
        guard
            .ticket
            .as_mut()
            .ok_or("fixture lost ticket")?
            .try_release()?;
    }
    let invalid: Result<(), IndexError> = registry.with_ticket(
        42,
        query_ticket.session,
        &budget(),
        || Ok(()),
        |_ticket, _attempt, ()| Ok(()),
    );
    check_eq!(invalid, Err(IndexError::SnapshotUnavailable));
    check_eq!(
        registry.status(42, query_ticket.session, &budget())?.state,
        NativeReadSessionState::Unknown
    );
    check_eq!(pin_count()?, 0);
    Ok(())
}

#[test]
fn retained_terminal_open_replay_preserves_state_and_selection() -> TestResult {
    for state in [
        NativeReadSessionState::Closing,
        NativeReadSessionState::Released,
        NativeReadSessionState::Unknown,
    ] {
        let registry =
            NativeReadSessions::new_unfunded_for_test(NativeReadSessionConfig::default())?;
        let entry = registered_entry(&registry, 42, 7, state)?;
        let expected = registry.status(42, entry.id, &budget())?;
        let replay = registry.open(42, [7; 32], selection(1), &budget(), |_selected| {
            Err(IndexError::BackendViolation(
                "terminal replay attempted a Catalog pin".into(),
            ))
        })?;
        check_eq!(replay, expected);
        check_eq!(
            registry.open(42, [7; 32], selection(2), &budget(), |_selected| {
                Err(IndexError::BackendViolation(
                    "conflicting terminal replay attempted a Catalog pin".into(),
                ))
            }),
            Err(IndexError::Conflict)
        );
    }
    Ok(())
}

#[test]
fn default_uid_replay_window_keeps_more_than_the_old_32_record_limit() -> TestResult {
    let config = NativeReadSessionConfig::default();
    let registry = NativeReadSessions::new_unfunded_for_test(config)?;
    for key in 0..33 {
        let entry = registered_entry(&registry, 42, key, NativeReadSessionState::Released)?;
        drop(entry);
    }
    {
        let mut table = registry
            .table
            .lock()
            .map_err(|error| std::io::Error::other(format!("table poisoned: {error}")))?;
        table.make_room_for_open(42, &config)?;
        check_eq!(table.by_id.len(), 33);
        check_eq!(
            table.by_open.get(&(42, [0; 32])),
            Some(&NativeReadSessionId([0; 32]))
        );
    }
    Ok(())
}

#[test]
fn replay_pressure_preserves_live_slots_and_owned_records() -> TestResult {
    let config = NativeReadSessionConfig {
        global_limit: 2,
        per_uid_limit: 1,
        sweep_limit: 1,
        text_order_memory: 0,
        ..NativeReadSessionConfig::default()
    };
    let registry = NativeReadSessions::new_unfunded_for_test(config)?;
    let oldest = registered_entry(&registry, 42, 7, NativeReadSessionState::Released)?;
    let other = registered_entry(&registry, 43, 8, NativeReadSessionState::Released)?.id;
    let live = registered_entry(&registry, 42, 9, NativeReadSessionState::Unknown)?;

    let pin_failure = IndexError::Storage("sentinel pin failure".into());
    let open = || {
        registry.open(42, [30; 32], selection(1), &budget(), |_selected| {
            Err(pin_failure.clone())
        })
    };
    check_eq!(open(), Err(IndexError::ResourceLimited));
    {
        let mut state = live
            .inner
            .write()
            .map_err(|error| std::io::Error::other(format!("entry poisoned: {error}")))?;
        state.record_maintenance_release(false)?;
        live.mark_released();
    }
    // Release frees the live slot even while both replay records are held.
    check_eq!(open(), Err(pin_failure.clone()));
    let oldest_id = oldest.id;
    drop(oldest);
    drop(live);
    for key in 10..16 {
        let fixture = registered_entry(&registry, 42, key, NativeReadSessionState::Released)?;
        drop(fixture);
    }
    // UID 42 has reached its retained limit. Release did free live capacity,
    // but a fresh key must wait instead of deleting the oldest replay record.
    check_eq!(open(), Err(IndexError::ResourceLimited));
    check_eq!(
        registry.status(42, oldest_id, &budget())?.state,
        NativeReadSessionState::Released
    );
    check_eq!(
        registry.open(42, [7; 32], selection(1), &budget(), |_selected| {
            Err(IndexError::BackendViolation(
                "pressure replay attempted Catalog pin".into(),
            ))
        })?,
        registry.status(42, oldest_id, &budget())?
    );
    check_eq!(
        registry
            .status(42, NativeReadSessionId([9; 32]), &budget())?
            .state,
        NativeReadSessionState::Released
    );
    check_eq!(
        registry.status(43, other, &budget())?.state,
        NativeReadSessionState::Released
    );
    let table = registry
        .table
        .lock()
        .map_err(|error| std::io::Error::other(format!("table poisoned: {error}")))?;
    check_eq!(table.by_open.get(&(42, [7; 32])), Some(&oldest_id));
    Ok(())
}

#[test]
fn admission_reclaims_only_expired_terminal_replay_records() -> TestResult {
    let registry = NativeReadSessions::new_unfunded_for_test(NativeReadSessionConfig {
        global_limit: 1,
        per_uid_limit: 1,
        sweep_limit: 1,
        text_order_memory: 0,
        ..NativeReadSessionConfig::default()
    })?;
    for key in 7..14 {
        let recent = registered_entry(&registry, 42, key, NativeReadSessionState::Released)?;
        drop(recent);
    }
    let expired = registered_entry(&registry, 42, 14, NativeReadSessionState::Unknown)?;
    {
        let mut state = expired
            .inner
            .write()
            .map_err(|error| std::io::Error::other(format!("entry poisoned: {error}")))?;
        state.record_maintenance_release(false)?;
    }
    let old_release = Instant::now()
        .checked_sub(Duration::from_secs(301))
        .ok_or("test clock cannot represent an expired release")?;
    check!(expired.terminal_since.set(old_release).is_ok());
    drop(expired);

    let pin_failure = IndexError::Storage("sentinel pin failure".into());
    check_eq!(
        registry.open(42, [15; 32], selection(1), &budget(), |_selected| {
            Err(pin_failure.clone())
        }),
        Err(pin_failure)
    );
    check_eq!(
        registry.status(42, NativeReadSessionId([14; 32]), &budget()),
        Err(IndexError::SnapshotUnavailable)
    );
    for key in 7..14 {
        check_eq!(
            registry
                .status(42, NativeReadSessionId([key; 32]), &budget())?
                .state,
            NativeReadSessionState::Released
        );
    }
    Ok(())
}

#[test]
fn opening_reservation_rejects_other_work_without_creating_a_second_pin() -> TestResult {
    let registry = Arc::new(NativeReadSessions::new_unfunded_for_test(
        NativeReadSessionConfig {
            global_limit: 1,
            per_uid_limit: 1,
            sweep_limit: 1,
            text_order_memory: NativeReadSessionConfig::full_text_order_memory(1)?,
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
fn open_reply_deadline_is_uncertain_after_the_pin_commits() {
    let expired = WorkBudget::with_request(
        WorkLimits {
            reads: 1,
            writes: 1,
            bytes: 1,
            mutations: 1,
        },
        RequestBudgetV1::for_duration(Duration::ZERO),
    );
    assert_eq!(expired.checkpoint(), Err(IndexError::ResourceLimited));
    assert_eq!(
        super::checkpoint_open_reply(&expired),
        Err(IndexError::Cancelled)
    );
}

#[test]
fn release_intent_blocks_new_query_before_waiting_and_survives_cancellation() -> TestResult {
    let registry = Arc::new(NativeReadSessions::new_unfunded_for_test(
        NativeReadSessionConfig::default(),
    )?);
    let entry = registered_entry(&registry, 42, 7, NativeReadSessionState::Active)?;
    let id = entry.id;

    let cancelled = budget();
    cancelled.request().cancel_handle().cancel();
    check_eq!(
        registry.release(42, id, &cancelled, |_ticket| Ok(false)),
        Err(IndexError::Cancelled)
    );
    check!(!entry.is_closing());

    // Holding shared ticket custody models an in-flight query. Release must
    // publish Closing without waiting for that query to finish.
    let guard = entry
        .inner
        .read()
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
    let waiting_status = registry.status(
        42,
        id,
        &WorkBudget::with_request(
            WorkLimits {
                reads: 1,
                writes: 1,
                bytes: 1,
                mutations: 1,
            },
            RequestBudgetV1::for_duration(Duration::from_millis(5)),
        ),
    );
    // The same post-lock predicate is used by with_ticket, closing the race
    // where a query passed its first check before Release published intent.
    let post_lock_query = entry.require_active_query(&guard);
    let new_query: Result<(), IndexError> = registry.with_ticket(
        42,
        id,
        &budget(),
        || Ok(()),
        |_ticket, _attempt, ()| {
            Err(IndexError::BackendViolation(
                "new query reached the held ticket".into(),
            ))
        },
    );
    cancel_release.cancel();
    let timed_out = worker.join().map_err(|_panic| "Release worker panicked")?;
    drop(guard);
    check_eq!(lifecycle_while_query_held, CLOSING_BIT | RELEASE_WAITER);
    check_eq!(status_while_query_held, Ok(NativeReadSessionState::Closing));
    check_eq!(waiting_status, Err(IndexError::ResourceLimited));
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
    let entry = registered_entry(&registry, 42, 7, NativeReadSessionState::Active)?;
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
        .read()
        .map_err(|error| std::io::Error::other(format!("entry poisoned: {error}")))?;
    check_eq!(entry.status(&guard)?.state, NativeReadSessionState::Closing);
    Ok(())
}

#[test]
fn maintenance_released_tombstone_cannot_replay_public_release_ack_or_settlement() -> TestResult {
    let registry = NativeReadSessions::new_unfunded_for_test(NativeReadSessionConfig::default())?;
    let entry = registered_entry(&registry, 42, 7, NativeReadSessionState::Released)?;
    let id = entry.id;
    {
        let mut guard = entry.inner.write().map_err(|error| {
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
        let mut guard = entry.inner.write().map_err(|error| {
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
