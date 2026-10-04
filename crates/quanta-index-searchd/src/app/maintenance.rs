//! The composition root's maintenance timer (QI-BB-016, QI-BB-015).
//!
//! The runtime owns a timer and its bounded disk-meter worker. The timer
//! ticks on the configured cadence and does work no request path does:
//!
//! - sweeps the lexical writer cache for writers nothing has touched for
//!   the policy's idle interval, so a producer that stops mid-generation
//!   does not pin its heap until another batch happens to arrive;
//! - refreshes the per-track generation disk-usage gauges from the
//!   adapters' own byte walkers, so a scrape reads a number no scrape had
//!   to walk a tree for;
//! - steps the integrity scrub (QI-BB-017) when its own interval is due, so
//!   the bytes of sealed generations are re-proven off the serving path
//!   without a second maintenance thread.
//!
//! Ordinary sweep and walk errors are counted and retried. Worker panic or
//! lost ownership persists a fatal readiness failure and ends the timer.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, Sender, TrySendError};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use quanta_index_contract::SearchCorpusActivationTokenV1;
use quanta_index_core::{
    CancelHandleV1, CoreError, MetricPointV1, MetricSourcePort, ProcessMemoryProbePort,
    RequestBudgetV1, SealedGenerationIdentityProbePort, TrackDiskUsagePort, WriterIdleSweepPort,
};
use quanta_index_search_plane::readiness::ActivationCatalog;
use quanta_index_search_plane::{SearchCorpusGenerationV1, SnapshotInventoryAdmission};

use crate::app::integrity_scrub::PacedIntegrityScrubV1;
use crate::app::supervisor::ChildExitKind;

const DISK_METER_SCAN_BUDGET: Duration = Duration::from_secs(30);

struct DiskMeterStop {
    stopping: AtomicBool,
    active: Mutex<Option<CancelHandleV1>>,
    tallies: Arc<MaintenanceTallies>,
}

impl DiskMeterStop {
    fn new(tallies: Arc<MaintenanceTallies>) -> Self {
        Self {
            stopping: AtomicBool::new(false),
            active: Mutex::new(None),
            tallies,
        }
    }

    fn fail(&self, reason: &'static str) -> CoreError {
        self.tallies.disk_meter_fatal.store(true, Ordering::Release);
        CoreError::Storage(reason.to_string())
    }

    fn stop(&self) {
        self.stopping.store(true, Ordering::Release);
        let cancel = match self.active.lock() {
            Ok(active) => active.clone(),
            Err(poisoned) => {
                // Keep custody of the cancel handle even after a poisoned
                // lock, but publish a persistent typed readiness failure.
                let _failure = self.fail("maintenance disk meter stop lock poisoned");
                poisoned.into_inner().clone()
            }
        };
        if let Some(cancel) = cancel {
            cancel.cancel();
        }
    }

    fn begin(&self, budget: &RequestBudgetV1) -> Result<bool, CoreError> {
        let mut active = self
            .active
            .lock()
            .map_err(|_poisoned| self.fail("maintenance disk meter begin lock poisoned"))?;
        if self.stopping.load(Ordering::Acquire) {
            return Ok(false);
        }
        if active.is_some() {
            return Err(self.fail("maintenance disk meter active budget already owned"));
        }
        *active = Some(budget.cancel_handle());
        drop(active);
        Ok(true)
    }

    fn finish(&self) -> Result<(), CoreError> {
        let missing = self
            .active
            .lock()
            .map_err(|_poisoned| self.fail("maintenance disk meter finish lock poisoned"))?
            .take()
            .is_none();
        if missing {
            return Err(self.fail("maintenance disk meter active budget owner lost"));
        }
        Ok(())
    }
}

/// Take the only disk-meter join handle. A poisoned owner lock is never
/// treated as success, but still yields its handle so shutdown can join it.
fn take_disk_meter(
    owner: &Mutex<Option<JoinHandle<()>>>,
    tallies: &MaintenanceTallies,
) -> Option<JoinHandle<()>> {
    let meter = match owner.lock() {
        Ok(mut guard) => guard.take(),
        Err(poisoned) => {
            tallies.disk_meter_fatal.store(true, Ordering::Release);
            let mut guard = poisoned.into_inner();
            guard.take()
        }
    };
    if meter.is_none() {
        tallies.disk_meter_fatal.store(true, Ordering::Release);
    }
    meter
}

/// Own the meter sender and join handle while the timer runs. On unwind,
/// close the sender, cancel the scan, and join before the timer can finish.
struct DiskMeterJoinGuard {
    sender: Option<mpsc::SyncSender<()>>,
    owner: Arc<Mutex<Option<JoinHandle<()>>>>,
    stop: Arc<DiskMeterStop>,
    tallies: Arc<MaintenanceTallies>,
    joined: bool,
}

impl DiskMeterJoinGuard {
    fn join(&mut self) -> ChildExitKind {
        self.stop.stop();
        drop(self.sender.take());
        let meter = take_disk_meter(&self.owner, &self.tallies);
        self.joined = true;
        match meter {
            Some(meter) => {
                if meter.join().is_err() {
                    self.tallies.disk_meter_fatal.store(true, Ordering::Release);
                    ChildExitKind::Panicked
                } else if self.tallies.disk_meter_fatal.load(Ordering::Acquire) {
                    ChildExitKind::Failed
                } else {
                    ChildExitKind::Completed
                }
            }
            None => ChildExitKind::Failed,
        }
    }
}

impl Drop for DiskMeterJoinGuard {
    fn drop(&mut self) {
        if !self.joined {
            if std::thread::panicking() {
                self.tallies.disk_meter_fatal.store(true, Ordering::Release);
            }
            let _kind = self.join();
        }
    }
}

