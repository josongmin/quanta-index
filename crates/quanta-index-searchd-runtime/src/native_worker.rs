//! Owned asynchronous execution for native `Submit` and `Resume` operations.
//! The synchronous IPC dispatcher only enqueues work and receives its result.

use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc as sync_mpsc,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use quanta_index_catalog::SqliteCatalog;
use quanta_index_core::index::{
    IndexError, OperationKey, SubmitOutcome, UpdateRequest, WorkBudget,
    effort::{Signal, SignalSample, UnknownReason},
};
use quanta_index_kernel::{Kernel, OperationExecutionClass, ResumeDispatch};
use tokio::sync::mpsc::{self, Sender, error::TrySendError};

type NativeKernel = Kernel<SqliteCatalog>;
type WorkerReply = sync_mpsc::SyncSender<Result<SubmitOutcome, IndexError>>;

/// All timestamps use this worker's monotonic origin, not wall-clock time.
#[derive(Clone)]
pub struct NativeWorkerObservation {
    telemetry: Arc<WorkerTelemetry>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeWorkerStatus {
    Running,
    Stopping,
    Stopped,
    Failed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NativeLaneSnapshot {
    /// Accepted operations not yet started, including a command just removed
    /// from the channel while its consumer enters the active state.
    pub queued: u8,
    pub active: u8,
    pub dispatch_lag_ms: Signal<u64>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NativeWorkerSnapshot {
    /// Relative to this observation handle's monotonic origin.
    pub observed_at_ms: u64,
    /// Counts remain inspectable after shutdown for cleanup checks. Only
    /// `Running` permits interpretation as current scheduling capacity.
    pub status: NativeWorkerStatus,
    pub core: NativeLaneSnapshot,
    pub enrichment: NativeLaneSnapshot,
}

impl NativeWorkerObservation {
    /// A coherent snapshot of the two execution lanes. A missing lag means no
    /// operation has been dequeued on that lane; it is not a zero-lag sample.
    pub fn observe(&self, max_sample_age: Duration) -> Result<NativeWorkerSnapshot, IndexError> {
        let valid_for_ms = u64::try_from(max_sample_age.as_millis())
            .map_err(|error| IndexError::InvalidInput(format!("dispatch lag TTL: {error}")))?;
        if valid_for_ms == 0 {
            return Err(IndexError::InvalidInput(
                "dispatch lag TTL must be positive".into(),
            ));
        }
        self.telemetry.snapshot(valid_for_ms)
    }
}

#[derive(Clone, Copy)]
enum Lane {
    Core,
    Enrichment,
}

impl From<OperationExecutionClass> for Lane {
    fn from(class: OperationExecutionClass) -> Self {
        match class {
            OperationExecutionClass::Core => Self::Core,
            OperationExecutionClass::Enrichment => Self::Enrichment,
        }
    }
}

#[derive(Clone, Copy, Default)]
struct LaneLoad {
    queued: u8,
    active: u8,
    last_dequeue_at_ms: Option<u64>,
    last_dispatch_lag_ms: Option<u64>,
}

impl LaneLoad {
    fn snapshot(
        self,
        now_ms: u64,
        valid_for_ms: u64,
        status: NativeWorkerStatus,
    ) -> NativeLaneSnapshot {
        let dispatch_lag_ms = match (status, self.last_dequeue_at_ms, self.last_dispatch_lag_ms) {
            (NativeWorkerStatus::Running, Some(at), Some(lag))
                if now_ms.checked_sub(at).is_some_and(|age| age < valid_for_ms) =>
            {
                Signal::Known(SignalSample {
                    value: lag,
                    observed_at_ms: at,
                    valid_for_ms,
                })
            }
            (NativeWorkerStatus::Running, None, None) => Signal::Unknown(UnknownReason::Missing),
            _ => Signal::Unknown(UnknownReason::Dropped),
        };
        NativeLaneSnapshot {
            queued: self.queued,
            active: self.active,
            dispatch_lag_ms,
        }
    }
}

struct WorkerLoad {
    status: NativeWorkerStatus,
    core: LaneLoad,
    enrichment: LaneLoad,
}

impl Default for WorkerLoad {
    fn default() -> Self {
        Self {
            status: NativeWorkerStatus::Running,
            core: LaneLoad::default(),
            enrichment: LaneLoad::default(),
        }
    }
}

impl WorkerLoad {
    fn lane(&mut self, lane: Lane) -> &mut LaneLoad {
        match lane {
            Lane::Core => &mut self.core,
            Lane::Enrichment => &mut self.enrichment,
        }
    }
}

struct WorkerTelemetry {
    origin: Instant,
    load: Mutex<WorkerLoad>,
    invalid: AtomicBool,
}

impl WorkerTelemetry {
    fn new() -> Self {
        Self {
            origin: Instant::now(),
            load: Mutex::new(WorkerLoad::default()),
            invalid: AtomicBool::new(false),
        }
    }

    fn now_ms(&self) -> Result<u64, IndexError> {
        match u64::try_from(self.origin.elapsed().as_millis()) {
            Ok(now_ms) => Ok(now_ms),
            Err(_overflow) => Err(IndexError::ResourceLimited),
        }
    }

    fn snapshot(&self, valid_for_ms: u64) -> Result<NativeWorkerSnapshot, IndexError> {
        if self.invalid.load(Ordering::Acquire) {
            return Err(IndexError::BackendViolation(
                "native worker telemetry invariant failed".into(),
            ));
        }
        let load = self.load.lock().map_err(|error| {
            IndexError::BackendViolation(format!("native worker telemetry lock poisoned: {error}"))
        })?;
        if self.invalid.load(Ordering::Acquire) {
            return Err(IndexError::BackendViolation(
                "native worker telemetry invariant failed".into(),
            ));
        }
        let observed_at_ms = self.now_ms()?;
        Ok(NativeWorkerSnapshot {
            observed_at_ms,
            status: load.status,
            core: load
                .core
                .snapshot(observed_at_ms, valid_for_ms, load.status),
            enrichment: load
                .enrichment
                .snapshot(observed_at_ms, valid_for_ms, load.status),
        })
    }

    fn transition(&self, status: NativeWorkerStatus) {
        match self.load.lock() {
            Ok(mut load) => load.status = status,
            Err(_poison) => self.invalid.store(true, Ordering::Release),
        }
    }

    fn mark_stopped(&self) {
        match self.load.lock() {
            Ok(mut load)
                if matches!(
                    load.status,
                    NativeWorkerStatus::Running | NativeWorkerStatus::Stopping
                ) =>
            {
                load.status = NativeWorkerStatus::Stopped;
            }
            Ok(_load) => {}
            Err(_poison) => self.invalid.store(true, Ordering::Release),
        }
    }

    fn mark_stopping(&self) {
        match self.load.lock() {
            Ok(mut load) if load.status == NativeWorkerStatus::Running => {
                load.status = NativeWorkerStatus::Stopping;
            }
            Ok(_load) => {}
            Err(_poison) => self.invalid.store(true, Ordering::Release),
        }
    }
}

enum LeasePhase {
    Queued,
    Active,
}

struct TelemetryLease {
    telemetry: Arc<WorkerTelemetry>,
    lane: Lane,
    enqueued_at_ms: u64,
    phase: LeasePhase,
}

impl TelemetryLease {
    fn activate(&mut self) -> Result<(), IndexError> {
        let now_ms = self.telemetry.now_ms()?;
        let mut load = self.telemetry.load.lock().map_err(|error| {
            IndexError::BackendViolation(format!("native worker telemetry lock poisoned: {error}"))
        })?;
        let lane = load.lane(self.lane);
        let Some(next_queued) = lane.queued.checked_sub(1) else {
            self.telemetry.invalid.store(true, Ordering::Release);
            return Err(IndexError::BackendViolation(
                "native worker dequeue without a queued operation".into(),
            ));
        };
        let Some(next_active) = lane.active.checked_add(1).filter(|count| *count <= 1) else {
            self.telemetry.invalid.store(true, Ordering::Release);
            return Err(IndexError::BackendViolation(
                "native worker lane exceeded one active operation".into(),
            ));
        };
        lane.queued = next_queued;
        lane.active = next_active;
        lane.last_dequeue_at_ms = Some(now_ms);
        lane.last_dispatch_lag_ms = now_ms.checked_sub(self.enqueued_at_ms);
        self.phase = LeasePhase::Active;
        drop(load);
        Ok(())
    }
}

impl Drop for TelemetryLease {
    fn drop(&mut self) {
        let Ok(mut load) = self.telemetry.load.lock() else {
            self.telemetry.invalid.store(true, Ordering::Release);
            return;
        };
        let lane = load.lane(self.lane);
        let count = match self.phase {
            LeasePhase::Queued => &mut lane.queued,
            LeasePhase::Active => &mut lane.active,
        };
        if let Some(next) = count.checked_sub(1) {
            *count = next;
        } else {
            self.telemetry.invalid.store(true, Ordering::Release);
        }
    }
}

struct QueuedCommand {
    command: WorkerCommand,
    lease: Option<TelemetryLease>,
}

struct LaneTaskGuard {
    telemetry: Arc<WorkerTelemetry>,
    completed: bool,
}

impl Drop for LaneTaskGuard {
    fn drop(&mut self) {
        if !self.completed {
            self.telemetry.transition(NativeWorkerStatus::Failed);
        }
    }
}

fn enqueue(
    sender: &Sender<QueuedCommand>,
    telemetry: &Arc<WorkerTelemetry>,
    class: OperationExecutionClass,
    command: WorkerCommand,
) -> Result<(), IndexError> {
    // Reserve the channel slot before recording it. The mutex covers send so
    // an observer never sees a queued count without a real queued command.
    let permit = sender.try_reserve().map_err(|error| match error {
        TrySendError::Full(()) => IndexError::ResourceLimited,
        TrySendError::Closed(()) => IndexError::Storage("native operation worker closed".into()),
    })?;
    let enqueued_at_ms = telemetry.now_ms()?;
    let mut load = telemetry.load.lock().map_err(|error| {
        IndexError::BackendViolation(format!("native worker telemetry lock poisoned: {error}"))
    })?;
    let lane = load.lane(class.into());
    let next_queued = lane
        .queued
        .checked_add(1)
        .filter(|count| {
            u16::from(*count)
                .checked_add(u16::from(lane.active))
                .is_some_and(|total| total <= 2)
        })
        .ok_or_else(|| {
            telemetry.invalid.store(true, Ordering::Release);
            IndexError::BackendViolation(
                "native worker lane exceeded two accepted operations".into(),
            )
        })?;
    lane.queued = next_queued;
    permit.send(QueuedCommand {
        command,
        lease: Some(TelemetryLease {
            telemetry: Arc::clone(telemetry),
            lane: class.into(),
            enqueued_at_ms,
            phase: LeasePhase::Queued,
        }),
    });
    drop(load);
    Ok(())
}

async fn run_lane(mut receiver: mpsc::Receiver<QueuedCommand>) {
    while let Some(queued) = receiver.recv().await {
        let QueuedCommand { command, mut lease } = queued;
        if let Some(ref mut guard) = lease
            && let Err(error) = guard.activate()
        {
            match command {
                WorkerCommand::Submit { reply, .. } | WorkerCommand::Resume { reply, .. } => {
                    drop(reply.send(Err(error)));
                }
                WorkerCommand::Stop => {}
                #[cfg(test)]
                WorkerCommand::TimerProbe { .. }
                | WorkerCommand::QueueBarrier { .. }
                | WorkerCommand::Panic { .. } => {}
            }
            continue;
        }
        match command {
            WorkerCommand::Submit {
                kernel,
                request,
                mut budget,
                reply,
            } => {
                let result = match budget.checkpoint() {
                    Ok(()) => kernel.submit(*request, &mut budget).await,
                    Err(interruption) => Err(interruption),
                };
                drop(reply.send(result));
            }
            WorkerCommand::Resume {
                kernel,
                key,
                mut budget,
                reply,
            } => {
                let result = match budget.checkpoint() {
                    Ok(()) => kernel.resume(key, &mut budget).await,
                    Err(interruption) => Err(interruption),
                };
                drop(reply.send(result));
            }
            WorkerCommand::Stop => break,
            #[cfg(test)]
            WorkerCommand::TimerProbe { finished } => {
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                finished.store(true, std::sync::atomic::Ordering::Release);
            }
            #[cfg(test)]
            WorkerCommand::QueueBarrier { entered, release } => {
                let _entered = entered.send(());
                drop(release.await);
            }
            #[cfg(test)]
            WorkerCommand::Panic { entered } => {
                let _ = entered.send(());
                std::panic::resume_unwind(Box::new("test native worker task panic"));
            }
        }
    }
}

enum WorkerCommand {
    Submit {
        kernel: Arc<NativeKernel>,
        request: Box<UpdateRequest>,
        budget: WorkBudget,
        reply: WorkerReply,
    },
    Resume {
        kernel: Arc<NativeKernel>,
        key: OperationKey,
        budget: WorkBudget,
        reply: WorkerReply,
    },
    Stop,
    #[cfg(test)]
    TimerProbe {
        finished: Arc<std::sync::atomic::AtomicBool>,
    },
    #[cfg(test)]
    QueueBarrier {
        entered: sync_mpsc::SyncSender<()>,
        release: tokio::sync::oneshot::Receiver<()>,
    },
    #[cfg(test)]
    Panic {
        entered: sync_mpsc::SyncSender<()>,
    },
}

/// Core and enrichment operations have independent serial consumers and one
/// waiting slot per lane.
///
/// The runtime is created and dropped by the
/// dedicated manager thread, never by an asynchronous task.
pub(super) struct NativeOperationWorker {
    core_sender: Option<Sender<QueuedCommand>>,
    enrichment_sender: Option<Sender<QueuedCommand>>,
    manager: Option<JoinHandle<()>>,
    telemetry: Arc<WorkerTelemetry>,
}

impl NativeOperationWorker {
    pub(super) fn start() -> Result<Self, IndexError> {
        let (core_sender, core_receiver) = mpsc::channel(1);
        let (enrichment_sender, enrichment_receiver) = mpsc::channel(1);
        let (started_tx, started_rx) = sync_mpsc::sync_channel(1);
        let telemetry = Arc::new(WorkerTelemetry::new());
        let manager_telemetry = Arc::clone(&telemetry);
        let manager = thread::Builder::new()
            .name("native-operation-runtime".into())
            .spawn(move || {
                let runtime = match tokio::runtime::Builder::new_multi_thread()
                    .worker_threads(2)
                    .max_blocking_threads(2)
                    .enable_all()
                    .build()
                {
                    Ok(runtime) => runtime,
                    Err(error) => {
                        drop(started_tx.send(Err(error.to_string())));
                        return;
                    }
                };
                let (done_tx, done_rx) = sync_mpsc::sync_channel(2);
                let core_done = done_tx.clone();
                let core_telemetry = Arc::clone(&manager_telemetry);
                let _core_task = runtime.spawn(async move {
                    let mut guard = LaneTaskGuard {
                        telemetry: core_telemetry,
                        completed: false,
                    };
                    run_lane(core_receiver).await;
                    guard.completed = true;
                    let _completion = core_done.send(());
                });
                let enrichment_telemetry = Arc::clone(&manager_telemetry);
                let _enrichment_task = runtime.spawn(async move {
                    let mut guard = LaneTaskGuard {
                        telemetry: enrichment_telemetry,
                        completed: false,
                    };
                    run_lane(enrichment_receiver).await;
                    guard.completed = true;
                    let _completion = done_tx.send(());
                });
                match started_tx.send(Ok(())) {
                    Ok(()) => {
                        // A panicking lane drops its completion sender; its
                        // closed receiver reports Storage to any waiter.
                        for _lane in 0..2 {
                            match done_rx.recv() {
                                Ok(()) => {}
                                Err(_lane_failed) => {
                                    manager_telemetry.transition(NativeWorkerStatus::Failed);
                                    break;
                                }
                            }
                        }
                    }
                    Err(_startup_receiver_closed) => {}
                }
                // Tokio runtime destruction stays on this manager thread.
            })
            .map_err(|error| IndexError::Storage(format!("start native runtime: {error}")))?;
        match started_rx.recv() {
            Ok(Ok(())) => Ok(Self {
                core_sender: Some(core_sender),
                enrichment_sender: Some(enrichment_sender),
                manager: Some(manager),
                telemetry,
            }),
            Ok(Err(message)) => {
                drop(manager.join());
                Err(IndexError::Storage(format!(
                    "initialize native runtime: {message}"
                )))
            }
            Err(error) => {
                drop(manager.join());
                Err(IndexError::Storage(format!(
                    "native runtime startup lost: {error}"
                )))
            }
        }
    }

    pub(super) fn observation(&self) -> NativeWorkerObservation {
        NativeWorkerObservation {
            telemetry: Arc::clone(&self.telemetry),
        }
    }

    #[cfg(test)]
    fn enqueue_test(
        &self,
        class: OperationExecutionClass,
        command: WorkerCommand,
    ) -> Result<(), IndexError> {
        let sender = match class {
            OperationExecutionClass::Core => &self.core_sender,
            OperationExecutionClass::Enrichment => &self.enrichment_sender,
        }
        .as_ref()
        .ok_or_else(|| IndexError::Storage("test native worker sender missing".into()))?;
        enqueue(sender, &self.telemetry, class, command)
    }

    pub(super) fn submit(
        &self,
        kernel: Arc<NativeKernel>,
        request: UpdateRequest,
        budget: WorkBudget,
    ) -> Result<SubmitOutcome, IndexError> {
        budget.checkpoint()?;
        let class = OperationExecutionClass::for_request(&request);
        self.execute(class, |reply| WorkerCommand::Submit {
            kernel,
            request: Box::new(request),
            budget,
            reply,
        })
    }

    pub(super) fn resume(
        &self,
        kernel: Arc<NativeKernel>,
        key: OperationKey,
        mut budget: WorkBudget,
    ) -> Result<SubmitOutcome, IndexError> {
        budget.checkpoint()?;
        match kernel.resume_dispatch(key, &mut budget)? {
            ResumeDispatch::Reply(outcome) => Ok(outcome),
            ResumeDispatch::Execute(class) => {
                budget.checkpoint()?;
                self.execute(class, |reply| WorkerCommand::Resume {
                    kernel,
                    key,
                    budget,
                    reply,
                })
            }
        }
    }

    fn execute(
        &self,
        class: OperationExecutionClass,
        command: impl FnOnce(WorkerReply) -> WorkerCommand,
    ) -> Result<SubmitOutcome, IndexError> {
        let (reply_tx, reply_rx) = sync_mpsc::sync_channel(1);
        let sender = match class {
            OperationExecutionClass::Core => &self.core_sender,
            OperationExecutionClass::Enrichment => &self.enrichment_sender,
        }
        .as_ref()
        .ok_or_else(|| IndexError::Storage("native operation worker is stopped".into()))?;
        enqueue(sender, &self.telemetry, class, command(reply_tx))?;
        reply_rx.recv().map_err(|error| {
            IndexError::Storage(format!("native operation worker lost result: {error}"))
        })?
    }
}

impl Drop for NativeOperationWorker {
    fn drop(&mut self) {
        self.telemetry.mark_stopping();
        for lane in [&mut self.core_sender, &mut self.enrichment_sender] {
            let Some(sender) = lane.take() else {
                continue;
            };
            // A full waiting slot drains its operation when the sender closes.
            drop(sender.try_send(QueuedCommand {
                command: WorkerCommand::Stop,
                lease: None,
            }));
            drop(sender);
        }
        if let Some(manager) = self.manager.take()
            && manager.join().is_err()
        {
            self.telemetry.transition(NativeWorkerStatus::Failed);
        }
        self.telemetry.mark_stopped();
    }
}

#[cfg(test)]
mod tests {
    use std::{
        error::Error,
        sync::atomic::{AtomicBool, Ordering},
        time::{Duration, Instant},
    };

    use quanta_index_core::{
        RequestBudgetV1,
        index::{
            AdmissionOutcome, CollectionId, Digest, ImmutableOperation, IndexInstanceId,
            IngressRef, OperationStatus, ProducerId, ProjectionId, RegisteredObject, SlotId,
            StreamId, TenantId, UpdateKind, ViewSelection, WorkClaim, WorkIntent, WorkLimits,
            WorkScope, WorkspaceKey, WorktreeId,
            ports::{ImmutableObjectPort, InputStagingPort, ObjectWriteSink, OperationJournalPort},
        },
    };
    use quanta_index_kernel::{AdapterRegistry, KernelServices};
    use quanta_index_storage::FileObjectStore;

    use super::*;

    type TestResult<T = ()> = Result<T, Box<dyn Error>>;

    struct JournalFixture {
        _directory: tempfile::TempDir,
        kernel: Arc<NativeKernel>,
        objects: Arc<FileObjectStore>,
        catalog: Arc<SqliteCatalog>,
        request: UpdateRequest,
    }

    struct FixtureIngressSink<'a> {
        catalog: &'a SqliteCatalog,
        ingress: IngressRef,
    }

    impl ObjectWriteSink for FixtureIngressSink<'_> {
        fn reserve(&self, object: &RegisteredObject) -> Result<(), IndexError> {
            self.catalog
                .reserve_inputs(self.ingress, std::slice::from_ref(object))
        }

        fn installed(&self, object: &RegisteredObject) -> Result<(), IndexError> {
            self.catalog
                .confirm_inputs(self.ingress, std::slice::from_ref(object))
        }

        fn reserve_batch(&self, objects: &[RegisteredObject]) -> Result<(), IndexError> {
            self.catalog.reserve_inputs(self.ingress, objects)
        }

        fn installed_batch(&self, objects: &[RegisteredObject]) -> Result<(), IndexError> {
            self.catalog.confirm_inputs(self.ingress, objects)
        }
    }

    fn admit_pending(fixture: &JournalFixture) -> TestResult {
        let key = fixture.request.operation;
        let children = match &fixture.request.kind {
            UpdateKind::Reconfigure { profile } => vec![*profile],
            UpdateKind::Materialize { .. } => Vec::new(),
            _ => return Err("fixture request kind has no test input staging".into()),
        };
        let ingress = fixture.catalog.open_ingress(key)?;
        let sink = FixtureIngressSink {
            catalog: &fixture.catalog,
            ingress,
        };
        let stored = fixture.objects.install(
            &serde_json::to_vec(&fixture.request)?,
            &children,
            &sink,
            &mut work_budget(),
        )?;
        fixture.catalog.seal_inputs(ingress, stored.reference)?;
        assert!(matches!(
            fixture.catalog.admit(
                ImmutableOperation {
                    key,
                    request: stored.reference,
                },
                ingress,
                &[],
                &mut work_budget(),
            )?,
            AdmissionOutcome::New(_)
        ));
        Ok(())
    }

    fn limits() -> WorkLimits {
        WorkLimits {
            reads: 1_000_000,
            writes: 1_000_000,
            bytes: 512 * 1024 * 1024,
            mutations: 1_000_000,
        }
    }

    fn work_budget() -> WorkBudget {
        WorkBudget::with_request(
            limits(),
            RequestBudgetV1::for_duration(Duration::from_secs(30)),
        )
    }

    fn journal_fixture() -> TestResult<JournalFixture> {
        let directory = tempfile::tempdir()?;
        let objects = Arc::new(FileObjectStore::open(
            directory.path().join("native-objects"),
        )?);
        let catalog = Arc::new(SqliteCatalog::open(
            directory.path(),
            Duration::from_secs(2),
        )?);
        let kernel = Arc::new(NativeKernel::new(
            KernelServices::new(objects.clone(), objects.clone(), catalog.clone()),
            AdapterRegistry::new(),
        ));
        let workspace = WorkspaceKey {
            tenant: TenantId([1; 16]),
            collection: CollectionId([2; 16]),
            worktree: WorktreeId([3; 16]),
            slot: SlotId([4; 16]),
        };
        let genesis = kernel.create_workspace(workspace, 1, &mut work_budget())?;
        let profile = kernel
            .pin_view(ViewSelection::Exact(genesis))?
            .view()
            .profile;
        let request = UpdateRequest {
            operation: OperationKey {
                workspace,
                epoch: 1,
                producer: ProducerId([5; 16]),
                stream: StreamId([6; 16]),
                stream_epoch: 1,
                sequence: 1,
            },
            expected_view: genesis,
            kind: UpdateKind::Reconfigure { profile },
        };
        Ok(JournalFixture {
            _directory: directory,
            kernel,
            objects,
            catalog,
            request,
        })
    }

    fn hold_consumer(
        worker: &NativeOperationWorker,
        class: OperationExecutionClass,
    ) -> TestResult<tokio::sync::oneshot::Sender<()>> {
        let (entered_tx, entered_rx) = sync_mpsc::sync_channel(1);
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        worker
            .enqueue_test(
                class,
                WorkerCommand::QueueBarrier {
                    entered: entered_tx,
                    release: release_rx,
                },
            )
            .map_err(|error| format!("enqueue test barrier: {error}"))?;
        entered_rx.recv_timeout(Duration::from_secs(5))?;
        Ok(release_tx)
    }

    #[derive(Clone, Copy)]
    enum Interruption {
        Cancel,
        Expire,
    }

    fn interrupt(
        mode: Interruption,
        request_budget: &RequestBudgetV1,
        deadline: Instant,
    ) -> IndexError {
        match mode {
            Interruption::Cancel => {
                request_budget.cancel_handle().cancel();
                IndexError::Cancelled
            }
            Interruption::Expire => {
                std::thread::sleep(deadline.saturating_duration_since(Instant::now()));
                while Instant::now() < deadline {
                    std::thread::yield_now();
                }
                IndexError::ResourceLimited
            }
        }
    }

    fn queued_submit(mode: Interruption) -> TestResult {
        let fixture = journal_fixture()?;
        let key = fixture.request.operation;
        let worker = NativeOperationWorker::start()?;
        let observation = worker.observation();
        let release = hold_consumer(&worker, OperationExecutionClass::Core)?;
        let deadline = Instant::now()
            .checked_add(Duration::from_millis(250))
            .ok_or("test deadline overflow")?;
        let request_budget = RequestBudgetV1::until(deadline);
        let (reply_tx, reply_rx) = sync_mpsc::sync_channel(1);
        worker
            .enqueue_test(
                OperationExecutionClass::Core,
                WorkerCommand::Submit {
                    kernel: Arc::clone(&fixture.kernel),
                    request: Box::new(fixture.request),
                    budget: WorkBudget::with_request(limits(), request_budget.clone()),
                    reply: reply_tx,
                },
            )
            .map_err(|error| format!("enqueue real submit: {error}"))?;
        let waiting = observation.observe(Duration::from_secs(1))?;
        assert_eq!((waiting.core.queued, waiting.core.active), (1, 1));
        assert_eq!(fixture.kernel.status(key)?, OperationStatus::Unknown);
        let expected = interrupt(mode, &request_budget, deadline);
        release
            .send(())
            .map_err(|_unsent_unit| "test barrier closed")?;
        assert_eq!(
            reply_rx.recv_timeout(Duration::from_secs(5))?,
            Err(expected)
        );
        assert_eq!(fixture.kernel.status(key)?, OperationStatus::Unknown);
        drop(worker);
        let stopped = observation.observe(Duration::from_secs(1))?;
        assert_eq!(stopped.status, NativeWorkerStatus::Stopped);
        assert_eq!((stopped.core.queued, stopped.core.active), (0, 0));
        assert_eq!(
            stopped.core.dispatch_lag_ms,
            Signal::Unknown(UnknownReason::Dropped)
        );
        Ok(())
    }

    fn queued_resume(mode: Interruption) -> TestResult {
        let fixture = journal_fixture()?;
        let key = fixture.request.operation;
        admit_pending(&fixture)?;
        let pending = fixture.kernel.status(key)?;
        assert!(matches!(pending, OperationStatus::Pending(_)));

        let worker = NativeOperationWorker::start()?;
        let observation = worker.observation();
        let release = hold_consumer(&worker, OperationExecutionClass::Core)?;
        let deadline = Instant::now()
            .checked_add(Duration::from_millis(250))
            .ok_or("test deadline overflow")?;
        let request_budget = RequestBudgetV1::until(deadline);
        let (reply_tx, reply_rx) = sync_mpsc::sync_channel(1);
        worker
            .enqueue_test(
                OperationExecutionClass::Core,
                WorkerCommand::Resume {
                    kernel: Arc::clone(&fixture.kernel),
                    key,
                    budget: WorkBudget::with_request(limits(), request_budget.clone()),
                    reply: reply_tx,
                },
            )
            .map_err(|error| format!("enqueue real resume: {error}"))?;
        let waiting = observation.observe(Duration::from_secs(1))?;
        assert_eq!((waiting.core.queued, waiting.core.active), (1, 1));
        assert_eq!(fixture.kernel.status(key)?, pending);
        let expected = interrupt(mode, &request_budget, deadline);
        release
            .send(())
            .map_err(|_unsent_unit| "test barrier closed")?;
        assert_eq!(
            reply_rx.recv_timeout(Duration::from_secs(5))?,
            Err(expected)
        );
        assert_eq!(fixture.kernel.status(key)?, pending);
        drop(worker);
        let stopped = observation.observe(Duration::from_secs(1))?;
        assert_eq!(stopped.status, NativeWorkerStatus::Stopped);
        assert_eq!((stopped.core.queued, stopped.core.active), (0, 0));
        Ok(())
    }

    #[test]
    fn queued_submit_interruption_does_not_admit_an_operation() -> TestResult {
        queued_submit(Interruption::Cancel)?;
        queued_submit(Interruption::Expire)
    }

    #[test]
    fn queued_resume_interruption_preserves_the_pending_fence() -> TestResult {
        queued_resume(Interruption::Cancel)?;
        queued_resume(Interruption::Expire)
    }

    #[test]
    fn unknown_resume_replies_without_entering_a_full_core_lane() -> TestResult {
        let fixture = journal_fixture()?;
        let key = fixture.request.operation;
        let worker = NativeOperationWorker::start()?;
        let release = hold_consumer(&worker, OperationExecutionClass::Core)?;
        let finished = Arc::new(AtomicBool::new(false));
        worker
            .enqueue_test(
                OperationExecutionClass::Core,
                WorkerCommand::TimerProbe {
                    finished: Arc::clone(&finished),
                },
            )
            .map_err(|error| format!("fill core waiting slot: {error}"))?;

        assert_eq!(fixture.kernel.status(key)?, OperationStatus::Unknown);
        let outcome = worker.resume(Arc::clone(&fixture.kernel), key, work_budget());
        release
            .send(())
            .map_err(|_unsent_unit| "test core barrier closed")?;
        drop(worker);

        assert_eq!(outcome?, SubmitOutcome::Unknown(key));
        assert_eq!(fixture.kernel.status(key)?, OperationStatus::Unknown);
        assert!(finished.load(Ordering::Acquire));
        Ok(())
    }

    #[test]
    fn full_enrichment_lane_does_not_block_real_core_submit_or_resume() -> TestResult {
        let worker = NativeOperationWorker::start()?;
        let mut enrichment = journal_fixture()?;
        let enrichment_key = enrichment.request.operation;
        let source = enrichment
            .kernel
            .pin_view(ViewSelection::Exact(enrichment.request.expected_view))?
            .view()
            .source;
        let claim = WorkClaim {
            intent: WorkIntent {
                workspace: enrichment_key.workspace,
                epoch: enrichment_key.epoch,
                instance: IndexInstanceId([7; 16]),
                scope: WorkScope::ProjectionDomain {
                    projection: ProjectionId([8; 16]),
                    source,
                    producer_policy: Digest([9; 32]),
                    dependency_context: Digest([10; 32]),
                },
            },
            fence: 1,
            lease_until_ms: u64::MAX,
        };
        enrichment.request.kind = UpdateKind::Materialize { claim };
        admit_pending(&enrichment)?;
        let original = enrichment.kernel.status(enrichment_key)?;
        assert!(matches!(original, OperationStatus::Pending(_)));

        let release = hold_consumer(&worker, OperationExecutionClass::Enrichment)?;
        let queued_budget = RequestBudgetV1::for_duration(Duration::from_secs(30));
        let (reply_tx, reply_rx) = sync_mpsc::sync_channel(1);
        worker
            .enqueue_test(
                OperationExecutionClass::Enrichment,
                WorkerCommand::Resume {
                    kernel: Arc::clone(&enrichment.kernel),
                    key: enrichment_key,
                    budget: WorkBudget::with_request(limits(), queued_budget.clone()),
                    reply: reply_tx,
                },
            )
            .map_err(|error| format!("enqueue real enrichment resume: {error}"))?;
        assert_eq!(
            worker.resume(
                Arc::clone(&enrichment.kernel),
                enrichment_key,
                work_budget(),
            ),
            Err(IndexError::ResourceLimited)
        );
        let mut second_enrichment = enrichment.request.clone();
        second_enrichment.operation.sequence = 2;
        assert_eq!(
            worker.submit(
                Arc::clone(&enrichment.kernel),
                second_enrichment,
                work_budget(),
            ),
            Err(IndexError::ResourceLimited)
        );
        assert_eq!(enrichment.kernel.status(enrichment_key)?, original);

        let core_submit = journal_fixture()?;
        let submit_key = core_submit.request.operation;
        assert!(matches!(
            worker.submit(
                Arc::clone(&core_submit.kernel),
                core_submit.request,
                work_budget(),
            )?,
            SubmitOutcome::Terminal(OperationStatus::Published(_))
        ));
        assert!(matches!(
            core_submit.kernel.status(submit_key)?,
            OperationStatus::Published(_)
        ));

        let core_resume = journal_fixture()?;
        let resume_key = core_resume.request.operation;
        admit_pending(&core_resume)?;
        assert!(matches!(
            worker.resume(Arc::clone(&core_resume.kernel), resume_key, work_budget(),)?,
            SubmitOutcome::Terminal(OperationStatus::Published(_))
        ));
        assert!(matches!(
            core_resume.kernel.status(resume_key)?,
            OperationStatus::Published(_)
        ));

        queued_budget.cancel_handle().cancel();
        release
            .send(())
            .map_err(|_unsent_unit| "test enrichment barrier closed")?;
        assert_eq!(
            reply_rx.recv_timeout(Duration::from_secs(5))?,
            Err(IndexError::Cancelled)
        );
        assert_eq!(enrichment.kernel.status(enrichment_key)?, original);
        drop(worker);
        Ok(())
    }

    #[test]
    fn owned_shutdown_waits_for_both_lanes() -> TestResult {
        let worker = NativeOperationWorker::start()?;
        let observation = worker.observation();
        let core_release = hold_consumer(&worker, OperationExecutionClass::Core)?;
        let enrichment_release = hold_consumer(&worker, OperationExecutionClass::Enrichment)?;
        let active = observation.observe(Duration::from_secs(1))?;
        assert_eq!((active.core.active, active.enrichment.active), (1, 1));
        let (done_tx, done_rx) = sync_mpsc::sync_channel(1);
        let shutdown = std::thread::spawn(move || {
            drop(worker);
            let _done = done_tx.send(());
        });
        assert!(matches!(
            done_rx.recv_timeout(Duration::from_millis(30)),
            Err(sync_mpsc::RecvTimeoutError::Timeout)
        ));
        let status_deadline = Instant::now()
            .checked_add(Duration::from_secs(5))
            .ok_or("test status deadline overflow")?;
        loop {
            let stopping = observation.observe(Duration::from_secs(1))?;
            if stopping.status == NativeWorkerStatus::Stopping {
                assert_eq!((stopping.core.active, stopping.enrichment.active), (1, 1));
                assert_eq!(
                    stopping.core.dispatch_lag_ms,
                    Signal::Unknown(UnknownReason::Dropped)
                );
                break;
            }
            if Instant::now() >= status_deadline {
                return Err("native worker never entered Stopping".into());
            }
            std::thread::yield_now();
        }
        core_release
            .send(())
            .map_err(|_unsent_unit| "core barrier closed")?;
        assert!(matches!(
            done_rx.recv_timeout(Duration::from_millis(30)),
            Err(sync_mpsc::RecvTimeoutError::Timeout)
        ));
        enrichment_release
            .send(())
            .map_err(|_unsent_unit| "enrichment barrier closed")?;
        done_rx.recv_timeout(Duration::from_secs(5))?;
        shutdown
            .join()
            .map_err(|_panic_payload| "shutdown thread panicked")?;
        let stopped = observation.observe(Duration::from_secs(1))?;
        assert_eq!(stopped.status, NativeWorkerStatus::Stopped);
        assert_eq!((stopped.core.active, stopped.enrichment.active), (0, 0));
        Ok(())
    }

    #[test]
    fn observation_counts_real_queue_and_expires_old_dispatch_lag() -> TestResult {
        let worker = NativeOperationWorker::start()?;
        let observation = worker.observation();
        let before = observation.observe(Duration::from_secs(1))?;
        assert_eq!(before.status, NativeWorkerStatus::Running);
        assert_eq!(
            before.core.dispatch_lag_ms,
            Signal::Unknown(UnknownReason::Missing)
        );
        let release = hold_consumer(&worker, OperationExecutionClass::Core)?;
        let entered = observation.observe(Duration::from_secs(1))?;
        assert_eq!((entered.core.queued, entered.core.active), (0, 1));
        assert!(matches!(entered.core.dispatch_lag_ms, Signal::Known(_)));
        let finished = Arc::new(AtomicBool::new(false));
        worker.enqueue_test(
            OperationExecutionClass::Core,
            WorkerCommand::TimerProbe {
                finished: Arc::clone(&finished),
            },
        )?;
        let waiting = observation.observe(Duration::from_secs(1))?;
        assert_eq!((waiting.core.queued, waiting.core.active), (1, 1));
        assert_eq!(
            worker.enqueue_test(
                OperationExecutionClass::Core,
                WorkerCommand::TimerProbe {
                    finished: Arc::new(AtomicBool::new(false)),
                },
            ),
            Err(IndexError::ResourceLimited)
        );
        assert_eq!(observation.observe(Duration::from_secs(1))?.core.queued, 1);
        let deadline = Instant::now()
            .checked_add(Duration::from_secs(1))
            .ok_or("test deadline overflow")?;
        while observation
            .observe(Duration::from_millis(1))?
            .core
            .dispatch_lag_ms
            != Signal::Unknown(UnknownReason::Dropped)
        {
            if Instant::now() >= deadline {
                return Err("dispatch sample never expired".into());
            }
            std::thread::yield_now();
        }
        release.send(()).map_err(|_unit| "barrier closed")?;
        drop(worker);
        assert!(finished.load(Ordering::Acquire));
        let stopped = observation.observe(Duration::from_secs(1))?;
        assert_eq!(stopped.status, NativeWorkerStatus::Stopped);
        assert_eq!((stopped.core.queued, stopped.core.active), (0, 0));
        assert_eq!(
            stopped.core.dispatch_lag_ms,
            Signal::Unknown(UnknownReason::Dropped)
        );
        Ok(())
    }

    #[test]
    fn dequeued_command_and_new_waiter_are_both_counted_before_activation() -> TestResult {
        let telemetry = Arc::new(WorkerTelemetry::new());
        let observation = NativeWorkerObservation {
            telemetry: Arc::clone(&telemetry),
        };
        let (sender, mut receiver) = mpsc::channel(1);
        let first_done = Arc::new(AtomicBool::new(false));
        enqueue(
            &sender,
            &telemetry,
            OperationExecutionClass::Core,
            WorkerCommand::TimerProbe {
                finished: Arc::clone(&first_done),
            },
        )?;
        let mut first = receiver.try_recv()?;
        let second_done = Arc::new(AtomicBool::new(false));
        enqueue(
            &sender,
            &telemetry,
            OperationExecutionClass::Core,
            WorkerCommand::TimerProbe {
                finished: Arc::clone(&second_done),
            },
        )?;
        let handoff = observation.observe(Duration::from_secs(1))?;
        assert_eq!((handoff.core.queued, handoff.core.active), (2, 0));
        first
            .lease
            .as_mut()
            .ok_or("first lease missing")?
            .activate()?;
        let running = observation.observe(Duration::from_secs(1))?;
        assert_eq!((running.core.queued, running.core.active), (1, 1));
        drop(first);
        let mut second = receiver.try_recv()?;
        second
            .lease
            .as_mut()
            .ok_or("second lease missing")?
            .activate()?;
        drop(second);
        let drained = observation.observe(Duration::from_secs(1))?;
        assert_eq!((drained.core.queued, drained.core.active), (0, 0));
        Ok(())
    }

    #[test]
    fn panicking_lane_releases_active_telemetry_and_marks_health_failed() -> TestResult {
        let worker = NativeOperationWorker::start()?;
        let observation = worker.observation();
        let (entered_tx, entered_rx) = sync_mpsc::sync_channel(1);
        worker.enqueue_test(
            OperationExecutionClass::Core,
            WorkerCommand::Panic {
                entered: entered_tx,
            },
        )?;
        entered_rx.recv_timeout(Duration::from_secs(5))?;
        drop(worker);
        let failed = observation.observe(Duration::from_secs(1))?;
        assert_eq!(failed.status, NativeWorkerStatus::Failed);
        assert_eq!((failed.core.queued, failed.core.active), (0, 0));
        assert_eq!(
            failed.core.dispatch_lag_ms,
            Signal::Unknown(UnknownReason::Dropped)
        );
        Ok(())
    }

    #[test]
    fn tokio_timer_runs_and_shutdown_waits_for_queued_work() -> Result<(), IndexError> {
        let worker = NativeOperationWorker::start()?;
        let finished = Arc::new(AtomicBool::new(false));
        worker
            .enqueue_test(
                OperationExecutionClass::Core,
                WorkerCommand::TimerProbe {
                    finished: Arc::clone(&finished),
                },
            )
            .map_err(|error| IndexError::Storage(format!("test probe queue failed: {error}")))?;
        drop(worker);
        assert!(finished.load(Ordering::Acquire));
        Ok(())
    }
}
