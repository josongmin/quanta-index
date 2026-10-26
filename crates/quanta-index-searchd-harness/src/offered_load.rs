//! Bounded, replayable offered-arrival scheduling shared by query adapters.
//!
//! Endpoint callbacks own request deadlines and outcome classification. This
//! module only schedules arrivals, bounds the dispatch queue, and accounts for
//! every offered task. A cancelled or panicked run never returns a partial row.

use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail, ensure};

pub const MAX_TOTAL_REQUESTS: u128 = 100_000;
const CANCEL_POLL_INTERVAL: Duration = Duration::from_millis(50);

/// Arrival process selected before any endpoint request is dispatched.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ArrivalModel {
    DeterministicPeriodic,
    SeededPoisson,
}

impl ArrivalModel {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::DeterministicPeriodic => "deterministic_periodic",
            Self::SeededPoisson => "seeded_poisson",
        }
    }
}

#[derive(Clone, Debug)]
pub struct OfferedConfig {
    pub seed: u64,
    pub arrival_model: ArrivalModel,
    pub rate_qps: u32,
    pub duration: Duration,
    pub workers: usize,
    pub queue_capacity: usize,
    pub request_timeout: Duration,
}

impl OfferedConfig {
    pub fn validate(&self) -> Result<()> {
        ensure!(self.rate_qps > 0, "offered QPS must be positive");
        ensure!(
            self.duration >= Duration::from_millis(100),
            "duration must be at least 100 ms"
        );
        ensure!(
            self.request_timeout >= Duration::from_millis(50)
                && self.request_timeout <= Duration::from_secs(30),
            "request timeout must be 50 ms..30 s"
        );
        ensure!((1..=128).contains(&self.workers), "workers must be 1..128");
        ensure!(
            (1..=4096).contains(&self.queue_capacity),
            "queue capacity must be 1..4096"
        );
        ensure!(
            scheduled_count(self.duration, self.rate_qps)? <= MAX_TOTAL_REQUESTS,
            "at most {MAX_TOTAL_REQUESTS} requests may be offered"
        );
        Ok(())
    }
}

#[derive(Clone, Copy, Debug)]
pub struct OfferedTask {
    pub id: u64,
    pub scheduled: Instant,
}

#[derive(Debug)]
pub struct OfferedCompletion<C> {
    pub task: OfferedTask,
    pub value: C,
}

#[derive(Debug)]
pub struct OfferedRun<C> {
    pub offered: u64,
    pub dropped_queue_full: u64,
    pub dropped_scheduler_late: u64,
    pub completions: Vec<OfferedCompletion<C>>,
    pub drain_secs: f64,
}

pub fn scheduled_count(duration: Duration, rate: u32) -> Result<u128> {
    ensure!(rate > 0, "offered QPS must be positive");
    let scaled = duration
        .as_nanos()
        .checked_mul(u128::from(rate))
        .context("offered request count overflow")?;
    Ok(scaled.div_ceil(1_000_000_000))
}

pub fn scheduled_offset(index: u128, rate: u32) -> Result<Duration> {
    let scaled = index
        .checked_mul(1_000_000_000)
        .context("scheduled offset overflow")?;
    let nanos = scaled
        .checked_div(u128::from(rate))
        .context("offered QPS must be positive")?;
    Ok(Duration::from_nanos(u64::try_from(nanos)?))
}

