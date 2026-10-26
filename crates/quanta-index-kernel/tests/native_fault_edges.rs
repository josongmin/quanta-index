//! Independent behavioral regressions at native catalog boundaries.

use std::{
    error::Error,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use futures::executor::block_on;
use quanta_index_catalog::{CatalogClockPort, SqliteCatalog};
use quanta_index_core::index::{
    CollectionId, Digest, GroupActivation, IndexInstanceId, IndexKind, IndexProfile, InputDomain,
    MapRoot, OperationKey, OperationStatus, ProducerId, PublicationGroupId, SchemaId,
    ScratchAllocation, ScratchRequest, ScratchSpacePort, SlotId, StorageFormatId, StreamId,
    SubmitOutcome, TenantId, UpdateKind, UpdateRequest, ViewSelection, WorkBudget, WorkLimits,
    WorkspaceKey, WorktreeId, ports::WorkClaimPort,
};
use quanta_index_kernel::{AdapterDescriptor, AdapterRegistry, Kernel, KernelServices};
use quanta_index_lexical::native::NativeLiteralIndex;
use quanta_index_storage::{FileObjectStore, FileScratchSpace};
use rusqlite::Connection;

type TestResult = Result<(), Box<dyn Error>>;

struct ManualClock(AtomicU64);

impl CatalogClockPort for ManualClock {
    fn now_unix_ms(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }
}

#[test]
#[expect(
    clippy::panic_in_result_fn,
    reason = "the assertion is an external SQLite behavior oracle"
)]
fn manual_ready_discovery_does_not_advance_durable_clock() -> TestResult {
    let root = tempfile::tempdir()?;
    let clock = Arc::new(ManualClock(AtomicU64::new(1_000)));
    let catalog = Arc::new(SqliteCatalog::open_with_clock(
        root.path(),
        Duration::from_secs(1),
        clock.clone(),
    )?);
    let workspace = WorkspaceKey {
        tenant: TenantId([1; 16]),
        collection: CollectionId([2; 16]),
        worktree: WorktreeId([3; 16]),
        slot: SlotId([4; 16]),
    };
    let scratch = Arc::new(FileScratchSpace::open_standalone(
        root.path().join("scratch"),
        16 * 1024 * 1024,
    )?);
    let objects = Arc::new(FileObjectStore::open_with_scratch(
        root.path().join("native-objects"),
        scratch.clone(),
    )?);
    let kernel = Kernel::new(
        KernelServices::new(
            objects.clone(),
            objects,
            catalog.clone(),
            scratch,
            Duration::from_secs(30),
        )?,
        AdapterRegistry::new(),
    );
    let mut genesis_budget = WorkBudget::new(
        WorkLimits {
            reads: 100,
            writes: 100,
            bytes: 4096,
            mutations: 100,
        },
        Instant::now() + Duration::from_secs(10),
    );
    let _genesis = kernel.create_workspace(workspace, 1, &mut genesis_budget)?;

    let durable_clock = || -> Result<i64, rusqlite::Error> {
        Connection::open(catalog.path())?.query_row(
            "SELECT last_ms FROM native_work_clock_v1 WHERE singleton=1",
            [],
            |row| row.get(0),
        )
    };
    let initial = durable_clock()?;
    assert_eq!(initial, 0);
    let mut budget = WorkBudget::new(
        WorkLimits {
            reads: 100,
            writes: 0,
            bytes: 4096,
            mutations: 0,
        },
        Instant::now() + Duration::from_secs(10),
    );
    clock.0.store(2_000, Ordering::SeqCst);
    assert!(
        catalog
            .ready_work(workspace, 1, 16, &mut budget)?
            .is_empty()
    );
    assert_eq!(
        durable_clock()?,
        initial,
        "idle discovery wrote durable state"
    );
    Ok(())
}

struct DenyingScratch {
    inner: FileScratchSpace,
    attempts: AtomicU64,
    deny: AtomicBool,
}

impl ScratchSpacePort for DenyingScratch {
    fn allocate(
        &self,
        request: ScratchRequest,
        budget: &mut WorkBudget,
    ) -> Result<Box<dyn ScratchAllocation>, quanta_index_core::index::IndexError> {
        let _previous = self.attempts.fetch_add(1, Ordering::SeqCst);
        if self.deny.load(Ordering::SeqCst) {
            return Err(quanta_index_core::index::IndexError::ResourceLimited);
        }
        self.inner.allocate(request, budget)
    }
}