/// What the timer has done, read by the scrape.
#[derive(Debug, Default)]
pub struct MaintenanceTallies {
    last_completed_tick: Mutex<Option<Instant>>,
    backend_observation: Mutex<Option<BackendObservation>>,
    backend_probe_failures: AtomicU64,
    inventory_admission_failures: AtomicU64,
    ticks: AtomicU64,
    idle_writer_releases: AtomicU64,
    sweep_failures: AtomicU64,
    disk_refreshes: AtomicU64,
    disk_refresh_failures: AtomicU64,
    disk_refresh_skipped: AtomicU64,
    disk_meter_fatal: AtomicBool,
    lexical_generation_disk_bytes: AtomicU64,
    semantic_generation_disk_bytes: AtomicU64,
    lexical_disk_measured_at: Mutex<Option<Instant>>,
    semantic_disk_measured_at: Mutex<Option<Instant>>,
    /// Ticks that could not step the scrub because an earlier step
    /// panicked with the scheduler locked; the scrub stays stopped and the
    /// count says so instead of resuming from a torn state.
    scrub_poisoned: AtomicU64,
}

#[derive(Debug)]
struct BackendObservation {
    identity: Vec<(SearchCorpusGenerationV1, SearchCorpusActivationTokenV1)>,
    completed_at: Instant,
}

impl MaintenanceTallies {
    /// A stalled or dead timer is unhealthy even when earlier ticks succeeded.
    pub fn heartbeat_fresh(&self, cadence: Duration) -> Result<bool, CoreError> {
        if self.disk_meter_fatal.load(Ordering::Acquire) {
            return Err(CoreError::Storage(
                "maintenance disk meter worker failed".to_string(),
            ));
        }
        let Some(limit) = cadence.checked_mul(3) else {
            return Ok(false);
        };
        let last = *self.last_completed_tick.lock().map_err(|error| {
            CoreError::Storage(format!("maintenance heartbeat lock poisoned: {error}"))
        })?;
        Ok(last.is_some_and(|last| last.elapsed() <= limit))
    }

    /// Only an exact, recent observation of the current active identities
    /// proves that the required track backends remain present.
    pub fn required_backend_fresh(
        &self,
        identity: &[(SearchCorpusGenerationV1, SearchCorpusActivationTokenV1)],
        cadence: Duration,
    ) -> Result<bool, CoreError> {
        let Some(limit) = cadence.checked_mul(3) else {
            return Ok(false);
        };
        let observation = self.backend_observation.lock().map_err(|error| {
            CoreError::Storage(format!("backend observation lock poisoned: {error}"))
        })?;
        Ok(observation.as_ref().is_some_and(|observed| {
            observed.identity == identity && observed.completed_at.elapsed() <= limit
        }))
    }

    /// A successful physical door proof for a newly active identity can
    /// seed the same observation immediately; the timer takes ownership of
    /// subsequent liveness checks on its next tick.
    pub fn record_backend_proof(
        &self,
        identity: &[(SearchCorpusGenerationV1, SearchCorpusActivationTokenV1)],
    ) -> Result<(), CoreError> {
        {
            let mut observed = self.backend_observation.lock().map_err(|error| {
                CoreError::Storage(format!("backend observation lock poisoned: {error}"))
            })?;
            *observed = Some(BackendObservation {
                identity: identity.to_vec(),
                completed_at: Instant::now(),
            });
        }
        Ok(())
    }

    /// Ticks the timer has run.
    #[must_use]
    pub fn ticks(&self) -> u64 {
        self.ticks.load(Ordering::Acquire)
    }

    /// Writers the timer's sweeps released.
    #[must_use]
    pub fn idle_writer_releases(&self) -> u64 {
        self.idle_writer_releases.load(Ordering::Acquire)
    }

    /// The last measured bytes of the lexical track's generations.
    #[must_use]
    pub fn lexical_generation_disk_bytes(&self) -> u64 {
        self.lexical_generation_disk_bytes.load(Ordering::Acquire)
    }

    /// The last measured bytes of the semantic track's generations.
    #[must_use]
    pub fn semantic_generation_disk_bytes(&self) -> u64 {
        self.semantic_generation_disk_bytes.load(Ordering::Acquire)
    }

    fn disk_measurement_ages(&self) -> Result<(u64, u64), CoreError> {
        let age = |measured_at: &Mutex<Option<Instant>>| -> Result<u64, CoreError> {
            let last = *measured_at.lock().map_err(|error| {
                CoreError::Storage(format!("disk measurement age lock poisoned: {error}"))
            })?;
            Ok(last.map_or(u64::MAX, |instant| instant.elapsed().as_secs()))
        };
        Ok((
            age(&self.lexical_disk_measured_at)?,
            age(&self.semantic_disk_measured_at)?,
        ))
    }
}

/// The ports one tick works through.
pub struct MaintenanceParts {
    pub writer_sweep: Arc<dyn WriterIdleSweepPort>,
    pub lexical_disk_usage: Arc<dyn TrackDiskUsagePort>,
    pub semantic_disk_usage: Arc<dyn TrackDiskUsagePort>,
    /// Active-generation identity liveness, distinct from the scrub's deep
    /// content proof. Missing means unproven, never healthy.
    pub backend_probe: Option<BackendProbeParts>,
    /// Evicts cached handles whose physical identity no longer agrees with
    /// durable authority. Checked before any scheduled deep scrub step.
    pub inventory_admission: Option<Arc<SnapshotInventoryAdmission>>,
    /// The integrity scrub, stepped on its own interval; `None` when no
    /// adapter scrubs.
    pub integrity_scrub: Option<Mutex<PacedIntegrityScrubV1>>,
}

pub struct BackendProbeParts {
    pub catalog: Arc<ActivationCatalog>,
    pub lexical: Arc<dyn SealedGenerationIdentityProbePort>,
    pub semantic: Arc<dyn SealedGenerationIdentityProbePort>,
}

