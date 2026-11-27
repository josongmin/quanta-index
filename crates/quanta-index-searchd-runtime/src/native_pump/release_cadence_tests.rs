//! Ticket 07.9 / E07: after the last pin release, the pump's next due rounds
//! retire the released view and drain its reclaim work.
//!
//! Rounds are driven through the pump's own round functions in pump order:
//! demand hand-off, then one retention round, then one reclaim round. No wall
//! clock decides any assertion; each round count follows from the drivers'
//! configured cadence.

use super::*;
use quanta_index_contract::index::{
    CollectionId, Digest, OperationKey, OperationStatus, OwnerKey, OwnerMutation, ProducerId,
    SlotId, SourceEvent, SourceTextAdmission, SourceTransition, StableOwnerId, StreamId,
    SubmitOutcome, TenantId, UpdateRequest, ViewRef, ViewSelection, WorktreeId,
};
use quanta_index_core::index::{
    WorkLimits,
    ports::{CatalogExactHistoryPort, CatalogReclaimPort},
};
use quanta_index_kernel::{AdapterRegistry, KernelServices};
use quanta_index_storage::FileScratchSpace;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

/// Head probes are configured far away so no assertion depends on them.
const HEAD_PROBE_EVERY: u32 = 64;

fn workspace() -> WorkspaceKey {
    WorkspaceKey {
        tenant: TenantId([1; 16]),
        collection: CollectionId([2; 16]),
        worktree: WorktreeId([3; 16]),
        slot: SlotId([4; 16]),
    }
}

fn key(sequence: u64) -> OperationKey {
    OperationKey {
        workspace: workspace(),
        epoch: 1,
        producer: ProducerId([5; 16]),
        stream: StreamId([6; 16]),
        stream_epoch: 1,
        sequence,
    }
}

fn limits() -> WorkLimits {
    WorkLimits {
        reads: 1_000_000,
        writes: 1_000_000,
        bytes: 512 * 1024 * 1024,
        mutations: 1_000_000,
    }
}

/// The bounded allowance of a full eight-step reclaim round.
fn reclaim_limits() -> WorkLimits {
    WorkLimits {
        reads: 8 * 16_384,
        writes: 8 * 256,
        bytes: 8 * 134_217_728,
        mutations: 8 * 256,
    }
}

fn budget() -> WorkBudget {
    WorkBudget::with_request(
        limits(),
        RequestBudgetV1::for_duration(Duration::from_secs(30)),
    )
}

struct Fixture {
    _root: tempfile::TempDir,
    catalog: Arc<SqliteCatalog>,
    objects: Arc<FileObjectStore>,
    kernel: Kernel<SqliteCatalog>,
    views: Vec<ViewRef>,
}

