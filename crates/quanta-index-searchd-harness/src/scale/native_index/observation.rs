//! Diagnostic child-process and allocated-block observations for the native rail.

#[cfg(target_os = "linux")]
use std::fs;
#[cfg(target_os = "macos")]
use std::io::Read as _;
use std::path::{Path, PathBuf};
#[cfg(target_os = "macos")]
use std::process::{Child, Command, Stdio};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result};
use serde_json::{Value, json};

#[path = "aggregate_rss.rs"]
mod aggregate_rss;
use aggregate_rss::AggregateRss;

const RSS_INTERVAL: Duration = Duration::from_millis(100);
const PROCESS_IO_INTERVAL: Duration = Duration::from_millis(100);
const DISK_INTERVAL: Duration = Duration::from_millis(500);
#[cfg(target_os = "macos")]
const PS_DEADLINE: Duration = Duration::from_secs(2);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ObservationMode {
    Disabled,
    Enabled,
}

impl ObservationMode {
    pub(super) fn from_env() -> Result<Self> {
        match std::env::var_os("QUANTA_INDEX_NATIVE_RESOURCE_SAMPLING_V1") {
            None => Ok(Self::Disabled),
            Some(value) if value == "1" => Ok(Self::Enabled),
            Some(_) => {
                anyhow::bail!("QUANTA_INDEX_NATIVE_RESOURCE_SAMPLING_V1 must be exactly 1 when set")
            }
        }
    }

    pub(super) fn enabled(self) -> bool {
        self == Self::Enabled
    }
}

#[derive(Default)]
struct ProbeSummary {
    first_bytes: Option<u64>,
    last_bytes: Option<u64>,
    sampled_max_bytes: Option<u64>,
    samples: u64,
    missing: u64,
    max_observed_gap_ms: f64,
    previous_at: Option<Instant>,
    observer_wall_ms: f64,
    last_missing_reason: Option<String>,
}

impl ProbeSummary {
    fn record(&mut self, at: Instant, elapsed: Duration, result: Result<u64>) {
        self.observer_wall_ms += elapsed.as_secs_f64() * 1_000.0;
        if let Some(previous) = self.previous_at {
            self.max_observed_gap_ms = self
                .max_observed_gap_ms
                .max(at.saturating_duration_since(previous).as_secs_f64() * 1_000.0);
        }
        self.previous_at = Some(at);
        match result {
            Ok(bytes) => {
                let _first = self.first_bytes.get_or_insert(bytes);
                self.last_bytes = Some(bytes);
                self.sampled_max_bytes =
                    Some(self.sampled_max_bytes.map_or(bytes, |old| old.max(bytes)));
                self.samples = self.samples.saturating_add(1);
            }
            Err(error) => {
                self.missing = self.missing.saturating_add(1);
                self.last_missing_reason = Some(format!("{error:#}"));
            }
        }
    }

    fn detail(&self, interval: Duration, method: &str) -> Value {
        json!({
            "method":method,
            "sample_interval_ms":interval.as_millis(),
            "first_bytes":self.first_bytes,
            "last_bytes":self.last_bytes,
            "sampled_max_bytes":self.sampled_max_bytes,
            "sample_count":self.samples,
            "missing_count":self.missing,
            "last_missing_reason":self.last_missing_reason,
            "max_observed_gap_ms":self.max_observed_gap_ms,
            "observer_wall_ms":self.observer_wall_ms,
        })
    }
}

/// Linux process-accounted filesystem IO, not device-media traffic.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ChildProcessIoBytes {
    read: u64,
    write: u64,
    cancelled_write: u64,
}

impl ChildProcessIoBytes {
    fn checked_delta(self, before: Self) -> Result<Self> {
        Ok(Self {
            read: self
                .read
                .checked_sub(before.read)
                .context("child read_bytes regressed")?,
            write: self
                .write
                .checked_sub(before.write)
                .context("child write_bytes regressed")?,
            cancelled_write: self
                .cancelled_write
                .checked_sub(before.cancelled_write)
                .context("child cancelled_write_bytes regressed")?,
        })
    }
}

