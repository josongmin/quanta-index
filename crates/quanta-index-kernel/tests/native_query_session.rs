//! Contract tests for a retained native query session.
//!
//! These use the real catalog, ticket and object store, with a deterministic
//! marker reader. Native
//! lexical/semantic ranking and an Auto controller require separate tests.
#![expect(
    clippy::disallowed_methods,
    reason = "test-only synchronous driver executes the Kernel future"
)]
#![expect(
    clippy::panic_in_result_fn,
    reason = "integration assertions are independent behavioral oracles"
)]

use futures::executor::block_on;
use quanta_index_catalog::{CatalogClockPort, SqliteCatalog, SystemCatalogClock};
use quanta_index_core::{
    RequestBudgetV1,
    diagnostic_trace::{
        DiagnosticCaptureIdV1, DiagnosticClockV1, DiagnosticContextV1, DiagnosticEmitV1,
        DiagnosticEventSinkV1, DiagnosticLaneV1, DiagnosticRootKindV1, DiagnosticTerminalV1,
    },
    index::{
        CollectionId, CoverageBinding, CoverageStatus, Digest, GroupActivation, IndexError,
        IndexInstanceId, IndexKind, IndexProfile, IndexQueryPort, InputBinding, InputDomain,
        InstanceSpec, MapRoot, NativeReadSettlementScope, OperationKey, OperationStatus,
        ProducerId, PublicationGroupId, QueryCoverage, QueryOutcome, QueryReadContext, SchemaId,
        SlotId, StorageFormatId, StreamId, SubmitOutcome, TenantId, UpdateKind, UpdateRequest,
        ViewRef, ViewSelection, WorkBudget, WorkLimits, WorkUsage, WorkspaceKey, WorktreeId,
        effort::{ReservationGuard, ResourceNeed, ResourceScheduler},
        ports::CatalogExactHistoryPort,
    },
};
use quanta_index_kernel::{
    AdapterDescriptor, AdapterRegistry, Kernel, KernelServices, QueryAdapterRegistry,
};
use quanta_index_lexical::native::NativeLiteralIndex;
use quanta_index_storage::{FileObjectStore, FileScratchSpace};
use std::{
    error::Error,
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, AtomicUsize, Ordering},
    },
    time::Duration,
};

type TestResult<T = ()> = Result<T, Box<dyn Error>>;

fn workspace() -> WorkspaceKey {
    WorkspaceKey {
        tenant: TenantId([1; 16]),
        collection: CollectionId([2; 16]),
        worktree: WorktreeId([3; 16]),
        slot: SlotId([4; 16]),
    }
}

fn operation(sequence: u64) -> OperationKey {
    OperationKey {
        workspace: workspace(),
        epoch: 1,
        producer: ProducerId([5; 16]),
        stream: StreamId([6; 16]),
        stream_epoch: 1,
        sequence,
    }
}

fn instance() -> InstanceSpec {
    InstanceSpec {
        instance: IndexInstanceId([7; 16]),
        kind: IndexKind([8; 16]),
        group: PublicationGroupId([9; 16]),
        activation: GroupActivation::CoreRequired,
        input_domain: InputDomain::SourceLocal,
        schema: SchemaId([10; 16]),
        policy: Digest([11; 32]),
        format: StorageFormatId([12; 16]),
    }
}

fn limits(mutations: u64) -> WorkLimits {
    WorkLimits {
        reads: 1_000_000,
        writes: 1_000_000,
        bytes: 512 * 1024 * 1024,
        mutations,
    }
}

fn budget() -> WorkBudget {
    WorkBudget::with_request(
        limits(1_000_000),
        RequestBudgetV1::for_duration(Duration::from_secs(30)),
    )
}

fn kernel(root: &Path) -> TestResult<(Kernel<SqliteCatalog>, Arc<SqliteCatalog>)> {
    kernel_with_clock(root, Arc::new(SystemCatalogClock))
}

fn kernel_with_clock(
    root: &Path,
    clock: Arc<dyn CatalogClockPort>,
) -> TestResult<(Kernel<SqliteCatalog>, Arc<SqliteCatalog>)> {
    let scratch = Arc::new(FileScratchSpace::open_standalone(
        root.join("scratch"),
        1024 * 1024 * 1024,
    )?);
    let objects = Arc::new(FileObjectStore::open_with_scratch(
        root.join("objects"),
        scratch.clone(),
    )?);
    let catalog = Arc::new(SqliteCatalog::open_with_clock(
        root,
        Duration::from_millis(100),
        clock,
    )?);
    let mut adapters = AdapterRegistry::new();
    adapters.register(
        AdapterDescriptor {
            kind: instance().kind,
            schema: instance().schema,
            format: instance().format,
        },
        NativeLiteralIndex,
        |delta| Ok(delta.clone()),
    )?;
    let services = KernelServices::new(
        objects.clone(),
        objects,
        catalog.clone(),
        scratch,
        Duration::from_secs(30),
    )?;
    Ok((Kernel::new(services, adapters), catalog))
}

fn published(outcome: SubmitOutcome) -> TestResult<ViewRef> {
    match outcome {
        SubmitOutcome::Terminal(OperationStatus::Published(view)) => Ok(view),
        other @ (SubmitOutcome::Accepted(_)
        | SubmitOutcome::Terminal(_)
        | SubmitOutcome::Unknown(_)) => {
            Err(format!("expected published view, got {other:?}").into())
        }
    }
}

fn reconfigure(
    kernel: &Kernel<SqliteCatalog>,
    expected: ViewRef,
    sequence: u64,
) -> TestResult<ViewRef> {
    let ingress = kernel.begin_input(operation(sequence))?;
    let profile = kernel.stage_profile(
        ingress,
        &IndexProfile {
            instances: vec![instance()],
        },
        &mut budget(),
    )?;
    published(block_on(kernel.submit(
        UpdateRequest {
            operation: operation(sequence),
            expected_view: expected,
            kind: UpdateKind::Reconfigure { profile },
        },
        &mut budget(),
    ))?)
}

fn configured(kernel: &Kernel<SqliteCatalog>) -> TestResult<ViewRef> {
    let genesis = kernel.create_workspace(workspace(), 1, &mut budget())?;
    reconfigure(kernel, genesis, 1)
}

fn native_pin_count(catalog: &SqliteCatalog) -> TestResult<i64> {
    Ok(rusqlite::Connection::open(catalog.path())?.query_row(
        "SELECT COUNT(*) FROM native_pin_v1",
        [],
        |row| row.get(0),
    )?)
}

struct TestScheduler;

