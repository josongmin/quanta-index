//! Explicit, service-owned polling of durable catalog work.

use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    fmt::Debug,
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, RecvTimeoutError, TryRecvError},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use quanta_index_catalog::SqliteCatalog;
use quanta_index_core::{
    CancelHandleV1, DiagnosticContextV1, DiagnosticLaneV1, DiagnosticSpanGuardV1,
    DiagnosticStageV1, DiagnosticTerminalV1, RequestBudgetV1,
    index::{
        IndexError, OwnedReadyOutcome, OwnedWorkPolicy, OwnedWorkReport, Refusal, UpdateKind,
        WorkBudget, WorkBudgetDenial, WorkspaceKey,
        effort::{ReservationGuard, ResourceNeed},
        ports::CatalogReadPort,
    },
};
use quanta_index_ipc::ShutdownHandle;
use quanta_index_kernel::Kernel;
use quanta_index_storage::FileObjectStore;

use crate::{
    native_reclaim_driver::{
        NativeReclaimConfig, NativeReclaimDriver, NativeReclaimRound, reclaim_one_attempt,
    },
    native_resources::{NativeClassScheduler, NativeResourceLedger, NativeResourceSnapshot},
    native_retention::{NativeRetentionConfig, NativeRetentionDriver},
    native_scratch_io::NativeScratchIo,
    native_worker::{
        NativeOperationWorker, NativeWorkerSnapshot, OwnedRunHandle, OwnedRunSkip, OwnedRunStart,
    },
};

// The pump depends on execution capabilities, never on the IPC dispatcher.
// These handles share the service's worker and resource ledger.
pub(super) struct NativePumpServices {
    pub read_sessions: Arc<OnceLock<Arc<crate::native_read_sessions::NativeReadSessions>>>,
    pub kernel: Arc<Kernel<SqliteCatalog>>,
    pub catalog: Arc<SqliteCatalog>,
    pub objects: Arc<FileObjectStore>,
    pub worker: Arc<NativeOperationWorker>,
    pub resources: Arc<NativeResourceLedger>,
    pub pump_control_scheduler: Arc<NativeClassScheduler>,
    pub maintenance_scheduler: Arc<NativeClassScheduler>,
    pub control_need: ResourceNeed,
    pub io: Arc<NativeScratchIo>,
    pub wal_checkpoint_bytes: u64,
    pub diagnostic_context: Option<DiagnosticContextV1>,
}

/// Observations keep missing/stale worker signals intact. A policy cannot
/// authorize a claim or change frozen dispatch limits.
#[derive(Clone, Debug)]
pub struct NativePumpObservation {
    pub worker: Result<NativeWorkerSnapshot, IndexError>,
    pub resources: Result<NativeResourceSnapshot, IndexError>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativePumpDecision {
    Poll,
    Defer(Duration),
}

/// Pluggable cadence only. Fixed governor bounds every suggested interval.
pub trait NativePumpPolicy: Debug + Send + Sync + 'static {
    fn decide(&self, observation: &NativePumpObservation) -> NativePumpDecision;
}

#[derive(Debug)]
pub struct FixedPumpPolicy;