#[cfg(any(target_os = "linux", test))]
fn parse_child_process_io(source: &str) -> Result<ChildProcessIoBytes> {
    fn field(source: &str, name: &str) -> Result<u64> {
        let mut found = None;
        for line in source.lines() {
            let Some((key, raw)) = line.split_once(':') else {
                continue;
            };
            if key == name {
                anyhow::ensure!(found.is_none(), "duplicate child {name}");
                let value = raw.trim();
                anyhow::ensure!(
                    !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()),
                    "invalid child {name}"
                );
                found = Some(
                    value
                        .parse::<u64>()
                        .with_context(|| format!("invalid child {name}"))?,
                );
            }
        }
        found.with_context(|| format!("missing child {name}"))
    }

    Ok(ChildProcessIoBytes {
        read: field(source, "read_bytes")?,
        write: field(source, "write_bytes")?,
        cancelled_write: field(source, "cancelled_write_bytes")?,
    })
}

#[cfg(target_os = "linux")]
fn child_process_io(pid: u32) -> Result<ChildProcessIoBytes> {
    parse_child_process_io(&fs::read_to_string(format!("/proc/{pid}/io"))?)
}

#[cfg(not(target_os = "linux"))]
fn child_process_io(_pid: u32) -> Result<ChildProcessIoBytes> {
    anyhow::bail!("child process IO unsupported on this platform")
}

#[derive(Default)]
struct ProcessIoSummary {
    first: Option<ChildProcessIoBytes>,
    last: Option<ChildProcessIoBytes>,
    first_at: Option<Instant>,
    last_at: Option<Instant>,
    samples: u64,
    missing: u64,
    max_observed_gap_ms: f64,
    previous_at: Option<Instant>,
    observer_wall_ms: f64,
    last_missing_reason: Option<String>,
    regression_error: Option<String>,
}

impl ProcessIoSummary {
    fn record(
        &mut self,
        at: Instant,
        elapsed: Duration,
        result: Result<ChildProcessIoBytes>,
    ) -> Result<()> {
        self.observer_wall_ms += elapsed.as_secs_f64() * 1_000.0;
        if let Some(previous) = self.previous_at {
            self.max_observed_gap_ms = self
                .max_observed_gap_ms
                .max(at.saturating_duration_since(previous).as_secs_f64() * 1_000.0);
        }
        self.previous_at = Some(at);
        match result {
            Ok(sample) => {
                if let Some(previous) = self.last
                    && let Err(error) = sample.checked_delta(previous)
                {
                    let reason = format!("child process IO counter regression: {error:#}");
                    self.missing = self.missing.saturating_add(1);
                    self.last_missing_reason = Some(reason.clone());
                    self.regression_error = Some(reason.clone());
                    anyhow::bail!(reason);
                }
                let _first = self.first.get_or_insert(sample);
                let _first_at = self.first_at.get_or_insert(at);
                self.last = Some(sample);
                self.last_at = Some(at);
                self.samples = self.samples.saturating_add(1);
            }
            Err(error) => {
                self.missing = self.missing.saturating_add(1);
                self.last_missing_reason = Some(format!("{error:#}"));
            }
        }
        Ok(())
    }

    fn detail(&self, started: Option<Instant>) -> Value {
        let mut regression_error = self.regression_error.clone();
        let delta = match self.first.zip(self.last) {
            Some((first, last)) if regression_error.is_none() => match last.checked_delta(first) {
                Ok(delta) => Some(delta),
                Err(error) => {
                    regression_error = Some(format!("child process IO delta failed: {error:#}"));
                    None
                }
            },
            _ => None,
        };
        let offset_ms = |at: Option<Instant>| {
            started.zip(at).map(|(start, sample)| {
                sample.saturating_duration_since(start).as_secs_f64() * 1_000.0
            })
        };
        json!({
            "method":if cfg!(target_os = "linux") { "proc_pid_io" } else { "unsupported" },
            "scope":"unreaped_child_after_socket_ready_until_shutdown_signal; kernel_process_accounting_not_device_media_bytes",
            "sample_interval_ms":PROCESS_IO_INTERVAL.as_millis(),
            "read_bytes":{"first":self.first.map(|value| value.read),
                "last":self.last.map(|value| value.read),
                "delta":delta.map(|value| value.read)},
            "write_bytes":{"first":self.first.map(|value| value.write),
                "last":self.last.map(|value| value.write),
                "delta":delta.map(|value| value.write)},
            "cancelled_write_bytes":{"first":self.first.map(|value| value.cancelled_write),
                "last":self.last.map(|value| value.cancelled_write),
                "delta":delta.map(|value| value.cancelled_write)},
            "first_sample_offset_ms":offset_ms(self.first_at),
            "last_sample_offset_ms":offset_ms(self.last_at),
            "observed_window_ms":self.first_at.zip(self.last_at)
                .map(|(first,last)| last.saturating_duration_since(first).as_secs_f64() * 1_000.0),
            "sample_count":self.samples,
            "missing_count":self.missing,
            "last_missing_reason":self.last_missing_reason,
            "max_observed_gap_ms":self.max_observed_gap_ms,
            "observer_wall_ms":self.observer_wall_ms,
            "regression_error":regression_error,
        })
    }
}