impl ResourceScheduler for TestScheduler {
    type Token = ();
    fn try_reserve(&self, _need: ResourceNeed) -> Option<Self::Token> {
        Some(())
    }
    fn release(&self, _token: Self::Token) {}
}

fn need() -> ResourceNeed {
    ResourceNeed {
        cpu_units: 1,
        io_bytes: 1024,
        memory_bytes: 1024,
        provider_units: 0,
    }
}

fn guard(need: ResourceNeed) -> TestResult<ReservationGuard<TestScheduler>> {
    ReservationGuard::try_acquire(Arc::new(TestScheduler), need)
        .ok_or_else(|| "test scheduler refused reservation".into())
}

#[derive(Clone, Copy)]
enum Fault {
    None,
    WrongView,
    WrongCoverage,
    CancelAfterRead,
    RefuseAfterCancel,
}

struct MarkerReader {
    calls: Arc<AtomicUsize>,
    completed: Arc<AtomicUsize>,
    charge_mutations: u64,
    fault: Fault,
}

struct DriftingReservationReader {
    reservations: Arc<AtomicUsize>,
    queries: Arc<AtomicUsize>,
}

struct ExpiringAfterReadReader {
    clock: Arc<ManualClock>,
    calls: Arc<AtomicUsize>,
    completed: Arc<AtomicUsize>,
}

impl IndexQueryPort<u8, u8> for ExpiringAfterReadReader {
    fn validate_instance(&self, selected: &InstanceSpec) -> Result<(), IndexError> {
        if *selected != instance() {
            return Err(IndexError::InvalidInput(
                "unexpected expiring instance".into(),
            ));
        }
        Ok(())
    }

    fn reservation(&self, _request: &u8) -> Result<ResourceNeed, IndexError> {
        Ok(need())
    }

    fn query(
        &self,
        request: &u8,
        context: &mut QueryReadContext<'_>,
    ) -> Result<QueryOutcome<u8>, IndexError> {
        let _previous = self.calls.fetch_add(1, Ordering::SeqCst);
        let outcome = QueryOutcome::core_scanned(context.ticket, context.instance, true, *request)?;
        self.clock.0.store(31_000, Ordering::SeqCst);
        let _previous = self.completed.fetch_add(1, Ordering::SeqCst);
        Ok(outcome)
    }
}

impl IndexQueryPort<u8, u8> for DriftingReservationReader {
    fn validate_instance(&self, selected: &InstanceSpec) -> Result<(), IndexError> {
        if *selected != instance() {
            return Err(IndexError::InvalidInput(
                "unexpected drifting instance".into(),
            ));
        }
        Ok(())
    }

    fn reservation(&self, _request: &u8) -> Result<ResourceNeed, IndexError> {
        let invocation = self.reservations.fetch_add(1, Ordering::SeqCst);
        Ok(ResourceNeed {
            cpu_units: if invocation == 0 { 1 } else { 2 },
            ..need()
        })
    }

    fn query(
        &self,
        request: &u8,
        context: &mut QueryReadContext<'_>,
    ) -> Result<QueryOutcome<u8>, IndexError> {
        let _prior = self.queries.fetch_add(1, Ordering::SeqCst);
        QueryOutcome::core_scanned(context.ticket, context.instance, true, *request)
    }
}

struct ManualClock(AtomicU64);

impl CatalogClockPort for ManualClock {
    fn now_unix_ms(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }
}

impl IndexQueryPort<u8, u8> for MarkerReader {
    fn validate_instance(&self, selected: &InstanceSpec) -> Result<(), IndexError> {
        if *selected != instance() {
            return Err(IndexError::InvalidInput(
                "unexpected marker instance".into(),
            ));
        }
        Ok(())
    }

    fn reservation(&self, _request: &u8) -> Result<ResourceNeed, IndexError> {
        Ok(need())
    }

    fn query(
        &self,
        request: &u8,
        context: &mut QueryReadContext<'_>,
    ) -> Result<QueryOutcome<u8>, IndexError> {
        let _prior = self.calls.fetch_add(1, Ordering::SeqCst);
        context.budget.charge(WorkUsage {
            mutations: self.charge_mutations,
            ..WorkUsage::default()
        })?;
        let result = match self.fault {
            Fault::None => {
                QueryOutcome::core_scanned(context.ticket, context.instance, true, *request)
            }
            Fault::WrongView => {
                let mut ticket = context.ticket.clone();
                ticket.view.identity.sequence = ticket
                    .view
                    .identity
                    .sequence
                    .checked_add(1)
                    .ok_or(IndexError::ResourceLimited)?;
                QueryOutcome::core_scanned(&ticket, context.instance, true, *request)
            }
            Fault::WrongCoverage => {
                let mut fake = context.instance.clone();
                fake.activation = GroupActivation::AsyncCoverage;
                let desired = InputBinding {
                    root: MapRoot(None),
                    schema: fake.schema,
                    policy: fake.policy,
                    context: Digest([0; 32]),
                };
                let binding = CoverageBinding {
                    instance: fake.instance,
                    desired,
                    covered: None,
                    searchable: None,
                    status: CoverageStatus::Pending {
                        previous_missing: MapRoot(None),
                    },
                    owners: MapRoot(None),
                };
                QueryOutcome::async_ranked(context.ticket, &fake, &binding, false, *request)
            }
            Fault::CancelAfterRead => {
                let outcome =
                    QueryOutcome::core_scanned(context.ticket, context.instance, true, *request)?;
                context.budget.request().cancel_handle().cancel();
                Ok(outcome)
            }
            Fault::RefuseAfterCancel => {
                context.budget.request().cancel_handle().cancel();
                Err(IndexError::CorruptObject("marker reader corruption".into()))
            }
        }?;
        let _prior = self.completed.fetch_add(1, Ordering::SeqCst);
        Ok(result)
    }
}

type MarkerQueries = (
    QueryAdapterRegistry<u8, u8>,
    Arc<AtomicUsize>,
    Arc<AtomicUsize>,
);

fn queries(charge_mutations: u64, fault: Fault) -> TestResult<MarkerQueries> {
    let calls = Arc::new(AtomicUsize::new(0));
    let completed = Arc::new(AtomicUsize::new(0));
    let mut registry = QueryAdapterRegistry::new();
    registry.register(
        AdapterDescriptor {
            kind: instance().kind,
            schema: instance().schema,
            format: instance().format,
        },
        need(),
        MarkerReader {
            calls: calls.clone(),
            completed: completed.clone(),
            charge_mutations,
            fault,
        },
    )?;
    Ok((registry, calls, completed))
}

#[derive(Debug, Default)]
struct QueryTraceClock(AtomicU64);