fn observe_backend(parts: &MaintenanceParts, tallies: &MaintenanceTallies) {
    let observation = parts.backend_probe.as_ref().map(|probe| {
        let (identity, _) = probe.catalog.active_inventory_v1()?;
        for (generation, _token) in &identity {
            probe
                .lexical
                .probe_sealed_generation_identity(generation.lexical())?;
            probe
                .semantic
                .probe_sealed_generation_identity(generation.semantic())?;
        }
        if probe.catalog.active_inventory_v1()?.0 != identity {
            return Err(CoreError::Storage(
                "active identity changed during backend observation".to_owned(),
            ));
        }
        Ok(BackendObservation {
            identity,
            completed_at: Instant::now(),
        })
    });
    let observation = match observation {
        Some(Ok(observed)) => Some(observed),
        Some(Err(_failure)) => {
            let _prior = tallies
                .backend_probe_failures
                .fetch_add(1, Ordering::AcqRel);
            None
        }
        None => None,
    };
    if let Ok(mut observed) = tallies.backend_observation.lock() {
        *observed = observation;
    }
}

/// One tick's work, shared by the timer thread and the boot-time first
/// measurement.
fn tick(parts: &MaintenanceParts, tallies: &MaintenanceTallies) {
    let _tick = tallies.ticks.fetch_add(1, Ordering::AcqRel);
    reconcile_inventory(parts, tallies);
    observe_backend(parts, tallies);
    match parts.writer_sweep.sweep_idle_writers() {
        Ok(released) => {
            let _prior = tallies
                .idle_writer_releases
                .fetch_add(released, Ordering::AcqRel);
        }
        Err(_failed) => {
            let _prior = tallies.sweep_failures.fetch_add(1, Ordering::AcqRel);
        }
    }
    if let Some(scrub) = &parts.integrity_scrub {
        match scrub.lock() {
            // What the step did is the scheduler's own tallies.
            Ok(mut paced) => {
                let _stepped = paced.step_if_due(Instant::now());
            }
            Err(_poisoned) => {
                let _prior = tallies.scrub_poisoned.fetch_add(1, Ordering::AcqRel);
            }
        }
    }
    if let Ok(mut last) = tallies.last_completed_tick.lock() {
        *last = Some(Instant::now());
    }
}

fn reconcile_inventory(parts: &MaintenanceParts, tallies: &MaintenanceTallies) {
    if let Some(admission) = &parts.inventory_admission
        && admission.reconcile().is_err()
    {
        let _prior = tallies
            .inventory_admission_failures
            .fetch_add(1, Ordering::AcqRel);
    }
}

/// Measure both tracks; a track whose walk fails keeps its last value and
/// counts the failure.
fn refresh_disk_usage(
    lexical: &Arc<dyn TrackDiskUsagePort>,
    semantic: &Arc<dyn TrackDiskUsagePort>,
    tallies: &MaintenanceTallies,
    budget: &RequestBudgetV1,
) {
    let _prior = tallies.disk_refreshes.fetch_add(1, Ordering::AcqRel);
    for (port, gauge, measured_at) in [
        (
            lexical,
            &tallies.lexical_generation_disk_bytes,
            &tallies.lexical_disk_measured_at,
        ),
        (
            semantic,
            &tallies.semantic_generation_disk_bytes,
            &tallies.semantic_disk_measured_at,
        ),
    ] {
        match port.track_disk_bytes(budget) {
            Ok(bytes) => {
                gauge.store(bytes, Ordering::Release);
                if let Ok(mut last) = measured_at.lock() {
                    *last = Some(Instant::now());
                }
            }
            Err(_failed) => {
                let _prior = tallies.disk_refresh_failures.fetch_add(1, Ordering::AcqRel);
            }
        }
    }
}

/// The running timer; dropping it stops the thread and joins it.
pub struct MaintenanceTimer {
    stop: Sender<()>,
    meter_stop: Arc<DiskMeterStop>,
    thread: Option<JoinHandle<()>>,
    terminal: mpsc::Receiver<ChildExitKind>,
    tallies: Arc<MaintenanceTallies>,
}