#[derive(Default)]
struct Epoch {
    rss: ProbeSummary,
    aggregate_rss: ProbeSummary,
    fixture_disk: ProbeSummary,
    disk: ProbeSummary,
    scratch: ProbeSummary,
    process_io: ProcessIoSummary,
    started: Option<Instant>,
    stopped: Option<Instant>,
    worker_failure: Option<String>,
}

struct Shared {
    epochs: Vec<(String, Arc<Mutex<Epoch>>)>,
    unavailable: Vec<Value>,
}

pub(super) struct NativeObservation {
    mode: ObservationMode,
    shared: Mutex<Shared>,
}

impl NativeObservation {
    pub(super) fn new(mode: ObservationMode) -> Self {
        Self {
            mode,
            shared: Mutex::new(Shared {
                epochs: Vec::new(),
                unavailable: Vec::new(),
            }),
        }
    }

    pub(super) fn start(&self, label: &str, pid: u32, root: &Path) -> Option<NativeSampler> {
        if !self.mode.enabled() {
            return None;
        }
        let epoch = Arc::new(Mutex::new(Epoch {
            started: Some(Instant::now()),
            ..Epoch::default()
        }));
        match NativeSampler::start(pid, root, Arc::clone(&epoch)) {
            Ok(sampler) => {
                if let Ok(mut shared) = self.shared.lock() {
                    shared.epochs.push((label.to_owned(), epoch));
                }
                Some(sampler)
            }
            Err(error) => {
                if let Ok(mut shared) = self.shared.lock() {
                    shared
                        .unavailable
                        .push(json!({"epoch":label,"reason":format!("{error:#}")}));
                }
                None
            }
        }
    }

    pub(super) fn detail(&self) -> Option<Value> {
        if !self.mode.enabled() {
            return None;
        }
        let Ok(shared) = self.shared.lock() else {
            return Some(json!({
                "mode":"opt_in_diagnostic_v2",
                "unavailable_reason":"observation_lock_poisoned"
            }));
        };
        let epochs: Vec<Value> = shared.epochs.iter().map(|(name, epoch)| {
            let Ok(epoch) = epoch.lock() else {
                return json!({"epoch":name,"unavailable_reason":"observation_lock_poisoned"});
            };
            json!({
                "epoch":name,
                "observed_elapsed_ms":epoch.started.zip(epoch.stopped).map(|(start,end)| end.saturating_duration_since(start).as_secs_f64()*1_000.0),
                "child_rss":epoch.rss.detail(RSS_INTERVAL, rss_method()),
                "aggregate_process_rss":epoch.aggregate_rss.detail(RSS_INTERVAL,
                    if cfg!(target_os = "linux") { "proc_stat_identity_and_status_vmrss_sum" } else { "unsupported" }),
                "allocated_fixture_root":epoch.fixture_disk.detail(DISK_INTERVAL, "unique_inode_st_blocks_x_512"),
                "allocated_state_root":epoch.disk.detail(DISK_INTERVAL, "unique_inode_st_blocks_x_512"),
                "allocated_native_scratch":epoch.scratch.detail(DISK_INTERVAL, "unique_inode_st_blocks_x_512"),
                "child_process_io":epoch.process_io.detail(epoch.started),
                "worker_failure":epoch.worker_failure,
            })
        }).collect();
        Some(json!({
            "mode":"opt_in_diagnostic_v2",
            "scope":"child_daemon_after_socket_ready_until_shutdown_signal; startup and drain completion excluded",
            "sampling":"periodic and boundary observations; sampled maximum is not a continuous peak",
            "observer_cost_scope":"probe and traversal wall time; scheduling and aggregation excluded",
            "stop_limit":"cooperative between filesystem entries; an individual metadata syscall may block",
            "continuous_peak":false,
            "sampled_only":true,
            "aggregate_rss_scope":"harness and daemon plus observed descendants, including previously seen reparented children; per-process RSS sum may double-count shared pages",
            "aggregate_rss_census":"non-atomic, bounded; processes entirely between observations may be missed; numeric PID reuse is excluded by start-time identity",
            "aggregate_rss_max_processes":aggregate_rss::MAX_PROCESSES,
            "disk_observer_cost_scope":"single fixture traversal charged to allocated_fixture_root; state and scratch are subsets with zero additional traversal cost",
            "fixture_scope":"owned native temporary fixture including state, scratch, config and in-fixture diagnostics; external retained JSONL/stderr and other temporary roots excluded",
            "scratch_scope":"state_root/native-scratch-v1 allocated blocks; linked objects and other temporary roots excluded",
            "epochs":epochs,
            "unavailable":shared.unavailable,
        }))
    }
}