impl DiagnosticClockV1 for QueryTraceClock {
    fn now_ns(&self) -> u64 {
        self.0.fetch_add(1, Ordering::Relaxed)
    }
}

#[derive(Debug)]
struct QueryTraceSink;

impl DiagnosticEventSinkV1 for QueryTraceSink {
    fn try_emit(&self, _event: DiagnosticEmitV1) -> bool {
        true
    }
}

struct ScopedTraceReader {
    original: RequestBudgetV1,
    original_context: DiagnosticContextV1,
    stage_context: DiagnosticContextV1,
    selected_view: ViewRef,
    observed_mutations: Arc<Mutex<Vec<(u8, u64)>>>,
}

impl IndexQueryPort<u8, u8> for ScopedTraceReader {
    fn validate_instance(&self, selected: &InstanceSpec) -> Result<(), IndexError> {
        if *selected != instance() {
            return Err(IndexError::InvalidInput(
                "unexpected trace reader instance".into(),
            ));
        }
        Ok(())
    }

    fn reservation(&self, _request: &u8) -> Result<ResourceNeed, IndexError> {
        Ok(need())
    }

    fn query(
        &self,
        request: &u8,
        context: &mut QueryReadContext<'_>,
    ) -> Result<QueryOutcome<u8>, IndexError> {
        let expected = if *request == 3 {
            &self.original_context
        } else {
            &self.stage_context
        };
        let observed = context
            .budget
            .request()
            .diagnostic_context()
            .ok_or_else(|| {
                IndexError::BackendViolation("query reader lost diagnostic context".into())
            })?;
        if !observed.shares_capture_session(expected)
            || observed.capture_id() != expected.capture_id()
            || !observed.has_parent()
            || !context.budget.request().same_request_as(&self.original)
            || context.budget.request().deadline() != self.original.deadline()
            || context.budget.request().is_cancelled()
            || context.ticket.view.identity != self.selected_view
        {
            return Err(IndexError::BackendViolation(
                "query reader received a different context or request".into(),
            ));
        }
        self.observed_mutations
            .lock()
            .map_err(|_error| IndexError::BackendViolation("trace probe poisoned".into()))?
            .push((*request, context.budget.usage().mutations));
        let mutations = match *request {
            1 => 2,
            2 => 3,
            3 => 1,
            4 => 4,
            _ => {
                return Err(IndexError::InvalidInput(
                    "unexpected trace reader request".into(),
                ));
            }
        };
        context.budget.charge(WorkUsage {
            mutations,
            ..WorkUsage::default()
        })?;
        if *request == 2 {
            return Err(IndexError::CorruptObject("trace reader failure".into()));
        }
        let result = QueryOutcome::core_scanned(context.ticket, context.instance, true, *request)?;
        if *request == 4 {
            context.budget.request().cancel_handle().cancel();
        }
        Ok(result)
    }
}

#[test]
fn prepared_stage_trace_scope_preserves_original_request_and_restores_on_all_exits() -> TestResult {
    let root = tempfile::tempdir()?;
    let (kernel, catalog) = kernel(root.path())?;
    let view = configured(&kernel)?;
    let original_trace = DiagnosticContextV1::new(
        DiagnosticCaptureIdV1([81; 16]),
        Arc::new(QueryTraceClock::default()),
        Arc::new(QueryTraceSink),
    );
    let original_root =
        original_trace.enter_root(DiagnosticRootKindV1::Request, DiagnosticLaneV1(1))?;
    let original_child = original_trace.child_context(&original_root);
    let stage_trace = DiagnosticContextV1::new(
        DiagnosticCaptureIdV1([82; 16]),
        Arc::new(QueryTraceClock::default()),
        Arc::new(QueryTraceSink),
    );
    let stage_root = stage_trace.enter_root(DiagnosticRootKindV1::Request, DiagnosticLaneV1(2))?;
    let stage_child = stage_trace.child_context(&stage_root);
    let request = RequestBudgetV1::for_duration(Duration::from_secs(30))
        .with_diagnostic_context(original_child.clone());
    let observed_mutations = Arc::new(Mutex::new(Vec::new()));
    let mut readers = QueryAdapterRegistry::new();
    readers.register(
        AdapterDescriptor {
            kind: instance().kind,
            schema: instance().schema,
            format: instance().format,
        },
        need(),
        ScopedTraceReader {
            original: request.clone(),
            original_context: original_child.clone(),
            stage_context: stage_child.clone(),
            selected_view: view,
            observed_mutations: Arc::clone(&observed_mutations),
        },
    )?;
    let mut work = WorkBudget::with_request(limits(1_000_000), request.clone());
    let mut session = kernel.begin_query_session(ViewSelection::Exact(view), &mut work)?;
    assert_eq!(native_pin_count(&catalog)?, 1);
    let stage_context_count = stage_trace.strong_count();

    let abandoned = session
        .prepare_stage(&readers, instance().instance, 1_u8)?
        .with_diagnostic_context(Some(stage_child.clone()));
    drop(abandoned);
    assert_eq!(stage_trace.strong_count(), stage_context_count);
    let first = session
        .prepare_stage(&readers, instance().instance, 1_u8)?
        .with_diagnostic_context(Some(stage_child.clone()));
    assert_eq!(first.required_resources(), need());
    assert_eq!(*first.execute(&guard(need())?)?.result(), 1);
    assert_eq!(stage_trace.strong_count(), stage_context_count);
    let failed = session
        .prepare_stage(&readers, instance().instance, 2_u8)?
        .with_diagnostic_context(Some(stage_child.clone()));
    assert!(matches!(
        failed.execute(&guard(need())?),
        Err(IndexError::CorruptObject(ref message)) if message == "trace reader failure"
    ));
    assert_eq!(stage_trace.strong_count(), stage_context_count);
    let restored = session.prepare_stage(&readers, instance().instance, 3_u8)?;
    assert_eq!(*restored.execute(&guard(need())?)?.result(), 3);
    let cancelled = session
        .prepare_stage(&readers, instance().instance, 4_u8)?
        .with_diagnostic_context(Some(stage_child.clone()));
    assert!(matches!(
        cancelled.execute(&guard(need())?),
        Err(IndexError::Cancelled)
    ));
    assert_eq!(stage_trace.strong_count(), stage_context_count);
    assert_eq!(native_pin_count(&catalog)?, 1);
    session.close()?;
    assert_eq!(native_pin_count(&catalog)?, 0);

    let restored = work
        .request()
        .diagnostic_context()
        .ok_or("lost original context")?;
    assert!(restored.shares_capture_session(&original_child));
    assert!(!restored.shares_capture_session(&stage_child));
    assert_eq!(restored.capture_id(), original_child.capture_id());
    assert!(work.request().same_request_as(&request));
    assert_eq!(work.request().deadline(), request.deadline());
    assert!(work.request().is_cancelled());
    let observed = observed_mutations
        .lock()
        .map_err(|_error| "trace probe poisoned")?;
    let [
        (first_request, first_usage),
        (failed_request, failed_usage),
        (restored_request, restored_usage),
        (cancelled_request, cancelled_usage),
    ] = observed.as_slice()
    else {
        return Err("reader did not observe all four stages".into());
    };
    assert_eq!(
        (
            *first_request,
            *failed_request,
            *restored_request,
            *cancelled_request
        ),
        (1, 2, 3, 4)
    );
    assert_eq!(first_usage.checked_add(2), Some(*failed_usage));
    assert_eq!(failed_usage.checked_add(3), Some(*restored_usage));
    assert_eq!(restored_usage.checked_add(1), Some(*cancelled_usage));
    assert_eq!(cancelled_usage.checked_add(4), Some(work.usage().mutations));
    drop(observed);
    original_root.finish(DiagnosticTerminalV1::Ok)?;
    stage_root.finish(DiagnosticTerminalV1::Ok)?;
    Ok(())
}

