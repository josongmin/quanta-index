//! Owned asynchronous execution for native `Submit` and `Resume` operations.
//! The synchronous IPC dispatcher only enqueues work and receives its result.

use std::{
    collections::BTreeSet,
    sync::{
        Arc, Condvar, Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
        mpsc as sync_mpsc,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use quanta_index_catalog::SqliteCatalog;
use quanta_index_core::index::{
    IndexError, OperationKey, OperationStatus, OwnedWorkAssignment, OwnedWorkBegin, OwnedWorkDefer,
    OwnedWorkGrant, OwnedWorkReport, OwnedWorkTerminal, SubmitOutcome, UpdateKind, UpdateRequest,
    WorkBudget, WorkspaceKey,
    effort::{ReservationGuard, ResourceNeed, Signal, SignalSample, UnknownReason},
};
use quanta_index_core::{
    DiagnosticContextV1, DiagnosticLaneV1, DiagnosticSpanGuardV1, DiagnosticStageV1,
    DiagnosticTerminalV1, RequestBudgetV1,
};
use quanta_index_kernel::{Kernel, OperationExecutionClass, ResumeDispatch};
use tokio::sync::mpsc::{self, OwnedPermit, Sender, error::TrySendError};

use crate::native_provider_custody::NativeProviderExecutionCustody;
use crate::native_resources::{
    NativeClassScheduler, NativeResourceClass, NativeResourceLedger, NativeResourcePlan,
};

use crate::native_work_diagnostic::{self, OwnedWorkObservation};

type ProviderOwner = Arc<OnceLock<Arc<dyn Send + Sync>>>;

enum HeldExecution {
    Local(ReservationGuard<NativeClassScheduler>),
    Provider(Arc<NativeProviderExecutionCustody>),
}

impl HeldExecution {
    fn bind(&self, budget: &mut WorkBudget) -> Result<(), IndexError> {
        match self {
            Self::Local(reservation) => budget.bind_memory_scope(reservation.memory_scope()),
            Self::Provider(custody) => {
                budget.bind_provider_attempt_custody(&custody.port())?;
                budget.bind_memory_scope(custody.memory_scope());
            }
        }
        Ok(())
    }
}

fn execution_custody(
    reservation: ReservationGuard<NativeClassScheduler>,
    owner: &ProviderOwner,
) -> Result<HeldExecution, IndexError> {
    if reservation.need().provider_units == 0 {
        return Ok(HeldExecution::Local(reservation));
    }
    let lifetime_owner = owner.get().cloned().map_or_else(
        || {
            #[cfg(test)]
            {
                let owner: Arc<dyn Send + Sync> = Arc::new(());
                Ok(owner)
            }
            #[cfg(not(test))]
            {
                Err(IndexError::BackendViolation(
                    "native provider execution has no state-root custody".into(),
                ))
            }
        },
        Ok,
    )?;
    let custody = NativeProviderExecutionCustody::new_with_owner(reservation, lifetime_owner)?;
    Ok(HeldExecution::Provider(custody))
}

type NativeKernel = Kernel<SqliteCatalog>;
type WorkerReply = sync_mpsc::SyncSender<Result<SubmitOutcome, IndexError>>;

/// Maximum direct Enrichment requests that can occupy IPC dispatch slots.
pub(super) const DIRECT_ENRICHMENT_DISPATCH_LIMIT: usize = 2;

pub(super) struct OwnedEnrichmentSlot {
    permit: OwnedPermit<QueuedCommand>,
    scope: ScopePermit,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct ExecutionScope {
    workspace: WorkspaceKey,
    epoch: u64,
}

impl From<OperationKey> for ExecutionScope {
    fn from(key: OperationKey) -> Self {
        Self {
            workspace: key.workspace,
            epoch: key.epoch,
        }
    }
}

#[derive(Default)]
struct ScopeRegistry {
    accepted: Mutex<BTreeSet<ExecutionScope>>,
}

struct ScopePermit {
    registry: Arc<ScopeRegistry>,
    scope: ExecutionScope,
}

/// Direct Enrichment requests occupy IPC dispatch slots while awaiting replies.
///
/// The direct admission limit is included in the native IPC dispatch
/// policy, preserving Core ingress when owned execution capacity increases.
#[derive(Default)]
struct DirectEnrichmentAdmission {
    accepted: Mutex<usize>,
    invalid: AtomicBool,
}

struct DirectEnrichmentPermit {
    admission: Arc<DirectEnrichmentAdmission>,
}

impl DirectEnrichmentAdmission {
    fn acquire(self: &Arc<Self>) -> Result<DirectEnrichmentPermit, IndexError> {
        if self.invalid.load(Ordering::Acquire) {
            return Err(IndexError::BackendViolation(
                "native direct enrichment admission invariant failed".into(),
            ));
        }
        let mut accepted = self.accepted.lock().map_err(|_poison| {
            IndexError::BackendViolation("native direct enrichment admission lock poisoned".into())
        })?;
        if *accepted >= DIRECT_ENRICHMENT_DISPATCH_LIMIT {
            return Err(IndexError::ResourceLimited);
        }
        *accepted = (*accepted)
            .checked_add(1)
            .ok_or(IndexError::ResourceLimited)?;
        drop(accepted);
        Ok(DirectEnrichmentPermit {
            admission: Arc::clone(self),
        })
    }
}

impl Drop for DirectEnrichmentPermit {
    fn drop(&mut self) {
        match self.admission.accepted.lock() {
            Ok(mut accepted) => {
                if let Some(next) = (*accepted).checked_sub(1) {
                    *accepted = next;
                } else {
                    self.admission.invalid.store(true, Ordering::Release);
                }
            }
            Err(_poison) => self.admission.invalid.store(true, Ordering::Release),
        }
    }
}

impl ScopeRegistry {
    fn acquire(self: &Arc<Self>, scope: ExecutionScope) -> Result<ScopePermit, IndexError> {
        let mut accepted = self.accepted.lock().map_err(|error| {
            IndexError::Storage(format!("native enrichment scope lock: {error}"))
        })?;
        if !accepted.insert(scope) {
            return Err(IndexError::ResourceLimited);
        }
        drop(accepted);
        Ok(ScopePermit {
            registry: Arc::clone(self),
            scope,
        })
    }
}

impl Drop for ScopePermit {
    fn drop(&mut self) {
        if let Ok(mut accepted) = self.registry.accepted.lock() {
            let _observed = accepted.remove(&self.scope);
        }
    }
}

pub(super) struct OwnedRunStarted {
    pub grant: OwnedWorkGrant,
    pub cancel: quanta_index_core::CancelHandleV1,
}

pub(super) enum OwnedRunSkip {
    Deferred(OwnedWorkDefer),
    Terminal(OwnedWorkTerminal),
}

#[expect(
    clippy::large_enum_variant,
    reason = "the bounded owned grant stays inline across the worker rendezvous without an unadmitted allocation"
)]
pub(super) enum OwnedRunStart {
    Granted(OwnedRunStarted),
    Skipped(OwnedRunSkip),
}

pub(super) struct OwnedRunHandle {
    pub started: sync_mpsc::Receiver<Result<OwnedRunStart, IndexError>>,
    pub finished: sync_mpsc::Receiver<Result<SubmitOutcome, IndexError>>,
    /// Logical worker and scope completion. Abandoned physical provider work
    /// can still hold the original resource reservation until its actual exit.
    pub quiesced: sync_mpsc::Receiver<()>,
    pub cancellation: OwnedRunCancellation,
}

#[cfg(test)]
struct OwnedTestBarrier {
    reached: sync_mpsc::SyncSender<()>,
    release: sync_mpsc::Receiver<()>,
}

#[cfg(test)]
impl OwnedTestBarrier {
    fn wait(self) {
        let _sent = self.reached.send(());
        let _released = self.release.recv();
    }
}

#[cfg(test)]
#[derive(Default)]
struct OwnedTestBarriers {
    before_grant: Option<OwnedTestBarrier>,
    after_grant: Option<OwnedTestBarrier>,
    after_start: Option<OwnedTestBarrier>,
}

struct DirectReplyCustody {
    scope: Option<ScopePermit>,
    lease: Option<TelemetryLease>,
    #[cfg(test)]
    after_reply: Option<OwnedTestBarrier>,
}

impl DirectReplyCustody {
    fn new(scope: Option<ScopePermit>, lease: Option<TelemetryLease>) -> Self {
        #[cfg(test)]
        {
            Self {
                scope,
                lease,
                after_reply: None,
            }
        }
        #[cfg(not(test))]
        {
            Self { scope, lease }
        }
    }

    #[cfg(test)]
    fn with_after_reply(mut self, after_reply: Option<OwnedTestBarrier>) -> Self {
        self.after_reply = after_reply;
        self
    }
}

fn finish_direct_reply(
    reply: WorkerReply,
    result: Result<SubmitOutcome, IndexError>,
    custody: DirectReplyCustody,
) {
    // A direct reply is the caller's permission to retry this scope. Release
    // worker custody before making that reply visible to the IPC dispatcher.
    drop(custody.scope);
    drop(custody.lease);
    drop(reply.send(result));
    #[cfg(test)]
    if let Some(barrier) = custody.after_reply {
        barrier.wait();
    }
    drop(reply);
}

/// Cancellation belongs to the receiver before the command can start. Its
/// deadline is not execution authority; only the durable grant sets that limit.
pub(super) struct OwnedRunCancellation {
    relay: RequestBudgetV1,
}

impl Drop for OwnedRunCancellation {
    fn drop(&mut self) {
        self.relay.cancel_handle().cancel();
    }
}

fn bind_owned_cancellation(
    relay: &RequestBudgetV1,
    request: &RequestBudgetV1,
) -> quanta_index_core::request_budget::CancelWaiterGuardV1 {
    let cancel = request.cancel_handle();
    let guard = relay.cancel_waiter(Arc::new(move || cancel.cancel()));
    // A cancellation before registration never invokes the new waiter. A
    // later cancellation invokes it, including during this flag check.
    if relay.is_cancelled() {
        request.cancel_handle().cancel();
    }
    guard
}

fn begin_owned_execution(
    request: RequestBudgetV1,
    parent: Option<&DiagnosticContextV1>,
) -> (RequestBudgetV1, Option<DiagnosticSpanGuardV1>) {
    let Some(parent) = parent else {
        return (request, None);
    };
    let Some(span) = crate::runtime_diagnostic_span(
        parent,
        parent.enter(DiagnosticStageV1::Operation, DiagnosticLaneV1(1)),
    ) else {
        return (request, None);
    };
    // Rebind the original grant deadline and cancellation, not a fresh budget.
    let child = parent.child_context(&span);
    (request.with_diagnostic_context(child), Some(span))
}

fn finish_owned_execution(
    span: &mut Option<DiagnosticSpanGuardV1>,
    result: &Result<SubmitOutcome, IndexError>,
) {
    let Some(span) = span.take() else {
        return;
    };
    let terminal = match result {
        Ok(SubmitOutcome::Terminal(OperationStatus::Published(_))) => DiagnosticTerminalV1::Ok,
        Err(IndexError::Cancelled) => DiagnosticTerminalV1::Cancelled { code: 1 },
        Ok(SubmitOutcome::Terminal(OperationStatus::Refused(_))) | Err(_) => {
            DiagnosticTerminalV1::Error { code: 1 }
        }
        Ok(
            SubmitOutcome::Unknown(_)
            | SubmitOutcome::Accepted(_)
            | SubmitOutcome::Terminal(OperationStatus::Unknown | OperationStatus::Pending(_)),
        ) => DiagnosticTerminalV1::Aborted,
    };
    let _finished = span.finish(terminal);
}

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
    pub queued: u16,
    pub active: u16,
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
    queued: u16,
    active: u16,
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
    first_failure: Option<IndexError>,
    core: LaneLoad,
    enrichment: LaneLoad,
}