pub(super) struct NativeSampler {
    stopped: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
    epoch: Arc<Mutex<Epoch>>,
    aggregate_rss: Arc<Mutex<AggregateRss>>,
    pid: u32,
    root: PathBuf,
}

impl NativeSampler {
    fn start(pid: u32, root: &Path, epoch: Arc<Mutex<Epoch>>) -> Result<Self> {
        // Bracket even a short reopen epoch: the background worker need not
        // get scheduled before stop. Both probes retain the same unreaped PID.
        let at = Instant::now();
        let initial_io = child_process_io(pid);
        if let Ok(mut observed) = epoch.lock() {
            observed.process_io.record(at, at.elapsed(), initial_io)?;
        }
        let stopped = Arc::new(AtomicBool::new(false));
        let aggregate_rss = Arc::new(Mutex::new(AggregateRss::new(pid)));
        let at = Instant::now();
        let initial_rss = aggregate_sample(&aggregate_rss, &stopped);
        if let Ok(mut observed) = epoch.lock() {
            observed.aggregate_rss.record(at, at.elapsed(), initial_rss);
        }
        let worker_stopped = Arc::clone(&stopped);
        let worker_aggregate = Arc::clone(&aggregate_rss);
        let worker_epoch = Arc::clone(&epoch);
        let worker_root = root.to_path_buf();
        let worker = thread::Builder::new()
            .name("native-resource-observer".into())
            .spawn(move || {
                let begun = Instant::now();
                let mut last_rss = begun.checked_sub(RSS_INTERVAL).unwrap_or(begun);
                let mut last_process_io = begun.checked_sub(PROCESS_IO_INTERVAL).unwrap_or(begun);
                let mut last_disk = begun.checked_sub(DISK_INTERVAL).unwrap_or(begun);
                while !worker_stopped.load(Ordering::Acquire) {
                    let now = Instant::now();
                    if now.saturating_duration_since(last_rss) >= RSS_INTERVAL {
                        last_rss = now;
                        let at = Instant::now();
                        let result = child_rss_bytes(pid, &worker_stopped);
                        if let Ok(mut summary) = worker_epoch.lock() {
                            summary.rss.record(at, at.elapsed(), result);
                        }
                        let at = Instant::now();
                        let result = aggregate_sample(&worker_aggregate, &worker_stopped);
                        if let Ok(mut summary) = worker_epoch.lock() {
                            summary.aggregate_rss.record(at, at.elapsed(), result);
                        }
                    }
                    if worker_stopped.load(Ordering::Acquire) {
                        break;
                    }
                    if now.saturating_duration_since(last_process_io) >= PROCESS_IO_INTERVAL {
                        last_process_io = now;
                        let at = Instant::now();
                        let result = child_process_io(pid);
                        if let Ok(mut epoch) = worker_epoch.lock()
                            && let Err(error) = epoch.process_io.record(at, at.elapsed(), result)
                        {
                            epoch.worker_failure = Some(format!("{error:#}"));
                            break;
                        }
                    }
                    if worker_stopped.load(Ordering::Acquire) {
                        break;
                    }
                    if now.saturating_duration_since(last_disk) >= DISK_INTERVAL {
                        last_disk = now;
                        let at = Instant::now();
                        let result = super::super::allocated_native_fixture_with_cancel(
                            &worker_root,
                            || worker_stopped.load(Ordering::Acquire),
                        );
                        let elapsed = at.elapsed();
                        if let Ok(mut summary) = worker_epoch.lock() {
                            record_disk(&mut summary, at, elapsed, result);
                        }
                    }
                    thread::park_timeout(Duration::from_millis(10));
                }
            })
            .context("spawn native resource observer")?;
        Ok(Self {
            stopped,
            worker: Some(worker),
            epoch,
            aggregate_rss,
            pid,
            root: root.to_path_buf(),
        })
    }