#[test]
fn one_ticket_retains_view_a_across_fast_publish_b_and_deep_then_closes_once() -> TestResult {
    let root = tempfile::tempdir()?;
    let (kernel, catalog) = kernel(root.path())?;
    let view_a = configured(&kernel)?;
    let (readers, calls, _) = queries(1, Fault::None)?;
    let mut work = budget();
    let mut session = kernel.begin_query_session(ViewSelection::Exact(view_a), &mut work)?;
    assert_eq!(session.view(), view_a);
    assert_eq!(native_pin_count(&catalog)?, 1);

    let fast = session.prepare_stage(&readers, instance().instance, 1_u8)?;
    let fast_need = fast.required_resources();
    let fast_result = fast.execute(&guard(fast_need)?)?;
    assert_eq!(fast_result.view(), view_a);
    assert_eq!(*fast_result.result(), 1);
    assert!(matches!(fast_result.coverage(), QueryCoverage::Core));
    assert_eq!(native_pin_count(&catalog)?, 1);

    let view_b = reconfigure(&kernel, view_a, 2)?;
    assert_ne!(view_b, view_a);
    let deep = session.prepare_stage(&readers, instance().instance, 2_u8)?;
    let deep_need = deep.required_resources();
    let deep_result = deep.execute(&guard(deep_need)?)?;
    assert_eq!(deep_result.view(), view_a);
    assert_eq!(*deep_result.result(), 2);
    assert!(matches!(deep_result.coverage(), QueryCoverage::Core));
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert_eq!(native_pin_count(&catalog)?, 1);
    session.close()?;
    assert_eq!(native_pin_count(&catalog)?, 0);
    Ok(())
}

#[test]
fn borrowed_ticket_queries_below_exact_floor_without_repin_or_transfer() -> TestResult {
    let root = tempfile::tempdir()?;
    let (kernel, catalog) = kernel(root.path())?;
    let view_a = configured(&kernel)?;
    let (readers, calls, _) = queries(1, Fault::None)?;
    let mut held = kernel.pin_view(ViewSelection::Exact(view_a), &budget())?;
    let view_b = reconfigure(&kernel, view_a, 2)?;
    assert_eq!(
        catalog.advance_exact_history_floor(workspace(), 1, view_b.sequence, &mut budget())?,
        view_b.sequence
    );
    assert!(matches!(
        kernel.pin_view(ViewSelection::Exact(view_a), &budget()),
        Err(IndexError::SnapshotUnavailable)
    ));
    assert_eq!(native_pin_count(&catalog)?, 1);

    for expected in [3_u8, 4_u8] {
        let mut request = budget();
        let mut session = kernel.begin_query_session_with_ticket(&mut held, &mut request)?;
        assert_eq!(session.view(), view_a);
        let stage = session.prepare_stage(&readers, instance().instance, expected)?;
        let result = stage.execute(&guard(need())?)?;
        assert_eq!(result.view(), view_a);
        assert_eq!(*result.result(), expected);
        session.close()?;
        assert_eq!(native_pin_count(&catalog)?, 1);
    }
    assert_eq!(calls.load(Ordering::SeqCst), 2);

    let other_root = tempfile::tempdir()?;
    let (other_kernel, _) = kernel_with_clock(other_root.path(), Arc::new(SystemCatalogClock))?;
    let mut foreign_request = budget();
    assert!(matches!(
        other_kernel.begin_query_session_with_ticket(&mut held, &mut foreign_request),
        Err(IndexError::InvalidInput(_))
    ));
    assert_eq!(native_pin_count(&catalog)?, 1);
    let scope = NativeReadSettlementScope {
        workspace: workspace(),
        epoch: 1,
        target: view_a,
    };
    assert!(matches!(
        kernel.release_read_ticket(
            &mut held,
            NativeReadSettlementScope {
                target: view_b,
                ..scope
            },
            &mut budget(),
        ),
        Err(IndexError::InvalidInput(_))
    ));
    assert_eq!(native_pin_count(&catalog)?, 1);
    let release = kernel.release_read_ticket(&mut held, scope, &mut budget())?;
    assert_eq!(release.scope, scope);
    assert!(!release.has_remaining_target_pins);
    assert_eq!(native_pin_count(&catalog)?, 0);
    let observed = kernel.observe_read_settlement(&held, scope, &mut budget())?;
    assert_eq!(observed.scope, scope);
    assert_eq!(held.validate(), Err(IndexError::SnapshotUnavailable));
    Ok(())
}

#[test]
fn stages_share_original_mutation_budget_without_refresh() -> TestResult {
    let root = tempfile::tempdir()?;
    let (kernel, catalog) = kernel(root.path())?;
    let view = configured(&kernel)?;
    let (readers, _, completed) = queries(900, Fault::None)?;
    let mut work = WorkBudget::with_request(
        limits(1_000),
        RequestBudgetV1::for_duration(Duration::from_secs(30)),
    );
    let mut session = kernel.begin_query_session(ViewSelection::Exact(view), &mut work)?;
    let fast = session.prepare_stage(&readers, instance().instance, 1_u8)?;
    assert_eq!(*fast.execute(&guard(need())?)?.result(), 1);
    let second = session.prepare_stage(&readers, instance().instance, 2_u8);
    match second {
        Ok(stage) => assert!(matches!(
            stage.execute(&guard(need())?),
            Err(IndexError::ResourceLimited)
        )),
        Err(error) => assert_eq!(error, IndexError::ResourceLimited),
    }
    assert_eq!(completed.load(Ordering::SeqCst), 1);
    assert_eq!(native_pin_count(&catalog)?, 1);
    session.close()?;
    assert!(work.usage().mutations >= 900);
    assert_eq!(native_pin_count(&catalog)?, 0);
    Ok(())
}