/// Genesis plus `count` source views, each replacing owner 1's record, so
/// every retired view leaves an unreferenced record behind.
#[expect(
    clippy::disallowed_methods,
    reason = "test-only kernel submissions run on a current-thread runtime owned by this fixture"
)]
fn fixture(count: u64) -> TestResult<Fixture> {
    let root = tempfile::tempdir()?;
    let scratch = Arc::new(FileScratchSpace::open_standalone(
        root.path().join("scratch"),
        16 * 1024 * 1024 * 1024,
    )?);
    let catalog = Arc::new(SqliteCatalog::open(root.path(), Duration::from_secs(2))?);
    let objects = Arc::new(FileObjectStore::open_managed_with_scratch(
        root.path().join("native-objects"),
        scratch.clone(),
        catalog.clone(),
    )?);
    let kernel = Kernel::new(
        KernelServices::new(
            objects.clone(),
            objects.clone(),
            catalog.clone(),
            scratch,
            Duration::from_secs(30),
        )?,
        AdapterRegistry::new(),
    );
    let runtime = tokio::runtime::Builder::new_current_thread().build()?;
    let owner = OwnerKey {
        producer: ProducerId([5; 16]),
        stable_id: StableOwnerId([7; 16]),
    };
    let mut view = kernel.create_workspace(workspace(), 1, &mut budget())?;
    let mut views = vec![view];
    let mut prior = None;
    for sequence in 1..=count {
        let ingress = kernel.begin_input(key(sequence))?;
        let record = kernel.stage_source_text(
            ingress,
            "a.rs",
            format!("body {sequence}").as_bytes(),
            SourceTextAdmission::Utf8 {
                policy: Digest([31; 32]),
            },
            &mut budget(),
        )?;
        let owners = kernel.stage_mutations(
            ingress,
            [Ok(OwnerMutation {
                owner,
                expected: prior,
                next: Some(record),
            })],
            &mut budget(),
        )?;
        let ticket = kernel.pin_view(ViewSelection::Exact(view), &budget())?;
        let expected_source = ticket.view().source;
        ticket.release()?;
        let mut transition = SourceTransition {
            expected_source,
            owners,
            events: Vec::new(),
        };
        transition.events.push(SourceEvent {
            producer: key(sequence).producer,
            stream: key(sequence).stream,
            stream_epoch: 1,
            predecessor: sequence.checked_sub(1).ok_or("sequence underflow")?,
            sequence,
            payload: transition.owner_digest()?,
        });
        let outcome = runtime.block_on(kernel.submit(
            UpdateRequest {
                operation: key(sequence),
                expected_view: view,
                kind: UpdateKind::SourceAdvance(transition),
            },
            &mut budget(),
        ))?;
        let SubmitOutcome::Terminal(OperationStatus::Published(next)) = outcome else {
            return Err(format!("source view {sequence} did not publish: {outcome:?}").into());
        };
        view = next;
        views.push(next);
        prior = Some(record);
    }
    // The fixture's own Source reads precede any observed release.
    let _initial_demand = catalog.take_maintenance_demand()?;
    Ok(Fixture {
        _root: root,
        catalog,
        objects,
        kernel,
        views,
    })
}

fn drivers() -> TestResult<(NativeRetentionDriver, NativeReclaimDriver)> {
    let retention = NativeRetentionDriver::new(NativeRetentionConfig {
        scopes: vec![crate::native_retention::NativeRetentionScope {
            workspace: workspace(),
            epoch: 1,
        }],
        sequence_window: 1,
        page_limit: 1,
        custody_page_limit: 1,
        interval: Duration::from_millis(20),
        round_timeout: Duration::from_secs(5),
        work_limits: limits(),
        head_probe_every: HEAD_PROBE_EVERY,
    })?;
    let reclaim = NativeReclaimDriver::new(
        NativeReclaimConfig {
            interval: Duration::from_millis(20),
            wal_reclaim: crate::native_wal_reclaim::NativeWalReclaimPolicy {
                trigger_bytes: 8 * 1024 * 1024,
                min_interval: Duration::from_secs(1),
            },
            max_steps_per_round: 8,
            child_limit: 32,
            work_limits: reclaim_limits(),
            round_timeout: Duration::from_secs(5),
            head_probe_every: HEAD_PROBE_EVERY,
        },
        Instant::now(),
    )?;
    Ok((retention, reclaim))
}

/// The retention half of one pump iteration: demand hand-off, then one
/// retention round due now.
fn retention_step(
    fixture: &Fixture,
    retention: &mut NativeRetentionDriver,
    reclaim: &mut NativeReclaimDriver,
) -> TestResult {
    schedule_maintenance_demand(&fixture.catalog, Some(&mut *retention), Some(&mut *reclaim))?;
    retention.run_round(fixture.catalog.as_ref(), &mut budget())?;
    Ok(())
}