#[test]
#[expect(
    clippy::panic_in_result_fn,
    clippy::disallowed_methods,
    reason = "test-only synchronous Kernel driver; scratch count and durable work rows are external behavior oracles"
)]
fn unchanged_core_profile_reconfigure_needs_no_work_spool() -> TestResult {
    let root = tempfile::tempdir()?;
    let objects = Arc::new(FileObjectStore::open(root.path().join("objects"))?);
    let catalog = Arc::new(SqliteCatalog::open(
        root.path(),
        Duration::from_millis(100),
    )?);
    let scratch = Arc::new(DenyingScratch {
        inner: FileScratchSpace::open_standalone(root.path().join("scratch"), 256 * 1024 * 1024)?,
        attempts: AtomicU64::new(0),
        deny: AtomicBool::new(false),
    });
    let instance = quanta_index_core::index::InstanceSpec {
        instance: IndexInstanceId([7; 16]),
        kind: IndexKind([8; 16]),
        group: PublicationGroupId([9; 16]),
        activation: GroupActivation::CoreRequired,
        input_domain: InputDomain::SourceLocal,
        schema: SchemaId([10; 16]),
        policy: Digest([11; 32]),
        format: StorageFormatId([12; 16]),
    };
    let mut registry = AdapterRegistry::new();
    registry.register(
        AdapterDescriptor {
            kind: instance.kind,
            schema: instance.schema,
            format: instance.format,
        },
        NativeLiteralIndex,
        |delta| Ok(delta.clone()),
    )?;
    let services = KernelServices::new(
        objects.clone(),
        objects,
        catalog.clone(),
        scratch.clone(),
        Duration::from_secs(30),
    )?;
    let kernel = Kernel::new(services, registry);
    let workspace = WorkspaceKey {
        tenant: TenantId([1; 16]),
        collection: CollectionId([2; 16]),
        worktree: WorktreeId([3; 16]),
        slot: SlotId([4; 16]),
    };
    let operation = |sequence| OperationKey {
        workspace,
        epoch: 1,
        producer: ProducerId([5; 16]),
        stream: StreamId([6; 16]),
        stream_epoch: 1,
        sequence,
    };
    let work = || {
        WorkBudget::new(
            WorkLimits {
                reads: 100_000,
                writes: 100_000,
                bytes: 64 * 1024 * 1024,
                mutations: 100_000,
            },
            Instant::now() + Duration::from_secs(30),
        )
    };
    let genesis = kernel.create_workspace(workspace, 1, &mut work())?;
    let first_ingress = kernel.begin_input(operation(1))?;
    let first_profile = kernel.stage_profile(
        first_ingress,
        &IndexProfile {
            instances: vec![instance.clone()],
        },
        &mut work(),
    )?;
    let first = match block_on(kernel.submit(
        UpdateRequest {
            operation: operation(1),
            expected_view: genesis,
            kind: UpdateKind::Reconfigure {
                profile: first_profile,
            },
        },
        &mut work(),
    ))? {
        SubmitOutcome::Terminal(OperationStatus::Published(view)) => view,
        other @ (SubmitOutcome::Accepted(_)
        | SubmitOutcome::Terminal(_)
        | SubmitOutcome::Unknown(_)) => {
            return Err(format!("initial profile was not published: {other:?}").into());
        }
    };
    let first_ticket = kernel.pin_view(ViewSelection::Exact(first), &work())?;
    let first_view = first_ticket.view().clone();
    first_ticket.release()?;
    assert!(first_view.members.0.is_some());
    assert_eq!(first_view.coverage, MapRoot(None));
    let before = scratch.attempts.load(Ordering::SeqCst);

    let second_ingress = kernel.begin_input(operation(2))?;
    let second_profile = kernel.stage_profile(
        second_ingress,
        &IndexProfile {
            instances: vec![instance],
        },
        &mut work(),
    )?;
    scratch.deny.store(true, Ordering::SeqCst);
    let second = match block_on(kernel.submit(
        UpdateRequest {
            operation: operation(2),
            expected_view: first,
            kind: UpdateKind::Reconfigure {
                profile: second_profile,
            },
        },
        &mut work(),
    ))? {
        SubmitOutcome::Terminal(OperationStatus::Published(view)) => view,
        other @ (SubmitOutcome::Accepted(_)
        | SubmitOutcome::Terminal(_)
        | SubmitOutcome::Unknown(_)) => {
            return Err(format!("unchanged profile was not published: {other:?}").into());
        }
    };
    assert_eq!(scratch.attempts.load(Ordering::SeqCst), before);
    let second_ticket = kernel.pin_view(ViewSelection::Exact(second), &work())?;
    assert_eq!(second_ticket.view().members, first_view.members);
    assert_eq!(second_ticket.view().coverage, first_view.coverage);
    assert_eq!(second_ticket.view().source, first_view.source);
    second_ticket.release()?;
    let work_rows: i64 = Connection::open(catalog.path())?.query_row(
        "SELECT COUNT(*) FROM native_work_v1",
        [],
        |row| row.get(0),
    )?;
    assert_eq!(work_rows, 0);
    Ok(())
}