impl MaintenanceTimer {
    /// Measure once now, then keep ticking every `cadence` until dropped.
    ///
    /// The first measurement runs on the caller's thread so the disk
    /// gauges are correct at the first scrape, not after the first tick.
    pub fn start(parts: MaintenanceParts, cadence: Duration) -> Result<Self, CoreError> {
        let tallies = Arc::new(MaintenanceTallies::default());
        reconcile_inventory(&parts, &tallies);
        observe_backend(&parts, &tallies);
        refresh_disk_usage(
            &parts.lexical_disk_usage,
            &parts.semantic_disk_usage,
            &tallies,
            &RequestBudgetV1::for_duration(DISK_METER_SCAN_BUDGET),
        );
        if let Ok(mut last) = tallies.last_completed_tick.lock() {
            *last = Some(Instant::now());
        }
        // Disk walks can exceed several health cadences on a large tree.
        // One owned worker and a single pending request bound both execution
        // and queued work without delaying identity probes or heartbeat.
        let (meter_tx, meter_rx) = mpsc::sync_channel::<()>(1);
        let meter_stop = Arc::new(DiskMeterStop::new(Arc::clone(&tallies)));
        let meter = {
            let lexical = Arc::clone(&parts.lexical_disk_usage);
            let semantic = Arc::clone(&parts.semantic_disk_usage);
            let tallies = Arc::clone(&tallies);
            let meter_stop = Arc::clone(&meter_stop);
            std::thread::Builder::new()
                .name("searchd-disk-meter".to_string())
                .spawn(move || {
                    while meter_rx.recv().is_ok() {
                        let budget = RequestBudgetV1::for_duration(DISK_METER_SCAN_BUDGET);
                        match meter_stop.begin(&budget) {
                            Ok(true) => {}
                            Ok(false) | Err(_) => break,
                        }
                        refresh_disk_usage(&lexical, &semantic, &tallies, &budget);
                        if meter_stop.finish().is_err() {
                            break;
                        }
                    }
                })
                .map_err(|error| {
                    CoreError::Storage(format!("maintenance disk meter: spawn thread: {error}"))
                })?
        };
        let (stop, stop_rx) = mpsc::channel();
        let meter_owner = Arc::new(Mutex::new(Some(meter)));
        let (terminal_tx, terminal) = mpsc::channel();
        let thread = {
            let timer_tallies = Arc::clone(&tallies);
            let meter_for_timer = Arc::clone(&meter_owner);
            let meter_stop_for_timer = Arc::clone(&meter_stop);

            let spawned = std::thread::Builder::new()
                .name("searchd-maintenance".to_string())
                .spawn(move || {
                    let mut meter_guard = DiskMeterJoinGuard {
                        sender: Some(meter_tx),
                        owner: meter_for_timer,
                        stop: meter_stop_for_timer,
                        tallies: Arc::clone(&timer_tallies),
                        joined: false,
                    };
                    loop {
                        match stop_rx.recv_timeout(cadence) {
                            Ok(()) | Err(RecvTimeoutError::Disconnected) => break,
                            Err(RecvTimeoutError::Timeout) => {
                                tick(&parts, &timer_tallies);
                                let Some(sender) = meter_guard.sender.as_ref() else {
                                    timer_tallies
                                        .disk_meter_fatal
                                        .store(true, Ordering::Release);
                                    break;
                                };
                                match sender.try_send(()) {
                                    Ok(()) => {}
                                    Err(TrySendError::Full(())) => {
                                        let _prior = timer_tallies
                                            .disk_refresh_skipped
                                            .fetch_add(1, Ordering::AcqRel);
                                    }
                                    Err(TrySendError::Disconnected(())) => {
                                        if !meter_guard.stop.stopping.load(Ordering::Acquire) {
                                            timer_tallies
                                                .disk_meter_fatal
                                                .store(true, Ordering::Release);
                                        }
                                        break;
                                    }
                                }
                            }
                        }
                    }
                    let kind = meter_guard.join();
                    let _reported = terminal_tx.send(kind);
                });
            match spawned {
                Ok(thread) => thread,
                Err(error) => {
                    meter_stop.stop();
                    // The failed spawn drops its channel sender. Retain and
                    // join the meter before returning the startup failure.
                    let meter_failed = take_disk_meter(&meter_owner, &tallies)
                        .is_none_or(|meter| meter.join().is_err());
                    if meter_failed || tallies.disk_meter_fatal.load(Ordering::Acquire) {
                        return Err(CoreError::Storage(format!(
                            "maintenance timer: spawn thread: {error}; disk meter ownership or join failed"
                        )));
                    }
                    return Err(CoreError::Storage(format!(
                        "maintenance timer: spawn thread: {error}"
                    )));
                }
            }
        };
        Ok(Self {
            stop,
            meter_stop,
            thread: Some(thread),
            terminal,
            tallies,
        })
    }

    /// The timer's tallies, for the scrape and for tests.
    #[must_use]
    pub fn tallies(&self) -> Arc<MaintenanceTallies> {
        Arc::clone(&self.tallies)
    }

    /// Hand the timer to a supervisor (SEP-21 P08): the stop signal is
    /// sent now-or-by-the-closure and the join plus terminal authority become
    /// the supervisor's to own, so the timer is never a detached thread or joined
    /// after the runtime guards drop.
    ///
    /// The stop closure is idempotent with [`Drop`]: whichever runs
    /// first signals; the join handle is out of this value so a later
    /// drop of the timer stops nothing and joins nothing. A second
    /// hand-off is a typed error, not a silent stub.
    pub fn into_supervised_parts(
        mut self,
    ) -> Result<
        (
            MaintenanceStop,
            JoinHandle<()>,
            mpsc::Receiver<ChildExitKind>,
        ),
        CoreError,
    > {
        let (dummy, _dummy_rx) = mpsc::channel();
        let stop = std::mem::replace(&mut self.stop, dummy);
        let Some(handle) = self.thread.take() else {
            return Err(CoreError::Storage(
                "maintenance timer: supervised hand-off attempted twice".to_string(),
            ));
        };
        Ok((
            MaintenanceStop {
                stop: Arc::new(stop),
                meter_stop: Arc::clone(&self.meter_stop),
            },
            handle,
            std::mem::replace(&mut self.terminal, mpsc::channel().1),
        ))
    }
}

/// The supervisor-owned stop for one maintenance timer: sends the stop
/// signal when called or when dropped, whichever comes first.
pub struct MaintenanceStop {
    stop: Arc<Sender<()>>,
    meter_stop: Arc<DiskMeterStop>,
}

impl MaintenanceStop {
    /// Signal the timer to stop; idempotent.
    pub fn stop(&self) {
        self.meter_stop.stop();
        let _sent = self.stop.send(());
    }
}

impl Drop for MaintenanceStop {
    fn drop(&mut self) {
        self.stop();
    }
}

impl Drop for MaintenanceTimer {
    fn drop(&mut self) {
        if let Some(thread) = self.thread.take() {
            self.meter_stop.stop();
            let _stop_result = self.stop.send(());
            if thread.join().is_err() {
                self.tallies.disk_meter_fatal.store(true, Ordering::Release);
            }
        }
    }
}

/// The maintenance tallies, the per-track disk gauges and the process
/// resident-memory gauge as scrape points.
pub struct MaintenanceMetricSource {
    tallies: Arc<MaintenanceTallies>,
    memory_probe: Arc<dyn ProcessMemoryProbePort>,
}