/// The reclaim half of one pump iteration: demand hand-off, then one reclaim
/// round due now.
fn reclaim_step(
    fixture: &Fixture,
    retention: &mut NativeRetentionDriver,
    reclaim: &mut NativeReclaimDriver,
) -> TestResult {
    schedule_maintenance_demand(&fixture.catalog, Some(&mut *retention), Some(&mut *reclaim))?;
    let stop = AtomicBool::new(false);
    let _round = reclaim_round(
        &fixture.catalog,
        &fixture.objects,
        reclaim,
        reclaim.next_due(),
        &mut WorkBudget::with_request(
            reclaim_limits(),
            RequestBudgetV1::for_duration(Duration::from_secs(30)),
        ),
        &stop,
    )?;
    Ok(())
}

/// One pump iteration in pump order, with each driver's round due now.
fn pump_iteration(
    fixture: &Fixture,
    retention: &mut NativeRetentionDriver,
    reclaim: &mut NativeReclaimDriver,
) -> TestResult {
    retention_step(fixture, retention, reclaim)?;
    reclaim_step(fixture, retention, reclaim)
}

fn view_present(fixture: &Fixture, view: ViewRef) -> TestResult<bool> {
    let present: i64 = rusqlite::Connection::open(fixture.catalog.path())?.query_row(
        "SELECT EXISTS(SELECT 1 FROM native_view_v1 WHERE sequence=?1)",
        [i64::try_from(view.sequence)?],
        |row| row.get(0),
    )?;
    Ok(present != 0)
}

fn reclaim_debt(fixture: &Fixture) -> TestResult<bool> {
    Ok(!fixture
        .catalog
        .reclaim_work_page(None, 1, &mut budget())?
        .items
        .is_empty())
}

/// Hold the Catalog's `SQLite` write lock from a second connection so every
/// release attempt of a dropped ticket fails and the pin is abandoned.
fn abandon(fixture: &Fixture, ticket: quanta_index_kernel::ReadTicket) -> TestResult {
    let outage = rusqlite::Connection::open(fixture.catalog.path())?;
    outage.execute_batch("BEGIN IMMEDIATE")?;
    drop(ticket);
    outage.execute_batch("ROLLBACK")?;
    Ok(())
}

#[test]
#[expect(
    clippy::panic_in_result_fn,
    reason = "test assertions name the round whose expected effect is missing"
)]
fn last_release_is_retired_and_drained_by_the_next_pump_rounds() -> TestResult {
    let fixture = fixture(6)?;
    let [genesis, first, second, ..] = fixture.views[..] else {
        return Err("fixture views missing".into());
    };
    let explicit = fixture
        .kernel
        .pin_view(ViewSelection::Exact(first), &budget())?;
    let abandoned = fixture
        .kernel
        .pin_view(ViewSelection::Exact(second), &budget())?;
    let (mut retention, mut reclaim) = drivers()?;

    // Phases rotate Work, Views, Custody per round. Six rounds give two Views
    // turns: genesis retires, then the forward cursor passes pinned view 1.
    for _ in 0..6 {
        pump_iteration(&fixture, &mut retention, &mut reclaim)?;
    }
    assert!(!view_present(&fixture, genesis)?);
    assert!(view_present(&fixture, first)?);
    assert!(view_present(&fixture, second)?);

    // Explicit release: the very next retention round retires view 1, and
    // the reclaim round of the same pump iteration drains the queue. Without
    // the release demand, the forward cursor would first walk views 2..=5 and
    // view 1 would wait for the cursor to wrap or for a head probe. The
    // setup rounds use their full step budget, so no debt precedes the
    // release; the debt checked below is the released view's own.
    assert!(!reclaim_debt(&fixture)?);
    explicit.release()?;
    retention_step(&fixture, &mut retention, &mut reclaim)?;
    assert!(!view_present(&fixture, first)?);
    assert!(reclaim_debt(&fixture)?);
    reclaim_step(&fixture, &mut retention, &mut reclaim)?;
    assert!(!reclaim_debt(&fixture)?);

    // Abandoned pin (both owner release attempts fail): the next reclaim
    // round settles it, the next retention round retires view 2, and the
    // reclaim round after it drains the queue again.
    abandon(&fixture, abandoned)?;
    assert_eq!(fixture.catalog.abandoned_read_pins()?.len(), 1);
    retention_step(&fixture, &mut retention, &mut reclaim)?;
    assert!(view_present(&fixture, second)?);
    reclaim_step(&fixture, &mut retention, &mut reclaim)?;
    assert!(fixture.catalog.abandoned_read_pins()?.is_empty());
    assert!(view_present(&fixture, second)?);
    retention_step(&fixture, &mut retention, &mut reclaim)?;
    assert!(!view_present(&fixture, second)?);
    assert!(reclaim_debt(&fixture)?);
    reclaim_step(&fixture, &mut retention, &mut reclaim)?;
    assert!(!reclaim_debt(&fixture)?);
    Ok(())
}