#[test]
fn cancelled_after_fast_prevents_deep_reader_and_releases_on_close() -> TestResult {
    let root = tempfile::tempdir()?;
    let (kernel, catalog) = kernel(root.path())?;
    let view = configured(&kernel)?;
    let (readers, calls, _) = queries(1, Fault::None)?;
    let request = RequestBudgetV1::for_duration(Duration::from_secs(30));
    let cancel = request.cancel_handle();
    let mut work = WorkBudget::with_request(limits(1_000), request);
    let mut session = kernel.begin_query_session(ViewSelection::Exact(view), &mut work)?;
    let fast = session.prepare_stage(&readers, instance().instance, 1_u8)?;
    assert_eq!(*fast.execute(&guard(need())?)?.result(), 1);
    cancel.cancel();
    let second = session.prepare_stage(&readers, instance().instance, 2_u8);
    match second {
        Ok(stage) => assert!(matches!(
            stage.execute(&guard(need())?),
            Err(IndexError::Cancelled)
        )),
        Err(error) => assert_eq!(error, IndexError::Cancelled),
    }
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(native_pin_count(&catalog)?, 1);
    session.close()?;
    assert_eq!(native_pin_count(&catalog)?, 0);
    Ok(())
}

#[test]
fn wrong_guard_aborted_stage_and_bad_binding_preserve_session() -> TestResult {
    let root = tempfile::tempdir()?;
    let (kernel, catalog) = kernel(root.path())?;
    let view = configured(&kernel)?;
    let (good, calls, _) = queries(1, Fault::None)?;
    let (wrong_view, _, _) = queries(0, Fault::WrongView)?;
    let (wrong_coverage, _, _) = queries(0, Fault::WrongCoverage)?;
    let mut work = budget();
    let mut session = kernel.begin_query_session(ViewSelection::Exact(view), &mut work)?;

    let aborted = session.prepare_stage(&good, instance().instance, 0_u8)?;
    drop(aborted);
    assert_eq!(native_pin_count(&catalog)?, 1);
    let mismatched = session.prepare_stage(&good, instance().instance, 1_u8)?;
    let wrong_need = ResourceNeed {
        cpu_units: 2,
        ..need()
    };
    assert!(matches!(
        mismatched.execute(&guard(wrong_need)?),
        Err(IndexError::BackendViolation(_))
    ));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(native_pin_count(&catalog)?, 1);

    let wrong = session.prepare_stage(&wrong_view, instance().instance, 2_u8)?;
    assert!(matches!(
        wrong.execute(&guard(need())?),
        Err(IndexError::BackendViolation(_))
    ));
    let wrong = session.prepare_stage(&wrong_coverage, instance().instance, 3_u8)?;
    assert!(matches!(
        wrong.execute(&guard(need())?),
        Err(IndexError::BackendViolation(_))
    ));
    assert_eq!(native_pin_count(&catalog)?, 1);
    let good_stage = session.prepare_stage(&good, instance().instance, 4_u8)?;
    assert_eq!(*good_stage.execute(&guard(need())?)?.result(), 4);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    session.close()?;
    assert_eq!(native_pin_count(&catalog)?, 0);
    Ok(())
}

#[test]
fn changed_reservation_after_prepare_never_calls_reader_or_closes_session() -> TestResult {
    let root = tempfile::tempdir()?;
    let (kernel, catalog) = kernel(root.path())?;
    let view = configured(&kernel)?;
    let reservations = Arc::new(AtomicUsize::new(0));
    let queries_called = Arc::new(AtomicUsize::new(0));
    let mut drifting = QueryAdapterRegistry::new();
    drifting.register(
        AdapterDescriptor {
            kind: instance().kind,
            schema: instance().schema,
            format: instance().format,
        },
        ResourceNeed {
            cpu_units: 2,
            ..need()
        },
        DriftingReservationReader {
            reservations: Arc::clone(&reservations),
            queries: Arc::clone(&queries_called),
        },
    )?;
    let (good, good_calls, _) = queries(0, Fault::None)?;
    let mut work = budget();
    let mut session = kernel.begin_query_session(ViewSelection::Exact(view), &mut work)?;
    let prepared = session.prepare_stage(&drifting, instance().instance, 1_u8)?;
    assert_eq!(prepared.required_resources(), need());
    assert!(matches!(
        prepared.execute(&guard(need())?),
        Err(IndexError::BackendViolation(_))
    ));
    assert_eq!(reservations.load(Ordering::SeqCst), 2);
    assert_eq!(queries_called.load(Ordering::SeqCst), 0);
    assert_eq!(native_pin_count(&catalog)?, 1);
    let valid = session.prepare_stage(&good, instance().instance, 2_u8)?;
    assert_eq!(*valid.execute(&guard(need())?)?.result(), 2);
    assert_eq!(good_calls.load(Ordering::SeqCst), 1);
    session.close()?;
    assert_eq!(native_pin_count(&catalog)?, 0);
    Ok(())
}

#[test]
fn failed_stage_preparation_keeps_selected_ticket_and_allows_valid_stage() -> TestResult {
    let root = tempfile::tempdir()?;
    let (kernel, catalog) = kernel(root.path())?;
    let view = configured(&kernel)?;
    let (good, good_calls, _) = queries(0, Fault::None)?;
    let over_calls = Arc::new(AtomicUsize::new(0));
    let mut too_small = QueryAdapterRegistry::new();
    too_small.register(
        AdapterDescriptor {
            kind: instance().kind,
            schema: instance().schema,
            format: instance().format,
        },
        ResourceNeed {
            io_bytes: 512,
            ..need()
        },
        MarkerReader {
            calls: Arc::clone(&over_calls),
            completed: Arc::new(AtomicUsize::new(0)),
            charge_mutations: 0,
            fault: Fault::None,
        },
    )?;
    let mut work = budget();
    let mut session = kernel.begin_query_session(ViewSelection::Exact(view), &mut work)?;
    assert!(matches!(
        session.prepare_stage(&good, IndexInstanceId([99; 16]), 1_u8),
        Err(IndexError::SnapshotUnavailable)
    ));
    assert_eq!(native_pin_count(&catalog)?, 1);
    assert!(matches!(
        session.prepare_stage(&too_small, instance().instance, 2_u8),
        Err(IndexError::BackendViolation(_))
    ));
    assert_eq!(over_calls.load(Ordering::SeqCst), 0);
    assert_eq!(native_pin_count(&catalog)?, 1);
    let valid = session.prepare_stage(&good, instance().instance, 3_u8)?;
    assert_eq!(*valid.execute(&guard(need())?)?.result(), 3);
    assert_eq!(good_calls.load(Ordering::SeqCst), 1);
    session.close()?;
    assert_eq!(native_pin_count(&catalog)?, 0);
    Ok(())
}

