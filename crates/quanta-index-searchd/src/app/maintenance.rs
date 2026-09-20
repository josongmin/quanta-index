//! The composition root's maintenance timer (QI-BB-016, QI-BB-015).
//!
//! One bounded thread, owned by the runtime and stopped with it, ticks on
//! the configured cadence and does two things no request path does:
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
//! Everything the timer does is counted, and a failed sweep or walk is a
//! counted failure the next tick retries, never a silent stop.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use quanta_index_core::{
    CoreError, MetricPointV1, MetricSourcePort, ProcessMemoryProbePort, TrackDiskUsagePort,
    WriterIdleSweepPort,
};

use crate::app::integrity_scrub::PacedIntegrityScrubV1;

/// What the timer has done, read by the scrape.
#[derive(Debug, Default)]
pub struct MaintenanceTallies {
    ticks: AtomicU64,
    idle_writer_releases: AtomicU64,
    sweep_failures: AtomicU64,
    disk_refreshes: AtomicU64,
    disk_refresh_failures: AtomicU64,
    lexical_generation_disk_bytes: AtomicU64,
    semantic_generation_disk_bytes: AtomicU64,
    /// Ticks that could not step the scrub because an earlier step
    /// panicked with the scheduler locked; the scrub stays stopped and the
    /// count says so instead of resuming from a torn state.
    scrub_poisoned: AtomicU64,
}

impl MaintenanceTallies {
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
}

/// The ports one tick works through.
pub struct MaintenanceParts {
    pub writer_sweep: Arc<dyn WriterIdleSweepPort>,
    pub lexical_disk_usage: Arc<dyn TrackDiskUsagePort>,
    pub semantic_disk_usage: Arc<dyn TrackDiskUsagePort>,
    /// The integrity scrub, stepped on its own interval; `None` when no
    /// adapter scrubs.
    pub integrity_scrub: Option<Mutex<PacedIntegrityScrubV1>>,
}

/// One tick's work, shared by the timer thread and the boot-time first
/// measurement.
fn tick(parts: &MaintenanceParts, tallies: &MaintenanceTallies) {
    let _tick = tallies.ticks.fetch_add(1, Ordering::AcqRel);
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
    refresh_disk_usage(parts, tallies);
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
}

/// Measure both tracks; a track whose walk fails keeps its last value and
/// counts the failure.
fn refresh_disk_usage(parts: &MaintenanceParts, tallies: &MaintenanceTallies) {
    let _prior = tallies.disk_refreshes.fetch_add(1, Ordering::AcqRel);
    for (port, gauge) in [
        (
            &parts.lexical_disk_usage,
            &tallies.lexical_generation_disk_bytes,
        ),
        (
            &parts.semantic_disk_usage,
            &tallies.semantic_generation_disk_bytes,
        ),
    ] {
        match port.track_disk_bytes() {
            Ok(bytes) => gauge.store(bytes, Ordering::Release),
            Err(_failed) => {
                let _prior = tallies.disk_refresh_failures.fetch_add(1, Ordering::AcqRel);
            }
        }
    }
}

/// The running timer; dropping it stops the thread and joins it.
pub struct MaintenanceTimer {
    stop: Sender<()>,
    thread: Option<JoinHandle<()>>,
    tallies: Arc<MaintenanceTallies>,
}

impl MaintenanceTimer {
    /// Measure once now, then keep ticking every `cadence` until dropped.
    ///
    /// The first measurement runs on the caller's thread so the disk
    /// gauges are correct at the first scrape, not after the first tick.
    pub fn start(parts: MaintenanceParts, cadence: Duration) -> Result<Self, CoreError> {
        let tallies = Arc::new(MaintenanceTallies::default());
        refresh_disk_usage(&parts, &tallies);
        let (stop, stop_rx) = mpsc::channel();
        let thread = {
            let tallies = Arc::clone(&tallies);
            std::thread::Builder::new()
                .name("searchd-maintenance".to_string())
                .spawn(move || {
                    loop {
                        match stop_rx.recv_timeout(cadence) {
                            Ok(()) | Err(RecvTimeoutError::Disconnected) => break,
                            Err(RecvTimeoutError::Timeout) => tick(&parts, &tallies),
                        }
                    }
                })
                .map_err(|error| {
                    CoreError::Storage(format!("maintenance timer: spawn thread: {error}"))
                })?
        };
        Ok(Self {
            stop,
            thread: Some(thread),
            tallies,
        })
    }

    /// The timer's tallies, for the scrape and for tests.
    #[must_use]
    pub fn tallies(&self) -> Arc<MaintenanceTallies> {
        Arc::clone(&self.tallies)
    }
}

impl Drop for MaintenanceTimer {
    fn drop(&mut self) {
        let _stop_result = self.stop.send(());
        if let Some(thread) = self.thread.take() {
            let _joined = thread.join();
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
                "maintenance_disk_refreshes_total",
                tallies.disk_refreshes.load(Ordering::Acquire),
            ),
            MetricPointV1::counter(
                "maintenance_disk_refresh_failures_total",
                tallies.disk_refresh_failures.load(Ordering::Acquire),
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
                "process_resident_bytes",
                self.memory_probe.resident_bytes()?,
            ),
        ])
    }
}

#[cfg(test)]
mod tests {
    use super::{MaintenanceParts, MaintenanceTimer};
    use quanta_index_core::{CoreError, TrackDiskUsagePort, WriterIdleSweepPort};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{Duration, Instant};

    struct CountingSweep(AtomicU64);

    impl WriterIdleSweepPort for CountingSweep {
        fn sweep_idle_writers(&self) -> Result<u64, CoreError> {
            let _prior = self.0.fetch_add(1, Ordering::AcqRel);
            Ok(2)
        }
    }

    struct ScriptedDisk(AtomicU64);

    impl TrackDiskUsagePort for ScriptedDisk {
        fn track_disk_bytes(&self) -> Result<u64, CoreError> {
            Ok(self.0.load(Ordering::Acquire))
        }
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