/// Local deterministic PRNG; ambient thread RNG cannot alter a replay.
fn next_random(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9e37_79b9_7f4a_7c15);
    let mut value = *state;
    value = (value ^ (value >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    value ^ (value >> 31)
}

fn poisson_offsets(duration: Duration, rate: u32, seed: u64) -> Result<Vec<Duration>> {
    let expected = scheduled_count(duration, rate)?;
    let maximum = expected
        .checked_mul(2)
        .and_then(|value| value.checked_add(128))
        .context("Poisson schedule limit overflow")?;
    let mut state = seed ^ u64::from(rate).rotate_left(17);
    let mut elapsed_secs = 0.0_f64;
    let duration_secs = duration.as_secs_f64();
    let mut offsets = Vec::new();
    loop {
        if elapsed_secs >= duration_secs {
            break;
        }
        // Map 53 random bits into (0, 1] so ln is finite and zero intervals
        // cannot create duplicate scheduled arrivals.
        let random = next_random(&mut state) >> 11;
        let high = u32::try_from(random >> 21).context("Poisson random high bits overflow")?;
        let low = u32::try_from(random & ((1_u64 << 21) - 1))
            .context("Poisson random low bits overflow")?;
        let unit = (f64::from(high) * 2_097_152.0 + f64::from(low)) / 9_007_199_254_740_992.0;
        let open_unit = unit.clamp(f64::MIN_POSITIVE, 1.0);
        elapsed_secs += -open_unit.ln() / f64::from(rate);
        if elapsed_secs >= duration_secs {
            break;
        }
        if u128::try_from(offsets.len()).context("Poisson schedule length overflow")? >= maximum {
            bail!("Poisson schedule exceeded its bounded replay limit");
        }
        offsets.push(Duration::from_secs_f64(elapsed_secs));
    }
    Ok(offsets)
}

pub fn scheduled_offsets(config: &OfferedConfig) -> Result<Vec<Duration>> {
    config.validate()?;
    let offsets = match config.arrival_model {
        ArrivalModel::DeterministicPeriodic => {
            let count = scheduled_count(config.duration, config.rate_qps)?;
            (0..count)
                .map(|index| scheduled_offset(index, config.rate_qps))
                .collect::<Result<Vec<_>>>()?
        }
        ArrivalModel::SeededPoisson => {
            poisson_offsets(config.duration, config.rate_qps, config.seed)?
        }
    };
    ensure!(
        offsets.len() <= usize::try_from(MAX_TOTAL_REQUESTS)?,
        "at most {MAX_TOTAL_REQUESTS} requests may be offered"
    );
    Ok(offsets)
}

struct WorkerGuard<C> {
    sender: Option<mpsc::SyncSender<OfferedTask>>,
    handles: Vec<JoinHandle<()>>,
    results_receiver: mpsc::Receiver<OfferedCompletion<C>>,
    cancel: Arc<AtomicBool>,
    finished: bool,
}

impl<C> WorkerGuard<C> {
    fn join_all(&mut self) -> Result<()> {
        let mut first_panic = None;
        for handle in self.handles.drain(..) {
            match handle.join() {
                Err(panic) if first_panic.is_none() => {
                    first_panic = Some(anyhow!("load worker panicked: {panic:?}"));
                }
                Ok(()) | Err(_) => {}
            }
        }
        first_panic.map_or(Ok(()), Err)
    }

    fn finish(mut self) -> Result<Vec<OfferedCompletion<C>>> {
        drop(self.sender.take());
        self.join_all()?;
        let completions = self.results_receiver.try_iter().collect();
        self.finished = true;
        Ok(completions)
    }
}

impl<C> Drop for WorkerGuard<C> {
    fn drop(&mut self) {
        if !self.finished {
            self.cancel.store(true, Ordering::Release);
            drop(self.sender.take());
            drop(self.join_all());
        }
    }
}

fn wait_until(scheduled: Instant, cancel: &AtomicBool) -> Result<()> {
    loop {
        ensure!(
            !cancel.load(Ordering::Acquire),
            "offered-load run cancelled"
        );
        let Some(wait) = scheduled.checked_duration_since(Instant::now()) else {
            return Ok(());
        };
        if wait.is_zero() {
            return Ok(());
        }
        thread::sleep(wait.min(CANCEL_POLL_INTERVAL));
    }
}

/// Run one offered-load point.
///
/// Every normal return accounts for every arrival. Cancellation, a worker panic,
/// or an early scheduling error joins all workers and returns an error instead
/// of publishing a partial measurement.
pub fn run_point<C, F>(
    config: &OfferedConfig,
    cancel: &Arc<AtomicBool>,
    dispatch: F,
) -> Result<OfferedRun<C>>
where
    C: Send + 'static,
    F: Fn(OfferedTask, Duration) -> C + Send + Sync + 'static,
{
    let offsets = scheduled_offsets(config)?;
    let offered = u64::try_from(offsets.len())?;
    ensure!(offered > 0, "open-loop schedule offered no requests");
    ensure!(
        !cancel.load(Ordering::Acquire),
        "offered-load run cancelled"
    );

    let (sender, receiver) = mpsc::sync_channel::<OfferedTask>(config.queue_capacity);
    let shared_receiver = Arc::new(Mutex::new(receiver));
    let (results_sender, results_receiver) = mpsc::channel::<OfferedCompletion<C>>();
    let mut workers = WorkerGuard {
        sender: Some(sender),
        handles: Vec::with_capacity(config.workers),
        results_receiver,
        cancel: Arc::clone(cancel),
        finished: false,
    };
    let callback = Arc::new(dispatch);
    for _ in 0..config.workers {
        let shared_receiver = Arc::clone(&shared_receiver);
        let results_sender = results_sender.clone();
        let callback = Arc::clone(&callback);
        let worker_cancel = Arc::clone(cancel);
        let timeout = config.request_timeout;
        workers.handles.push(
            thread::Builder::new()
                .spawn(move || {
                    loop {
                        if worker_cancel.load(Ordering::Acquire) {
                            return;
                        }
                        let task = {
                            let Ok(receiver) = shared_receiver.lock() else {
                                return;
                            };
                            let Ok(task) = receiver.recv() else {
                                return;
                            };
                            task
                        };
                        if worker_cancel.load(Ordering::Acquire) {
                            return;
                        }
                        let value = match catch_unwind(AssertUnwindSafe(|| callback(task, timeout)))
                        {
                            Ok(value) => value,
                            Err(panic) => {
                                worker_cancel.store(true, Ordering::Release);
                                resume_unwind(panic);
                            }
                        };
                        if results_sender
                            .send(OfferedCompletion { task, value })
                            .is_err()
                        {
                            return;
                        }
                    }
                })
                .context("spawn offered-load worker")?,
        );
    }
    drop(results_sender);

    let started = Instant::now();
    let mut dropped_queue_full = 0_u64;
    let mut dropped_scheduler_late = 0_u64;
    for (index, offset) in offsets.into_iter().enumerate() {
        let scheduled = started
            .checked_add(offset)
            .context("scheduled arrival instant overflow")?;
        wait_until(scheduled, cancel)?;
        if Instant::now().saturating_duration_since(scheduled) >= config.request_timeout {
            dropped_scheduler_late = dropped_scheduler_late
                .checked_add(1)
                .context("scheduler-late drop count overflow")?;
            continue;
        }
        let task = OfferedTask {
            id: u64::try_from(index)?
                .checked_add(1)
                .context("request id overflow")?,
            scheduled,
        };
        match workers
            .sender
            .as_ref()
            .context("offered-load sender closed before scheduling ended")?
            .try_send(task)
        {
            Ok(()) => {}
            Err(mpsc::TrySendError::Full(_)) => {
                dropped_queue_full = dropped_queue_full
                    .checked_add(1)
                    .context("full-queue drop count overflow")?;
            }
            Err(mpsc::TrySendError::Disconnected(_)) => {
                bail!("all load workers disconnected during scheduling");
            }
        }
    }
    let horizon = started
        .checked_add(config.duration)
        .context("offered-load horizon instant overflow")?;
    wait_until(horizon, cancel)?;
    let completions = workers.finish()?;
    ensure!(
        !cancel.load(Ordering::Acquire),
        "offered-load run cancelled"
    );
    let drain_secs = started.elapsed().as_secs_f64();
    let accounted = u64::try_from(completions.len())?
        .checked_add(dropped_queue_full)
        .and_then(|count| count.checked_add(dropped_scheduler_late))
        .context("offered request accounting overflow")?;
    ensure!(
        accounted == offered,
        "offered requests were lost before accounting"
    );
    Ok(OfferedRun {
        offered,
        dropped_queue_full,
        dropped_scheduler_late,
        completions,
        drain_secs,
    })
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicUsize;

    use super::*;

    fn config(model: ArrivalModel, rate_qps: u32, duration: Duration) -> OfferedConfig {
        OfferedConfig {
            seed: 7,
            arrival_model: model,
            rate_qps,
            duration,
            workers: 1,
            queue_capacity: 1,
            request_timeout: Duration::from_secs(1),
        }
    }

    #[test]
    fn fixed_and_seeded_arrivals_match_source_independent_goldens() -> Result<()> {
        let fixed = config(
            ArrivalModel::DeterministicPeriodic,
            10,
            Duration::from_millis(250),
        );
        let fixed_actual = scheduled_offsets(&fixed)?;
        let fixed_expected = [
            Duration::ZERO,
            Duration::from_millis(100),
            Duration::from_millis(200),
        ];
        ensure!(
            fixed_actual == fixed_expected,
            "fixed arrival golden mismatch: {fixed_actual:?}"
        );
        let poisson = config(ArrivalModel::SeededPoisson, 10, Duration::from_secs(1));
        // Seed 7, 10 QPS, 1 s; fixed microsecond intervals independently
        // computed from the documented SplitMix64/exponential schedule.
        let actual = scheduled_offsets(&poisson)?
            .into_iter()
            .map(|offset| offset.as_micros())
            .collect::<Vec<_>>();
        let expected = [
            72_406, 117_355, 155_225, 319_229, 399_306, 411_229, 419_452, 699_989, 768_940,
        ];
        ensure!(
            actual == expected,
            "seeded arrival golden mismatch: {actual:?}"
        );
        Ok(())
    }

    #[test]
    fn request_cap_rejects_a_run_before_dispatch() {
        let oversized = config(
            ArrivalModel::DeterministicPeriodic,
            100_001,
            Duration::from_secs(1),
        );
        assert!(oversized.validate().is_err());
    }

    #[test]
    fn full_queue_is_counted_in_the_offered_denominator() -> Result<()> {
        let mut load = config(
            ArrivalModel::DeterministicPeriodic,
            10_000,
            Duration::from_millis(100),
        );
        load.request_timeout = Duration::from_secs(30);
        let entered = Arc::new(AtomicBool::new(false));
        let release = Arc::new(AtomicBool::new(false));
        let entered_for_release = Arc::clone(&entered);
        let release_for_thread = Arc::clone(&release);
        let releaser = thread::spawn(move || {
            let started = Instant::now();
            while !entered_for_release.load(Ordering::Acquire)
                && started.elapsed() < Duration::from_secs(1)
            {
                thread::sleep(Duration::from_millis(1));
            }
            thread::sleep(Duration::from_millis(500));
            release_for_thread.store(true, Ordering::Release);
        });
        let cancel = Arc::new(AtomicBool::new(false));
        let run_result = run_point(&load, &cancel, move |_, _| {
            entered.store(true, Ordering::Release);
            while !release.load(Ordering::Acquire) {
                thread::sleep(Duration::from_millis(1));
            }
        });
        releaser
            .join()
            .map_err(|panic| anyhow!("releaser panicked: {panic:?}"))?;
        let run = run_result?;
        ensure!(
            run.offered == 1_000,
            "unexpected offered count: {}",
            run.offered
        );
        ensure!(run.dropped_queue_full > 0, "expected full-queue drops");
        let accounted = u64::try_from(run.completions.len())?
            .checked_add(run.dropped_queue_full)
            .and_then(|count| count.checked_add(run.dropped_scheduler_late))
            .context("offered request accounting overflow")?;
        ensure!(run.offered == accounted, "offered denominator mismatch");
        ensure!(
            run.completions.iter().all(|item| item.task.id > 0),
            "completion carried a zero request id"
        );
        Ok(())
    }

    #[test]
    fn cancellation_joins_callbacks_and_returns_no_partial_run() {
        let load = config(
            ArrivalModel::DeterministicPeriodic,
            2,
            Duration::from_secs(1),
        );
        let cancel = Arc::new(AtomicBool::new(false));
        let callback_cancel = Arc::clone(&cancel);
        let returned = Arc::new(AtomicBool::new(false));
        let callback_returned = Arc::clone(&returned);
        let result = run_point(&load, &cancel, move |_, _| {
            callback_cancel.store(true, Ordering::Release);
            callback_returned.store(true, Ordering::Release);
        });
        assert!(result.is_err());
        assert!(returned.load(Ordering::Acquire));
    }

    #[test]
    fn callback_panic_joins_every_worker_before_error() {
        struct Active(Arc<AtomicUsize>);
        impl Drop for Active {
            fn drop(&mut self) {
                let _previous = self.0.fetch_sub(1, Ordering::AcqRel);
            }
        }

        let mut load = config(
            ArrivalModel::DeterministicPeriodic,
            100,
            Duration::from_millis(100),
        );
        load.workers = 2;
        let active = Arc::new(AtomicUsize::new(0));
        let callback_active = Arc::clone(&active);
        let cancel = Arc::new(AtomicBool::new(false));
        let result = run_point(&load, &cancel, move |task, _| {
            let _previous = callback_active.fetch_add(1, Ordering::AcqRel);
            let _guard = Active(Arc::clone(&callback_active));
            assert_ne!(task.id, 1, "intentional callback failure");
        });
        assert!(result.is_err());
        assert_eq!(active.load(Ordering::Acquire), 0);
    }
}