impl Default for WorkerLoad {
    fn default() -> Self {
        Self {
            status: NativeWorkerStatus::Running,
            first_failure: None,
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
    enrichment_max_active: u16,
}

impl WorkerTelemetry {
    #[cfg(test)]
    fn new() -> Self {
        Self::with_enrichment_limit(1)
    }

    fn with_enrichment_limit(enrichment_max_active: u16) -> Self {
        Self {
            origin: Instant::now(),
            load: Mutex::new(WorkerLoad::default()),
            invalid: AtomicBool::new(false),
            enrichment_max_active,
        }
    }

    fn max_active(&self, lane: Lane) -> u16 {
        match lane {
            Lane::Core => 1,
            Lane::Enrichment => self.enrichment_max_active,
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

    fn record_failure(&self, error: IndexError) {
        match self.load.lock() {
            Ok(mut load) => {
                let _first_failure = load.first_failure.get_or_insert(error);
                load.status = NativeWorkerStatus::Failed;
            }
            Err(_poison) => self.invalid.store(true, Ordering::Release),
        }
    }

    fn terminal_error(&self) -> Result<Option<IndexError>, IndexError> {
        let load = self.load.lock().map_err(|error| {
            IndexError::BackendViolation(format!("native worker telemetry lock poisoned: {error}"))
        })?;
        if let Some(error) = &load.first_failure {
            return Ok(Some(error.clone()));
        }
        if self.invalid.load(Ordering::Acquire) {
            return Err(IndexError::BackendViolation(
                "native worker telemetry invariant failed".into(),
            ));
        }
        Ok((load.status == NativeWorkerStatus::Failed)
            .then(|| IndexError::Storage("native operation worker failed".into())))
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
        let maximum = self.telemetry.max_active(self.lane);
        let Some(next_active) = lane.active.checked_add(1).filter(|count| *count <= maximum) else {
            self.telemetry.invalid.store(true, Ordering::Release);
            return Err(IndexError::BackendViolation(
                "native worker lane exceeded configured active operations".into(),
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
    _scope: Option<ScopePermit>,
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
    scope: Option<ScopePermit>,
) -> Result<(), IndexError> {
    // Reserve the channel slot before recording it. The mutex covers send so
    // an observer never sees a queued count without a real queued command.
    let permit = sender.try_reserve().map_err(|error| match error {
        TrySendError::Full(()) => IndexError::ResourceLimited,
        TrySendError::Closed(()) => IndexError::Storage("native operation worker closed".into()),
    })?;
    enqueue_reserved(telemetry, class, command, scope, |queued| {
        permit.send(queued);
    })
}

fn enqueue_reserved(
    telemetry: &Arc<WorkerTelemetry>,
    class: OperationExecutionClass,
    command: WorkerCommand,
    scope: Option<ScopePermit>,
    send: impl FnOnce(QueuedCommand),
) -> Result<(), IndexError> {
    let enqueued_at_ms = telemetry.now_ms()?;
    let mut load = telemetry.load.lock().map_err(|error| {
        IndexError::BackendViolation(format!("native worker telemetry lock poisoned: {error}"))
    })?;
    let lane = load.lane(class.into());
    let max_accepted = telemetry
        .max_active(class.into())
        .checked_add(1)
        .ok_or(IndexError::ResourceLimited)?;
    let next_queued = lane
        .queued
        .checked_add(1)
        .filter(|count| {
            count
                .checked_add(lane.active)
                .is_some_and(|total| total <= max_accepted)
        })
        .ok_or_else(|| {
            telemetry.invalid.store(true, Ordering::Release);
            IndexError::BackendViolation(
                "native worker lane exceeded configured accepted operations".into(),
            )
        })?;
    lane.queued = next_queued;
    send(QueuedCommand {
        command,
        lease: Some(TelemetryLease {
            telemetry: Arc::clone(telemetry),
            lane: class.into(),
            enqueued_at_ms,
            phase: LeasePhase::Queued,
        }),
        _scope: scope,
    });
    drop(load);
    Ok(())
}

#[cfg(test)]
fn take_direct_after_reply(command: &mut WorkerCommand) -> Option<OwnedTestBarrier> {
    match command {
        WorkerCommand::Submit { after_reply, .. } | WorkerCommand::Resume { after_reply, .. } => {
            after_reply.take()
        }
        WorkerCommand::Owned { .. }
        | WorkerCommand::Stop
        | WorkerCommand::TimerProbe { .. }
        | WorkerCommand::QueueBarrier { .. }
        | WorkerCommand::Panic { .. } => None,
    }
}

#[cfg(test)]
fn take_owned_test_barriers(mut command: WorkerCommand) -> (WorkerCommand, OwnedTestBarriers) {
    let barriers = match &mut command {
        WorkerCommand::Owned { barriers, .. } => std::mem::take(barriers),
        WorkerCommand::Submit { .. }
        | WorkerCommand::Resume { .. }
        | WorkerCommand::Stop
        | WorkerCommand::TimerProbe { .. }
        | WorkerCommand::QueueBarrier { .. }
        | WorkerCommand::Panic { .. } => OwnedTestBarriers::default(),
    };
    (command, barriers)
}

async fn run_command(
    queued: QueuedCommand,
    resources: Arc<NativeClassScheduler>,
    need: ResourceNeed,
    provider_owner: ProviderOwner,
) {
    let quiesced = match &queued.command {
        WorkerCommand::Owned { quiesced, .. } => Some(quiesced.clone()),
        WorkerCommand::Submit { .. } | WorkerCommand::Resume { .. } | WorkerCommand::Stop => None,
        #[cfg(test)]
        WorkerCommand::TimerProbe { .. }
        | WorkerCommand::QueueBarrier { .. }
        | WorkerCommand::Panic { .. } => None,
    };
    run_command_inner(queued, resources, need, provider_owner).await;
    if let Some(quiesced) = quiesced {
        let _notified = quiesced.send(());
    }
}

async fn run_command_inner(
    queued: QueuedCommand,
    resources: Arc<NativeClassScheduler>,
    need: ResourceNeed,
    provider_owner: ProviderOwner,
) {
    let QueuedCommand {
        command,
        mut lease,
        _scope: scope,
    } = queued;
    if let Some(ref mut guard) = lease
        && let Err(error) = guard.activate()
    {
        #[cfg(test)]
        let mut command = command;
        #[cfg(test)]
        let after_reply = take_direct_after_reply(&mut command);
        match command {
            WorkerCommand::Submit { reply, .. } | WorkerCommand::Resume { reply, .. } => {
                let custody = DirectReplyCustody::new(scope, lease);
                #[cfg(test)]
                let custody = custody.with_after_reply(after_reply);
                finish_direct_reply(reply, Err(error), custody);
            }
            WorkerCommand::Owned { started, reply, .. } => {
                drop(started.send(Err(error.clone())));
                drop(reply.send(Err(error)));
            }
            WorkerCommand::Stop => {}
            #[cfg(test)]
            WorkerCommand::TimerProbe { .. }
            | WorkerCommand::QueueBarrier { .. }
            | WorkerCommand::Panic { .. } => {}
        }
        return;
    }
    #[cfg(test)]
    let (mut command, barriers) = take_owned_test_barriers(command);
    #[cfg(test)]
    let after_reply = take_direct_after_reply(&mut command);
    match command {
        WorkerCommand::Submit {
            kernel,
            request,
            mut budget,
            reply,
            ..
        } => {
            let key = request.operation;
            let result = match budget.checkpoint() {
                Ok(()) => match ReservationGuard::try_acquire(Arc::clone(&resources), need) {
                    Some(reservation) => match execution_custody(reservation, &provider_owner) {
                        Ok(custody) => match custody.bind(&mut budget) {
                            Ok(()) => kernel.submit(*request, &mut budget).await,
                            Err(error) => Err(error),
                        },
                        Err(error) => Err(error),
                    },
                    None => Err(resources.denial()),
                },
                Err(interruption) => Err(interruption),
            };
            native_work_diagnostic::record_if_enabled(key, &budget, &result);
            let custody = DirectReplyCustody::new(scope, lease);
            #[cfg(test)]
            let custody = custody.with_after_reply(after_reply);
            finish_direct_reply(reply, result, custody);
        }
        WorkerCommand::Resume {
            kernel,
            key,
            mut budget,
            reply,
            ..
        } => {
            let result = match budget.checkpoint() {
                Ok(()) => match ReservationGuard::try_acquire(Arc::clone(&resources), need) {
                    Some(reservation) => match execution_custody(reservation, &provider_owner) {
                        Ok(custody) => match custody.bind(&mut budget) {
                            Ok(()) => kernel.resume(key, &mut budget).await,
                            Err(error) => Err(error),
                        },
                        Err(error) => Err(error),
                    },
                    None => Err(resources.denial()),
                },
                Err(interruption) => Err(interruption),
            };
            native_work_diagnostic::record_if_enabled(key, &budget, &result);
            let custody = DirectReplyCustody::new(scope, lease);
            #[cfg(test)]
            let custody = custody.with_after_reply(after_reply);
            finish_direct_reply(reply, result, custody);
        }
        WorkerCommand::Owned {
            kernel,
            assignment,
            cancellation,
            diagnostic,
            started,
            reply,
            ..
        } => {
            let key = assignment.request.operation;
            if cancellation.is_cancelled() {
                native_work_diagnostic::record_owned_if_enabled(
                    OwnedWorkObservation::BeforeGrant(key),
                    &Err(IndexError::Cancelled),
                );
                drop(started.send(Err(IndexError::Cancelled)));
                drop(reply.send(Err(IndexError::Cancelled)));
                return;
            }
            let Some(reservation) = ReservationGuard::try_acquire(Arc::clone(&resources), need)
            else {
                let error = resources.denial();
                native_work_diagnostic::record_owned_if_enabled(
                    OwnedWorkObservation::BeforeGrant(key),
                    &Err(error.clone()),
                );
                drop(started.send(Err(error.clone())));
                drop(reply.send(Err(error)));
                return;
            };
            #[cfg(test)]
            if let Some(barrier) = barriers.before_grant {
                barrier.wait();
            }
            let grant = match kernel.begin_owned_work(&assignment) {
                Ok(OwnedWorkBegin::Granted(grant)) => grant,
                Ok(OwnedWorkBegin::Deferred(reason)) => {
                    drop(started.send(Ok(OwnedRunStart::Skipped(OwnedRunSkip::Deferred(reason)))));
                    return;
                }
                Ok(OwnedWorkBegin::Terminal(status)) => {
                    native_work_diagnostic::record_owned_terminal_before_grant_if_enabled(
                        key, status,
                    );
                    drop(started.send(Ok(OwnedRunStart::Skipped(OwnedRunSkip::Terminal(status)))));
                    return;
                }
                Err(error) => {
                    native_work_diagnostic::record_owned_if_enabled(
                        OwnedWorkObservation::BeforeGrant(key),
                        &Err(error.clone()),
                    );
                    drop(started.send(Err(error.clone())));
                    drop(reply.send(Err(error)));
                    return;
                }
            };
            #[cfg(test)]
            if let Some(barrier) = barriers.after_grant {
                barrier.wait();
            }
            let (request_budget, mut span) = begin_owned_execution(
                RequestBudgetV1::until(grant.execution_deadline),
                diagnostic.as_ref(),
            );
            // Retain the actual worker identity before finishing consumes its guard.
            let trace = span.as_ref().map(DiagnosticSpanGuardV1::trace_ref);
            let _cancel_relay = bind_owned_cancellation(&cancellation, &request_budget);
            let mut budget = WorkBudget::with_request(
                grant.assignment.policy.dispatch_limits,
                request_budget.clone(),
            );
            let held_execution = match execution_custody(reservation, &provider_owner) {
                Ok(custody) => custody,
                Err(error) => {
                    let error = settle_abandoned_owned(&kernel, &grant)
                        .err()
                        .unwrap_or(error);
                    let result = Err(error.clone());
                    finish_owned_execution(&mut span, &result);
                    native_work_diagnostic::record_owned_if_enabled(
                        OwnedWorkObservation::Granted {
                            grant: &grant,
                            budget: &budget,
                            trace,
                        },
                        &result,
                    );
                    drop(started.send(Err(error.clone())));
                    drop(reply.send(Err(error)));
                    return;
                }
            };
            if let Err(error) = held_execution.bind(&mut budget) {
                let error = settle_abandoned_owned(&kernel, &grant)
                    .err()
                    .unwrap_or(error);
                let result = Err(error.clone());
                finish_owned_execution(&mut span, &result);
                native_work_diagnostic::record_owned_if_enabled(
                    OwnedWorkObservation::Granted {
                        grant: &grant,
                        budget: &budget,
                        trace,
                    },
                    &result,
                );
                drop(started.send(Err(error.clone())));
                drop(reply.send(Err(error)));
                return;
            }
            let started_message = OwnedRunStarted {
                grant: grant.clone(),
                cancel: request_budget.cancel_handle(),
            };
            if started
                .send(Ok(OwnedRunStart::Granted(started_message)))
                .is_err()
            {
                request_budget.cancel_handle().cancel();
                // No receiver accepted the grant. The worker still owns
                // settlement and must clear durable active state.
                let settlement = settle_abandoned_owned(&kernel, &grant);
                if let Err(error) = &settlement
                    && let Some(lease) = &lease
                {
                    lease.telemetry.record_failure(error.clone());
                }
                let error = settlement.err().unwrap_or(IndexError::Cancelled);
                let result = Err(error.clone());
                finish_owned_execution(&mut span, &result);
                native_work_diagnostic::record_owned_if_enabled(
                    OwnedWorkObservation::Granted {
                        grant: &grant,
                        budget: &budget,
                        trace,
                    },
                    &result,
                );
                drop(reply.send(Err(error)));
                return;
            }
            #[cfg(test)]
            if let Some(barrier) = barriers.after_start {
                barrier.wait();
            }
            let request = grant.assignment.request.clone();
            let result = match budget.checkpoint() {
                Err(error) => Err(error),
                Ok(()) if !matches!(&request.kind, UpdateKind::Materialize { .. }) => Err(
                    IndexError::BackendViolation("owned work is not materialization".into()),
                ),
                Ok(()) => match kernel.status(
                    request.operation,
                    quanta_index_core::index::ports::OperationInspectAccess::Request(&budget),
                ) {
                    Ok(OperationStatus::Unknown) => {
                        kernel.submit_owned_work(request, &mut budget).await
                    }
                    Ok(OperationStatus::Pending(_)) => {
                        kernel.resume_owned_work(&request, &mut budget).await
                    }
                    Ok(status @ (OperationStatus::Published(_) | OperationStatus::Refused(_))) => {
                        Ok(SubmitOutcome::Terminal(status))
                    }
                    Err(error) => Err(error),
                },
            };
            // Exclude the zero-capacity reply handoff from kernel execution.
            finish_owned_execution(&mut span, &result);
            native_work_diagnostic::record_owned_if_enabled(
                OwnedWorkObservation::Granted {
                    grant: &grant,
                    budget: &budget,
                    trace,
                },
                &result,
            );
            // A zero-capacity reply transfers settlement ownership only
            // when the pump actually receives the result. If its receiver
            // disappeared, the worker settles after execution quiesces.
            if reply.send(result).is_err()
                && let Err(error) = settle_abandoned_owned(&kernel, &grant)
                && let Some(lease) = &lease
            {
                lease.telemetry.record_failure(error);
            }
        }
        WorkerCommand::Stop => {}
        #[cfg(test)]
        WorkerCommand::TimerProbe { finished } => {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            finished.store(true, std::sync::atomic::Ordering::Release);
        }
        #[cfg(test)]
        WorkerCommand::QueueBarrier { entered, release } => {
            let _sent = entered.send(());
            drop(release.await);
        }
        #[cfg(test)]
        WorkerCommand::Panic { entered } => {
            let _sent = entered.send(());
            std::panic::resume_unwind(Box::new("test native worker task panic"));
        }
    }
}

async fn run_lane(
    mut receiver: mpsc::Receiver<QueuedCommand>,
    resources: Arc<NativeClassScheduler>,
    need: ResourceNeed,
    max_active: usize,
    provider_owner: ProviderOwner,
) -> Result<(), IndexError> {
    let mut active = tokio::task::JoinSet::new();
    let mut accepting = true;
    let mut failure = None;
    loop {
        if !accepting && active.is_empty() {
            break;
        }
        tokio::select! {
            completed = active.join_next(), if !active.is_empty() => {
                if let Some(Err(error)) = completed {
                    let _observed = failure.get_or_insert_with(|| IndexError::Storage(format!(
                        "native operation task failed: {error}"
                    )));
                    receiver.close();
                    while receiver.try_recv().is_ok() {}
                    accepting = false;
                }
            }
            queued = receiver.recv(), if accepting && active.len() < max_active => {
                if let Some(queued) = queued {
                    let _observed = active.spawn(run_command(queued, Arc::clone(&resources), need, Arc::clone(&provider_owner)));
                } else {
                    accepting = false;
                }
            }
        }
    }
    failure.map_or(Ok(()), Err)
}

fn settle_abandoned_owned(kernel: &NativeKernel, grant: &OwnedWorkGrant) -> Result<(), IndexError> {
    match kernel.finish_owned_work(grant, OwnedWorkReport::Interrupted) {
        Ok(()) | Err(IndexError::WorkFenceLost) => Ok(()),
        Err(error) => Err(error),
    }
}

enum WorkerCommand {
    Submit {
        kernel: Arc<NativeKernel>,
        request: Box<UpdateRequest>,
        budget: WorkBudget,
        reply: WorkerReply,
        #[cfg(test)]
        after_reply: Option<OwnedTestBarrier>,
    },
    Resume {
        kernel: Arc<NativeKernel>,
        key: OperationKey,
        budget: WorkBudget,
        reply: WorkerReply,
        #[cfg(test)]
        after_reply: Option<OwnedTestBarrier>,
    },
    Owned {
        kernel: Arc<NativeKernel>,
        assignment: Box<OwnedWorkAssignment>,
        cancellation: RequestBudgetV1,
        diagnostic: Option<DiagnosticContextV1>,
        started: sync_mpsc::SyncSender<Result<OwnedRunStart, IndexError>>,
        reply: WorkerReply,
        quiesced: sync_mpsc::SyncSender<()>,
        #[cfg(test)]
        barriers: OwnedTestBarriers,
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

/// Core has one active operation. Enrichment has an explicitly bounded number
/// of active operations across distinct scopes; each lane has one waiting slot.
///
/// The runtime is created and dropped by the
/// dedicated manager thread, never by an asynchronous task.
struct WorkerShutdown {
    core_sender: Option<Sender<QueuedCommand>>,
    enrichment_sender: Option<Sender<QueuedCommand>>,
    manager: Option<JoinHandle<()>>,
    joining: bool,
    result: Option<Result<(), IndexError>>,
}

pub(super) struct NativeOperationWorker {
    provider_owner: ProviderOwner,
    shutdown: Mutex<WorkerShutdown>,
    shutdown_done: Condvar,
    telemetry: Arc<WorkerTelemetry>,
    scope_registry: Arc<ScopeRegistry>,
    direct_enrichment: Arc<DirectEnrichmentAdmission>,
    control_scheduler: Arc<NativeClassScheduler>,
    control_need: ResourceNeed,
    #[cfg(test)]
    owned_test_barriers: Mutex<OwnedTestBarriers>,
    #[cfg(test)]
    direct_after_reply: Mutex<Option<OwnedTestBarrier>>,
}

impl NativeOperationWorker {
    pub(super) fn bind_provider_owner(
        &self,
        owner: Arc<dyn Send + Sync>,
    ) -> Result<(), IndexError> {
        self.provider_owner
            .set(owner)
            .map_err(|_owner| IndexError::Conflict)
    }
    pub(super) fn reserve_owned_slot(
        &self,
        workspace: WorkspaceKey,
        epoch: u64,
    ) -> Result<OwnedEnrichmentSlot, IndexError> {
        let scope = self
            .scope_registry
            .acquire(ExecutionScope { workspace, epoch })?;
        let shutdown = self.shutdown.lock().map_err(|error| {
            IndexError::Storage(format!("native worker shutdown lock: {error}"))
        })?;
        let sender = shutdown
            .enrichment_sender
            .as_ref()
            .ok_or_else(|| IndexError::Storage("native enrichment worker is stopped".into()))?;
        let permit = sender
            .clone()
            .try_reserve_owned()
            .map_err(|error| match error {
                TrySendError::Full(_sender) => IndexError::ResourceLimited,
                TrySendError::Closed(_sender) => {
                    IndexError::Storage("native enrichment worker closed".into())
                }
            })?;
        drop(shutdown);
        Ok(OwnedEnrichmentSlot { permit, scope })
    }

    pub(super) fn dispatch_owned(
        &self,
        slot: OwnedEnrichmentSlot,
        kernel: Arc<NativeKernel>,
        assignment: OwnedWorkAssignment,
        diagnostic: Option<DiagnosticContextV1>,
    ) -> Result<OwnedRunHandle, IndexError> {
        // Rendezvous makes successful send an explicit transfer of settlement
        // ownership to the pump, never an unread buffered message.
        let (started_tx, started_rx) = sync_mpsc::sync_channel(0);
        let (reply_tx, reply_rx) = sync_mpsc::sync_channel(0);
        let (quiesced_tx, quiesced_rx) = sync_mpsc::sync_channel(1);
        let cancellation = OwnedRunCancellation {
            relay: RequestBudgetV1::for_duration(Duration::from_millis(
                assignment.policy.total_timeout_ms,
            )),
        };
        let shutdown = self.shutdown.lock().map_err(|error| {
            IndexError::Storage(format!("native worker shutdown lock: {error}"))
        })?;
        if shutdown.joining || shutdown.result.is_some() || shutdown.enrichment_sender.is_none() {
            return Err(IndexError::Storage(
                "native enrichment worker is stopped".into(),
            ));
        }
        let OwnedEnrichmentSlot { permit, scope } = slot;
        if scope.scope != ExecutionScope::from(assignment.request.operation) {
            return Err(IndexError::BackendViolation(
                "owned assignment differs from reserved enrichment scope".into(),
            ));
        }
        enqueue_reserved(
            &self.telemetry,
            OperationExecutionClass::Enrichment,
            WorkerCommand::Owned {
                kernel,
                assignment: Box::new(assignment),
                cancellation: cancellation.relay.clone(),
                diagnostic,
                started: started_tx,
                reply: reply_tx,
                quiesced: quiesced_tx,
                #[cfg(test)]
                barriers: std::mem::take(&mut *self.owned_test_barriers.lock().map_err(
                    |error| IndexError::Storage(format!("owned test barrier lock: {error}")),
                )?),
            },
            Some(scope),
            |queued| {
                let _sender = permit.send(queued);
            },
        )?;
        drop(shutdown);
        Ok(OwnedRunHandle {
            started: started_rx,
            finished: reply_rx,
            quiesced: quiesced_rx,
            cancellation,
        })
    }

    pub(super) fn start_with_resources(
        resources: &Arc<NativeResourceLedger>,
        plan: NativeResourcePlan,
        core_demand: ResourceNeed,
        enrichment_demand: ResourceNeed,
        max_enrichment_active: u32,
    ) -> Result<Self, IndexError> {
        plan.validate()?;
        if !(1..=256).contains(&max_enrichment_active)
            || !core_demand.fits(plan.core)
            || !enrichment_demand.fits(plan.enrichment)
        {
            return Err(IndexError::InvalidInput(
                "native builder demand or enrichment concurrency is invalid".into(),
            ));
        }
        let core_need = ResourceNeed {
            provider_units: core_demand.provider_units,
            ..plan.core
        };
        let enrichment_need = ResourceNeed {
            provider_units: enrichment_demand.provider_units,
            ..plan.enrichment
        };
        let max_enrichment = usize::try_from(max_enrichment_active)
            .map_err(|error| IndexError::InvalidInput(format!("native concurrency: {error}")))?;
        let worker_threads = max_enrichment
            .checked_add(2)
            .ok_or(IndexError::ResourceLimited)?;
        let (core_sender, core_receiver) = mpsc::channel(1);
        let (enrichment_sender, enrichment_receiver) = mpsc::channel(1);
        let (started_tx, started_rx) = sync_mpsc::sync_channel(1);
        let telemetry = Arc::new(WorkerTelemetry::with_enrichment_limit(
            u16::try_from(max_enrichment_active)
                .map_err(|_overflow| IndexError::ResourceLimited)?,
        ));
        let manager_telemetry = Arc::clone(&telemetry);
        let core_resources = resources.scheduler(NativeResourceClass::Core);
        let enrichment_resources = resources.scheduler(NativeResourceClass::Enrichment);
        let control_scheduler = resources.scheduler(NativeResourceClass::Control);
        let provider_owner = Arc::new(OnceLock::new());
        let core_owner = Arc::clone(&provider_owner);
        let enrichment_owner = Arc::clone(&provider_owner);
        let manager = thread::Builder::new()
            .name("native-operation-runtime".into())
            .spawn(move || {
                let runtime = match tokio::runtime::Builder::new_multi_thread()
                    .worker_threads(worker_threads)
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
                    let result =
                        run_lane(core_receiver, core_resources, core_need, 1, core_owner).await;
                    if let Err(error) = result {
                        guard.telemetry.record_failure(error);
                    }
                    guard.completed = true;
                    let _completion = core_done.send(());
                });
                let enrichment_telemetry = Arc::clone(&manager_telemetry);
                let _enrichment_task = runtime.spawn(async move {
                    let mut guard = LaneTaskGuard {
                        telemetry: enrichment_telemetry,
                        completed: false,
                    };
                    let result = run_lane(
                        enrichment_receiver,
                        enrichment_resources,
                        enrichment_need,
                        max_enrichment,
                        enrichment_owner,
                    )
                    .await;
                    if let Err(error) = result {
                        guard.telemetry.record_failure(error);
                    }
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
                provider_owner,
                shutdown: Mutex::new(WorkerShutdown {
                    core_sender: Some(core_sender),
                    enrichment_sender: Some(enrichment_sender),
                    manager: Some(manager),
                    joining: false,
                    result: None,
                }),
                shutdown_done: Condvar::new(),
                telemetry,
                scope_registry: Arc::new(ScopeRegistry::default()),
                direct_enrichment: Arc::new(DirectEnrichmentAdmission::default()),
                control_scheduler,
                control_need: plan.control,
                #[cfg(test)]
                owned_test_barriers: Mutex::new(OwnedTestBarriers::default()),
                #[cfg(test)]
                direct_after_reply: Mutex::new(None),
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

    #[cfg(test)]
    fn start() -> Result<Self, IndexError> {
        let unit = ResourceNeed {
            cpu_units: 1,
            io_bytes: 1024,
            // Real fixture objects coexist with their encoded read buffer.
            memory_bytes: 64 * 1024,
            provider_units: 0,
        };
        let plan = NativeResourcePlan {
            capacity: ResourceNeed {
                cpu_units: 4,
                io_bytes: 4096,
                memory_bytes: unit
                    .memory_bytes
                    .checked_mul(5)
                    .ok_or(IndexError::ResourceLimited)?,
                ..unit
            },
            control: unit,
            interactive: unit,
            core: unit,
            enrichment: unit,
        };
        let resources = NativeResourceLedger::new(plan)?;
        Self::start_with_resources(
            &resources,
            plan,
            ResourceNeed::default(),
            ResourceNeed::default(),
            1,
        )
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
        let shutdown = self.shutdown.lock().map_err(|error| {
            IndexError::Storage(format!("native worker shutdown lock: {error}"))
        })?;
        let sender = match class {
            OperationExecutionClass::Core => &shutdown.core_sender,
            OperationExecutionClass::Enrichment => &shutdown.enrichment_sender,
        }
        .as_ref()
        .ok_or_else(|| IndexError::Storage("test native worker sender missing".into()))?;
        // Keep the shutdown lock through enqueue, so stop_join cannot close
        // the lane between the sender check and this accepted command.
        let result = enqueue(sender, &self.telemetry, class, command, None);
        drop(shutdown);
        result
    }

    pub(super) fn submit(
        &self,
        kernel: Arc<NativeKernel>,
        request: UpdateRequest,
        budget: WorkBudget,
    ) -> Result<SubmitOutcome, IndexError> {
        budget.checkpoint()?;
        let class = OperationExecutionClass::for_request(&request);
        let key = request.operation;
        #[cfg(test)]
        let after_reply = self
            .direct_after_reply
            .lock()
            .map_err(|error| {
                IndexError::Storage(format!("native direct test barrier lock: {error}"))
            })?
            .take();
        self.execute(class, key, |reply| WorkerCommand::Submit {
            kernel,
            request: Box::new(request),
            budget,
            reply,
            #[cfg(test)]
            after_reply,
        })
    }

    pub(super) fn resume(
        &self,
        kernel: Arc<NativeKernel>,
        key: OperationKey,
        mut budget: WorkBudget,
    ) -> Result<SubmitOutcome, IndexError> {
        budget.checkpoint()?;
        let dispatch = {
            let _reservation = ReservationGuard::try_acquire(
                Arc::clone(&self.control_scheduler),
                self.control_need,
            )
            .ok_or_else(|| self.control_scheduler.denial())?;
            kernel.resume_dispatch(key, &mut budget)?
        };
        match dispatch {
            ResumeDispatch::Reply(outcome) => Ok(outcome),
            ResumeDispatch::Execute(class) => {
                budget.checkpoint()?;
                #[cfg(test)]
                let after_reply = self
                    .direct_after_reply
                    .lock()
                    .map_err(|error| {
                        IndexError::Storage(format!("native direct test barrier lock: {error}"))
                    })?
                    .take();
                self.execute(class, key, |reply| WorkerCommand::Resume {
                    kernel,
                    key,
                    budget,
                    reply,
                    #[cfg(test)]
                    after_reply,
                })
            }
        }
    }

    fn execute(
        &self,
        class: OperationExecutionClass,
        key: OperationKey,
        command: impl FnOnce(WorkerReply) -> WorkerCommand,
    ) -> Result<SubmitOutcome, IndexError> {
        let _direct_admission = match class {
            OperationExecutionClass::Core => None,
            OperationExecutionClass::Enrichment => Some(self.direct_enrichment.acquire()?),
        };
        let scope = match class {
            OperationExecutionClass::Core => None,
            OperationExecutionClass::Enrichment => Some(self.scope_registry.acquire(key.into())?),
        };
        let (reply_tx, reply_rx) = sync_mpsc::sync_channel(1);
        {
            let shutdown = self.shutdown.lock().map_err(|error| {
                IndexError::Storage(format!("native worker shutdown lock: {error}"))
            })?;
            let sender = match class {
                OperationExecutionClass::Core => &shutdown.core_sender,
                OperationExecutionClass::Enrichment => &shutdown.enrichment_sender,
            }
            .as_ref()
            .ok_or_else(|| IndexError::Storage("native operation worker is stopped".into()))?;
            enqueue(sender, &self.telemetry, class, command(reply_tx), scope)?;
            drop(shutdown);
        }
        reply_rx.recv().map_err(|error| {
            IndexError::Storage(format!("native operation worker lost result: {error}"))
        })?
    }
}

impl NativeOperationWorker {
    /// Close both lanes, wait for accepted work to quiesce, then expose the
    /// first worker-owned settlement failure.
    ///
    /// Concurrent callers share one
    /// result; no shutdown mutex is held while joining the runtime thread.
    pub(super) fn stop_join(&self) -> Result<(), IndexError> {
        let (core, enrichment, manager) = {
            let mut shutdown = self.shutdown.lock().map_err(|error| {
                IndexError::Storage(format!("native worker shutdown lock: {error}"))
            })?;
            loop {
                if let Some(result) = &shutdown.result {
                    return result.clone();
                }
                if !shutdown.joining {
                    shutdown.joining = true;
                    break (
                        shutdown.core_sender.take(),
                        shutdown.enrichment_sender.take(),
                        shutdown.manager.take(),
                    );
                }
                shutdown = self.shutdown_done.wait(shutdown).map_err(|error| {
                    IndexError::Storage(format!("native worker shutdown wait: {error}"))
                })?;
            }
        };
        self.telemetry.mark_stopping();
        for sender in [core, enrichment].into_iter().flatten() {
            // A full waiting slot drains its operation when the sender closes.
            drop(sender.try_send(QueuedCommand {
                command: WorkerCommand::Stop,
                lease: None,
                _scope: None,
            }));
            drop(sender);
        }
        let manager_failed = manager.is_some_and(|manager| manager.join().is_err());
        if manager_failed {
            self.telemetry.transition(NativeWorkerStatus::Failed);
        }
        self.telemetry.mark_stopped();
        let result = self
            .telemetry
            .terminal_error()
            .and_then(|error| error.map_or(Ok(()), Err));
        let mut shutdown = self.shutdown.lock().map_err(|error| {
            IndexError::Storage(format!("native worker shutdown lock: {error}"))
        })?;
        shutdown.result = Some(result.clone());
        shutdown.joining = false;
        drop(shutdown);
        self.shutdown_done.notify_all();
        result
    }
}

impl Drop for NativeOperationWorker {
    fn drop(&mut self) {
        drop(self.stop_join());
    }
}

#[cfg(test)]
mod tests {
    use std::{
        error::Error,
        io::Write as _,
        sync::atomic::{AtomicBool, Ordering},
        time::{Duration, Instant},
    };

    use quanta_index_core::{
        DiagnosticLaneV1, DiagnosticRootKindV1, RequestBudgetV1,
        index::{
            AdmissionOutcome, CollectionId, Digest, ImmutableOperation, IndexInstanceId,
            IngressRef, OperationStatus, OwnedWorkPolicy, OwnerInputBinding, OwnerKey,
            PreparedPublication, ProducerId, ProjectionId, RegisteredObject, SlotId, StableOwnerId,
            StreamId, TenantId, UpdateKind, ViewSelection, WorkAction, WorkActionDigest,
            WorkActionSet, WorkClaim, WorkIntent, WorkLimits, WorkScope, WorkspaceKey, WorktreeId,
            ports::{
                CatalogPublicationPort, ImmutableObjectPort, InputStagingPort, ObjectWriteSink,
                OperationJournalPort, WorkActionCursor, WorkClaimPort,
            },
        },
    };
    use quanta_index_kernel::{AdapterRegistry, KernelServices};
    use quanta_index_searchd::{
        DiagnosticCaptureLimitsV1, DiagnosticCaptureStatusV1, DiagnosticCaptureV1,
    };
    use quanta_index_storage::FileObjectStore;
    use serde_json::Value;

    use super::*;

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
            UpdateKind::SourceAdvance(_)
            | UpdateKind::SourceResync { .. }
            | UpdateKind::ProjectionAdvance(_) => {
                return Err("fixture request kind has no test input staging".into());
            }
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
        check!(matches!(
            fixture.catalog.admit(
                ImmutableOperation {
                    key,
                    request: stored.reference,
                    source_mode: fixture.request.kind.source_mode(),
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
            KernelServices::new(
                objects.clone(),
                objects.clone(),
                catalog.clone(),
                Arc::new(quanta_index_storage::FileScratchSpace::open_standalone(
                    directory.path().join("scratch"),
                    16 * 1024 * 1024 * 1024,
                )?),
                Duration::from_secs(30),
            )?,
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
            .pin_view(ViewSelection::Exact(genesis), &work_budget())?
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

    struct OneAction(Option<WorkAction>);

    impl WorkActionCursor for OneAction {
        fn next_action(
            &mut self,
            budget: &mut WorkBudget,
        ) -> Result<Option<WorkAction>, IndexError> {
            budget.checkpoint()?;
            Ok(self.0.take())
        }
    }

    fn owned_assignment_fixture(fixture: &JournalFixture) -> TestResult<OwnedWorkAssignment> {
        owned_assignment_fixture_with_policy(
            fixture,
            OwnedWorkPolicy {
                dispatch_limits: limits(),
                max_dispatches: 2,
                total_timeout_ms: 360_000,
                lease_ms: 300_000,
                retry_backoff_ms: 1,
            },
        )
    }

    fn owned_assignment_fixture_with_policy(
        fixture: &JournalFixture,
        policy: OwnedWorkPolicy,
    ) -> TestResult<OwnedWorkAssignment> {
        admit_pending(fixture)?;
        let OperationStatus::Pending(attempt) = fixture.catalog.inspect(
            fixture.request.operation,
            quanta_index_core::index::ports::OperationInspectAccess::Request(&work_budget()),
        )?
        else {
            return Err("fixture admission did not produce pending attempt".into());
        };
        let genesis = fixture
            .kernel
            .pin_view(
                ViewSelection::Exact(fixture.request.expected_view),
                &work_budget(),
            )?
            .view()
            .clone();
        let mut next = genesis.clone();
        next.identity.sequence = next
            .identity
            .sequence
            .checked_add(1)
            .ok_or("fixture view sequence overflow")?;
        next.identity.digest = next.identity_digest()?;
        let intent = WorkIntent {
            workspace: fixture.request.operation.workspace,
            epoch: fixture.request.operation.epoch,
            instance: IndexInstanceId([40; 16]),
            scope: WorkScope::Owner {
                owner: OwnerKey {
                    producer: ProducerId([5; 16]),
                    stable_id: StableOwnerId([41; 16]),
                },
                expected: OwnerInputBinding {
                    source: None,
                    projection: None,
                    producer_context: Digest([42; 32]),
                    derivation_policy: Digest([43; 32]),
                },
            },
        };
        let action = WorkAction::Create(intent);
        let mut summary = WorkActionDigest::default();
        summary.push(&action)?;
        let published = fixture.catalog.commit_transition(
            PreparedPublication {
                attempt,
                source_mode: fixture.request.kind.source_mode(),
                expected: genesis.identity,
                next: next.clone(),
                work_actions: WorkActionSet {
                    count: summary.count(),
                    digest: summary.finish(),
                },
                complete_work: None,
                source_events: vec![],
                projection_events: vec![],
                expected_source: None,
            },
            &mut OneAction(Some(action)),
            &mut work_budget(),
        )?;
        check_eq!(published, OperationStatus::Published(next.identity));
        check_eq!(
            fixture.catalog.work_status(&intent, &work_budget())?,
            quanta_index_core::index::WorkStatus::Pending
        );
        let assignment = fixture
            .catalog
            .acquire_owned_work(&intent, policy, &work_budget())?
            .ok_or("fixture owned assignment missing")?;
        Ok(assignment)
    }

    fn assert_owned_settled(fixture: &JournalFixture, key: OperationKey) -> TestResult {
        let database = rusqlite::Connection::open(fixture.catalog.path())?;
        let payload: Vec<u8> = database.query_row(
            "SELECT payload FROM native_owned_work_v1 WHERE active=0",
            [],
            |row| row.get(0),
        )?;
        let stored: OwnedWorkAssignment = serde_json::from_slice(&payload)?;
        check_eq!(stored.request.operation, key);
        check_eq!(stored.dispatches, 1);
        check!(!stored.active);
        check_eq!(
            fixture.kernel.status(
                key,
                quanta_index_core::index::ports::OperationInspectAccess::Request(&work_budget())
            )?,
            OperationStatus::Unknown
        );
        Ok(())
    }

    fn install_owned_test_barriers(
        worker: &NativeOperationWorker,
        barriers: OwnedTestBarriers,
    ) -> Result<(), IndexError> {
        *worker.owned_test_barriers.lock().map_err(|error| {
            IndexError::Storage(format!("native owned test barrier lock: {error}"))
        })? = barriers;
        Ok(())
    }

    #[test]
    fn owned_execution_trace_rebinds_original_grant_and_closes_before_reply() -> TestResult {
        let deadline = Instant::now()
            .checked_add(Duration::from_secs(30))
            .ok_or("owned trace fixture deadline overflow")?;
        let off_original = RequestBudgetV1::until(deadline);
        let (off, off_span) = begin_owned_execution(off_original.clone(), None);
        check!(off_span.is_none() && off.diagnostic_context().is_none());
        check!(off.same_request_as(&off_original));
        check_eq!(off.deadline(), deadline);

        let directory = tempfile::tempdir()?;
        let path = directory.path().join("owned.jsonl");
        let mut capture =
            DiagnosticCaptureV1::start(path.clone(), DiagnosticCaptureLimitsV1::default())?;
        let lease = capture.lease().ok_or("owned trace capture lease absent")?;
        let root = lease
            .context()
            .enter_root(DiagnosticRootKindV1::Operation, DiagnosticLaneV1(0))?;
        let root_id = root.span_id().get();
        let parent = lease.context().child_context(&root);
        let original = RequestBudgetV1::until(deadline);
        let (record, mut span) = begin_owned_execution(original.clone(), Some(&parent));
        check!(record.same_request_as(&original));
        check_eq!(record.deadline(), off.deadline());
        let span_id = span
            .as_ref()
            .ok_or("owned execution span absent")?
            .span_id()
            .get();
        let budget = WorkBudget::with_request(limits(), record);
        check!(budget.request().same_request_as(&original));
        let nested = budget
            .request()
            .diagnostic_context()
            .ok_or("owned execution child context absent")?
            .enter(DiagnosticStageV1::SqliteWork, DiagnosticLaneV1(1))?;
        let nested_id = nested.span_id().get();
        nested.finish(DiagnosticTerminalV1::Ok)?;
        let result: Result<SubmitOutcome, IndexError> = Err(IndexError::ResourceLimited);
        finish_owned_execution(&mut span, &result);
        check!(span.is_none());
        let (cancel_request, mut cancelled_span) =
            begin_owned_execution(RequestBudgetV1::until(deadline), Some(&parent));
        let cancelled_span_id = cancelled_span
            .as_ref()
            .ok_or("owned cancellation span absent")?
            .span_id()
            .get();
        cancel_request.cancel_handle().cancel();
        finish_owned_execution(&mut cancelled_span, &Err(IndexError::Cancelled));
        check!(cancelled_span.is_none());
        let (unknown_request, mut unknown_span) =
            begin_owned_execution(RequestBudgetV1::until(deadline), Some(&parent));
        let unknown_span_id = unknown_span
            .as_ref()
            .ok_or("owned indeterminate result span absent")?
            .span_id()
            .get();
        finish_owned_execution(
            &mut unknown_span,
            &Ok(SubmitOutcome::Terminal(OperationStatus::Unknown)),
        );
        check!(unknown_span.is_none());
        original.cancel_handle().cancel();
        off_original.cancel_handle().cancel();
        check!(matches!(budget.checkpoint(), Err(IndexError::Cancelled)));
        let off_budget = WorkBudget::with_request(limits(), off);
        check!(matches!(
            off_budget.checkpoint(),
            Err(IndexError::Cancelled)
        ));
        drop(budget);
        drop(off_budget);
        drop(cancel_request);
        drop(unknown_request);
        drop(parent);
        root.finish(DiagnosticTerminalV1::Ok)?;
        drop(lease);
        let closed = capture.close(Duration::from_secs(2))?;
        check_eq!(closed.status, DiagnosticCaptureStatusV1::Complete);
        let rows: Vec<Value> = std::fs::read_to_string(path)?
            .lines()
            .map(serde_json::from_str)
            .collect::<Result<_, _>>()?;
        check!(rows.iter().any(|row| {
            row.pointer("/kind/type").and_then(Value::as_str) == Some("span_begin")
                && row.pointer("/kind/stage").and_then(Value::as_str) == Some("operation")
                && row.pointer("/kind/span_id").and_then(Value::as_u64) == Some(span_id)
                && row.pointer("/kind/parent_span_id").and_then(Value::as_u64) == Some(root_id)
        }));
        check!(rows.iter().any(|row| {
            row.pointer("/kind/type").and_then(Value::as_str) == Some("span_begin")
                && row.pointer("/kind/span_id").and_then(Value::as_u64) == Some(nested_id)
                && row.pointer("/kind/parent_span_id").and_then(Value::as_u64) == Some(span_id)
        }));
        check!(rows.iter().any(|row| {
            row.pointer("/kind/type").and_then(Value::as_str) == Some("span_end")
                && row.pointer("/kind/span_id").and_then(Value::as_u64) == Some(span_id)
                && row.pointer("/kind/outcome").and_then(Value::as_str) == Some("error")
        }));
        check!(rows.iter().any(|row| {
            row.pointer("/kind/type").and_then(Value::as_str) == Some("span_end")
                && row.pointer("/kind/span_id").and_then(Value::as_u64) == Some(cancelled_span_id)
                && row.pointer("/kind/outcome").and_then(Value::as_str) == Some("cancelled")
        }));
        check!(rows.iter().any(|row| {
            row.pointer("/kind/type").and_then(Value::as_str) == Some("span_end")
                && row.pointer("/kind/span_id").and_then(Value::as_u64) == Some(unknown_span_id)
                && row.pointer("/kind/outcome").and_then(Value::as_str) == Some("aborted")
        }));
        Ok(())
    }

    #[test]
    fn owned_pre_grant_wait_is_not_subtracted_twice() -> TestResult {
        let fixture = journal_fixture()?;
        let assignment = owned_assignment_fixture_with_policy(
            &fixture,
            OwnedWorkPolicy {
                dispatch_limits: limits(),
                max_dispatches: 2,
                total_timeout_ms: 2_000,
                lease_ms: 2_000,
                retry_backoff_ms: 1,
            },
        )?;
        let worker = NativeOperationWorker::start()?;
        let (reached_tx, reached_rx) = sync_mpsc::sync_channel(1);
        let (release_tx, release_rx) = sync_mpsc::sync_channel(1);
        install_owned_test_barriers(
            &worker,
            OwnedTestBarriers {
                before_grant: Some(OwnedTestBarrier {
                    reached: reached_tx,
                    release: release_rx,
                }),
                ..OwnedTestBarriers::default()
            },
        )?;
        let run = worker.dispatch_owned(
            worker.reserve_owned_slot(
                assignment.request.operation.workspace,
                assignment.request.operation.epoch,
            )?,
            Arc::clone(&fixture.kernel),
            assignment,
            None,
        )?;
        reached_rx.recv_timeout(Duration::from_secs(5))?;
        // Time before Catalog samples its clock is already absent from the
        // returned allowance. It must not also move the worker deadline back.
        thread::sleep(Duration::from_millis(1_100));
        release_tx.send(())?;
        let Ok(OwnedRunStart::Granted(started)) =
            run.started.recv_timeout(Duration::from_secs(5))?
        else {
            return Err("owned grant was not started after pre-grant release".into());
        };
        let result = run.finished.recv_timeout(Duration::from_secs(5))?;
        fixture
            .kernel
            .finish_owned_work(&started.grant, OwnedWorkReport::Reconcile)?;
        drop(run);
        drop(worker);
        // This fixture has no matching profile instance: executing it must
        // reach the public Conflict receipt, rather than expire at entry.
        check_eq!(
            result,
            Ok(SubmitOutcome::Terminal(OperationStatus::Refused(
                quanta_index_core::index::Refusal::Conflict
            )))
        );
        Ok(())
    }

    #[test]
    fn owned_grant_delay_does_not_restart_original_deadline() -> TestResult {
        const CHILD_ENV: &str = "QIDX_OWNED_GRANT_TRACE_JOIN_TEST_CHILD";
        const TRACE_ENV: &str = "QIDX_OWNED_GRANT_TRACE_JOIN_TEST_PATH";
        const TEST_NAME: &str =
            "native_worker::tests::owned_grant_delay_does_not_restart_original_deadline";
        const EVIDENCE: &str = "QIDX_OWNED_GRANT_TRACE_JOIN_EVIDENCE=";

        if std::env::var_os(CHILD_ENV).as_deref() != Some(std::ffi::OsStr::new("1")) {
            let directory = tempfile::tempdir()?;
            let trace_path = directory.path().join("owned-grant-join.jsonl");
            let output = std::process::Command::new(std::env::current_exe()?)
                .args(["--exact", TEST_NAME, "--nocapture", "--test-threads=1"])
                .env(CHILD_ENV, "1")
                .env(TRACE_ENV, &trace_path)
                .env("QUANTA_INDEX_NATIVE_WORK_DIAGNOSTICS_V1", "1")
                .output()?;
            let tail = |bytes: &[u8]| {
                String::from_utf8_lossy(
                    bytes
                        .get(bytes.len().saturating_sub(4096)..)
                        .unwrap_or_default(),
                )
                .into_owned()
            };
            if !output.status.success()
                || output.stdout.len() > 64 * 1024
                || output.stderr.len() > 64 * 1024
            {
                return Err(format!(
                        "owned grant child status={} stdout_bytes={} stderr_bytes={} stdout_tail={} stderr_tail={}",
                        output.status,
                        output.stdout.len(),
                        output.stderr.len(),
                        tail(&output.stdout),
                        tail(&output.stderr)
                    )
                    .into());
            }
            let validation = (|| -> TestResult {
                let stdout = std::str::from_utf8(&output.stdout)?;
                let stderr = std::str::from_utf8(&output.stderr)?;
                check!(stdout.contains("running 1 test"));
                check!(stdout.contains("test result: ok. 1 passed; 0 failed;"));
                let evidence_lines: Vec<_> = stdout
                    .lines()
                    .filter_map(|line| line.split_once(EVIDENCE).map(|(_, value)| value))
                    .collect();
                check_eq!(evidence_lines.len(), 1);
                let evidence: Value =
                    serde_json::from_str(evidence_lines.first().ok_or("grant evidence absent")?)?;
                let refusal_lines: Vec<_> = stderr
                    .lines()
                    .filter(|line| line.contains("\"event\":\"native_owned_work_refusal\""))
                    .collect();
                check_eq!(refusal_lines.len(), 1);
                let refusal: Value =
                    serde_json::from_str(refusal_lines.first().ok_or("owned refusal absent")?)?;
                check_eq!(refusal.get("operation_key"), evidence.get("operation_key"));
                check_eq!(
                    refusal.get("phase"),
                    Some(&serde_json::json!("granted_execution"))
                );
                check_eq!(
                    refusal.get("outcome"),
                    Some(&serde_json::json!("Err(ResourceLimited)"))
                );
                check_eq!(refusal.get("sample_ordinal"), Some(&serde_json::json!(1)));
                check_eq!(refusal.get("sample_limit"), Some(&serde_json::json!(8)));
                check_eq!(
                    refusal.pointer("/execution/dispatch_ordinal"),
                    evidence.get("dispatch_ordinal")
                );
                check_eq!(
                    refusal.pointer("/execution/assignment_dispatches"),
                    evidence.get("assignment_dispatches")
                );
                check_eq!(
                    refusal.pointer("/execution/assignment_deadline_ms"),
                    evidence.get("assignment_deadline_ms")
                );
                check_eq!(refusal.pointer("/execution/claim"), evidence.get("claim"));
                check_eq!(
                    refusal.pointer("/execution/trace/capture_id"),
                    evidence.get("capture_id")
                );
                let worker_span = refusal
                    .pointer("/execution/trace/span_id")
                    .and_then(Value::as_u64)
                    .ok_or("owned refusal has no worker span")?;
                check!(worker_span != 0);
                check!(Some(worker_span) != evidence.get("root_span_id").and_then(Value::as_u64));
                check!(Some(worker_span) != evidence.get("prior_span_id").and_then(Value::as_u64));
                check_eq!(
                    evidence.get("dispatch_ordinal"),
                    evidence.get("assignment_dispatches")
                );

                let rows: Vec<Value> = std::fs::read_to_string(&trace_path)?
                    .lines()
                    .map(serde_json::from_str)
                    .collect::<Result<_, _>>()?;
                let header = rows.first().ok_or("capture header absent")?;
                let footer = rows.last().ok_or("capture footer absent")?;
                check_eq!(
                    header.get("record_type"),
                    Some(&serde_json::json!("header"))
                );
                check_eq!(
                    footer.get("record_type"),
                    Some(&serde_json::json!("footer"))
                );
                check_eq!(header.get("capture_id"), evidence.get("capture_id"));
                check_eq!(footer.get("capture_id"), evidence.get("capture_id"));
                check_eq!(footer.get("status"), Some(&serde_json::json!("complete")));
                for field in [
                    "events_dropped",
                    "events_unwritten",
                    "open_spans",
                    "producer_leases",
                ] {
                    check_eq!(footer.get(field), Some(&serde_json::json!(0)));
                }
                let root_span = evidence
                    .get("root_span_id")
                    .and_then(Value::as_u64)
                    .ok_or("root span ID absent")?;
                let prior_span = evidence
                    .get("prior_span_id")
                    .and_then(Value::as_u64)
                    .ok_or("prior span ID absent")?;
                check!(rows.iter().any(|row| {
                    row.pointer("/kind/type").and_then(Value::as_str) == Some("span_begin")
                        && row.pointer("/kind/stage").and_then(Value::as_str) == Some("operation")
                        && row.pointer("/kind/span_id").and_then(Value::as_u64) == Some(prior_span)
                        && row.pointer("/kind/parent_span_id").and_then(Value::as_u64)
                            == Some(root_span)
                }));
                let worker_begins: Vec<_> = rows
                    .iter()
                    .filter(|row| {
                        row.pointer("/kind/type").and_then(Value::as_str) == Some("span_begin")
                            && row.pointer("/kind/span_id").and_then(Value::as_u64)
                                == Some(worker_span)
                    })
                    .collect();
                check_eq!(worker_begins.len(), 1);
                let begin = worker_begins.first().ok_or("worker span begin absent")?;
                check_eq!(begin.get("capture_id"), evidence.get("capture_id"));
                check_eq!(
                    begin.pointer("/kind/stage"),
                    Some(&serde_json::json!("operation"))
                );
                check_eq!(begin.pointer("/kind/lane_id"), Some(&serde_json::json!(1)));
                check_eq!(
                    begin.pointer("/kind/parent_span_id"),
                    Some(&serde_json::json!(root_span))
                );
                check!(rows.iter().any(|row| {
                    row.pointer("/kind/type").and_then(Value::as_str) == Some("span_end")
                        && row.pointer("/kind/span_id").and_then(Value::as_u64) == Some(worker_span)
                        && row.pointer("/kind/outcome").and_then(Value::as_str) == Some("error")
                }));
                Ok(())
            })();
            if let Err(error) = validation {
                return Err(format!(
                    "owned grant join validation failed: {error}; stdout_tail={}; stderr_tail={}",
                    tail(&output.stdout),
                    tail(&output.stderr)
                )
                .into());
            }
            return Ok(());
        }

        let fixture = journal_fixture()?;
        let assignment = owned_assignment_fixture_with_policy(
            &fixture,
            OwnedWorkPolicy {
                dispatch_limits: limits(),
                max_dispatches: 2,
                total_timeout_ms: 1_000,
                lease_ms: 1_000,
                retry_backoff_ms: 1,
            },
        )?;
        let trace_path = std::path::PathBuf::from(
            std::env::var_os(TRACE_ENV).ok_or("owned grant trace child path absent")?,
        );
        let mut capture =
            DiagnosticCaptureV1::start(trace_path, DiagnosticCaptureLimitsV1::default())?;
        let lease = capture.lease().ok_or("owned grant capture lease absent")?;
        let root = lease
            .context()
            .enter_root(DiagnosticRootKindV1::Operation, DiagnosticLaneV1(0))?;
        let root_span_id = root.span_id().get();
        let parent = lease.context().child_context(&root);
        // A previous operation in the same capture must never satisfy this grant's join.
        let prior = parent.enter(DiagnosticStageV1::Operation, DiagnosticLaneV1(1))?;
        let prior_span_id = prior.span_id().get();
        prior.finish(DiagnosticTerminalV1::Error { code: 1 })?;
        let worker = NativeOperationWorker::start()?;
        let (reached_tx, reached_rx) = sync_mpsc::sync_channel(1);
        let (release_tx, release_rx) = sync_mpsc::sync_channel(1);
        install_owned_test_barriers(
            &worker,
            OwnedTestBarriers {
                after_grant: Some(OwnedTestBarrier {
                    reached: reached_tx,
                    release: release_rx,
                }),
                ..OwnedTestBarriers::default()
            },
        )?;
        let run = worker.dispatch_owned(
            worker.reserve_owned_slot(
                assignment.request.operation.workspace,
                assignment.request.operation.epoch,
            )?,
            Arc::clone(&fixture.kernel),
            assignment,
            Some(parent.clone()),
        )?;
        reached_rx.recv_timeout(Duration::from_secs(5))?;
        // The original post-grant delay spends the same 1s Catalog grant.
        thread::sleep(Duration::from_millis(1_100));
        release_tx.send(())?;
        let Ok(OwnedRunStart::Granted(started)) =
            run.started.recv_timeout(Duration::from_secs(5))?
        else {
            return Err("owned grant was not started after release".into());
        };
        let result = run.finished.recv_timeout(Duration::from_secs(5))?;
        fixture
            .kernel
            .finish_owned_work(&started.grant, OwnedWorkReport::Interrupted)?;
        drop(run);
        drop(worker);
        check_eq!(result, Err(IndexError::ResourceLimited));
        let UpdateKind::Materialize { claim } = &started.grant.assignment.request.kind else {
            return Err("owned grant did not retain materialization claim".into());
        };
        drop(parent);
        root.finish(DiagnosticTerminalV1::Ok)?;
        drop(lease);
        let closed = capture.close(Duration::from_secs(2))?;
        check_eq!(closed.status, DiagnosticCaptureStatusV1::Complete);
        check_eq!(closed.events_dropped, 0);
        check_eq!(closed.events_unwritten, 0);
        let capture_id = native_work_diagnostic::capture_id_hex(closed.capture_id);
        writeln!(
            &mut std::io::stdout().lock(),
            "{EVIDENCE}{}",
            serde_json::json!({
                "capture_id": capture_id,
                "root_span_id": root_span_id,
                "prior_span_id": prior_span_id,
                "operation_key": started.grant.assignment.request.operation,
                "dispatch_ordinal": started.grant.ordinal,
                "assignment_dispatches": started.grant.assignment.dispatches,
                "assignment_deadline_ms": started.grant.assignment.deadline_ms,
                "claim": claim,
            })
        )?;
        Ok(())
    }

    #[test]
    fn owned_receiver_drop_after_grant_before_start_settles_active_assignment() -> TestResult {
        let fixture = journal_fixture()?;
        let assignment = owned_assignment_fixture(&fixture)?;
        let key = assignment.request.operation;
        let worker = NativeOperationWorker::start()?;
        let (reached_tx, reached_rx) = sync_mpsc::sync_channel(1);
        let (release_tx, release_rx) = sync_mpsc::sync_channel(1);
        install_owned_test_barriers(
            &worker,
            OwnedTestBarriers {
                after_grant: Some(OwnedTestBarrier {
                    reached: reached_tx,
                    release: release_rx,
                }),
                ..OwnedTestBarriers::default()
            },
        )?;
        let run = worker.dispatch_owned(
            worker.reserve_owned_slot(
                assignment.request.operation.workspace,
                assignment.request.operation.epoch,
            )?,
            Arc::clone(&fixture.kernel),
            assignment.clone(),
            None,
        )?;
        reached_rx.recv_timeout(Duration::from_secs(5))?;
        check_eq!(
            fixture.catalog.begin_owned_work(&assignment)?,
            OwnedWorkBegin::Deferred(OwnedWorkDefer::Active)
        );
        drop(run);
        release_tx.send(())?;
        drop(worker);
        assert_owned_settled(&fixture, key)
    }

    #[test]
    fn active_owned_assignment_skips_without_execution_receipt() -> TestResult {
        let fixture = journal_fixture()?;
        let assignment = owned_assignment_fixture(&fixture)?;
        let OwnedWorkBegin::Granted(external_grant) =
            fixture.catalog.begin_owned_work(&assignment)?
        else {
            return Err("fixture did not create active grant".into());
        };
        let worker = NativeOperationWorker::start()?;
        let run = worker.dispatch_owned(
            worker.reserve_owned_slot(
                assignment.request.operation.workspace,
                assignment.request.operation.epoch,
            )?,
            Arc::clone(&fixture.kernel),
            assignment.clone(),
            None,
        )?;
        check!(matches!(
            run.started.recv_timeout(Duration::from_secs(5))?,
            Ok(OwnedRunStart::Skipped(OwnedRunSkip::Deferred(
                OwnedWorkDefer::Active
            )))
        ));
        run.quiesced.recv_timeout(Duration::from_secs(5))?;
        check!(matches!(
            run.finished.recv_timeout(Duration::from_secs(5)),
            Err(sync_mpsc::RecvTimeoutError::Disconnected)
        ));
        check_eq!(
            fixture.catalog.begin_owned_work(&assignment)?,
            OwnedWorkBegin::Deferred(OwnedWorkDefer::Active)
        );
        fixture
            .kernel
            .finish_owned_work(&external_grant, OwnedWorkReport::Interrupted)?;
        drop(run);
        drop(worker);
        Ok(())
    }

    #[test]
    fn owned_receiver_drop_after_start_before_result_settles_active_assignment() -> TestResult {
        let fixture = journal_fixture()?;
        let assignment = owned_assignment_fixture(&fixture)?;
        let key = assignment.request.operation;
        let worker = NativeOperationWorker::start()?;
        let (reached_tx, reached_rx) = sync_mpsc::sync_channel(1);
        let (release_tx, release_rx) = sync_mpsc::sync_channel(1);
        install_owned_test_barriers(
            &worker,
            OwnedTestBarriers {
                after_start: Some(OwnedTestBarrier {
                    reached: reached_tx,
                    release: release_rx,
                }),
                ..OwnedTestBarriers::default()
            },
        )?;
        let run = worker.dispatch_owned(
            worker.reserve_owned_slot(
                assignment.request.operation.workspace,
                assignment.request.operation.epoch,
            )?,
            Arc::clone(&fixture.kernel),
            assignment.clone(),
            None,
        )?;
        check!(matches!(
            run.started.recv_timeout(Duration::from_secs(5))?,
            Ok(OwnedRunStart::Granted(_))
        ));
        reached_rx.recv_timeout(Duration::from_secs(5))?;
        check_eq!(
            fixture.catalog.begin_owned_work(&assignment)?,
            OwnedWorkBegin::Deferred(OwnedWorkDefer::Active)
        );
        drop(run);
        release_tx.send(())?;
        drop(worker);
        assert_owned_settled(&fixture, key)
    }

    #[test]
    fn abandoned_grant_settlement_storage_failure_survives_worker_closeout() -> TestResult {
        let fixture = journal_fixture()?;
        let assignment = owned_assignment_fixture(&fixture)?;
        let worker = NativeOperationWorker::start()?;
        let observation = worker.observation();
        let (reached_tx, reached_rx) = sync_mpsc::sync_channel(1);
        let (release_tx, release_rx) = sync_mpsc::sync_channel(1);
        install_owned_test_barriers(
            &worker,
            OwnedTestBarriers {
                after_grant: Some(OwnedTestBarrier {
                    reached: reached_tx,
                    release: release_rx,
                }),
                ..OwnedTestBarriers::default()
            },
        )?;
        let run = worker.dispatch_owned(
            worker.reserve_owned_slot(
                assignment.request.operation.workspace,
                assignment.request.operation.epoch,
            )?,
            Arc::clone(&fixture.kernel),
            assignment,
            None,
        )?;
        reached_rx.recv_timeout(Duration::from_secs(5))?;
        let database = rusqlite::Connection::open(fixture.catalog.path())?;
        database.execute_batch(
            "CREATE TRIGGER deny_owned_settlement_v1
             BEFORE UPDATE OF active ON native_owned_work_v1
             WHEN OLD.active=1 AND NEW.active=0
             BEGIN SELECT RAISE(ABORT, 'owned settlement blocked'); END;",
        )?;
        drop(run);
        release_tx.send(())?;
        let first = worker.stop_join();
        check!(matches!(
            &first,
            Err(IndexError::Storage(message)) if message.contains("owned settlement blocked")
        ));
        check_eq!(worker.stop_join(), first);
        check_eq!(
            observation.observe(Duration::from_secs(5))?.status,
            NativeWorkerStatus::Failed
        );
        let still_active: i64 =
            database.query_row("SELECT active FROM native_owned_work_v1", [], |row| {
                row.get(0)
            })?;
        check_eq!(still_active, 1);
        Ok(())
    }

    #[test]
    fn owned_cancellation_before_binding_reaches_the_grant_budget() {
        let relay = RequestBudgetV1::for_duration(Duration::from_secs(30));
        let receiver_guard = OwnedRunCancellation {
            relay: relay.clone(),
        };
        drop(receiver_guard);
        let execution = RequestBudgetV1::for_duration(Duration::from_secs(5));
        let _registration = bind_owned_cancellation(&relay, &execution);
        assert!(execution.is_cancelled());
    }

    #[test]
    fn owned_handle_drop_after_start_send_cancels_without_receiving_start() -> TestResult {
        let relay = RequestBudgetV1::for_duration(Duration::from_secs(30));
        let execution = RequestBudgetV1::for_duration(Duration::from_secs(5));
        let _registration = bind_owned_cancellation(&relay, &execution);
        let (started_tx, started_rx) = sync_mpsc::sync_channel(1);
        let (_finished_tx, finished_rx) = sync_mpsc::sync_channel(1);
        let (_quiesced_tx, quiesced_rx) = sync_mpsc::sync_channel(1);
        let run = OwnedRunHandle {
            started: started_rx,
            finished: finished_rx,
            quiesced: quiesced_rx,
            cancellation: OwnedRunCancellation { relay },
        };
        // A successful buffered send does not mean the pump has received the
        // cancellation handle. Dropping that unread message must still cancel.
        started_tx
            .send(Ok(OwnedRunStart::Skipped(OwnedRunSkip::Deferred(
                OwnedWorkDefer::NotDue,
            ))))
            .map_err(|_unsent_start| "test start receiver closed")?;
        check!(!execution.is_cancelled());
        drop(run);
        check!(execution.is_cancelled());
        Ok(())
    }

    #[test]
    fn queued_owned_handle_cancellation_prevents_grant_acquisition() -> TestResult {
        let fixture = journal_fixture()?;
        let worker = NativeOperationWorker::start()?;
        let release = hold_consumer(&worker, OperationExecutionClass::Enrichment)?;
        let key = fixture.request.operation;
        let assignment = OwnedWorkAssignment {
            request: fixture.request,
            policy: quanta_index_core::index::OwnedWorkPolicy {
                dispatch_limits: limits(),
                max_dispatches: 2,
                total_timeout_ms: 30_000,
                lease_ms: 1_000,
                retry_backoff_ms: 10,
            },
            deadline_ms: u64::MAX,
            dispatches: 0,
            due_ms: 0,
            active: false,
        };
        let run = worker.dispatch_owned(
            worker.reserve_owned_slot(
                assignment.request.operation.workspace,
                assignment.request.operation.epoch,
            )?,
            Arc::clone(&fixture.kernel),
            assignment,
            None,
        )?;
        let OwnedRunHandle {
            started,
            finished,
            quiesced,
            cancellation,
        } = run;
        drop(cancellation);
        release
            .send(())
            .map_err(|_unsent_unit| "test enrichment barrier closed")?;
        check!(matches!(
            started.recv_timeout(Duration::from_secs(5))?,
            Err(IndexError::Cancelled)
        ));
        check_eq!(
            finished.recv_timeout(Duration::from_secs(5))?,
            Err(IndexError::Cancelled)
        );
        quiesced.recv_timeout(Duration::from_secs(5))?;
        check_eq!(
            fixture.kernel.status(
                key,
                quanta_index_core::index::ports::OperationInspectAccess::Request(&work_budget())
            )?,
            OperationStatus::Unknown
        );
        drop(worker);
        Ok(())
    }

    #[test]
    fn exhausted_execution_reservation_does_not_admit_and_is_reusable() -> TestResult {
        let fixture = journal_fixture()?;
        let unit = ResourceNeed {
            cpu_units: 1,
            io_bytes: 1024,
            // Real fixture objects coexist with their encoded read buffer.
            memory_bytes: 64 * 1024,
            provider_units: 0,
        };
        let plan = NativeResourcePlan {
            capacity: ResourceNeed {
                cpu_units: 4,
                io_bytes: 4 * unit.io_bytes,
                memory_bytes: (4 + 1) * unit.memory_bytes,
                ..unit
            },
            control: unit,
            interactive: unit,
            core: unit,
            enrichment: unit,
        };
        let resources = NativeResourceLedger::new(plan)?;
        let held =
            ReservationGuard::try_acquire(resources.scheduler(NativeResourceClass::Core), unit)
                .ok_or("test could not reserve its sole execution slot")?;
        let worker = NativeOperationWorker::start_with_resources(
            &resources,
            plan,
            ResourceNeed::default(),
            ResourceNeed::default(),
            1,
        )?;
        let request = fixture.request.clone();
        check_eq!(
            worker.submit(Arc::clone(&fixture.kernel), request.clone(), work_budget()),
            Err(IndexError::ResourceLimited)
        );
        check_eq!(
            fixture.kernel.status(
                request.operation,
                quanta_index_core::index::ports::OperationInspectAccess::Request(&work_budget())
            )?,
            OperationStatus::Unknown
        );
        drop(held);
        check!(
            worker
                .submit(Arc::clone(&fixture.kernel), request, work_budget())
                .is_ok()
        );
        check_eq!(resources.snapshot()?.reserved, ResourceNeed::default());
        drop(worker);
        check_eq!(resources.snapshot()?.reserved, ResourceNeed::default());
        Ok(())
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

    fn parallel_worker(max_enrichment_active: u32) -> TestResult<NativeOperationWorker> {
        let unit = ResourceNeed {
            cpu_units: 1,
            io_bytes: 1024,
            // Real fixture objects coexist with their encoded read buffer.
            memory_bytes: 64 * 1024,
            provider_units: 0,
        };
        let units = max_enrichment_active
            .checked_add(3)
            .ok_or("test native capacity overflow")?;
        let plan = NativeResourcePlan {
            capacity: ResourceNeed {
                cpu_units: u64::from(units),
                io_bytes: u64::from(units)
                    .checked_mul(unit.io_bytes)
                    .ok_or("test native IO capacity overflow")?,
                memory_bytes: u64::from(units)
                    .checked_add(1)
                    .and_then(|units| units.checked_mul(unit.memory_bytes))
                    .ok_or("test native memory capacity overflow")?,
                provider_units: 0,
            },
            control: unit,
            interactive: unit,
            core: unit,
            enrichment: unit,
        };
        let resources = NativeResourceLedger::new(plan)?;
        Ok(NativeOperationWorker::start_with_resources(
            &resources,
            plan,
            ResourceNeed::default(),
            ResourceNeed::default(),
            max_enrichment_active,
        )?)
    }

    #[test]
    fn enrichment_scope_reservation_excludes_same_scope_until_release() -> TestResult {
        let worker = parallel_worker(2)?;
        let first = worker.reserve_owned_slot(
            WorkspaceKey {
                tenant: TenantId([1; 16]),
                collection: CollectionId([2; 16]),
                worktree: WorktreeId([3; 16]),
                slot: SlotId([4; 16]),
            },
            1,
        )?;
        let scope = first.scope.scope;
        check!(matches!(
            worker.scope_registry.acquire(scope),
            Err(IndexError::ResourceLimited)
        ));
        let second = worker
            .scope_registry
            .acquire(ExecutionScope { epoch: 2, ..scope })?;
        drop(first);
        let resumed = worker.scope_registry.acquire(scope)?;
        drop((resumed, second, worker));
        Ok(())
    }

    #[test]
    fn direct_enrichment_submit_cannot_enter_reserved_owned_scope() -> TestResult {
        let fixture = journal_fixture()?;
        let assignment = owned_assignment_fixture(&fixture)?;
        let worker = parallel_worker(2)?;
        let key = assignment.request.operation;
        let held = worker.reserve_owned_slot(key.workspace, key.epoch)?;
        check_eq!(
            worker.submit(
                Arc::clone(&fixture.kernel),
                assignment.request,
                work_budget(),
            ),
            Err(IndexError::ResourceLimited)
        );
        drop(held);
        drop(worker);
        Ok(())
    }

    #[test]
    fn direct_enrichment_admission_preserves_core_capacity_at_parallel_limit() -> TestResult {
        let worker = parallel_worker(2)?;
        let first = worker.direct_enrichment.acquire()?;
        let second = worker.direct_enrichment.acquire()?;
        check!(matches!(
            worker.direct_enrichment.acquire(),
            Err(IndexError::ResourceLimited)
        ));
        let fixture = journal_fixture()?;
        check!(matches!(
            worker.submit(Arc::clone(&fixture.kernel), fixture.request, work_budget())?,
            SubmitOutcome::Terminal(OperationStatus::Published(_))
        ));
        drop(first);
        let replacement = worker.direct_enrichment.acquire()?;
        drop((replacement, second, worker));
        Ok(())
    }

    fn direct_reply_releases_scope_before_followup(resume: bool) -> TestResult {
        let mut fixture = journal_fixture()?;
        let key = fixture.request.operation;
        let source = fixture
            .kernel
            .pin_view(
                ViewSelection::Exact(fixture.request.expected_view),
                &work_budget(),
            )?
            .view()
            .source;
        fixture.request.kind = UpdateKind::Materialize {
            claim: WorkClaim {
                intent: WorkIntent {
                    workspace: key.workspace,
                    epoch: key.epoch,
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
            },
        };
        if resume {
            admit_pending(&fixture)?;
        }
        let worker = NativeOperationWorker::start()?;
        let (reached_tx, reached_rx) = sync_mpsc::sync_channel(1);
        let (release_tx, release_rx) = sync_mpsc::sync_channel(1);
        *worker
            .direct_after_reply
            .lock()
            .map_err(|error| format!("native direct test barrier lock: {error}"))? =
            Some(OwnedTestBarrier {
                reached: reached_tx,
                release: release_rx,
            });
        // A malformed claim can fail in Kernel, but it still crosses the
        // direct worker reply boundary and holds the task at the test barrier.
        let _result = if resume {
            worker.resume(Arc::clone(&fixture.kernel), key, work_budget())
        } else {
            worker.submit(
                Arc::clone(&fixture.kernel),
                fixture.request.clone(),
                work_budget(),
            )
        };
        reached_rx.recv_timeout(Duration::from_secs(5))?;
        let same_scope = worker.scope_registry.acquire(key.into())?;
        drop(same_scope);
        release_tx
            .send(())
            .map_err(|_closed| "direct reply barrier closed")?;
        drop(worker);
        Ok(())
    }

    #[test]
    fn direct_submit_and_resume_release_scope_before_reply_is_observed() -> TestResult {
        direct_reply_releases_scope_before_followup(false)?;
        direct_reply_releases_scope_before_followup(true)
    }

    #[test]
    fn parallel_enrichment_admits_two_active_and_drains_both_on_shutdown() -> TestResult {
        let worker = parallel_worker(2)?;
        let observation = worker.observation();
        let first = hold_consumer(&worker, OperationExecutionClass::Enrichment)?;
        let second = hold_consumer(&worker, OperationExecutionClass::Enrichment)?;
        check_eq!(
            observation
                .observe(Duration::from_secs(1))?
                .enrichment
                .active,
            2
        );
        let (stopped_tx, stopped_rx) = sync_mpsc::sync_channel(1);
        let shutdown = std::thread::spawn(move || {
            let result = worker.stop_join();
            let _sent = stopped_tx.send(result);
        });
        check!(matches!(
            stopped_rx.recv_timeout(Duration::from_millis(30)),
            Err(sync_mpsc::RecvTimeoutError::Timeout)
        ));
        first.send(()).map_err(|_closed| "first barrier closed")?;
        check!(matches!(
            stopped_rx.recv_timeout(Duration::from_millis(30)),
            Err(sync_mpsc::RecvTimeoutError::Timeout)
        ));
        second.send(()).map_err(|_closed| "second barrier closed")?;
        stopped_rx.recv_timeout(Duration::from_secs(5))??;
        shutdown.join().map_err(|_panic| "shutdown panicked")?;
        let stopped = observation.observe(Duration::from_secs(1))?;
        check_eq!(stopped.status, NativeWorkerStatus::Stopped);
        check_eq!(stopped.enrichment.active, 0);
        Ok(())
    }

    #[test]
    fn panicking_parallel_job_drains_surviving_active_job_before_worker_exit() -> TestResult {
        let worker = parallel_worker(2)?;
        let observation = worker.observation();
        let surviving = hold_consumer(&worker, OperationExecutionClass::Enrichment)?;
        let (entered_tx, entered_rx) = sync_mpsc::sync_channel(1);
        worker.enqueue_test(
            OperationExecutionClass::Enrichment,
            WorkerCommand::Panic {
                entered: entered_tx,
            },
        )?;
        entered_rx.recv_timeout(Duration::from_secs(5))?;
        let (stopped_tx, stopped_rx) = sync_mpsc::sync_channel(1);
        let shutdown = std::thread::spawn(move || {
            let result = worker.stop_join();
            let _sent = stopped_tx.send(result);
        });
        // `entered` is sent before panic unwinds. Wait for stop_join to
        // start and the panicking task's telemetry lease to drop while the
        // surviving job stays held.
        let deadline = Instant::now()
            .checked_add(Duration::from_secs(5))
            .ok_or("panic observation deadline overflow")?;
        loop {
            let snapshot = observation.observe(Duration::from_secs(1))?;
            if snapshot.status == NativeWorkerStatus::Stopping && snapshot.enrichment.active == 1 {
                break;
            }
            check!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(1));
        }
        check!(matches!(
            stopped_rx.try_recv(),
            Err(sync_mpsc::TryRecvError::Empty)
        ));
        surviving
            .send(())
            .map_err(|_closed| "surviving barrier closed")?;
        let result = stopped_rx.recv_timeout(Duration::from_secs(5))?;
        shutdown.join().map_err(|_panic| "shutdown panicked")?;
        let error = result.err().ok_or("panicking job did not fail worker")?;
        check!(matches!(error, IndexError::Storage(_)));
        let stopped = observation.observe(Duration::from_secs(1))?;
        check_eq!(stopped.status, NativeWorkerStatus::Failed);
        check_eq!(stopped.enrichment.active, 0);
        Ok(())
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
                    after_reply: None,
                },
            )
            .map_err(|error| format!("enqueue real submit: {error}"))?;
        let waiting = observation.observe(Duration::from_secs(1))?;
        check_eq!((waiting.core.queued, waiting.core.active), (1, 1));
        check_eq!(
            fixture.kernel.status(
                key,
                quanta_index_core::index::ports::OperationInspectAccess::Request(&work_budget())
            )?,
            OperationStatus::Unknown
        );
        let expected = interrupt(mode, &request_budget, deadline);
        release
            .send(())
            .map_err(|_unsent_unit| "test barrier closed")?;
        check_eq!(
            reply_rx.recv_timeout(Duration::from_secs(5))?,
            Err(expected)
        );
        check_eq!(
            fixture.kernel.status(
                key,
                quanta_index_core::index::ports::OperationInspectAccess::Request(&work_budget())
            )?,
            OperationStatus::Unknown
        );
        drop(worker);
        let stopped = observation.observe(Duration::from_secs(1))?;
        check_eq!(stopped.status, NativeWorkerStatus::Stopped);
        check_eq!((stopped.core.queued, stopped.core.active), (0, 0));
        check_eq!(
            stopped.core.dispatch_lag_ms,
            Signal::Unknown(UnknownReason::Dropped)
        );
        Ok(())
    }

    fn queued_resume(mode: Interruption) -> TestResult {
        let fixture = journal_fixture()?;
        let key = fixture.request.operation;
        admit_pending(&fixture)?;
        let pending = fixture.kernel.status(
            key,
            quanta_index_core::index::ports::OperationInspectAccess::Request(&work_budget()),
        )?;
        check!(matches!(pending, OperationStatus::Pending(_)));

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
                    after_reply: None,
                },
            )
            .map_err(|error| format!("enqueue real resume: {error}"))?;
        let waiting = observation.observe(Duration::from_secs(1))?;
        check_eq!((waiting.core.queued, waiting.core.active), (1, 1));
        check_eq!(
            fixture.kernel.status(
                key,
                quanta_index_core::index::ports::OperationInspectAccess::Request(&work_budget())
            )?,
            pending
        );
        let expected = interrupt(mode, &request_budget, deadline);
        release
            .send(())
            .map_err(|_unsent_unit| "test barrier closed")?;
        check_eq!(
            reply_rx.recv_timeout(Duration::from_secs(5))?,
            Err(expected)
        );
        check_eq!(
            fixture.kernel.status(
                key,
                quanta_index_core::index::ports::OperationInspectAccess::Request(&work_budget())
            )?,
            pending
        );
        drop(worker);
        let stopped = observation.observe(Duration::from_secs(1))?;
        check_eq!(stopped.status, NativeWorkerStatus::Stopped);
        check_eq!((stopped.core.queued, stopped.core.active), (0, 0));
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

        check_eq!(
            fixture.kernel.status(
                key,
                quanta_index_core::index::ports::OperationInspectAccess::Request(&work_budget())
            )?,
            OperationStatus::Unknown
        );
        let outcome = worker.resume(Arc::clone(&fixture.kernel), key, work_budget());
        release
            .send(())
            .map_err(|_unsent_unit| "test core barrier closed")?;
        drop(worker);

        check_eq!(outcome?, SubmitOutcome::Unknown(key));
        check_eq!(
            fixture.kernel.status(
                key,
                quanta_index_core::index::ports::OperationInspectAccess::Request(&work_budget())
            )?,
            OperationStatus::Unknown
        );
        check!(finished.load(Ordering::Acquire));
        Ok(())
    }

    #[test]
    fn full_enrichment_lane_does_not_block_real_core_submit_or_resume() -> TestResult {
        let worker = NativeOperationWorker::start()?;
        let mut enrichment = journal_fixture()?;
        let enrichment_key = enrichment.request.operation;
        let source = enrichment
            .kernel
            .pin_view(
                ViewSelection::Exact(enrichment.request.expected_view),
                &work_budget(),
            )?
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
        let original = enrichment.kernel.status(
            enrichment_key,
            quanta_index_core::index::ports::OperationInspectAccess::Request(&work_budget()),
        )?;
        check!(matches!(original, OperationStatus::Pending(_)));

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
                    after_reply: None,
                },
            )
            .map_err(|error| format!("enqueue real enrichment resume: {error}"))?;
        check_eq!(
            worker.resume(
                Arc::clone(&enrichment.kernel),
                enrichment_key,
                work_budget(),
            ),
            Err(IndexError::ResourceLimited)
        );
        let mut second_enrichment = enrichment.request.clone();
        second_enrichment.operation.sequence = 2;
        check_eq!(
            worker.submit(
                Arc::clone(&enrichment.kernel),
                second_enrichment,
                work_budget(),
            ),
            Err(IndexError::ResourceLimited)
        );
        check_eq!(
            enrichment.kernel.status(
                enrichment_key,
                quanta_index_core::index::ports::OperationInspectAccess::Request(&work_budget())
            )?,
            original
        );

        let core_submit = journal_fixture()?;
        let submit_key = core_submit.request.operation;
        check!(matches!(
            worker.submit(
                Arc::clone(&core_submit.kernel),
                core_submit.request,
                work_budget(),
            )?,
            SubmitOutcome::Terminal(OperationStatus::Published(_))
        ));
        check!(matches!(
            core_submit.kernel.status(
                submit_key,
                quanta_index_core::index::ports::OperationInspectAccess::Request(&work_budget())
            )?,
            OperationStatus::Published(_)
        ));

        let core_resume = journal_fixture()?;
        let resume_key = core_resume.request.operation;
        admit_pending(&core_resume)?;
        check!(matches!(
            worker.resume(Arc::clone(&core_resume.kernel), resume_key, work_budget(),)?,
            SubmitOutcome::Terminal(OperationStatus::Published(_))
        ));
        check!(matches!(
            core_resume.kernel.status(
                resume_key,
                quanta_index_core::index::ports::OperationInspectAccess::Request(&work_budget())
            )?,
            OperationStatus::Published(_)
        ));

        queued_budget.cancel_handle().cancel();
        release
            .send(())
            .map_err(|_unsent_unit| "test enrichment barrier closed")?;
        check_eq!(
            reply_rx.recv_timeout(Duration::from_secs(5))?,
            Err(IndexError::Cancelled)
        );
        check_eq!(
            enrichment.kernel.status(
                enrichment_key,
                quanta_index_core::index::ports::OperationInspectAccess::Request(&work_budget())
            )?,
            original
        );
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
        check_eq!((active.core.active, active.enrichment.active), (1, 1));
        let (done_tx, done_rx) = sync_mpsc::sync_channel(1);
        let shutdown = std::thread::spawn(move || {
            drop(worker);
            let _done = done_tx.send(());
        });
        check!(matches!(
            done_rx.recv_timeout(Duration::from_millis(30)),
            Err(sync_mpsc::RecvTimeoutError::Timeout)
        ));
        let status_deadline = Instant::now()
            .checked_add(Duration::from_secs(5))
            .ok_or("test status deadline overflow")?;
        loop {
            let stopping = observation.observe(Duration::from_secs(1))?;
            if stopping.status == NativeWorkerStatus::Stopping {
                check_eq!((stopping.core.active, stopping.enrichment.active), (1, 1));
                check_eq!(
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
        check!(matches!(
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
        check_eq!(stopped.status, NativeWorkerStatus::Stopped);
        check_eq!((stopped.core.active, stopped.enrichment.active), (0, 0));
        Ok(())
    }

    #[test]
    fn observation_counts_real_queue_and_expires_old_dispatch_lag() -> TestResult {
        let worker = NativeOperationWorker::start()?;
        let observation = worker.observation();
        let before = observation.observe(Duration::from_secs(1))?;
        check_eq!(before.status, NativeWorkerStatus::Running);
        check_eq!(
            before.core.dispatch_lag_ms,
            Signal::Unknown(UnknownReason::Missing)
        );
        let release = hold_consumer(&worker, OperationExecutionClass::Core)?;
        let entered = observation.observe(Duration::from_secs(1))?;
        check_eq!((entered.core.queued, entered.core.active), (0, 1));
        check!(matches!(entered.core.dispatch_lag_ms, Signal::Known(_)));
        let finished = Arc::new(AtomicBool::new(false));
        worker.enqueue_test(
            OperationExecutionClass::Core,
            WorkerCommand::TimerProbe {
                finished: Arc::clone(&finished),
            },
        )?;
        let waiting = observation.observe(Duration::from_secs(1))?;
        check_eq!((waiting.core.queued, waiting.core.active), (1, 1));
        check_eq!(
            worker.enqueue_test(
                OperationExecutionClass::Core,
                WorkerCommand::TimerProbe {
                    finished: Arc::new(AtomicBool::new(false)),
                },
            ),
            Err(IndexError::ResourceLimited)
        );
        check_eq!(observation.observe(Duration::from_secs(1))?.core.queued, 1);
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
        check!(finished.load(Ordering::Acquire));
        let stopped = observation.observe(Duration::from_secs(1))?;
        check_eq!(stopped.status, NativeWorkerStatus::Stopped);
        check_eq!((stopped.core.queued, stopped.core.active), (0, 0));
        check_eq!(
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
            None,
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
            None,
        )?;
        let handoff = observation.observe(Duration::from_secs(1))?;
        check_eq!((handoff.core.queued, handoff.core.active), (2, 0));
        first
            .lease
            .as_mut()
            .ok_or("first lease missing")?
            .activate()?;
        let running = observation.observe(Duration::from_secs(1))?;
        check_eq!((running.core.queued, running.core.active), (1, 1));
        drop(first);
        let mut second = receiver.try_recv()?;
        second
            .lease
            .as_mut()
            .ok_or("second lease missing")?
            .activate()?;
        drop(second);
        let drained = observation.observe(Duration::from_secs(1))?;
        check_eq!((drained.core.queued, drained.core.active), (0, 0));
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
        check_eq!(failed.status, NativeWorkerStatus::Failed);
        check_eq!((failed.core.queued, failed.core.active), (0, 0));
        check_eq!(
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
        if !finished.load(Ordering::Acquire) {
            return Err(IndexError::Storage(
                "timer did not complete before shutdown".into(),
            ));
        }
        Ok(())
    }
}