impl MaintenanceMetricSource {
    #[must_use]
    pub fn new(
        tallies: Arc<MaintenanceTallies>,
        memory_probe: Arc<dyn ProcessMemoryProbePort>,
    ) -> Self {
        Self {
            tallies,
            memory_probe,
        }
    }
}

impl MetricSourcePort for MaintenanceMetricSource {
    fn scrape(&self) -> Result<Vec<MetricPointV1>, CoreError> {
        let tallies = &self.tallies;
        let (lexical_disk_age, semantic_disk_age) = tallies.disk_measurement_ages()?;
        Ok(vec![
            MetricPointV1::counter("maintenance_ticks_total", tallies.ticks()),
            MetricPointV1::counter(
                "maintenance_idle_writer_releases_total",
                tallies.idle_writer_releases(),
            ),
            MetricPointV1::counter(
                "maintenance_sweep_failures_total",
                tallies.sweep_failures.load(Ordering::Acquire),
            ),
            MetricPointV1::counter(
                "maintenance_scrub_poisoned_total",
                tallies.scrub_poisoned.load(Ordering::Acquire),
            ),
            MetricPointV1::counter(
                "maintenance_backend_probe_failures_total",
                tallies.backend_probe_failures.load(Ordering::Acquire),
            ),
            MetricPointV1::counter(
                "maintenance_inventory_admission_failures_total",
                tallies.inventory_admission_failures.load(Ordering::Acquire),
            ),
            MetricPointV1::counter(
                "maintenance_disk_refreshes_total",
                tallies.disk_refreshes.load(Ordering::Acquire),
            ),
            MetricPointV1::counter(
                "maintenance_disk_refresh_failures_total",
                tallies.disk_refresh_failures.load(Ordering::Acquire),
            ),
            MetricPointV1::counter(
                "maintenance_disk_refresh_skipped_total",
                tallies.disk_refresh_skipped.load(Ordering::Acquire),
            ),
            MetricPointV1::gauge_count(
                "search_corpus_lexical_generation_disk_bytes",
                tallies.lexical_generation_disk_bytes(),
            ),
            MetricPointV1::gauge_count(
                "search_corpus_semantic_generation_disk_bytes",
                tallies.semantic_generation_disk_bytes(),
            ),
            MetricPointV1::gauge_count(
                "search_corpus_lexical_generation_disk_age_seconds",
                lexical_disk_age,
            ),
            MetricPointV1::gauge_count(
                "search_corpus_semantic_generation_disk_age_seconds",
                semantic_disk_age,
            ),
            MetricPointV1::gauge_count(
                "process_resident_bytes",
                self.memory_probe.resident_bytes()?,
            ),
        ])
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ChildExitKind, DiskMeterStop, MaintenanceParts, MaintenanceTallies, MaintenanceTimer,
        take_disk_meter,
    };
    use quanta_index_core::{
        CoreError, RequestBudgetV1, TrackDiskUsagePort, WriterIdleSweepPort,
        unique_inode_tree_bytes_in_track,
    };
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::sync::{Arc, Condvar, Mutex, mpsc};
    use std::time::{Duration, Instant};

    #[test]
    fn maintenance_heartbeat_rejects_stale_or_unknown_tick() {
        let tallies = MaintenanceTallies::default();
        let cadence = Duration::from_secs(1);
        assert!(!tallies.heartbeat_fresh(cadence).expect("heartbeat check"));
        *tallies.last_completed_tick.lock().expect("fixture lock") = Some(
            Instant::now()
                .checked_sub(Duration::from_secs(4))
                .expect("representable instant"),
        );
        assert!(!tallies.heartbeat_fresh(cadence).expect("heartbeat check"));
        *tallies.last_completed_tick.lock().expect("fixture lock") = Some(Instant::now());
        assert!(tallies.heartbeat_fresh(cadence).expect("heartbeat check"));
    }

    #[test]
    fn maintenance_heartbeat_reports_poisoned_lock() {
        let tallies = MaintenanceTallies::default();
        let _caught = std::panic::catch_unwind(|| {
            let _guard = tallies.last_completed_tick.lock().expect("fixture lock");
            panic!("poison the heartbeat lock");
        });
        let error = tallies
            .heartbeat_fresh(Duration::from_secs(1))
            .expect_err("a poisoned heartbeat lock must not become a stale value");
        assert!(
            matches!(error, CoreError::Storage(message) if message.contains("heartbeat lock poisoned"))
        );
    }

    #[test]
    fn poisoned_or_missing_disk_meter_owner_is_a_typed_readiness_failure() {
        let tallies = Arc::new(MaintenanceTallies::default());
        let meter_stop = DiskMeterStop::new(Arc::clone(&tallies));
        let _caught = std::panic::catch_unwind(|| {
            let _guard = meter_stop.active.lock().expect("fixture owner lock");
            panic!("poison the active meter owner");
        });
        let budget = RequestBudgetV1::for_duration(Duration::from_secs(1));
        let error = meter_stop
            .begin(&budget)
            .expect_err("poisoned meter owner must refuse a new scan");
        assert!(
            matches!(error, CoreError::Storage(message) if message.contains("begin lock poisoned"))
        );
        assert!(matches!(
            tallies.heartbeat_fresh(Duration::from_secs(1)),
            Err(CoreError::Storage(message)) if message.contains("disk meter worker failed")
        ));
        meter_stop.stop();

        let missing = MaintenanceTallies::default();
        let owner = Mutex::<Option<std::thread::JoinHandle<()>>>::new(None);
        assert!(take_disk_meter(&owner, &missing).is_none());
        assert!(matches!(
            missing.heartbeat_fresh(Duration::from_secs(1)),
            Err(CoreError::Storage(message)) if message.contains("disk meter worker failed")
        ));
    }