    pub(super) fn stop(mut self) {
        self.join();
        // The Child has not yet been reaped. These boundary probes include
        // short-lived work that periodic sampling may miss.
        let rss_at = Instant::now();
        let uncancelled = AtomicBool::new(false);
        let rss = child_rss_bytes(self.pid, &uncancelled);
        let rss_elapsed = rss_at.elapsed();
        let aggregate_at = Instant::now();
        let aggregate = aggregate_sample(&self.aggregate_rss, &uncancelled);
        let aggregate_elapsed = aggregate_at.elapsed();
        let process_io_at = Instant::now();
        let process_io = child_process_io(self.pid);
        let process_io_elapsed = process_io_at.elapsed();
        let disk_at = Instant::now();
        let disk = super::super::allocated_native_fixture_with_cancel(&self.root, || false);
        let disk_elapsed = disk_at.elapsed();
        if let Ok(mut epoch) = self.epoch.lock() {
            epoch.rss.record(rss_at, rss_elapsed, rss);
            epoch
                .aggregate_rss
                .record(aggregate_at, aggregate_elapsed, aggregate);
            if let Err(error) =
                epoch
                    .process_io
                    .record(process_io_at, process_io_elapsed, process_io)
            {
                epoch.worker_failure = Some(format!("{error:#}"));
            }
            record_disk(&mut epoch, disk_at, disk_elapsed, disk);
            epoch.stopped = Some(Instant::now());
        }
    }

    fn join(&mut self) {
        self.stopped.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            worker.thread().unpark();
            if worker.join().is_err()
                && let Ok(mut epoch) = self.epoch.lock()
            {
                epoch.worker_failure = Some("observer_thread_panicked".into());
            }
        }
    }
}

impl Drop for NativeSampler {
    fn drop(&mut self) {
        self.join();
    }
}

fn record_disk(epoch: &mut Epoch, at: Instant, elapsed: Duration, result: Result<[u64; 3]>) {
    match result {
        Ok([fixture, state, scratch]) => {
            epoch.fixture_disk.record(at, elapsed, Ok(fixture));
            epoch.disk.record(at, Duration::ZERO, Ok(state));
            epoch.scratch.record(at, Duration::ZERO, Ok(scratch));
        }
        Err(error) => {
            let reason = format!("{error:#}");
            epoch
                .fixture_disk
                .record(at, elapsed, Err(anyhow::anyhow!(reason.clone())));
            epoch
                .disk
                .record(at, Duration::ZERO, Err(anyhow::anyhow!(reason.clone())));
            epoch
                .scratch
                .record(at, Duration::ZERO, Err(anyhow::anyhow!(reason)));
        }
    }
}

fn aggregate_sample(aggregate: &Mutex<AggregateRss>, stopped: &AtomicBool) -> Result<u64> {
    aggregate
        .lock()
        .map_err(|_poison| anyhow::anyhow!("aggregate RSS observer lock poisoned"))?
        .sample(stopped)
}

#[cfg(target_os = "linux")]
fn child_rss_bytes(pid: u32, _stopped: &AtomicBool) -> Result<u64> {
    let status = fs::read_to_string(format!("/proc/{pid}/status"))?;
    let kib = status
        .lines()
        .find_map(|line| line.strip_prefix("VmRSS:"))
        .context("child VmRSS absent")?;
    let value = kib
        .split_whitespace()
        .next()
        .context("child VmRSS value absent")?
        .parse::<u64>()?;
    anyhow::ensure!(value > 0, "child RSS unavailable or zero");
    value.checked_mul(1024).context("child RSS overflow")
}