#[test]
fn dropping_session_without_close_releases_its_only_ticket() -> TestResult {
    let root = tempfile::tempdir()?;
    let (kernel, catalog) = kernel(root.path())?;
    let view = configured(&kernel)?;
    let (readers, calls, _) = queries(0, Fault::None)?;
    let mut work = budget();
    {
        let mut session = kernel.begin_query_session(ViewSelection::Exact(view), &mut work)?;
        let abandoned = session.prepare_stage(&readers, instance().instance, 1_u8)?;
        drop(abandoned);
        assert_eq!(native_pin_count(&catalog)?, 1);
    }
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(native_pin_count(&catalog)?, 0);
    Ok(())
}

#[test]
fn expired_selected_ticket_refuses_prepared_stage_without_reader_call() -> TestResult {
    let root = tempfile::tempdir()?;
    let clock = Arc::new(ManualClock(AtomicU64::new(1_000)));
    let (kernel, catalog) = kernel_with_clock(root.path(), clock.clone())?;
    let view = configured(&kernel)?;
    let (readers, calls, _) = queries(0, Fault::None)?;
    let mut work = budget();
    let mut session = kernel.begin_query_session(ViewSelection::Exact(view), &mut work)?;
    let prepared = session.prepare_stage(&readers, instance().instance, 1_u8)?;
    assert_eq!(native_pin_count(&catalog)?, 1);
    clock.0.store(31_000, Ordering::SeqCst);
    assert!(matches!(
        prepared.execute(&guard(need())?),
        Err(IndexError::SnapshotUnavailable)
    ));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(native_pin_count(&catalog)?, 1);
    session.close()?;
    assert_eq!(native_pin_count(&catalog)?, 0);
    Ok(())
}

#[test]
fn cancellation_after_valid_reader_outcome_is_refused_and_keeps_session_pin() -> TestResult {
    let root = tempfile::tempdir()?;
    let (kernel, catalog) = kernel(root.path())?;
    let view = configured(&kernel)?;
    let (readers, calls, completed) = queries(0, Fault::CancelAfterRead)?;
    let mut work = budget();
    let mut session = kernel.begin_query_session(ViewSelection::Exact(view), &mut work)?;
    let prepared = session.prepare_stage(&readers, instance().instance, 1_u8)?;
    assert!(matches!(
        prepared.execute(&guard(need())?),
        Err(IndexError::Cancelled)
    ));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(completed.load(Ordering::SeqCst), 1);
    assert_eq!(native_pin_count(&catalog)?, 1);
    session.close()?;
    assert_eq!(native_pin_count(&catalog)?, 0);
    Ok(())
}

#[test]
fn absent_optional_reader_or_instance_skips_only_that_stage_and_keeps_fast_available() -> TestResult
{
    let root = tempfile::tempdir()?;
    let (kernel, catalog) = kernel(root.path())?;
    let view = configured(&kernel)?;
    let absent = QueryAdapterRegistry::<u8, u8>::new();
    let (fast, calls, _) = queries(0, Fault::None)?;
    let mut work = budget();
    let mut session = kernel.begin_query_session(ViewSelection::Exact(view), &mut work)?;
    assert!(matches!(
        session.prepare_optional_stage(&absent, instance().instance, 9_u8),
        Ok(None)
    ));
    assert!(matches!(
        session.prepare_optional_stage(&fast, IndexInstanceId([99; 16]), 9_u8),
        Ok(None)
    ));
    assert_eq!(native_pin_count(&catalog)?, 1);
    let prepared = session.prepare_stage(&fast, instance().instance, 1_u8)?;
    assert_eq!(*prepared.execute(&guard(need())?)?.result(), 1);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    session.close()?;
    assert_eq!(native_pin_count(&catalog)?, 0);
    Ok(())
}

#[test]
fn malformed_optional_descriptor_and_stale_ticket_are_errors_not_absence() -> TestResult {
    let root = tempfile::tempdir()?;
    let clock = Arc::new(ManualClock(AtomicU64::new(1_000)));
    let (kernel, catalog) = kernel_with_clock(root.path(), clock.clone())?;
    let view = configured(&kernel)?;
    let (fast, fast_calls, _) = queries(0, Fault::None)?;
    let malformed_calls = Arc::new(AtomicUsize::new(0));
    let mut malformed = QueryAdapterRegistry::new();
    malformed.register(
        AdapterDescriptor {
            kind: instance().kind,
            schema: SchemaId([88; 16]),
            format: instance().format,
        },
        need(),
        MarkerReader {
            calls: malformed_calls.clone(),
            completed: Arc::new(AtomicUsize::new(0)),
            charge_mutations: 0,
            fault: Fault::None,
        },
    )?;
    let mut work = budget();
    let mut session = kernel.begin_query_session(ViewSelection::Exact(view), &mut work)?;
    assert!(matches!(
        session.prepare_optional_stage(&malformed, instance().instance, 2_u8),
        Err(IndexError::SnapshotUnavailable)
    ));
    assert!(matches!(
        session.prepare_stage(&malformed, instance().instance, 2_u8),
        Err(IndexError::SnapshotUnavailable)
    ));
    assert_eq!(malformed_calls.load(Ordering::SeqCst), 0);
    assert_eq!(native_pin_count(&catalog)?, 1);
    let good = session.prepare_stage(&fast, instance().instance, 1_u8)?;
    assert_eq!(*good.execute(&guard(need())?)?.result(), 1);
    assert_eq!(fast_calls.load(Ordering::SeqCst), 1);

    clock.0.store(31_000, Ordering::SeqCst);
    let absent = QueryAdapterRegistry::<u8, u8>::new();
    assert!(matches!(
        session.prepare_optional_stage(&absent, instance().instance, 3_u8),
        Err(IndexError::SnapshotUnavailable)
    ));
    assert_eq!(native_pin_count(&catalog)?, 1);
    session.close()?;
    assert_eq!(native_pin_count(&catalog)?, 0);
    Ok(())
}