    fn controlled_disk_meter_failure() -> ! {
        panic!("controlled disk meter worker failure");
    }

    #[test]
    fn unexpected_disk_meter_panic_ends_required_timer_and_fails_readiness() {
        struct PanicOnSecondScan {
            calls: AtomicU64,
            entered: mpsc::Sender<()>,
        }

        impl TrackDiskUsagePort for PanicOnSecondScan {
            fn track_disk_bytes(&self, budget: &RequestBudgetV1) -> Result<u64, CoreError> {
                budget.checkpoint("panic-on-second-scan")?;
                if self.calls.fetch_add(1, Ordering::AcqRel) > 0 {
                    let _sent = self.entered.send(());
                    controlled_disk_meter_failure();
                }
                Ok(1)
            }
        }

        let (entered_tx, entered_rx) = mpsc::channel();
        let timer = MaintenanceTimer::start(
            MaintenanceParts {
                writer_sweep: Arc::new(CountingSweep(AtomicU64::new(0))),
                lexical_disk_usage: Arc::new(PanicOnSecondScan {
                    calls: AtomicU64::new(0),
                    entered: entered_tx,
                }),
                semantic_disk_usage: Arc::new(ScriptedDisk(AtomicU64::new(0))),
                backend_probe: None,
                inventory_admission: None,
                integrity_scrub: None,
            },
            Duration::from_millis(10),
        )
        .expect("boot scan succeeds");
        let tallies = timer.tallies();
        let (stop, thread, terminal) = timer.into_supervised_parts().expect("supervised transfer");
        entered_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("worker entered failing scan");
        let deadline = Instant::now()
            .checked_add(Duration::from_secs(2))
            .expect("test deadline");
        while !thread.is_finished() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(1));
        }
        let exited_without_stop = thread.is_finished();
        stop.stop();
        thread.join().expect("timer joins the failed worker");
        assert_eq!(
            terminal.try_recv().expect("timer reports terminal kind"),
            ChildExitKind::Panicked
        );
        assert!(
            exited_without_stop,
            "required timer did not exit after meter loss"
        );
        assert!(matches!(
            tallies.heartbeat_fresh(Duration::from_secs(1)),
            Err(CoreError::Storage(message)) if message.contains("disk meter worker failed")
        ));
    }

    fn controlled_timer_callback_failure() -> ! {
        panic!("controlled timer callback failure");
    }

    #[test]
    fn timer_callback_panic_cancels_and_joins_the_owned_walker_before_timer_exits() {
        struct PanicAfterWalkEntered {
            calls: AtomicU64,
            entered: Arc<AtomicBool>,
        }

        impl WriterIdleSweepPort for PanicAfterWalkEntered {
            fn sweep_idle_writers(&self) -> Result<u64, CoreError> {
                if self.calls.fetch_add(1, Ordering::AcqRel) > 0 {
                    let waiting = Instant::now();
                    while !self.entered.load(Ordering::Acquire)
                        && waiting.elapsed() < Duration::from_secs(2)
                    {
                        std::thread::sleep(Duration::from_millis(1));
                    }
                    controlled_timer_callback_failure();
                }
                Ok(0)
            }
        }

        struct ObservedWalker {
            root: PathBuf,
            calls: AtomicU64,
            entered: Arc<AtomicBool>,
            entered_tx: mpsc::Sender<()>,
            exited_tx: mpsc::Sender<bool>,
        }

        impl TrackDiskUsagePort for ObservedWalker {
            fn track_disk_bytes(&self, budget: &RequestBudgetV1) -> Result<u64, CoreError> {
                let pause = self.calls.fetch_add(1, Ordering::AcqRel) > 0;
                let announced = AtomicBool::new(false);
                unique_inode_tree_bytes_in_track(
                    &self.root,
                    &|_| {
                        if pause && !announced.swap(true, Ordering::AcqRel) {
                            self.entered.store(true, Ordering::Release);
                            let _sent = self.entered_tx.send(());
                            let waiting = Instant::now();
                            while !budget.is_cancelled()
                                && waiting.elapsed() < Duration::from_secs(2)
                            {
                                std::thread::sleep(Duration::from_millis(1));
                            }
                            let _exited = self.exited_tx.send(budget.is_cancelled());
                        }
                        false
                    },
                    budget,
                )
            }
        }

        let root = tempfile::tempdir().expect("track fixture");
        std::fs::write(root.path().join("file"), b"payload").expect("track bytes");
        let entered = Arc::new(AtomicBool::new(false));
        let (entered_tx, entered_rx) = mpsc::channel();
        let (exited_tx, exited_rx) = mpsc::channel();
        let timer = MaintenanceTimer::start(
            MaintenanceParts {
                writer_sweep: Arc::new(PanicAfterWalkEntered {
                    calls: AtomicU64::new(0),
                    entered: Arc::clone(&entered),
                }),
                lexical_disk_usage: Arc::new(ObservedWalker {
                    root: root.path().to_path_buf(),
                    calls: AtomicU64::new(0),
                    entered,
                    entered_tx,
                    exited_tx,
                }),
                semantic_disk_usage: Arc::new(ScriptedDisk(AtomicU64::new(0))),
                backend_probe: None,
                inventory_admission: None,
                integrity_scrub: None,
            },
            Duration::from_millis(10),
        )
        .expect("boot scan succeeds");
        let tallies = timer.tallies();
        let (stop, timer_thread, terminal) = timer
            .into_supervised_parts()
            .expect("supervisor takes timer ownership");
        entered_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("real walker entered before callback panic");
        let joined = timer_thread.join();
        let walker_cancelled = exited_rx
            .try_recv()
            .expect("walker exited before timer join returned");
        stop.stop();
        assert!(joined.is_err(), "callback panic must reach the timer owner");
        assert!(
            walker_cancelled,
            "walker exit must follow budget cancellation"
        );
        assert!(
            terminal.try_recv().is_err(),
            "panicked timer cannot publish completion"
        );
        assert!(matches!(
            tallies.heartbeat_fresh(Duration::from_secs(1)),
            Err(CoreError::Storage(message)) if message.contains("disk meter worker failed")
        ));
    }

    #[test]
    fn backend_observation_is_not_healthy_when_unknown_or_stale() {
        let tallies = MaintenanceTallies::default();
        let cadence = Duration::from_secs(1);
        assert!(
            !tallies
                .required_backend_fresh(&[], cadence)
                .expect("unknown state")
        );
        tallies.record_backend_proof(&[]).expect("fresh proof");
        assert!(
            tallies
                .required_backend_fresh(&[], cadence)
                .expect("fresh state")
        );
        tallies
            .backend_observation
            .lock()
            .expect("fixture observation lock")
            .as_mut()
            .expect("recorded observation")
            .completed_at = Instant::now()
            .checked_sub(Duration::from_secs(4))
            .expect("representable instant");
        assert!(
            !tallies
                .required_backend_fresh(&[], cadence)
                .expect("stale state")
        );
    }

    struct CountingSweep(AtomicU64);

    impl WriterIdleSweepPort for CountingSweep {
        fn sweep_idle_writers(&self) -> Result<u64, CoreError> {
            let _prior = self.0.fetch_add(1, Ordering::AcqRel);
            Ok(2)
        }
    }

    struct ScriptedDisk(AtomicU64);

    impl TrackDiskUsagePort for ScriptedDisk {
        fn track_disk_bytes(&self, budget: &RequestBudgetV1) -> Result<u64, CoreError> {
            budget.checkpoint("scripted-disk:measure")?;
            Ok(self.0.load(Ordering::Acquire))
        }
    }

    struct PausedWalker {
        root: PathBuf,
        calls: AtomicU64,
        entered: mpsc::Sender<()>,
    }

    impl TrackDiskUsagePort for PausedWalker {
        fn track_disk_bytes(&self, budget: &RequestBudgetV1) -> Result<u64, CoreError> {
            let pause = self.calls.fetch_add(1, Ordering::AcqRel) > 0;
            let announced = AtomicBool::new(false);
            unique_inode_tree_bytes_in_track(
                &self.root,
                &|_| {
                    if pause && !announced.swap(true, Ordering::AcqRel) {
                        let _sent = self.entered.send(());
                        while !budget.is_cancelled() {
                            std::thread::sleep(Duration::from_millis(1));
                        }
                    }
                    false
                },
                budget,
            )
        }
    }

    #[test]
    fn shutdown_cancels_an_owned_inflight_directory_walk_before_join() {
        let root = tempfile::tempdir().expect("track fixture");
        std::fs::write(root.path().join("file"), b"payload").expect("track bytes");
        let (entered_tx, entered_rx) = mpsc::channel();
        let timer = MaintenanceTimer::start(
            MaintenanceParts {
                writer_sweep: Arc::new(CountingSweep(AtomicU64::new(0))),
                lexical_disk_usage: Arc::new(PausedWalker {
                    root: root.path().to_path_buf(),
                    calls: AtomicU64::new(0),
                    entered: entered_tx,
                }),
                semantic_disk_usage: Arc::new(ScriptedDisk(AtomicU64::new(0))),
                backend_probe: None,
                inventory_admission: None,
                integrity_scrub: None,
            },
            Duration::from_millis(10),
        )
        .expect("boot measurement completes");
        entered_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("owned meter entered the real walker");
        let stopped = Instant::now();
        drop(timer);
        assert!(
            stopped.elapsed() < Duration::from_secs(2),
            "cooperative directory walk outlived shutdown"
        );
    }

    #[test]
    fn supervised_handoff_keeps_meter_live_until_its_owner_stops_and_joins() {
        let root = tempfile::tempdir().expect("track fixture");
        std::fs::write(root.path().join("file"), b"payload").expect("track bytes");
        let (entered_tx, entered_rx) = mpsc::channel();
        let timer = MaintenanceTimer::start(
            MaintenanceParts {
                writer_sweep: Arc::new(CountingSweep(AtomicU64::new(0))),
                lexical_disk_usage: Arc::new(PausedWalker {
                    root: root.path().to_path_buf(),
                    calls: AtomicU64::new(0),
                    entered: entered_tx,
                }),
                semantic_disk_usage: Arc::new(ScriptedDisk(AtomicU64::new(0))),
                backend_probe: None,
                inventory_admission: None,
                integrity_scrub: None,
            },
            Duration::from_millis(10),
        )
        .expect("boot measurement completes");
        let (stop, thread, terminal) = timer.into_supervised_parts().expect("transfer ownership");
        entered_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("handed-off meter entered the real walker");
        let stopped = Instant::now();
        stop.stop();
        thread
            .join()
            .expect("supervised timer and meter stop cleanly");
        assert_eq!(
            terminal.try_recv().expect("timer reports terminal kind"),
            ChildExitKind::Completed
        );
        assert!(
            stopped.elapsed() < Duration::from_secs(2),
            "supervised directory walk outlived shutdown"
        );
    }

    struct BlockingDisk {
        calls: AtomicU64,
        gate: Arc<(Mutex<(bool, bool)>, Condvar)>,
    }

    impl TrackDiskUsagePort for BlockingDisk {
        fn track_disk_bytes(&self, budget: &RequestBudgetV1) -> Result<u64, CoreError> {
            budget.checkpoint("blocking-disk:measure")?;
            if self.calls.fetch_add(1, Ordering::AcqRel) > 0 {
                let (lock, ready) = &*self.gate;
                let mut state = lock.lock().map_err(|error| {
                    CoreError::Storage(format!("blocking disk fixture poisoned: {error}"))
                })?;
                state.0 = true;
                ready.notify_all();
                while !state.1 {
                    budget.checkpoint("blocking-disk:wait")?;
                    state = ready
                        .wait_timeout(state, Duration::from_millis(10))
                        .map_err(|error| {
                            CoreError::Storage(format!("blocking disk fixture poisoned: {error}"))
                        })?
                        .0;
                }
                drop(state);
            }
            Ok(10)
        }
    }

    #[test]
    fn a_disk_walk_longer_than_three_cadences_does_not_stale_the_health_timer() {
        let gate = Arc::new((Mutex::new((false, false)), Condvar::new()));
        let lexical = Arc::new(BlockingDisk {
            calls: AtomicU64::new(0),
            gate: Arc::clone(&gate),
        });
        let cadence = Duration::from_millis(20);
        let timer = MaintenanceTimer::start(
            MaintenanceParts {
                writer_sweep: Arc::new(CountingSweep(AtomicU64::new(0))),
                lexical_disk_usage: lexical,
                semantic_disk_usage: Arc::new(ScriptedDisk(AtomicU64::new(20))),
                backend_probe: None,
                inventory_admission: None,
                integrity_scrub: None,
            },
            cadence,
        )
        .expect("timer starts after the first disk measurement");
        let tallies = timer.tallies();
        let entered = {
            let (lock, ready) = &*gate;
            let (state, timeout) = ready
                .wait_timeout_while(
                    lock.lock().expect("fixture gate"),
                    Duration::from_secs(2),
                    |state| !state.0,
                )
                .expect("fixture wait");
            let observed = state.0 && !timeout.timed_out();
            drop(state);
            observed
        };
        let deadline = Instant::now() + Duration::from_secs(2);
        while tallies.ticks() < 5 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        let ticks = tallies.ticks();
        let fresh = tallies.heartbeat_fresh(cadence).expect("heartbeat check");
        let skipped = tallies.disk_refresh_skipped.load(Ordering::Acquire);
        {
            let (lock, ready) = &*gate;
            let mut state = lock.lock().expect("fixture gate");
            state.1 = true;
            drop(state);
            ready.notify_all();
        }
        drop(timer);
        assert!(entered, "meter did not reach the controlled walk");
        assert!(ticks >= 5, "health timer stalled on disk walk: {ticks}");
        assert!(fresh, "disk walk made the health heartbeat stale");
        assert!(skipped > 0, "bounded queue did not record skipped walks");
    }

    /// The timer measures once at start, sweeps and re-measures on every
    /// tick, counts what it did, and stops when dropped.
    #[test]
    fn the_timer_measures_at_start_ticks_on_its_cadence_and_stops_on_drop() {
        let sweeps = Arc::new(CountingSweep(AtomicU64::new(0)));
        let lexical = Arc::new(ScriptedDisk(AtomicU64::new(10)));
        let semantic = Arc::new(ScriptedDisk(AtomicU64::new(20)));
        let writer_sweep: Arc<dyn WriterIdleSweepPort> = sweeps.clone();
        let lexical_disk_usage: Arc<dyn TrackDiskUsagePort> = lexical.clone();
        let semantic_disk_usage: Arc<dyn TrackDiskUsagePort> = semantic;
        let timer = MaintenanceTimer::start(
            MaintenanceParts {
                writer_sweep,
                lexical_disk_usage,
                semantic_disk_usage,
                backend_probe: None,
                inventory_admission: None,
                integrity_scrub: None,
            },
            Duration::from_millis(10),
        )
        .expect("timer starts");
        let tallies = timer.tallies();
        // The start-time measurement, before any tick.
        assert_eq!(tallies.lexical_generation_disk_bytes(), 10);
        assert_eq!(tallies.semantic_generation_disk_bytes(), 20);
        lexical.0.store(30, Ordering::Release);
        // Wait for ticks, bounded; the assertions are on counts the ticks
        // produced, not on how long they took.
        let deadline = Instant::now() + Duration::from_secs(10);
        while tallies.ticks() < 3 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        while tallies.lexical_generation_disk_bytes() != 30 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        let ticks = tallies.ticks();
        assert!(ticks >= 3, "the timer ticked: {ticks}");
        assert_eq!(
            sweeps.0.load(Ordering::Acquire),
            ticks,
            "one sweep per tick"
        );
        assert_eq!(
            tallies.idle_writer_releases(),
            ticks * 2,
            "every sweep's releases are counted"
        );
        assert_eq!(
            tallies.lexical_generation_disk_bytes(),
            30,
            "ticks refresh the gauge"
        );
        drop(timer);
        let after_drop = sweeps.0.load(Ordering::Acquire);
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(
            sweeps.0.load(Ordering::Acquire),
            after_drop,
            "a dropped timer sweeps no more"
        );
    }

    #[test]
    fn drop_interrupts_a_long_cadence_without_waiting_for_the_deadline() {
        let timer = MaintenanceTimer::start(
            MaintenanceParts {
                writer_sweep: Arc::new(CountingSweep(AtomicU64::new(0))),
                lexical_disk_usage: Arc::new(ScriptedDisk(AtomicU64::new(10))),
                semantic_disk_usage: Arc::new(ScriptedDisk(AtomicU64::new(20))),
                backend_probe: None,
                inventory_admission: None,
                integrity_scrub: None,
            },
            Duration::from_secs(30),
        )
        .expect("timer starts");

        let started = Instant::now();
        drop(timer);

        assert!(
            started.elapsed() < Duration::from_secs(1),
            "drop waited for the maintenance cadence: {:?}",
            started.elapsed()
        );
    }
}