#[test]
#[expect(
    clippy::panic_in_result_fn,
    reason = "test assertions identify the view retired or protected on each round"
)]
fn released_view_range_survives_single_row_pages_and_a_remaining_pin() -> TestResult {
    let fixture = fixture(7)?;
    let [older, _, _, first, protected, last, ..] = fixture.views.as_slice() else {
        return Err("fixture views missing".into());
    };
    let first_pin = fixture
        .kernel
        .pin_view(ViewSelection::Exact(*first), &budget())?;
    let protected_pin = fixture
        .kernel
        .pin_view(ViewSelection::Exact(*protected), &budget())?;
    let last_pin = fixture
        .kernel
        .pin_view(ViewSelection::Exact(*last), &budget())?;
    let (mut retention, mut reclaim) = drivers()?;

    last_pin.release()?;
    first_pin.release()?;
    retention_step(&fixture, &mut retention, &mut reclaim)?;
    assert!(!view_present(&fixture, *first)?);
    assert!(view_present(&fixture, *last)?);
    assert!(view_present(&fixture, *older)?);

    // The ordinary Work turn runs before the release cursor resumes.
    retention_step(&fixture, &mut retention, &mut reclaim)?;
    assert!(view_present(&fixture, *last)?);

    // Each release is its own one-row range. The second range reaches `last`
    // directly and leaves the intervening pinned view outside both ranges.
    retention_step(&fixture, &mut retention, &mut reclaim)?;
    assert!(view_present(&fixture, *protected)?);
    assert!(!view_present(&fixture, *last)?);
    assert!(view_present(&fixture, *older)?);

    // The ordinary Views turn still starts from its own cursor.
    retention_step(&fixture, &mut retention, &mut reclaim)?;
    assert!(!view_present(&fixture, *older)?);
    assert!(!view_present(&fixture, *last)?);
    assert!(view_present(&fixture, *protected)?);
    protected_pin.release()?;
    Ok(())
}

#[test]
#[expect(
    clippy::panic_in_result_fn,
    reason = "test assertions identify the released view and its surviving successor"
)]
fn released_view_range_leaves_the_successor_when_its_target_is_gone() -> TestResult {
    let fixture = fixture(6)?;
    let [_, before, released, successor, ..] = fixture.views.as_slice() else {
        return Err("fixture views missing".into());
    };
    let pin = fixture
        .kernel
        .pin_view(ViewSelection::Exact(*released), &budget())?;
    pin.release()?;
    let current = fixture.views.last().ok_or("fixture current view missing")?;
    let _floor = fixture.catalog.advance_exact_history_floor(
        workspace(),
        1,
        current.sequence,
        &mut budget(),
    )?;
    // Another retention caller has already retired the released view.
    let prior = fixture.catalog.retire_exact_history_page(
        workspace(),
        1,
        Some(before.sequence),
        1,
        &mut budget(),
    )?;
    assert_eq!(prior.after, Some(released.sequence));
    assert_eq!(prior.retired, 1);

    let (mut retention, mut reclaim) = drivers()?;
    retention_step(&fixture, &mut retention, &mut reclaim)?;
    assert!(!view_present(&fixture, *released)?);
    assert!(view_present(&fixture, *successor)?);
    Ok(())
}