#[test]
fn lease_expiry_after_valid_reader_outcome_is_refused_and_keeps_session_pin() -> TestResult {
    let root = tempfile::tempdir()?;
    let clock = Arc::new(ManualClock(AtomicU64::new(1_000)));
    let (kernel, catalog) = kernel_with_clock(root.path(), clock.clone())?;
    let view = configured(&kernel)?;
    let calls = Arc::new(AtomicUsize::new(0));
    let completed = Arc::new(AtomicUsize::new(0));
    let mut readers = QueryAdapterRegistry::new();
    readers.register(
        AdapterDescriptor {
            kind: instance().kind,
            schema: instance().schema,
            format: instance().format,
        },
        need(),
        ExpiringAfterReadReader {
            clock: Arc::clone(&clock),
            calls: Arc::clone(&calls),
            completed: Arc::clone(&completed),
        },
    )?;
    let mut work = budget();
    let mut session = kernel.begin_query_session(ViewSelection::Exact(view), &mut work)?;
    let prepared = session.prepare_stage(&readers, instance().instance, 1_u8)?;
    assert!(matches!(
        prepared.execute(&guard(need())?),
        Err(IndexError::SnapshotUnavailable)
    ));
    assert_eq!(clock.now_unix_ms(), 31_000);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(completed.load(Ordering::SeqCst), 1);
    assert_eq!(native_pin_count(&catalog)?, 1);
    session.close()?;
    assert_eq!(native_pin_count(&catalog)?, 0);
    Ok(())
}

/// Arm cancellation only for the next durable ticket validation. This places
/// cancellation after the query's last pre-validation checkpoint, without sleeps.
struct CancelOnValidationClock {
    request: RequestBudgetV1,
    armed: AtomicUsize,
}

impl CatalogClockPort for CancelOnValidationClock {
    fn now_unix_ms(&self) -> u64 {
        if self
            .armed
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |count| {
                count.checked_sub(1)
            })
            == Ok(1)
        {
            self.request.cancel_handle().cancel();
        }
        1_000
    }
}

struct ArmFinalValidationReader(Arc<CancelOnValidationClock>);

impl IndexQueryPort<u8, u8> for ArmFinalValidationReader {
    fn validate_instance(&self, selected: &InstanceSpec) -> Result<(), IndexError> {
        if *selected != instance() {
            return Err(IndexError::InvalidInput(
                "unexpected cancellation instance".into(),
            ));
        }
        Ok(())
    }

    fn reservation(&self, _request: &u8) -> Result<ResourceNeed, IndexError> {
        Ok(need())
    }

    fn query(
        &self,
        request: &u8,
        context: &mut QueryReadContext<'_>,
    ) -> Result<QueryOutcome<u8>, IndexError> {
        let outcome = QueryOutcome::core_scanned(context.ticket, context.instance, true, *request)?;
        self.0.armed.store(1, Ordering::SeqCst);
        Ok(outcome)
    }
}

#[test]
fn cancellation_inside_final_ticket_validation_refuses_retained_and_one_shot_results() -> TestResult
{
    for retained in [true, false] {
        let root = tempfile::tempdir()?;
        let request = RequestBudgetV1::for_duration(Duration::from_secs(30));
        let clock = Arc::new(CancelOnValidationClock {
            request: request.clone(),
            armed: AtomicUsize::new(0),
        });
        let (kernel, catalog) = kernel_with_clock(root.path(), clock.clone())?;
        let view = configured(&kernel)?;
        let mut readers = QueryAdapterRegistry::new();
        readers.register(
            AdapterDescriptor {
                kind: instance().kind,
                schema: instance().schema,
                format: instance().format,
            },
            need(),
            ArmFinalValidationReader(clock),
        )?;
        let mut work = WorkBudget::with_request(limits(1_000_000), request.clone());
        let result = if retained {
            let mut session = kernel.begin_query_session(ViewSelection::Exact(view), &mut work)?;
            let result = session
                .prepare_stage(&readers, instance().instance, 1)?
                .execute(&guard(need())?);
            assert_eq!(native_pin_count(&catalog)?, 1);
            session.close()?;
            result
        } else {
            kernel
                .prepare_query(
                    &readers,
                    ViewSelection::Exact(view),
                    instance().instance,
                    1,
                    &mut work,
                )?
                .execute(&guard(need())?)
        };
        assert!(
            request.is_cancelled(),
            "fixture did not cancel during validation"
        );
        assert_eq!(native_pin_count(&catalog)?, 0);
        assert!(
            matches!(result, Err(IndexError::Cancelled)),
            "cancelled query returned {result:?}"
        );
    }
    Ok(())
}

#[test]
fn cancellation_inside_optional_resolution_final_validation_is_not_stage_absence() -> TestResult {
    let root = tempfile::tempdir()?;
    let request = RequestBudgetV1::for_duration(Duration::from_secs(30));
    let clock = Arc::new(CancelOnValidationClock {
        request: request.clone(),
        armed: AtomicUsize::new(0),
    });
    let (kernel, catalog) = kernel_with_clock(root.path(), clock.clone())?;
    let view = configured(&kernel)?;
    let absent = QueryAdapterRegistry::<u8, u8>::new();
    let mut work = WorkBudget::with_request(limits(1_000_000), request.clone());
    let mut session = kernel.begin_query_session(ViewSelection::Exact(view), &mut work)?;
    clock.armed.store(2, Ordering::SeqCst);
    let result = session.prepare_optional_stage(&absent, instance().instance, 1);
    assert!(
        request.is_cancelled(),
        "fixture did not cancel during final resolution validation"
    );
    assert!(
        matches!(result, Err(IndexError::Cancelled)),
        "cancelled resolution was reported as stage absence"
    );
    drop(result);
    assert_eq!(native_pin_count(&catalog)?, 1);
    session.close()?;
    assert_eq!(native_pin_count(&catalog)?, 0);
    Ok(())
}

#[test]
fn cancellation_inside_renewal_final_validation_is_not_success() -> TestResult {
    let root = tempfile::tempdir()?;
    let request = RequestBudgetV1::for_duration(Duration::from_secs(30));
    let clock = Arc::new(CancelOnValidationClock {
        request: request.clone(),
        armed: AtomicUsize::new(0),
    });
    let (kernel, catalog) = kernel_with_clock(root.path(), clock.clone())?;
    let view = configured(&kernel)?;
    let mut work = WorkBudget::with_request(limits(1_000_000), request.clone());
    let mut session = kernel.begin_query_session(ViewSelection::Exact(view), &mut work)?;
    clock.armed.store(2, Ordering::SeqCst);
    let result = session.renew();
    assert!(
        request.is_cancelled(),
        "fixture did not cancel during final renewal validation"
    );
    assert!(
        matches!(result, Err(IndexError::Cancelled)),
        "cancelled renewal returned {result:?}"
    );
    assert_eq!(native_pin_count(&catalog)?, 1);
    session.close()?;
    assert_eq!(native_pin_count(&catalog)?, 0);
    Ok(())
}