#[cfg(target_os = "macos")]
fn child_rss_bytes(pid: u32, stopped: &AtomicBool) -> Result<u64> {
    struct ProbeChild(Child);
    impl Drop for ProbeChild {
        fn drop(&mut self) {
            drop(self.0.kill());
            drop(self.0.wait());
        }
    }
    let mut probe = ProbeChild(
        Command::new("ps")
            .args(["-o", "rss=", "-p", &pid.to_string()])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .context("start bounded child RSS probe")?,
    );
    let deadline = Instant::now()
        .checked_add(PS_DEADLINE)
        .context("RSS probe deadline overflow")?;
    loop {
        if stopped.load(Ordering::Acquire) || Instant::now() >= deadline {
            anyhow::bail!("child RSS probe cancelled or timed out");
        }
        if let Some(status) = probe.0.try_wait()? {
            anyhow::ensure!(status.success(), "child RSS probe failed or child exited");
            let mut output = String::new();
            let _read = probe
                .0
                .stdout
                .take()
                .context("child RSS probe stdout absent")?
                .read_to_string(&mut output)?;
            let kib = output
                .trim()
                .parse::<u64>()
                .context("child RSS probe returned no numeric RSS")?;
            anyhow::ensure!(kib > 0, "child RSS unavailable or zero");
            return kib.checked_mul(1024).context("child RSS overflow");
        }
        thread::sleep(Duration::from_millis(5));
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn child_rss_bytes(_pid: u32, _stopped: &AtomicBool) -> Result<u64> {
    anyhow::bail!("child RSS observation unsupported on this platform")
}

fn rss_method() -> &'static str {
    if cfg!(target_os = "linux") {
        "proc_pid_status_vmrss"
    } else if cfg!(target_os = "macos") {
        "ps_o_rss_kib"
    } else {
        "unsupported"
    }
}

#[cfg(test)]
mod tests {
    use super::super::product_tests::required_field;
    use super::*;

    fn io(read: u64, write: u64, cancelled_write: u64) -> ChildProcessIoBytes {
        ChildProcessIoBytes {
            read,
            write,
            cancelled_write,
        }
    }

    #[test]
    fn child_process_io_parser_requires_exact_unique_unsigned_counters() -> Result<()> {
        let valid = "rchar: 99\nread_bytes: 4096\nwrite_bytes: 8192\ncancelled_write_bytes: 64\n";
        anyhow::ensure!(
            parse_child_process_io(valid)? == io(4096, 8192, 64),
            "child IO parser changed exact counters"
        );
        for invalid in [
            "read_bytes: 1\nwrite_bytes: 2\n",
            "read_bytes: 1\nwrite_bytes: 2\nwrite_bytes: 3\ncancelled_write_bytes: 0\n",
            "read_bytes: 1\nwrite_bytes: -2\ncancelled_write_bytes: 0\n",
            "read_bytes: 1\nwrite_bytes: +2\ncancelled_write_bytes: 0\n",
            "read_bytes: 1\nwrite_bytes: 2 bytes\ncancelled_write_bytes: 0\n",
            "read_bytes: 1\nwrite_bytes: 2\ncancelled_write_bytes: 18446744073709551616\n",
        ] {
            anyhow::ensure!(
                parse_child_process_io(invalid).is_err(),
                "malformed child IO was accepted: {invalid:?}"
            );
        }
        Ok(())
    }

    #[test]
    fn child_process_io_summary_preserves_raw_deltas_missing_and_observation_window() -> Result<()>
    {
        let start = Instant::now();
        let mut observed = ProcessIoSummary::default();
        observed.record(start, Duration::from_millis(1), Ok(io(100, 200, 50)))?;
        observed.record(
            start
                .checked_add(Duration::from_millis(100))
                .context("missing sample time overflow")?,
            Duration::from_millis(2),
            Err(anyhow::anyhow!("sample unavailable")),
        )?;
        observed.record(
            start
                .checked_add(Duration::from_millis(250))
                .context("last sample time overflow")?,
            Duration::from_millis(3),
            Ok(io(125, 260, 80)),
        )?;
        let detail = observed.detail(Some(start));
        anyhow::ensure!(
            required_field(&detail, "/read_bytes/first")? == 100,
            "read baseline changed"
        );
        anyhow::ensure!(
            required_field(&detail, "/write_bytes/last")? == 260,
            "write endpoint changed"
        );
        anyhow::ensure!(
            required_field(&detail, "/read_bytes/delta")? == 25,
            "read delta changed"
        );
        anyhow::ensure!(
            required_field(&detail, "/write_bytes/delta")? == 60,
            "write delta changed"
        );
        anyhow::ensure!(
            required_field(&detail, "/cancelled_write_bytes/delta")? == 30,
            "cancelled writes were netted or lost"
        );
        anyhow::ensure!(
            required_field(&detail, "/sample_count")? == 2,
            "sample count changed"
        );
        anyhow::ensure!(
            required_field(&detail, "/missing_count")? == 1,
            "missing sample hidden"
        );
        anyhow::ensure!(
            required_field(&detail, "/last_missing_reason")? == "sample unavailable",
            "missing reason hidden"
        );
        anyhow::ensure!(
            required_field(&detail, "/max_observed_gap_ms")?
                .as_f64()
                .is_some_and(|gap| gap >= 150.0),
            "attempt gap hidden"
        );
        anyhow::ensure!(
            required_field(&detail, "/observer_wall_ms")?
                .as_f64()
                .is_some_and(|wall| wall >= 6.0),
            "probe cost hidden"
        );
        anyhow::ensure!(
            required_field(&detail, "/observed_window_ms")?
                .as_f64()
                .is_some_and(|window| window >= 250.0),
            "observation window hidden"
        );
        anyhow::ensure!(
            required_field(&detail, "/first_sample_offset_ms")? == 0.0
                && required_field(&detail, "/last_sample_offset_ms")?
                    .as_f64()
                    .is_some_and(|offset| offset >= 250.0),
            "sample offsets changed"
        );
        Ok(())
    }

    #[test]
    fn child_process_io_regression_refuses_delta_instead_of_wrapping() -> Result<()> {
        let start = Instant::now();
        let mut observed = ProcessIoSummary::default();
        observed.record(start, Duration::ZERO, Ok(io(100, 200, 50)))?;
        let later = start
            .checked_add(Duration::from_millis(100))
            .context("regression sample time overflow")?;
        anyhow::ensure!(
            observed
                .record(later, Duration::ZERO, Ok(io(101, 199, 51)))
                .is_err(),
            "regressing child write counter was accepted"
        );
        let detail = observed.detail(Some(start));
        anyhow::ensure!(
            required_field(&detail, "/write_bytes/delta")?.is_null(),
            "invalid delta published"
        );
        anyhow::ensure!(
            required_field(&detail, "/sample_count")? == 1,
            "regressing sample counted as valid"
        );
        anyhow::ensure!(
            required_field(&detail, "/missing_count")? == 1,
            "regression not recorded as missing"
        );
        anyhow::ensure!(
            required_field(&detail, "/regression_error")?
                .as_str()
                .is_some_and(|error| error.contains("write_bytes regressed")),
            "regression cause hidden"
        );
        Ok(())
    }

    #[cfg(not(target_os = "linux"))]
    #[test]
    fn unsupported_child_io_is_missing_with_reason_not_zero() -> Result<()> {
        let mut observed = ProcessIoSummary::default();
        observed.record(Instant::now(), Duration::ZERO, child_process_io(0))?;
        let detail = observed.detail(None);
        anyhow::ensure!(
            required_field(&detail, "/sample_count")? == 0,
            "unsupported IO became a sample"
        );
        anyhow::ensure!(
            required_field(&detail, "/missing_count")? == 1,
            "unsupported IO was hidden"
        );
        anyhow::ensure!(
            required_field(&detail, "/last_missing_reason")?
                .as_str()
                .is_some_and(|reason| reason.contains("unsupported")),
            "unsupported reason was hidden"
        );
        anyhow::ensure!(
            required_field(&detail, "/write_bytes/delta")?.is_null(),
            "unsupported IO became a zero-byte delta"
        );
        Ok(())
    }

    #[test]
    fn bounded_summary_keeps_fixed_observed_max_and_missing_samples() -> Result<()> {
        let first = Instant::now();
        let mut observed = ProbeSummary::default();
        observed.record(first, Duration::from_millis(1), Ok(4_096));
        observed.record(
            first
                .checked_add(Duration::from_millis(100))
                .context("sample time overflow")?,
            Duration::from_millis(2),
            Err(anyhow::anyhow!("child exited")),
        );
        observed.record(
            first
                .checked_add(Duration::from_millis(250))
                .context("sample time overflow")?,
            Duration::from_millis(3),
            Ok(2_048),
        );
        anyhow::ensure!(
            observed.first_bytes == Some(4_096),
            "first sampled RSS changed"
        );
        anyhow::ensure!(
            observed.last_bytes == Some(2_048),
            "last sampled RSS changed"
        );
        anyhow::ensure!(
            observed.sampled_max_bytes == Some(4_096),
            "maximum sample changed"
        );
        anyhow::ensure!(
            observed.samples == 2 && observed.missing == 1,
            "missing sample hidden"
        );
        anyhow::ensure!(observed.max_observed_gap_ms >= 150.0, "sample gap hidden");
        Ok(())
    }
}