impl NativePumpPolicy for FixedPumpPolicy {
    fn decide(&self, _observation: &NativePumpObservation) -> NativePumpDecision {
        NativePumpDecision::Poll
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct NativePumpScope {
    pub workspace: WorkspaceKey,
    pub epoch: u64,
}

/// Frozen policy for new assignments. A previously acquired assignment keeps
/// its catalog-persisted policy across service restarts and config changes.
#[derive(Clone, Debug)]
pub struct NativePumpConfig {
    pub scopes: Vec<NativePumpScope>,
    /// Maximum actually dispatched owned jobs per round; empty and active
    /// scopes do not consume this limit.
    pub batch_limit: u32,
    /// Maximum simultaneous enrichment grants across distinct scopes.
    /// Existing single-lane deployments use one.
    pub max_in_flight: u32,
    pub poll_interval: Duration,
    pub new_assignment: OwnedWorkPolicy,
    pub cadence: Arc<dyn NativePumpPolicy>,
}

impl NativePumpConfig {
    pub fn validate(&self) -> Result<(), IndexError> {
        self.new_assignment.validate()?;
        if self.scopes.is_empty() || self.scopes.len() > 1024 {
            return Err(IndexError::InvalidInput(
                "native pump needs 1..=1024 explicit scopes".into(),
            ));
        }
        if !(1..=256).contains(&self.batch_limit) {
            return Err(IndexError::InvalidInput(
                "native pump batch limit must be 1..=256".into(),
            ));
        }
        if !(1..=256).contains(&self.max_in_flight) {
            return Err(IndexError::InvalidInput(
                "native pump in-flight limit must be 1..=256".into(),
            ));
        }
        if self.poll_interval < Duration::from_millis(10)
            || self.poll_interval > Duration::from_secs(60)
        {
            return Err(IndexError::InvalidInput(
                "native pump poll interval must be 10ms..=60s".into(),
            ));
        }
        let mut seen = BTreeSet::new();
        if self.scopes.iter().any(|scope| !seen.insert(*scope)) {
            return Err(IndexError::InvalidInput(
                "native pump scope is duplicated".into(),
            ));
        }
        Ok(())
    }
}

/// The manager owns its thread. Dropping it cancels active provider work and
/// joins before the service releases the state-root lease.
pub(super) struct NativeOwnedPump {
    stop: Arc<AtomicBool>,
    active_reclaim: Arc<Mutex<Option<CancelHandleV1>>>,
    manager: Option<JoinHandle<Result<(), IndexError>>>,
    terminal: Option<Result<(), IndexError>>,
}

impl NativeOwnedPump {
    pub(super) fn start(
        dispatcher: Arc<NativePumpServices>,
        config: Option<NativePumpConfig>,
        reclaim: Option<NativeReclaimConfig>,
        retention: Option<NativeRetentionConfig>,
        shutdown: ShutdownHandle,
    ) -> Result<Self, IndexError> {
        if config.is_none() && reclaim.is_none() {
            return Err(IndexError::InvalidInput(
                "native pump needs owned work or automatic reclaim".into(),
            ));
        }
        if let Some(config) = &config {
            config.validate()?;
        }
        if let Some(reclaim) = &reclaim {
            reclaim.validate()?;
        }
        if let Some(retention) = &retention {
            retention.validate()?;
            if reclaim.is_none() {
                return Err(IndexError::InvalidInput(
                    "retention requires queued reclaim".into(),
                ));
            }
        }
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let active_reclaim = Arc::new(Mutex::new(None));
        let thread_reclaim = Arc::clone(&active_reclaim);
        let manager = thread::Builder::new()
            .name("native-owned-work-pump".into())
            .spawn(move || {
                let result = match catch_unwind(AssertUnwindSafe(|| {
                    pump_loop(
                        &dispatcher,
                        config.as_ref(),
                        reclaim,
                        retention,
                        &thread_stop,
                        &thread_reclaim,
                    )
                })) {
                    Ok(result) => result,
                    Err(_panic) => Err(IndexError::Storage(
                        "native owned work pump panicked".into(),
                    )),
                };
                if result.is_err() {
                    shutdown.trigger();
                }
                result
            })
            .map_err(|error| IndexError::Storage(format!("start native work pump: {error}")))?;
        Ok(Self {
            stop,
            active_reclaim,
            manager: Some(manager),
            terminal: None,
        })
    }

    pub(super) fn stop_join(&mut self) -> Result<(), IndexError> {
        if let Some(result) = &self.terminal {
            return result.clone();
        }
        self.stop.store(true, Ordering::Release);
        // Clone under the mutex and cancel after releasing it: a cancellation
        // callback may itself enter code that needs the same manager state.
        let active = {
            let locked = self.active_reclaim.lock();
            match locked {
                Ok(active) => active.clone(),
                Err(poison) => poison.into_inner().clone(),
            }
        };
        if let Some(cancel) = active {
            cancel.cancel();
        }
        let Some(manager) = self.manager.take() else {
            return Ok(());
        };
        manager.thread().unpark();
        let result = manager
            .join()
            .map_err(|_panic| IndexError::Storage("native owned work pump panicked".into()))
            .and_then(std::convert::identity);
        self.terminal = Some(result.clone());
        result
    }
}

impl Drop for NativeOwnedPump {
    fn drop(&mut self) {
        drop(self.stop_join());
    }
}

struct RunFinished {
    scope: NativePumpScope,
    result: Result<bool, IndexError>,
}

/// One renewal owner per accepted grant. The map bounds thread count and its
/// guard joins every owner before the service can release its root lease.
struct ActiveRuns {
    stop: Arc<AtomicBool>,
    handles: BTreeMap<NativePumpScope, JoinHandle<()>>,
    retry_after: BTreeMap<NativePumpScope, Instant>,
    retry_delay: Duration,
    finished_tx: mpsc::Sender<RunFinished>,
    finished_rx: mpsc::Receiver<RunFinished>,
}

impl ActiveRuns {
    fn new(stop: Arc<AtomicBool>) -> Self {
        let (finished_tx, finished_rx) = mpsc::channel();
        Self {
            stop,
            handles: BTreeMap::new(),
            retry_after: BTreeMap::new(),
            retry_delay: Duration::from_millis(10),
            finished_tx,
            finished_rx,
        }
    }

    fn len(&self) -> usize {
        self.handles.len()
    }

    fn contains(&self, scope: &NativePumpScope) -> bool {
        self.handles.contains_key(scope)
    }

    fn set_retry_delay(&mut self, delay: Duration) {
        self.retry_delay = delay;
    }

    fn eligible(&self, scope: &NativePumpScope, now: Instant) -> bool {
        self.retry_after.get(scope).is_none_or(|due| now >= *due)
    }

    fn next_retry_due(&self) -> Option<Instant> {
        self.retry_after.values().copied().min()
    }

    fn prune_expired_retries(&mut self, now: Instant) {
        self.retry_after.retain(|_scope, due| *due > now);
    }

    fn spawn(
        &mut self,
        scope: NativePumpScope,
        dispatcher: Arc<NativePumpServices>,
        run: OwnedRunHandle,
    ) -> Result<(), IndexError> {
        if self.contains(&scope) {
            return Err(IndexError::BackendViolation(
                "native pump already owns this scope".into(),
            ));
        }
        let stop = Arc::clone(&self.stop);
        let finished = self.finished_tx.clone();
        let handle = thread::Builder::new()
            .name("native-owned-work-renewal".into())
            .spawn(move || {
                let result = catch_unwind(AssertUnwindSafe(|| await_run(&dispatcher, run, &stop)))
                    .map_err(|_panic| IndexError::Storage("native owned renewal panicked".into()))
                    .and_then(std::convert::identity);
                let _sent = finished.send(RunFinished { scope, result });
            })
            .map_err(|error| IndexError::Storage(format!("start owned renewal: {error}")))?;
        if let Some(previous) = self.handles.insert(scope, handle) {
            self.stop.store(true, Ordering::Release);
            let _joined = previous.join();
            return Err(IndexError::BackendViolation(
                "native pump replaced an active renewal owner".into(),
            ));
        }
        Ok(())
    }

    fn accept(&mut self, finished: RunFinished, now: Instant) -> Result<bool, IndexError> {
        let handle = self.handles.remove(&finished.scope).ok_or_else(|| {
            IndexError::BackendViolation("native pump completion has no active scope".into())
        })?;
        handle
            .join()
            .map_err(|_panic| IndexError::Storage("native owned renewal panicked".into()))?;
        let executed = finished.result?;
        if executed {
            let _removed = self.retry_after.remove(&finished.scope);
        } else {
            let due = now
                .checked_add(self.retry_delay)
                .ok_or(IndexError::ResourceLimited)?;
            let _previous = self.retry_after.insert(finished.scope, due);
        }
        Ok(executed)
    }

    fn reap(&mut self) -> Result<bool, IndexError> {
        let mut executed = false;
        loop {
            match self.finished_rx.try_recv() {
                Ok(finished) => executed |= self.accept(finished, Instant::now())?,
                Err(TryRecvError::Empty) => return Ok(executed),
                Err(TryRecvError::Disconnected) => {
                    return Err(IndexError::Storage(
                        "native owned renewal completion channel closed".into(),
                    ));
                }
            }
        }
    }

    fn wait(&mut self, duration: Duration) -> Result<bool, IndexError> {
        if self.handles.is_empty() {
            thread::park_timeout(duration);
            return Ok(false);
        }
        match self.finished_rx.recv_timeout(duration) {
            Ok(finished) => {
                let _executed = self.accept(finished, Instant::now())?;
                // The freed slot may serve another scope even when this run
                // declined execution. The declined scope has its own delay.
                Ok(true)
            }
            Err(RecvTimeoutError::Timeout) => Ok(false),
            Err(RecvTimeoutError::Disconnected) => Err(IndexError::Storage(
                "native owned renewal completion channel closed".into(),
            )),
        }
    }

    fn stop_join_all(&mut self) -> Result<(), IndexError> {
        self.stop.store(true, Ordering::Release);
        let mut first_error = None;
        for (_scope, handle) in std::mem::take(&mut self.handles) {
            if handle.join().is_err() {
                let _observed = first_error.get_or_insert_with(|| {
                    IndexError::Storage("native owned renewal panicked".into())
                });
            }
        }
        while let Ok(finished) = self.finished_rx.try_recv() {
            if let Err(error) = finished.result {
                let _observed = first_error.get_or_insert(error);
            }
        }
        first_error.map_or(Ok(()), Err)
    }
}

impl Drop for ActiveRuns {
    fn drop(&mut self) {
        drop(self.stop_join_all());
    }
}

/// The manager publishes the request's cancellation handle before its first
/// checkpoint. Stop can then interrupt a long child drain before joining.
struct ActiveReclaimGuard {
    slot: Arc<Mutex<Option<CancelHandleV1>>>,
}

impl ActiveReclaimGuard {
    fn register(
        slot: &Arc<Mutex<Option<CancelHandleV1>>>,
        request: &RequestBudgetV1,
        stop: &AtomicBool,
    ) -> Result<Self, IndexError> {
        let mut active = slot.lock().map_err(|_poison| {
            IndexError::Storage("native reclaim cancellation slot poisoned".into())
        })?;
        if active.is_some() {
            return Err(IndexError::BackendViolation(
                "native reclaim already has an active request".into(),
            ));
        }
        *active = Some(request.cancel_handle());
        drop(active);
        if stop.load(Ordering::Acquire) {
            request.cancel_handle().cancel();
        }
        Ok(Self {
            slot: Arc::clone(slot),
        })
    }
}

impl Drop for ActiveReclaimGuard {
    fn drop(&mut self) {
        let mut active = {
            let locked = self.slot.lock();
            match locked {
                Ok(active) => active,
                Err(poison) => poison.into_inner(),
            }
        };
        *active = None;
        drop(active);
    }
}

fn pump_loop(
    dispatcher: &Arc<NativePumpServices>,
    config: Option<&NativePumpConfig>,
    reclaim: Option<NativeReclaimConfig>,
    retention: Option<NativeRetentionConfig>,
    stop: &Arc<AtomicBool>,
    active_reclaim: &Arc<Mutex<Option<CancelHandleV1>>>,
) -> Result<(), IndexError> {
    let mut scopes: VecDeque<NativePumpScope> = config
        .into_iter()
        .flat_map(|config| config.scopes.iter().copied())
        .collect();
    let scan_limits = config
        .map(|_| SqliteCatalog::owned_scan_limits(1))
        .transpose()?;
    let max_in_flight = config.map_or(Ok(1), |config| {
        usize::try_from(config.max_in_flight)
            .map_err(|error| IndexError::InvalidInput(format!("native pump concurrency: {error}")))
    })?;
    let mut reclaim = reclaim
        .map(|config| {
            NativeReclaimDriver::new(config, Instant::now()).map(|driver| (driver, config))
        })
        .transpose()?;
    let mut retention = retention
        .map(|config| NativeRetentionDriver::new(config).map(|driver| (driver, Instant::now())))
        .transpose()?;
    let read_session_limits = reclaim.as_ref().map(|(_, config)| config.work_limits);
    let mut next_read_sweep = Instant::now();
    let mut runs = ActiveRuns::new(Arc::clone(stop));
    if let Some(config) = config {
        runs.set_retry_delay(config.poll_interval);
    }
    let drive = catch_unwind(AssertUnwindSafe(|| {
        while !stop.load(Ordering::Acquire) {
            run_read_sessions_due(
                dispatcher,
                read_session_limits,
                &mut next_read_sweep,
                stop,
                active_reclaim,
            )?;
            runs.prune_expired_retries(Instant::now());
            let completed = runs.reap()?;
            if let Some((driver, next_due)) = &mut retention {
                run_retention_due(dispatcher, driver, next_due, stop, active_reclaim)?;
            }
            if let Some((driver, config)) = &mut reclaim {
                run_reclaim_due(dispatcher, driver, *config, stop, active_reclaim)?;
            }
            let launched = match (config, scan_limits) {
                (Some(config), Some(scan_limits)) if !stop.load(Ordering::Acquire) => scan_round(
                    dispatcher,
                    config,
                    stop,
                    &mut scopes,
                    scan_limits,
                    max_in_flight,
                    &mut runs,
                )?,
                _ => 0,
            };
            if completed
                || (launched > 0
                    && runs.len() < max_in_flight
                    && scopes
                        .iter()
                        .any(|scope| !runs.contains(scope) && runs.eligible(scope, Instant::now())))
            {
                continue;
            }
            let suggested = config.map_or_else(
                || Duration::from_secs(60),
                |config| {
                    let observation = NativePumpObservation {
                        worker: dispatcher
                            .worker
                            .observation()
                            .observe(Duration::from_secs(5)),
                        resources: dispatcher.resources.observation().observe(),
                    };
                    match config.cadence.decide(&observation) {
                        NativePumpDecision::Poll => config.poll_interval,
                        NativePumpDecision::Defer(duration) => duration,
                    }
                },
            );
            // Reclaim-only services use the driver deadline below. A declined
            // owned scope retains this bounded cadence as its retry delay.
            let cadence = suggested.clamp(Duration::from_millis(10), Duration::from_secs(60));
            runs.set_retry_delay(cadence);
            let deadline = Instant::now()
                .checked_add(cadence)
                .ok_or(IndexError::ResourceLimited)?;
            let deadline = reclaim
                .as_ref()
                .map_or(deadline, |(driver, _)| deadline.min(driver.next_due()));
            let deadline = retention
                .as_ref()
                .map_or(deadline, |(_, next_due)| deadline.min(*next_due));
            let deadline = if dispatcher.read_sessions.get().is_some() {
                deadline.min(next_read_sweep)
            } else {
                deadline
            };
            let deadline = runs
                .next_retry_due()
                .map_or(deadline, |retry_due| deadline.min(retry_due));
            while !stop.load(Ordering::Acquire) {
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    break;
                }
                if runs.wait(remaining.min(Duration::from_millis(50)))? {
                    break;
                }
            }
        }
        Ok(())
    }))
    .map_err(|_panic| IndexError::Storage("native owned work pump panicked".into()))
    .and_then(std::convert::identity);
    let drain = runs.stop_join_all();
    finish_pump_drive(drive, drain)
}

fn run_read_sessions_due(
    dispatcher: &Arc<NativePumpServices>,
    limits: Option<quanta_index_core::index::WorkLimits>,
    next_due: &mut Instant,
    stop: &AtomicBool,
    active: &Arc<Mutex<Option<CancelHandleV1>>>,
) -> Result<(), IndexError> {
    let now = Instant::now();
    if now < *next_due || stop.load(Ordering::Acquire) {
        return Ok(());
    }
    *next_due = now
        .checked_add(Duration::from_millis(100))
        .ok_or(IndexError::ResourceLimited)?;
    let Some(sessions) = dispatcher.read_sessions.get() else {
        return Ok(());
    };
    let limits = limits.ok_or_else(|| {
        IndexError::BackendViolation("read sessions lost maintenance funding".into())
    })?;
    let Some(reservation) = ReservationGuard::try_acquire(
        Arc::clone(&dispatcher.maintenance_scheduler),
        dispatcher.control_need,
    ) else {
        return match dispatcher.maintenance_scheduler.denial() {
            IndexError::ResourceLimited => Ok(()),
            error @ (IndexError::InvalidInput(_)
            | IndexError::FocusSubjectNotFound
            | IndexError::Conflict
            | IndexError::DigestConflict
            | IndexError::FenceLost
            | IndexError::WorkFenceLost
            | IndexError::WorkLimitExceeded
            | IndexError::Cancelled
            | IndexError::SnapshotUnavailable
            | IndexError::CorruptObject(_)
            | IndexError::BackendViolation(_)
            | IndexError::Storage(_)
            | IndexError::RebuildRequired
            | IndexError::MaintenanceRequired) => Err(error),
        };
    };
    let request = maintenance_request(
        Duration::from_millis(100),
        dispatcher.diagnostic_context.as_ref(),
    );
    let _active = ActiveReclaimGuard::register(active, &request, stop)?;
    let (request, span) = begin_maintenance_round(request);
    let gate = WorkBudget::with_request(limits, request.clone());
    let mut work = WorkBudget::with_request(limits, request);
    work.bind_memory_scope(reservation.memory_scope());
    let _physical = match dispatcher.io.enter_maintenance_scope() {
        Ok(scope) => scope,
        Err(error) => {
            let result = Err::<(), IndexError>(error);
            finish_maintenance_round(span, &result);
            return result;
        }
    };
    let round = sessions
        .sweep(&gate, |ticket| {
            let target = ticket.view().identity;
            dispatcher
                .kernel
                .release_read_ticket(
                    ticket,
                    quanta_index_core::index::NativeReadSettlementScope {
                        workspace: target.workspace,
                        epoch: target.epoch,
                        target,
                    },
                    &mut work,
                )
                .map(|observation| observation.has_remaining_target_pins)
        })
        .and_then(|report| report.first_error.map_or(Ok(()), Err));
    finish_maintenance_round(span, &round);
    match round {
        Ok(()) | Err(IndexError::ResourceLimited | IndexError::SnapshotUnavailable) => Ok(()),
        Err(IndexError::Cancelled) if stop.load(Ordering::Acquire) => Ok(()),
        Err(error) => Err(error),
    }
}

fn maintenance_request(
    timeout: Duration,
    context: Option<&DiagnosticContextV1>,
) -> RequestBudgetV1 {
    let request = RequestBudgetV1::for_duration(timeout);
    match context {
        Some(context) => request.with_diagnostic_context(context.clone()),
        None => request,
    }
}

fn begin_maintenance_round(
    request: RequestBudgetV1,
) -> (RequestBudgetV1, Option<DiagnosticSpanGuardV1>) {
    let Some(context) = request.diagnostic_context() else {
        return (request, None);
    };
    let Some(span) = crate::runtime_diagnostic_span(
        context,
        context.enter(DiagnosticStageV1::RuntimeMaintenance, DiagnosticLaneV1(0)),
    ) else {
        return (request, None);
    };
    // Rebind this same request: deadline, cancellation, and funding are not reset.
    let child = context.child_context(&span);
    (request.with_diagnostic_context(child), Some(span))
}

fn finish_maintenance_round<T>(
    span: Option<DiagnosticSpanGuardV1>,
    result: &Result<T, IndexError>,
) {
    if let Some(span) = span {
        let _finished = span.finish(DiagnosticTerminalV1::from_result(result));
    }
}

fn run_retention_due(
    dispatcher: &Arc<NativePumpServices>,
    driver: &mut NativeRetentionDriver,
    next_due: &mut Instant,
    stop: &AtomicBool,
    active: &Arc<Mutex<Option<CancelHandleV1>>>,
) -> Result<(), IndexError> {
    let now = Instant::now();
    if now < *next_due || stop.load(Ordering::Acquire) {
        return Ok(());
    }
    let config = driver.config();
    // Schedule before admission/IO: refusal cannot hot-loop or renew this
    // round's original request deadline.
    *next_due = now
        .checked_add(config.interval)
        .ok_or(IndexError::ResourceLimited)?;
    let Some(reservation) = ReservationGuard::try_acquire(
        Arc::clone(&dispatcher.maintenance_scheduler),
        dispatcher.control_need,
    ) else {
        let denial = dispatcher.maintenance_scheduler.denial();
        return if denial == IndexError::ResourceLimited {
            Ok(())
        } else {
            Err(denial)
        };
    };
    let request = maintenance_request(config.round_timeout, dispatcher.diagnostic_context.as_ref());
    let _active = ActiveReclaimGuard::register(active, &request, stop)?;
    let (request, span) = begin_maintenance_round(request);
    let mut budget = WorkBudget::with_request(config.work_limits, request);
    budget.bind_memory_scope(reservation.memory_scope());
    // Preserve the raw setup error before returning the original failure.
    let _physical = match dispatcher.io.enter_maintenance_scope() {
        Ok(scope) => scope,
        Err(error) => {
            let failed = Err::<(), IndexError>(error);
            finish_maintenance_round(span, &failed);
            return failed;
        }
    };
    let round = driver.run_round(dispatcher.catalog.as_ref(), &mut budget);
    finish_maintenance_round(span, &round);
    match round {
        Ok(()) | Err(IndexError::ResourceLimited | IndexError::SnapshotUnavailable) => Ok(()),
        Err(IndexError::Cancelled) if stop.load(Ordering::Acquire) => Ok(()),
        Err(error) => Err(error),
    }
}

fn run_reclaim_due(
    dispatcher: &Arc<NativePumpServices>,
    driver: &mut NativeReclaimDriver,
    config: NativeReclaimConfig,
    stop: &AtomicBool,
    active_reclaim: &Arc<Mutex<Option<CancelHandleV1>>>,
) -> Result<(), IndexError> {
    let now = Instant::now();
    if now < driver.next_due() || stop.load(Ordering::Acquire) {
        return Ok(());
    }
    // This is the protected MaintenanceControl class in the one service
    // ledger. Owned claim scans and lease renewal retain BackgroundControl.
    let Some(maintenance) = ReservationGuard::try_acquire(
        Arc::clone(&dispatcher.maintenance_scheduler),
        dispatcher.control_need,
    ) else {
        let denial = dispatcher.maintenance_scheduler.denial();
        if denial == IndexError::ResourceLimited {
            driver.defer(now)?;
            return Ok(());
        }
        return Err(denial);
    };
    let request = maintenance_request(config.round_timeout, dispatcher.diagnostic_context.as_ref());
    let _active = ActiveReclaimGuard::register(active_reclaim, &request, stop)?;
    let (request, span) = begin_maintenance_round(request);
    let mut budget = WorkBudget::with_request(config.work_limits, request);
    budget.bind_memory_scope(maintenance.memory_scope());
    let _physical = match dispatcher.io.enter_maintenance_scope() {
        Ok(scope) => scope,
        Err(error) => {
            let failed = Err::<(), IndexError>(error);
            finish_maintenance_round(span, &failed);
            return failed;
        }
    };
    let recover_sessions = driver.read_recovery_due();
    let recovery_after = driver.read_recovery_after();
    let mut recovery_page = None;
    let mut wal_checked = false;
    let round = driver.run_attempt_round(now, &mut budget, |after, child_limit, budget| {
        if stop.load(Ordering::Acquire) {
            budget.request().cancel_handle().cancel();
        }
        budget.checkpoint()?;
        if !wal_checked {
            let _checkpointed = dispatcher.catalog.checkpoint_wal_for_reclaim(
                dispatcher.wal_checkpoint_bytes,
                dispatcher.control_need.io_bytes,
                budget,
            )?;
            wal_checked = true;
        }
        // One bounded page shares this round's reservation, deadline and held
        // physical parent. The driver schedules cadence before this work.
        if recover_sessions && recovery_page.is_none() {
            recovery_page = Some(dispatcher.catalog.recover_read_sessions(
                recovery_after,
                child_limit,
                budget,
            )?);
        }
        reclaim_one_attempt(
            &dispatcher.catalog,
            &dispatcher.objects,
            after,
            child_limit,
            budget,
        )
    });
    if let Some(page) = recovery_page {
        // A successful recovery page committed independently even if the
        // following object step failed; retain its authoritative cursor.
        driver.record_read_recovery(page);
    }
    finish_maintenance_round(span, &round);
    finish_reclaim_round(round, stop)
}

fn finish_reclaim_round(
    round: Result<NativeReclaimRound, IndexError>,
    stop: &AtomicBool,
) -> Result<(), IndexError> {
    match round {
        Ok(_round) => Ok(()),
        Err(IndexError::Cancelled) if stop.load(Ordering::Acquire) => Ok(()),
        // This disposable automatic round has already scheduled its next
        // cadence. The catalog queue and any partial drain remain durable.
        Err(IndexError::ResourceLimited) => Ok(()),
        Err(error) => Err(error),
    }
}

fn finish_pump_drive(
    drive: Result<(), IndexError>,
    drain: Result<(), IndexError>,
) -> Result<(), IndexError> {
    match (drive, drain) {
        (Err(original), Err(cleanup)) => Err(IndexError::Storage(format!(
            "native pump failed: {original}; active owned drain failed: {cleanup}"
        ))),
        (Err(error), Ok(())) | (Ok(()), Err(error)) => Err(error),
        (Ok(()), Ok(())) => Ok(()),
    }
}

fn owned_scan_request(stop: &Arc<AtomicBool>) -> Result<RequestBudgetV1, IndexError> {
    let deadline = Instant::now()
        .checked_add(Duration::from_secs(10))
        .ok_or(IndexError::ResourceLimited)?;
    Ok(RequestBudgetV1::until_with_external_cancellation(
        deadline,
        Arc::clone(stop),
    ))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum OwnedAcquireErrorDisposition {
    Continue,
    RetryScope,
    Stop,
}

fn classify_owned_acquire_error(
    error: IndexError,
    denial: Option<WorkBudgetDenial>,
    stopped: bool,
) -> Result<OwnedAcquireErrorDisposition, IndexError> {
    match error {
        IndexError::Conflict | IndexError::WorkFenceLost => {
            Ok(OwnedAcquireErrorDisposition::Continue)
        }
        IndexError::Cancelled if stopped => Ok(OwnedAcquireErrorDisposition::Stop),
        IndexError::ResourceLimited if denial == Some(WorkBudgetDenial::Deadline) => {
            Ok(OwnedAcquireErrorDisposition::RetryScope)
        }
        error @ (IndexError::InvalidInput(_)
        | IndexError::FocusSubjectNotFound
        | IndexError::DigestConflict
        | IndexError::FenceLost
        | IndexError::ResourceLimited
        | IndexError::WorkLimitExceeded
        | IndexError::Cancelled
        | IndexError::SnapshotUnavailable
        | IndexError::CorruptObject(_)
        | IndexError::BackendViolation(_)
        | IndexError::Storage(_)
        | IndexError::RebuildRequired
        | IndexError::MaintenanceRequired) => Err(error),
    }
}

fn scan_round(
    dispatcher: &Arc<NativePumpServices>,
    config: &NativePumpConfig,
    stop: &Arc<AtomicBool>,
    scopes: &mut VecDeque<NativePumpScope>,
    scan_limits: quanta_index_core::index::WorkLimits,
    max_in_flight: usize,
    runs: &mut ActiveRuns,
) -> Result<u32, IndexError> {
    // An empty or currently active scope spends a bounded scan, not one of
    // this round's dispatched-work slots.
    let visit_limit = scopes.len();
    let mut launched = 0;
    for _visit in 0..visit_limit {
        if stop.load(Ordering::Acquire)
            || launched >= config.batch_limit
            || runs.len() >= max_in_flight
        {
            break;
        }
        let scope = scopes.pop_front().ok_or_else(|| {
            IndexError::BackendViolation("native pump lost a configured scope".into())
        })?;
        scopes.push_back(scope);
        if runs.contains(&scope) || !runs.eligible(&scope, Instant::now()) {
            continue;
        }
        let mut budget = WorkBudget::with_request(scan_limits, owned_scan_request(stop)?);
        let ready = {
            let Some(control) = ReservationGuard::try_acquire(
                Arc::clone(&dispatcher.pump_control_scheduler),
                dispatcher.control_need,
            ) else {
                continue;
            };
            budget.bind_memory_scope(control.memory_scope());
            dispatcher
                .kernel
                .ready_owned_work(scope.workspace, scope.epoch, 1, &mut budget)
        };
        budget.bind_memory_scope(None);
        let ready = match ready {
            Ok(OwnedReadyOutcome::Ready(ready)) => ready,
            Ok(OwnedReadyOutcome::RetryDeadline)
            | Err(IndexError::SnapshotUnavailable | IndexError::Conflict) => continue,
            Err(IndexError::Cancelled) if stop.load(Ordering::Acquire) => return Ok(launched),
            Err(error) => return Err(error),
        };
        for intent in ready {
            if stop.load(Ordering::Acquire) {
                break;
            }
            // Reserve the worker queue and scope before durable assignment.
            let slot = match dispatcher
                .worker
                .reserve_owned_slot(scope.workspace, scope.epoch)
            {
                Ok(slot) => slot,
                Err(IndexError::ResourceLimited) => break,
                Err(error) => return Err(error),
            };
            if stop.load(Ordering::Acquire) {
                break;
            }
            let assignment = {
                let Some(control) = ReservationGuard::try_acquire(
                    Arc::clone(&dispatcher.pump_control_scheduler),
                    dispatcher.control_need,
                ) else {
                    break;
                };
                budget.bind_memory_scope(control.memory_scope());
                dispatcher
                    .kernel
                    .acquire_owned_work(&intent, config.new_assignment, &budget)
            };
            budget.bind_memory_scope(None);
            let assignment = match assignment {
                Ok(assignment) => assignment,
                Err(error) => match classify_owned_acquire_error(
                    error,
                    budget.first_denial(),
                    stop.load(Ordering::Acquire),
                )? {
                    OwnedAcquireErrorDisposition::Continue => continue,
                    OwnedAcquireErrorDisposition::RetryScope => break,
                    OwnedAcquireErrorDisposition::Stop => return Ok(launched),
                },
            };
            if let Some(assignment) = assignment {
                if stop.load(Ordering::Acquire) {
                    break;
                }
                let run = dispatcher.worker.dispatch_owned(
                    slot,
                    Arc::clone(&dispatcher.kernel),
                    assignment,
                    dispatcher.diagnostic_context.clone(),
                )?;
                runs.spawn(scope, Arc::clone(dispatcher), run)?;
                launched = launched.checked_add(1).ok_or(IndexError::ResourceLimited)?;
            }
        }
    }
    Ok(launched)
}

fn await_run(
    dispatcher: &Arc<NativePumpServices>,
    run: OwnedRunHandle,
    stop: &AtomicBool,
) -> Result<bool, IndexError> {
    let OwnedRunHandle {
        started: start_rx,
        finished: finish_rx,
        quiesced: quiesced_rx,
        cancellation: _cancellation,
    } = run;
    let started = loop {
        if stop.load(Ordering::Acquire) {
            // The receiver-owned guard cancels queued and already-started
            // execution, even when the start message has not been received.
            return Ok(false);
        }
        match start_rx.recv_timeout(Duration::from_millis(50)) {
            Ok(Ok(OwnedRunStart::Granted(started))) => break started,
            Ok(Ok(OwnedRunStart::Skipped(reason))) => {
                match reason {
                    OwnedRunSkip::Deferred(_defer) => {}
                    OwnedRunSkip::Terminal(_status) => {}
                }
                await_worker_quiescence(&quiesced_rx)?;
                return Ok(false);
            }
            Ok(Err(error)) => {
                let _finished = finish_rx.recv();
                await_worker_quiescence(&quiesced_rx)?;
                return classify_owned_start_error(error);
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => {
                return Err(IndexError::Storage(
                    "owned work start channel closed".into(),
                ));
            }
        }
    };
    match started.grant.assignment.request.kind {
        UpdateKind::Materialize { .. } => {}
        UpdateKind::SourceAdvance(_)
        | UpdateKind::SourceResync { .. }
        | UpdateKind::ProjectionAdvance(_)
        | UpdateKind::Reconfigure { .. } => {
            started.cancel.cancel();
            let _finished = finish_rx.recv();
            await_worker_quiescence(&quiesced_rx)?;
            return Err(IndexError::BackendViolation(
                "owned work grant is not materialization".into(),
            ));
        }
    }
    let renew_period = renewal_period(started.grant.assignment.policy.lease_ms)?;
    let clock_start = renewal_origin(&started.grant)?;
    let mut last_renewal = clock_start;
    let mut renewal_samples = [None; crate::native_work_diagnostic::OWNED_FAILURE_SAMPLE_LIMIT];
    let mut renewal_sample_count = 0;
    let (result, interrupted, completion_error) = await_completion(
        renew_period,
        stop,
        |wait| finish_rx.recv_timeout(wait),
        || {
            let renewal_start = Instant::now();
            let interval = renewal_start.saturating_duration_since(last_renewal);
            last_renewal = renewal_start;
            let mut phase = "catalog_renewal";
            let result = if let Some(_control) = ReservationGuard::try_acquire(
                Arc::clone(&dispatcher.pump_control_scheduler),
                dispatcher.control_need,
            ) {
                dispatcher.kernel.renew_owned_work(&started.grant)
            } else {
                phase = "scheduler_admission";
                Err(dispatcher.pump_control_scheduler.denial())
            };
            if let Some(token) = crate::native_work_diagnostic::record_owned_renewal_if_enabled(
                &started.grant,
                started.grant.assignment.policy.lease_ms,
                renew_period,
                interval,
                renewal_start.elapsed(),
                phase,
                &result,
            ) && let Some(slot) = renewal_samples.get_mut(renewal_sample_count)
            {
                *slot = Some(token);
                renewal_sample_count = renewal_sample_count.saturating_add(1);
            }
            result.map(|_claim| ())
        },
        || started.cancel.cancel(),
        || clock_start.elapsed(),
    );
    let report = if interrupted {
        OwnedWorkReport::Interrupted
    } else {
        classify_result(&result)
    };
    // Finish may rotate a terminal old operation into a new fenced attempt.
    // Release the old worker's execution and physical custody first.
    let mut worker_quiesced = false;
    let finish = await_worker_quiescence(&quiesced_rx).and_then(|()| {
        worker_quiesced = true;
        dispatcher.kernel.finish_owned_work(&started.grant, report)
    });
    if renewal_sample_count > 0 {
        for token in renewal_samples
            .into_iter()
            .take(renewal_sample_count)
            .flatten()
        {
            crate::native_work_diagnostic::record_owned_renewal_followup(
                token,
                &result,
                interrupted,
                completion_error.as_ref(),
                report,
                worker_quiesced,
                &finish,
            );
        }
    }
    finish_after_completion(completion_error, finish)?;
    Ok(true)
}

fn classify_owned_start_error(error: IndexError) -> Result<bool, IndexError> {
    match error {
        IndexError::ResourceLimited | IndexError::WorkFenceLost => Ok(false),
        fatal @ (IndexError::InvalidInput(_)
        | IndexError::FocusSubjectNotFound
        | IndexError::Conflict
        | IndexError::DigestConflict
        | IndexError::FenceLost
        | IndexError::WorkLimitExceeded
        | IndexError::Cancelled
        | IndexError::SnapshotUnavailable
        | IndexError::CorruptObject(_)
        | IndexError::BackendViolation(_)
        | IndexError::Storage(_)
        | IndexError::RebuildRequired
        | IndexError::MaintenanceRequired) => Err(fatal),
    }
}

fn await_worker_quiescence(receiver: &mpsc::Receiver<()>) -> Result<(), IndexError> {
    receiver.recv().map_err(|error| {
        IndexError::Storage(format!("native owned worker did not quiesce: {error}"))
    })
}

fn renewal_origin(grant: &quanta_index_core::index::OwnedWorkGrant) -> Result<Instant, IndexError> {
    // begin_owned_work dates active due_ms at the lease endpoint in the same
    // transaction/clock sample as execution_deadline. Translate that endpoint
    // through their shared deadline; COMMIT and grant delivery already spend
    // this lease, rather than starting another full period on receipt.
    let tail_ms = grant
        .assignment
        .deadline_ms
        .checked_sub(grant.assignment.due_ms)
        .ok_or_else(|| IndexError::BackendViolation("owned lease exceeds deadline".into()))?;
    grant
        .execution_deadline
        .checked_sub(Duration::from_millis(tail_ms))
        .and_then(|lease_end| {
            lease_end.checked_sub(Duration::from_millis(grant.assignment.policy.lease_ms))
        })
        .ok_or_else(|| {
            IndexError::BackendViolation("owned renewal origin is not representable".into())
        })
}

fn finish_after_completion(
    completion_error: Option<IndexError>,
    finish: Result<(), IndexError>,
) -> Result<(), IndexError> {
    match (completion_error, finish) {
        (Some(original), Err(cleanup)) if cleanup != IndexError::WorkFenceLost => {
            Err(IndexError::Storage(format!(
                "owned completion failed: {original}; owned settlement failed: {cleanup}"
            )))
        }
        (Some(original), _) => Err(original),
        (None, Ok(()) | Err(IndexError::WorkFenceLost)) => Ok(()),
        (None, Err(error)) => Err(error),
    }
}

// The same wait/renew loop is driven by an elapsed clock and receive function
// so its scheduling can be checked without wall-clock sleeps.
type OwnedCompletion = Result<quanta_index_core::index::SubmitOutcome, IndexError>;

fn await_completion<Receive, Renew, Cancel, Now>(
    renew_period: Duration,
    stop: &AtomicBool,
    mut receive: Receive,
    mut renew: Renew,
    mut cancel: Cancel,
    now: Now,
) -> (OwnedCompletion, bool, Option<IndexError>)
where
    Receive: FnMut(Duration) -> Result<OwnedCompletion, RecvTimeoutError>,
    Renew: FnMut() -> Result<(), IndexError>,
    Cancel: FnMut(),
    Now: Fn() -> Duration,
{
    // The caller's elapsed clock starts when the durable lease was dated,
    // including any latency before this receiver accepted the grant.
    let mut last_renew_attempt = Duration::ZERO;
    let mut interrupted = false;
    let mut completion_error = None;
    let result = loop {
        if stop.load(Ordering::Acquire) {
            cancel();
            interrupted = true;
        }
        let wait = if interrupted {
            Duration::from_millis(50)
        } else {
            renewal_wait(renew_period, now().saturating_sub(last_renew_attempt))
        };
        match receive(wait) {
            Ok(result) => break result,
            Err(RecvTimeoutError::Disconnected) => {
                let error = IndexError::Storage("owned work result channel closed".into());
                if completion_error.is_none() {
                    completion_error = Some(error.clone());
                }
                break Err(error);
            }
            Err(RecvTimeoutError::Timeout) => {}
        }
        if !interrupted && now().saturating_sub(last_renew_attempt) >= renew_period {
            // The durable lease is dated inside the renewal transaction, before
            // its COMMIT returns. Count that call's latency in the cadence;
            // waiting another full period after it can outlive the same lease.
            last_renew_attempt = now();
            let renewed = renew();
            // Fast resource denials still wait a positive period. A slow call
            // has already consumed that period; poll completion before renewal.
            match renewed {
                Ok(()) | Err(IndexError::ResourceLimited) => {}
                Err(IndexError::WorkFenceLost) => {
                    cancel();
                    interrupted = true;
                }
                Err(error) => {
                    completion_error = Some(error);
                    cancel();
                    interrupted = true;
                }
            }
        }
    };
    (result, interrupted, completion_error)
}

fn renewal_period(lease_ms: u64) -> Result<Duration, IndexError> {
    // Divide the duration after conversion: a valid 1 ms lease must renew
    // before expiry rather than rounding its first renewal up to 1 ms.
    Duration::from_millis(lease_ms)
        .checked_div(3)
        .map(|period| period.min(Duration::from_secs(1)))
        .ok_or(IndexError::ResourceLimited)
}

fn renewal_wait(period: Duration, elapsed: Duration) -> Duration {
    period
        .saturating_sub(elapsed)
        .min(Duration::from_millis(50))
}

fn classify_result(
    result: &Result<quanta_index_core::index::SubmitOutcome, IndexError>,
) -> OwnedWorkReport {
    match result {
        Ok(_outcome) => OwnedWorkReport::Reconcile,
        Err(IndexError::InvalidInput(_) | IndexError::DigestConflict) => {
            OwnedWorkReport::Failed(Refusal::InvalidInput)
        }
        Err(IndexError::BackendViolation(_) | IndexError::CorruptObject(_)) => {
            OwnedWorkReport::Failed(Refusal::BackendViolation)
        }
        Err(IndexError::RebuildRequired) => OwnedWorkReport::Failed(Refusal::RebuildRequired),
        Err(IndexError::MaintenanceRequired) => {
            OwnedWorkReport::Failed(Refusal::MaintenanceRequired)
        }
        Err(_uncertain) => OwnedWorkReport::Interrupted,
    }
}
#[cfg(test)]
mod renewal_tests {
    use super::*;
    use crate::native_reclaim_driver::{NativeReclaimStep, ReclaimAttempt};
    use quanta_index_contract::index::{CollectionId, Digest, SlotId, TenantId, WorktreeId};
    use quanta_index_core::index::{ObjectRef, WorkBudgetDenial, WorkLimits, WorkUsage};
    use std::cell::{Cell, RefCell};

    fn require(condition: bool, message: &'static str) -> Result<(), IndexError> {
        if condition {
            Ok(())
        } else {
            Err(IndexError::BackendViolation(message.into()))
        }
    }

    #[test]
    fn owned_scan_request_observes_original_manager_stop() -> Result<(), IndexError> {
        let stop = Arc::new(AtomicBool::new(false));
        let budget = WorkBudget::with_request(
            SqliteCatalog::owned_scan_limits(1)?,
            owned_scan_request(&stop)?,
        );
        budget.checkpoint()?;
        stop.store(true, Ordering::Release);
        require(
            budget.checkpoint() == Err(IndexError::Cancelled)
                && budget.first_denial() == Some(WorkBudgetDenial::Cancelled),
            "owned scan did not observe the original manager stop",
        )
    }

    #[test]
    fn owned_acquire_retries_only_the_scan_deadline() -> Result<(), IndexError> {
        require(
            classify_owned_acquire_error(
                IndexError::ResourceLimited,
                Some(WorkBudgetDenial::Deadline),
                false,
            ) == Ok(OwnedAcquireErrorDisposition::RetryScope),
            "owned acquire did not retry the scan deadline",
        )?;
        for denial in [
            None,
            Some(WorkBudgetDenial::Reads),
            Some(WorkBudgetDenial::Memory),
        ] {
            require(
                matches!(
                    classify_owned_acquire_error(IndexError::ResourceLimited, denial, false),
                    Err(IndexError::ResourceLimited)
                ),
                "owned acquire swallowed a non-deadline resource refusal",
            )?;
        }
        require(
            classify_owned_acquire_error(IndexError::Cancelled, None, true)
                == Ok(OwnedAcquireErrorDisposition::Stop),
            "owned acquire did not stop on manager cancellation",
        )?;
        require(
            matches!(
                classify_owned_acquire_error(IndexError::Cancelled, None, false),
                Err(IndexError::Cancelled)
            ),
            "owned acquire swallowed an unrelated cancellation",
        )?;
        require(
            matches!(
                classify_owned_acquire_error(IndexError::Storage("storage".into()), None, false),
                Err(IndexError::Storage(_))
            ),
            "owned acquire swallowed a storage failure",
        )?;
        require(
            classify_owned_acquire_error(IndexError::Conflict, None, false)
                == Ok(OwnedAcquireErrorDisposition::Continue)
                && classify_owned_acquire_error(IndexError::WorkFenceLost, None, false)
                    == Ok(OwnedAcquireErrorDisposition::Continue),
            "owned acquire changed stale fence handling",
        )
    }

    #[test]
    fn reclaim_budget_refusal_preserves_cadence_cursor_and_errors() -> Result<(), IndexError> {
        let config = NativeReclaimConfig {
            interval: Duration::from_millis(20),
            max_steps_per_round: 1,
            child_limit: 1,
            work_limits: WorkLimits {
                reads: 16_384,
                writes: 256,
                bytes: 134_217_728,
                mutations: 256,
            },
            round_timeout: Duration::from_secs(5),
            head_probe_every: 4,
        };
        let first = ObjectRef(Digest([1; 32]));
        let second = ObjectRef(Digest([2; 32]));
        let stop = AtomicBool::new(false);
        let mut driver = NativeReclaimDriver::new(config, Instant::now())?;
        let fresh_budget = || {
            WorkBudget::with_request(
                config.work_limits,
                RequestBudgetV1::for_duration(config.round_timeout),
            )
        };
        let mut first_budget = fresh_budget();
        let first_round =
            driver.run_attempt_round(driver.next_due(), &mut first_budget, |after, _, _| {
                require(after.is_none(), "first sweep did not start at the head")?;
                Ok(ReclaimAttempt::Applied(NativeReclaimStep {
                    examined: Some(first),
                    drain: None,
                    resume_after: Some(first),
                    has_more: true,
                }))
            });
        require(
            matches!(&first_round, Ok(round) if round.steps == 1 && round.examined == 1),
            "initial reclaim step did not advance",
        )?;
        finish_reclaim_round(first_round, &stop)?;

        let due_before_refusal = driver.next_due();
        let mut limited_budget = fresh_budget();
        let limited = driver.run_attempt_round(
            due_before_refusal,
            &mut limited_budget,
            |after, _, budget| {
                require(after == Some(first), "forward cursor skipped the first key")?;
                budget.charge(WorkUsage {
                    reads: config.work_limits.reads,
                    ..WorkUsage::default()
                })?;
                budget.charge(WorkUsage {
                    reads: 1,
                    ..WorkUsage::default()
                })?;
                Err(IndexError::BackendViolation(
                    "over-limit charge unexpectedly succeeded".into(),
                ))
            },
        );
        require(
            limited == Err(IndexError::ResourceLimited),
            "budget cap did not refuse the round",
        )?;
        require(
            driver.next_due() > due_before_refusal,
            "resource refusal did not schedule a positive retry cadence",
        )?;
        finish_reclaim_round(limited, &stop)?;

        let mut retry_budget = fresh_budget();
        let retry =
            driver.run_attempt_round(driver.next_due(), &mut retry_budget, |after, _, _| {
                require(
                    after == Some(first),
                    "resource refusal changed the forward cursor",
                )?;
                Ok(ReclaimAttempt::Applied(NativeReclaimStep {
                    examined: Some(second),
                    drain: None,
                    resume_after: Some(second),
                    has_more: false,
                }))
            });
        require(
            matches!(&retry, Ok(round) if round.steps == 1 && round.examined == 1),
            "reclaim could not execute the next scheduled round",
        )?;
        finish_reclaim_round(retry, &stop)?;

        let due_before_timeout = driver.next_due();
        let mut expired_budget = WorkBudget::new(config.work_limits, Instant::now());
        let expired =
            driver.run_attempt_round(due_before_timeout, &mut expired_budget, |_, _, _| {
                Err(IndexError::BackendViolation(
                    "expired budget reached physical work".into(),
                ))
            });
        require(
            expired == Err(IndexError::ResourceLimited),
            "expired round did not refuse before physical work",
        )?;
        require(
            driver.next_due() > due_before_timeout,
            "expired round did not schedule a positive retry cadence",
        )?;
        finish_reclaim_round(expired, &stop)?;

        for fatal in [
            IndexError::Storage("reclaim IO failed".into()),
            IndexError::FenceLost,
            IndexError::CorruptObject("reclaim object changed".into()),
            IndexError::BackendViolation("reclaim binding changed".into()),
            IndexError::Conflict,
        ] {
            require(
                finish_reclaim_round(Err(fatal.clone()), &stop) == Err(fatal),
                "non-budget reclaim error was masked",
            )?;
        }
        Ok(())
    }

    fn test_scope(tenant: u8) -> NativePumpScope {
        NativePumpScope {
            workspace: WorkspaceKey {
                tenant: TenantId([tenant; 16]),
                collection: CollectionId([2; 16]),
                worktree: WorktreeId([3; 16]),
                slot: SlotId([4; 16]),
            },
            epoch: 1,
        }
    }

    fn finish_test_run(
        runs: &mut ActiveRuns,
        scope: NativePumpScope,
        executed: bool,
        now: Instant,
    ) -> Result<bool, IndexError> {
        let handle = thread::spawn(|| {});
        require(
            runs.handles.insert(scope, handle).is_none(),
            "test scope already active",
        )?;
        runs.accept(
            RunFinished {
                scope,
                result: Ok(executed),
            },
            now,
        )
    }

    #[test]
    fn false_owned_completion_wakes_manager() -> Result<(), IndexError> {
        let scope = test_scope(1);
        let mut runs = ActiveRuns::new(Arc::new(AtomicBool::new(false)));
        let handle = thread::spawn(|| {});
        require(
            runs.handles.insert(scope, handle).is_none(),
            "test scope already active",
        )?;
        runs.finished_tx
            .send(RunFinished {
                scope,
                result: Ok(false),
            })
            .map_err(|error| IndexError::Storage(format!("send owned completion: {error}")))?;

        let woke = runs.wait(Duration::from_millis(1))?;
        require(!runs.contains(&scope), "completed scope was not removed")?;
        require(woke, "false completion did not wake manager")
    }

    #[test]
    fn pregrant_resource_and_fence_declines_keep_manager_live() -> Result<(), IndexError> {
        let mut runs = ActiveRuns::new(Arc::new(AtomicBool::new(false)));
        runs.set_retry_delay(Duration::from_millis(10));
        let now = Instant::now();
        for (index, error) in [IndexError::ResourceLimited, IndexError::WorkFenceLost]
            .into_iter()
            .enumerate()
        {
            let declined = test_scope(u8::try_from(index + 1).map_err(|_error| {
                IndexError::BackendViolation("test scope index overflow".into())
            })?);
            let result = classify_owned_start_error(error)?;
            require(!result, "pregrant refusal reported execution")?;
            require(
                !finish_test_run(&mut runs, declined, result, now)?,
                "pregrant refusal stopped manager",
            )?;
        }
        require(
            runs.eligible(&test_scope(3), now),
            "peer scope inherited pregrant refusal cooldown",
        )?;
        require(
            classify_owned_start_error(IndexError::Storage("catalog failure".into()))
                == Err(IndexError::Storage("catalog failure".into())),
            "storage failure was swallowed",
        )
    }

    #[test]
    fn declined_scope_waits_one_positive_window_while_peer_remains_eligible()
    -> Result<(), IndexError> {
        let declined = test_scope(1);
        let peer = test_scope(2);
        let mut runs = ActiveRuns::new(Arc::new(AtomicBool::new(false)));
        let delay = Duration::from_millis(10);
        runs.set_retry_delay(delay);
        let now = Instant::now();
        require(
            !finish_test_run(&mut runs, declined, false, now)?,
            "declined run reported execution",
        )?;
        let due = now.checked_add(delay).ok_or(IndexError::ResourceLimited)?;
        let before_due = due
            .checked_sub(Duration::from_nanos(1))
            .ok_or(IndexError::ResourceLimited)?;
        require(
            !runs.eligible(&declined, now),
            "declined scope retried immediately",
        )?;
        require(
            runs.eligible(&peer, now),
            "peer inherited declined scope cooldown",
        )?;
        require(
            !runs.eligible(&declined, before_due),
            "declined scope retried before its deadline",
        )?;
        require(
            runs.eligible(&declined, due),
            "declined scope stayed blocked at its deadline",
        )?;
        require(
            runs.next_retry_due() == Some(due),
            "retry deadline differs from bounded cadence",
        )?;
        runs.prune_expired_retries(due);
        require(
            runs.next_retry_due().is_none(),
            "expired retry deadline was retained",
        )?;

        require(
            !finish_test_run(&mut runs, declined, false, due)?,
            "second declined run reported execution",
        )?;
        let next_due = due.checked_add(delay).ok_or(IndexError::ResourceLimited)?;
        require(
            !runs.eligible(&declined, due) && runs.next_retry_due() == Some(next_due),
            "consecutive denial created a zero-delay retry",
        )?;
        require(
            runs.eligible(&peer, due),
            "peer was blocked by consecutive denial",
        )
    }

    #[test]
    fn executed_scope_does_not_enter_retry_cooldown() -> Result<(), IndexError> {
        let scope = test_scope(1);
        let mut runs = ActiveRuns::new(Arc::new(AtomicBool::new(false)));
        let now = Instant::now();
        require(
            finish_test_run(&mut runs, scope, true, now)?,
            "executed run reported no execution",
        )?;
        require(
            runs.eligible(&scope, now) && runs.next_retry_due().is_none(),
            "executed scope entered retry cooldown",
        )
    }

    #[test]
    fn reclaim_registration_observes_stop_and_clears_active_slot() -> Result<(), IndexError> {
        let slot = Arc::new(Mutex::new(None));
        let stop = AtomicBool::new(true);
        let request = RequestBudgetV1::for_duration(Duration::from_secs(1));
        let active = ActiveReclaimGuard::register(&slot, &request, &stop)?;
        require(request.is_cancelled(), "registered request missed stop")?;
        require(
            slot.lock()
                .map_err(|_poison| IndexError::Storage("reclaim slot poisoned".into()))?
                .is_some(),
            "active reclaim slot was not published",
        )?;
        drop(active);
        require(
            slot.lock()
                .map_err(|_poison| IndexError::Storage("reclaim slot poisoned".into()))?
                .is_none(),
            "active reclaim slot was not cleared",
        )?;
        Ok(())
    }

    #[test]
    fn reclaim_registration_rejects_overlap_and_cloned_cancel_survives_drop()
    -> Result<(), IndexError> {
        let slot = Arc::new(Mutex::new(None));
        let stop = AtomicBool::new(false);
        let first = RequestBudgetV1::for_duration(Duration::from_secs(1));
        let active = ActiveReclaimGuard::register(&slot, &first, &stop)?;
        let second = RequestBudgetV1::for_duration(Duration::from_secs(1));
        require(
            matches!(
                ActiveReclaimGuard::register(&slot, &second, &stop),
                Err(IndexError::BackendViolation(_))
            ),
            "overlapping reclaim request was accepted",
        )?;
        let cancel = slot
            .lock()
            .map_err(|_poison| IndexError::Storage("reclaim slot poisoned".into()))?
            .clone()
            .ok_or_else(|| IndexError::BackendViolation("missing active reclaim".into()))?;
        drop(active);
        cancel.cancel();
        require(
            first.is_cancelled(),
            "cloned active handle failed to cancel",
        )?;
        require(!second.is_cancelled(), "peer request was cancelled")?;
        let next = ActiveReclaimGuard::register(&slot, &second, &stop)?;
        require(
            !second.is_cancelled(),
            "new active request inherited cancellation",
        )?;
        drop(next);
        Ok(())
    }

    #[test]
    fn stop_join_cancels_outside_active_reclaim_mutex_before_join() -> Result<(), IndexError> {
        let slot = Arc::new(Mutex::new(None));
        let stop = Arc::new(AtomicBool::new(false));
        let request = RequestBudgetV1::for_duration(Duration::from_secs(2));
        let active = ActiveReclaimGuard::register(&slot, &request, &stop)?;
        let callback_unlocked = Arc::new(AtomicBool::new(false));
        let callback_slot = Arc::clone(&slot);
        let callback_result = Arc::clone(&callback_unlocked);
        let _waiter = request.cancel_waiter(Arc::new(move || {
            callback_result.store(callback_slot.try_lock().is_ok(), Ordering::Release);
        }));
        let manager_request = request;
        let manager = thread::Builder::new()
            .name("native-reclaim-stop-test".into())
            .spawn(move || {
                let deadline = Instant::now()
                    .checked_add(Duration::from_secs(1))
                    .ok_or(IndexError::ResourceLimited)?;
                while !manager_request.is_cancelled() && Instant::now() < deadline {
                    thread::park_timeout(Duration::from_millis(1));
                }
                require(manager_request.is_cancelled(), "manager was not cancelled")
            })
            .map_err(|error| IndexError::Storage(format!("spawn reclaim test manager: {error}")))?;
        let mut pump = NativeOwnedPump {
            stop,
            active_reclaim: Arc::clone(&slot),
            manager: Some(manager),
            terminal: None,
        };
        pump.stop_join()?;
        require(
            callback_unlocked.load(Ordering::Acquire),
            "stop cancellation callback ran while active reclaim mutex was held",
        )?;
        drop(active);
        require(
            slot.lock()
                .map_err(|_poison| IndexError::Storage("reclaim slot poisoned".into()))?
                .is_none(),
            "active reclaim slot remained after drain",
        )
    }

    #[test]
    fn scan_and_active_drain_failures_are_both_returned() {
        let result = finish_pump_drive(
            Err(IndexError::Storage("catalog scan failed".into())),
            Err(IndexError::Storage("owned settlement failed".into())),
        );
        let Err(IndexError::Storage(message)) = result else {
            panic!("expected combined native pump failure");
        };
        assert!(message.contains("catalog scan failed"));
        assert!(message.contains("owned settlement failed"));
    }

    #[test]
    fn await_completion_uses_renewal_deadline_for_short_lease() {
        let elapsed = Cell::new(Duration::ZERO);
        let timed_out = Cell::new(false);
        let observed_waits = RefCell::new(Vec::new());
        let renew_calls = Cell::new(0_u32);
        let cancel_calls = Cell::new(0_u32);
        let stop = AtomicBool::new(false);
        let (result, interrupted, completion_error) = await_completion(
            Duration::from_millis(10),
            &stop,
            |wait| {
                observed_waits.borrow_mut().push(wait);
                if timed_out.replace(true) {
                    Ok(Err(IndexError::Cancelled))
                } else {
                    elapsed.set(elapsed.get().saturating_add(wait));
                    Err(RecvTimeoutError::Timeout)
                }
            },
            || {
                renew_calls.set(renew_calls.get().saturating_add(1));
                Ok(())
            },
            || cancel_calls.set(cancel_calls.get().saturating_add(1)),
            || elapsed.get(),
        );
        assert_eq!(
            observed_waits.borrow().as_slice(),
            &[Duration::from_millis(10), Duration::from_millis(10)]
        );
        assert_eq!(renew_calls.get(), 1);
        assert!(matches!(result, Err(IndexError::Cancelled)));
        assert!(!interrupted);
        assert_eq!(completion_error, None);
        assert_eq!(cancel_calls.get(), 0);
    }

    #[test]
    fn await_completion_renews_before_smallest_admitted_lease_expires() -> Result<(), IndexError> {
        for (lease_ms, expected_ns) in [(1, 333_333), (2, 666_666)] {
            let elapsed = Cell::new(Duration::ZERO);
            let timed_out = Cell::new(false);
            let observed_waits = RefCell::new(Vec::new());
            let renew_calls = Cell::new(0_u32);
            let cancel_calls = Cell::new(0_u32);
            let stop = AtomicBool::new(false);
            let (result, interrupted, completion_error) = await_completion(
                renewal_period(lease_ms)?,
                &stop,
                |wait| {
                    observed_waits.borrow_mut().push(wait);
                    if timed_out.replace(true) {
                        Ok(Err(IndexError::Cancelled))
                    } else {
                        elapsed.set(wait);
                        Err(RecvTimeoutError::Timeout)
                    }
                },
                || {
                    if elapsed.get() >= Duration::from_millis(lease_ms) {
                        return Err(IndexError::BackendViolation(format!(
                            "renewal reached lease expiry: lease_ms={lease_ms}, elapsed={:?}",
                            elapsed.get(),
                        )));
                    }
                    renew_calls.set(renew_calls.get().saturating_add(1));
                    Ok(())
                },
                || cancel_calls.set(cancel_calls.get().saturating_add(1)),
                || elapsed.get(),
            );
            let expected = Duration::from_nanos(expected_ns);
            let waits = observed_waits.borrow();
            if waits.as_slice() != [expected, expected]
                || renew_calls.get() != 1
                || cancel_calls.get() != 0
                || !matches!(&result, Err(IndexError::Cancelled))
                || interrupted
                || completion_error.is_some()
            {
                return Err(IndexError::BackendViolation(format!(
                    "short-lease renewal gold differs: lease_ms={lease_ms}, waits={waits:?}, \
                     renewals={}, cancellations={}, result={result:?}, interrupted={interrupted}, \
                     completion_error={completion_error:?}",
                    renew_calls.get(),
                    cancel_calls.get(),
                )));
            }
        }
        Ok(())
    }

    #[test]
    fn slow_successful_renewal_does_not_add_wait_past_durable_lease() -> Result<(), IndexError> {
        // The lease starts at zero; delivery can consume most of it.
        for delivery in [Duration::ZERO, Duration::from_millis(70)] {
            let elapsed = Cell::new(delivery);
            let lease = Duration::from_millis(90);
            let expires = Cell::new(lease);
            let renew_calls = Cell::new(0_u32);
            let receives = Cell::new(0_u32);
            let cancel_calls = Cell::new(0_u32);
            let stop = AtomicBool::new(false);
            let (result, interrupted, completion_error) = await_completion(
                renewal_period(90)?,
                &stop,
                |wait| {
                    receives.set(receives.get().saturating_add(1));
                    if renew_calls.get() >= 2 || receives.get() > 8 {
                        return Ok(Err(IndexError::Cancelled));
                    }
                    elapsed.set(elapsed.get().saturating_add(wait));
                    Err(RecvTimeoutError::Timeout)
                },
                || {
                    renew_calls.set(renew_calls.get().saturating_add(1));
                    if elapsed.get() >= expires.get() {
                        return Err(IndexError::WorkFenceLost);
                    }
                    // Model the independent Catalog contract: date the lease in
                    // the transaction, then spend time durably committing it.
                    expires.set(elapsed.get().saturating_add(lease));
                    elapsed.set(elapsed.get().saturating_add(Duration::from_millis(70)));
                    Ok(())
                },
                || cancel_calls.set(cancel_calls.get().saturating_add(1)),
                || elapsed.get(),
            );
            if renew_calls.get() != 2
                || receives.get() > 8
                || cancel_calls.get() != 0
                || interrupted
                || completion_error.is_some()
                || !matches!(result, Err(IndexError::Cancelled))
            {
                return Err(IndexError::BackendViolation(format!(
                    "slow durable renewal lost its unchanged lease: renewals={}, receives={}, \
                 cancels={}, interrupted={interrupted}, result={result:?}, \
                 completion_error={completion_error:?}",
                    renew_calls.get(),
                    receives.get(),
                    cancel_calls.get(),
                )));
            }
        }
        Ok(())
    }

    #[test]
    fn resource_denial_retries_on_positive_renewal_cadence() {
        let elapsed = Cell::new(Duration::ZERO);
        let receives = Cell::new(0_u32);
        let observed_waits = RefCell::new(Vec::new());
        let renew_calls = Cell::new(0_u32);
        let cancel_calls = Cell::new(0_u32);
        let stop = AtomicBool::new(false);
        let (result, interrupted, completion_error) = await_completion(
            Duration::from_millis(10),
            &stop,
            |wait| {
                observed_waits.borrow_mut().push(wait);
                receives.set(receives.get().saturating_add(1));
                if receives.get() <= 2 {
                    elapsed.set(elapsed.get().saturating_add(wait));
                    Err(RecvTimeoutError::Timeout)
                } else {
                    Ok(Err(IndexError::Cancelled))
                }
            },
            || {
                renew_calls.set(renew_calls.get().saturating_add(1));
                Err(IndexError::ResourceLimited)
            },
            || cancel_calls.set(cancel_calls.get().saturating_add(1)),
            || elapsed.get(),
        );
        assert_eq!(
            observed_waits.borrow().as_slice(),
            &[
                Duration::from_millis(10),
                Duration::from_millis(10),
                Duration::from_millis(10),
            ]
        );
        assert_eq!(renew_calls.get(), 2);
        assert_eq!(cancel_calls.get(), 0);
        assert!(matches!(result, Err(IndexError::Cancelled)));
        assert!(!interrupted);
        assert_eq!(completion_error, None);
    }

    #[test]
    fn lost_renewal_fence_cancels_and_drains_as_interrupted() {
        let elapsed = Cell::new(Duration::ZERO);
        let receives = Cell::new(0_u32);
        let observed_waits = RefCell::new(Vec::new());
        let renew_calls = Cell::new(0_u32);
        let cancel_calls = Cell::new(0_u32);
        let stop = AtomicBool::new(false);
        let (result, interrupted, completion_error) = await_completion(
            Duration::from_millis(10),
            &stop,
            |wait| {
                observed_waits.borrow_mut().push(wait);
                receives.set(receives.get().saturating_add(1));
                if receives.get() == 1 {
                    elapsed.set(elapsed.get().saturating_add(wait));
                    Err(RecvTimeoutError::Timeout)
                } else {
                    Ok(Err(IndexError::InvalidInput("late provider result".into())))
                }
            },
            || {
                renew_calls.set(renew_calls.get().saturating_add(1));
                Err(IndexError::WorkFenceLost)
            },
            || cancel_calls.set(cancel_calls.get().saturating_add(1)),
            || elapsed.get(),
        );
        assert_eq!(
            observed_waits.borrow().as_slice(),
            &[Duration::from_millis(10), Duration::from_millis(50)]
        );
        assert_eq!(renew_calls.get(), 1);
        assert_eq!(cancel_calls.get(), 1);
        assert!(matches!(result, Err(IndexError::InvalidInput(_))));
        assert!(interrupted);
        assert_eq!(
            finish_after_completion(completion_error, Err(IndexError::WorkFenceLost)),
            Ok(())
        );
    }

    #[test]
    fn renewal_storage_failure_cancels_drains_and_surfaces_after_settlement() {
        let elapsed = Cell::new(Duration::ZERO);
        let receives = Cell::new(0_u32);
        let observed_waits = RefCell::new(Vec::new());
        let renew_calls = Cell::new(0_u32);
        let cancel_calls = Cell::new(0_u32);
        let stop = AtomicBool::new(false);
        let (result, interrupted, completion_error) = await_completion(
            Duration::from_millis(10),
            &stop,
            |wait| {
                observed_waits.borrow_mut().push(wait);
                receives.set(receives.get().saturating_add(1));
                if receives.get() == 1 {
                    elapsed.set(wait);
                    Err(RecvTimeoutError::Timeout)
                } else {
                    Ok(Err(IndexError::Cancelled))
                }
            },
            || {
                renew_calls.set(renew_calls.get().saturating_add(1));
                Err(IndexError::Storage("renew write failed".into()))
            },
            || cancel_calls.set(cancel_calls.get().saturating_add(1)),
            || elapsed.get(),
        );
        assert_eq!(
            observed_waits.borrow().as_slice(),
            &[Duration::from_millis(10), Duration::from_millis(50)]
        );
        assert_eq!(renew_calls.get(), 1);
        assert_eq!(cancel_calls.get(), 1);
        assert_eq!(result, Err(IndexError::Cancelled));
        assert!(interrupted);
        assert_eq!(
            finish_after_completion(completion_error.clone(), Ok(())),
            Err(IndexError::Storage("renew write failed".into()))
        );
        assert_eq!(
            finish_after_completion(completion_error, Err(IndexError::WorkFenceLost)),
            Err(IndexError::Storage("renew write failed".into()))
        );
        let both = finish_after_completion(
            Some(IndexError::Storage("renew write failed".into())),
            Err(IndexError::Storage("finish write failed".into())),
        );
        assert!(matches!(both, Err(IndexError::Storage(message))
            if message.contains("renew write failed") && message.contains("finish write failed")));
    }

    #[test]
    fn stop_cancels_before_waiting_for_completion() {
        let observed_waits = RefCell::new(Vec::new());
        let renew_calls = Cell::new(0_u32);
        let cancel_calls = Cell::new(0_u32);
        let stop = AtomicBool::new(true);
        let (result, interrupted, completion_error) = await_completion(
            Duration::from_millis(10),
            &stop,
            |wait| {
                observed_waits.borrow_mut().push(wait);
                Ok(Err(IndexError::Cancelled))
            },
            || {
                renew_calls.set(renew_calls.get().saturating_add(1));
                Ok(())
            },
            || cancel_calls.set(cancel_calls.get().saturating_add(1)),
            || Duration::ZERO,
        );
        assert_eq!(
            observed_waits.borrow().as_slice(),
            &[Duration::from_millis(50)]
        );
        assert_eq!(renew_calls.get(), 0);
        assert_eq!(cancel_calls.get(), 1);
        assert!(matches!(result, Err(IndexError::Cancelled)));
        assert!(interrupted);
        assert_eq!(completion_error, None);
    }

    #[test]
    fn disconnected_result_channel_is_interrupted_without_renewal() {
        let observed_waits = RefCell::new(Vec::new());
        let renew_calls = Cell::new(0_u32);
        let cancel_calls = Cell::new(0_u32);
        let stop = AtomicBool::new(false);
        let (result, interrupted, completion_error) = await_completion(
            Duration::from_millis(10),
            &stop,
            |wait| {
                observed_waits.borrow_mut().push(wait);
                Err(RecvTimeoutError::Disconnected)
            },
            || {
                renew_calls.set(renew_calls.get().saturating_add(1));
                Ok(())
            },
            || cancel_calls.set(cancel_calls.get().saturating_add(1)),
            || Duration::ZERO,
        );
        assert_eq!(
            observed_waits.borrow().as_slice(),
            &[Duration::from_millis(10)]
        );
        assert_eq!(renew_calls.get(), 0);
        assert_eq!(cancel_calls.get(), 0);
        assert!(matches!(result, Err(IndexError::Storage(_))));
        assert!(!interrupted);
        assert!(matches!(
            completion_error.as_ref(),
            Some(IndexError::Storage(_))
        ));
        assert_eq!(classify_result(&result), OwnedWorkReport::Interrupted);
        assert!(matches!(
            finish_after_completion(completion_error, Ok(())),
            Err(IndexError::Storage(_))
        ));
    }
}

#[cfg(test)]
mod maintenance_diagnostic_tests {
    use super::*;
    use anyhow::{Context as _, Result, ensure};
    use quanta_index_core::{DiagnosticLaneV1, DiagnosticRootKindV1};
    use quanta_index_searchd::{DiagnosticCaptureLimitsV1, DiagnosticCaptureV1};
    use serde_json::Value;

    #[test]
    fn maintenance_round_keeps_operation_parent_and_raw_refusal_terminal() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("maintenance.jsonl");
        let mut recorder =
            DiagnosticCaptureV1::start(path.clone(), DiagnosticCaptureLimitsV1::default())?;
        let lease = recorder.lease().context("capture lease absent")?;
        let root = lease
            .context()
            .enter_root(DiagnosticRootKindV1::Operation, DiagnosticLaneV1(0))?;
        let root_id = root.span_id().get();
        let child = lease.context().child_context(&root);
        let request = maintenance_request(Duration::from_secs(1), Some(&child));
        let original_deadline = request.deadline();
        let original_request = request.clone();
        let original_cancel = request.cancel_handle();
        let (request, span) = begin_maintenance_round(request);
        ensure!(request.same_request_as(&original_request));
        ensure!(request.deadline() == original_deadline);
        let span = span.context("maintenance span absent")?;
        let span_id = span.span_id().get();
        let limits = quanta_index_core::index::WorkLimits {
            reads: 1,
            writes: 1,
            bytes: 1,
            mutations: 1,
        };
        let budget = WorkBudget::with_request(limits, request);
        ensure!(budget.request().same_request_as(&original_request));
        ensure!(budget.request().deadline() == original_deadline);
        let nested = budget
            .request()
            .diagnostic_context()
            .context("budget diagnostic context absent")?
            .enter(DiagnosticStageV1::SqliteWork, DiagnosticLaneV1(0))?;
        let nested_id = nested.span_id().get();
        nested.finish(DiagnosticTerminalV1::Ok)?;
        finish_maintenance_round(
            Some(span),
            &Err::<(), IndexError>(IndexError::ResourceLimited),
        );
        drop(budget);
        original_cancel.cancel();
        let resumed_budget = WorkBudget::with_request(limits, original_request);
        ensure!(resumed_budget.request().deadline() == original_deadline);
        ensure!(
            matches!(resumed_budget.checkpoint(), Err(IndexError::Cancelled)),
            "reconstructed work budget lost original cancellation"
        );
        drop(resumed_budget);
        let (off_request, off_span) =
            begin_maintenance_round(maintenance_request(Duration::from_secs(1), None));
        ensure!(off_span.is_none() && off_request.diagnostic_context().is_none());
        drop(child);
        root.finish(DiagnosticTerminalV1::Ok)?;
        drop(lease);
        let closed = recorder.close(Duration::from_secs(2))?;
        ensure!(
            closed.status == quanta_index_searchd::DiagnosticCaptureStatusV1::Complete,
            "maintenance diagnostic capture incomplete"
        );
        let rows: Vec<Value> = std::fs::read_to_string(path)?
            .lines()
            .map(serde_json::from_str)
            .collect::<std::result::Result<_, _>>()?;
        ensure!(rows.iter().any(|row| {
            row.pointer("/kind/type").and_then(Value::as_str) == Some("span_begin")
                && row.pointer("/kind/stage").and_then(Value::as_str) == Some("runtime_maintenance")
                && row.pointer("/kind/span_id").and_then(Value::as_u64) == Some(span_id)
                && row.pointer("/kind/parent_span_id").and_then(Value::as_u64) == Some(root_id)
        }));
        ensure!(rows.iter().any(|row| {
            row.pointer("/kind/type").and_then(Value::as_str) == Some("span_begin")
                && row.pointer("/kind/stage").and_then(Value::as_str) == Some("sqlite_work")
                && row.pointer("/kind/span_id").and_then(Value::as_u64) == Some(nested_id)
                && row.pointer("/kind/parent_span_id").and_then(Value::as_u64) == Some(span_id)
        }));
        ensure!(rows.iter().any(|row| {
            row.pointer("/kind/type").and_then(Value::as_str) == Some("span_end")
                && row.pointer("/kind/span_id").and_then(Value::as_u64) == Some(span_id)
                && row.pointer("/kind/outcome").and_then(Value::as_str) == Some("error")
        }));
        Ok(())
    }
}