#[test]
fn reader_failure_survives_late_cancellation_and_releases_one_shot_pin() -> TestResult {
    for retained in [false, true] {
        let root = tempfile::tempdir()?;
        let (kernel, catalog) = kernel(root.path())?;
        let view = configured(&kernel)?;
        let (readers, calls, completed) = queries(1, Fault::RefuseAfterCancel)?;
        let mut work = budget();
        let result = if retained {
            let mut session = kernel.begin_query_session(ViewSelection::Exact(view), &mut work)?;
            let result = session
                .prepare_stage(&readers, instance().instance, 1)?
                .execute(&guard(need())?);
            assert_eq!(native_pin_count(&catalog)?, 1);
            session.close()?;
            result
        } else {
            kernel
                .prepare_query(
                    &readers,
                    ViewSelection::Exact(view),
                    instance().instance,
                    1,
                    &mut work,
                )?
                .execute(&guard(need())?)
        };
        assert!(work.request().is_cancelled());
        assert!(matches!(result, Err(IndexError::CorruptObject(ref message))
            if message == "marker reader corruption"));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(completed.load(Ordering::SeqCst), 0);
        assert_eq!(native_pin_count(&catalog)?, 0);
    }
    Ok(())
}

#[test]
fn unique_reader_resolution_preserves_pin_and_descriptor_errors() -> TestResult {
    let root = tempfile::tempdir()?;
    let (kernel, catalog) = kernel(root.path())?;
    let view = configured(&kernel)?;
    let (readers, calls, _) = queries(0, Fault::None)?;
    let mut work = budget();
    let session = kernel.begin_query_session(ViewSelection::Exact(view), &mut work)?;
    let absent = QueryAdapterRegistry::<u8, u8>::new();
    assert!(matches!(
        session.unique_compatible_instance(&absent),
        Err(IndexError::SnapshotUnavailable)
    ));
    assert_eq!(
        session.unique_compatible_instance(&readers)?,
        instance().instance
    );
    let mut malformed = QueryAdapterRegistry::new();
    malformed.register(
        AdapterDescriptor {
            kind: instance().kind,
            schema: SchemaId([88; 16]),
            format: instance().format,
        },
        need(),
        MarkerReader {
            calls: calls.clone(),
            completed: Arc::new(AtomicUsize::new(0)),
            charge_mutations: 0,
            fault: Fault::None,
        },
    )?;
    assert!(matches!(
        session.unique_compatible_instance(&malformed),
        Err(IndexError::SnapshotUnavailable)
    ));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(session.view(), view);
    assert_eq!(native_pin_count(&catalog)?, 1);
    session.close()?;
    assert_eq!(native_pin_count(&catalog)?, 0);
    Ok(())
}

struct SelectionOnlyReader;

impl IndexQueryPort<u8, u8> for SelectionOnlyReader {
    fn validate_instance(&self, selected: &InstanceSpec) -> Result<(), IndexError> {
        if selected.kind != instance().kind
            || selected.schema != instance().schema
            || selected.format != instance().format
        {
            return Err(IndexError::SnapshotUnavailable);
        }
        Ok(())
    }

    fn reservation(&self, _request: &u8) -> Result<ResourceNeed, IndexError> {
        Err(IndexError::BackendViolation(
            "selection reserved a stage".into(),
        ))
    }

    fn query(
        &self,
        _request: &u8,
        _context: &mut QueryReadContext<'_>,
    ) -> Result<QueryOutcome<u8>, IndexError> {
        Err(IndexError::BackendViolation(
            "selection executed a stage".into(),
        ))
    }
}

#[test]
fn unique_reader_resolution_refuses_ambiguous_selected_instances() -> TestResult {
    let root = tempfile::tempdir()?;
    let (kernel, catalog) = kernel(root.path())?;
    let genesis = kernel.create_workspace(workspace(), 1, &mut budget())?;
    let mut second = instance();
    second.instance = IndexInstanceId([13; 16]);
    let profile = kernel.stage_profile(
        kernel.begin_input(operation(1))?,
        &IndexProfile {
            instances: vec![instance(), second],
        },
        &mut budget(),
    )?;
    let view = published(block_on(kernel.submit(
        UpdateRequest {
            operation: operation(1),
            expected_view: genesis,
            kind: UpdateKind::Reconfigure { profile },
        },
        &mut budget(),
    ))?)?;
    let mut readers = QueryAdapterRegistry::new();
    readers.register(
        AdapterDescriptor {
            kind: instance().kind,
            schema: instance().schema,
            format: instance().format,
        },
        need(),
        SelectionOnlyReader,
    )?;
    let mut work = budget();
    let session = kernel.begin_query_session(ViewSelection::Exact(view), &mut work)?;
    assert!(matches!(
        session.unique_compatible_instance(&readers),
        Err(IndexError::InvalidInput(message))
            if message == "multiple compatible selected query instances"
    ));
    assert_eq!(native_pin_count(&catalog)?, 1);
    session.close()?;
    assert_eq!(native_pin_count(&catalog)?, 0);
    Ok(())
}

#[test]
fn unique_reader_final_validation_preserves_original_cancellation() -> TestResult {
    let root = tempfile::tempdir()?;
    let request = RequestBudgetV1::for_duration(Duration::from_secs(30));
    let clock = Arc::new(CancelOnValidationClock {
        request: request.clone(),
        armed: AtomicUsize::new(0),
    });
    let (kernel, catalog) = kernel_with_clock(root.path(), clock.clone())?;
    let view = configured(&kernel)?;
    let mut work = WorkBudget::with_request(limits(1_000_000), request.clone());
    let session = kernel.begin_query_session(ViewSelection::Exact(view), &mut work)?;
    clock.armed.store(2, Ordering::SeqCst);
    let result = session.unique_compatible_instance(&QueryAdapterRegistry::<u8, u8>::new());
    assert!(request.is_cancelled());
    assert!(matches!(result, Err(IndexError::Cancelled)));
    assert_eq!(native_pin_count(&catalog)?, 1);
    session.close()?;
    assert_eq!(native_pin_count(&catalog)?, 0);
    Ok(())
}
