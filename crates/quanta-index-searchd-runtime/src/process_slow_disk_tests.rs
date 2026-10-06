//! Process-bound maintenance proof over the real state root and disk adapters.
//!
//! The child uses the production runtime composition. A crate-local wrapper
//! pauses one `TrackDiskUsagePort` call after the boot measurement; it then
//! delegates to the actual adapter. The gate never changes readiness or
//! health state itself.

use std::error::Error;
use std::io::{Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use quanta_index_contract::{
    ChunkId, ChunkRecord, ManifestGeneration, MetricsSnapshotV1, RepoId, RepoRelativePath,
    RevisionId, SourceFileKey, SourcePublicationEvent, lex::LanguageCode,
};
use quanta_index_core::{CoreError, RequestBudgetV1, TrackDiskUsagePort};
use quanta_index_sdk::{ConnectOptions, QuantaIndex, SearchCorpusBatch};
use quanta_index_searchd::app::{KernelResidentMemoryProbe, MaintenancePolicy};
use quanta_index_searchd_harness::{fixture_source_scope_v1, private_tempdir};

use super::{SearchdConfig, build_runtime_with_parts};

type TestResult = Result<(), Box<dyn Error>>;

const CADENCE: Duration = Duration::from_millis(100);
const WAIT: Duration = Duration::from_secs(10);
const CHILD_ROOT_ENV: &str = "QI_E3_04_DISK_CHILD_ROOT";
const CHILD_GATE_ENV: &str = "QI_E3_04_DISK_CHILD_GATE";
const CHILD_ARM_ENV: &str = "QI_E3_04_DISK_CHILD_ARM";
const CHILD_STOP_ENV: &str = "QI_E3_04_DISK_CHILD_STOP";

struct PausedActualDiskUsage {
    inner: Arc<dyn TrackDiskUsagePort>,
    gate: PathBuf,
    arm: PathBuf,
    entered: AtomicBool,
}

impl TrackDiskUsagePort for PausedActualDiskUsage {
    fn track_disk_bytes(&self, budget: &RequestBudgetV1) -> Result<u64, CoreError> {
        // The parent arms the gate only after the real runtime has published
        // and activated G1 through its SDK/UDS. Boot and preparation scans
        // delegate directly to the actual disk adapter.
        let mut gate = if self.arm.exists() && !self.entered.swap(true, Ordering::SeqCst) {
            let mut gate = UnixStream::connect(&self.gate).map_err(|error| {
                CoreError::Storage(format!("slow disk child gate connect: {error}"))
            })?;
            gate.set_read_timeout(Some(WAIT)).map_err(|error| {
                CoreError::Storage(format!("slow disk child gate deadline: {error}"))
            })?;
            gate.write_all(&[1]).map_err(|error| {
                CoreError::Storage(format!("slow disk child gate entered: {error}"))
            })?;
            let mut release = [0];
            gate.read_exact(&mut release).map_err(|error| {
                CoreError::Storage(format!("slow disk child gate release: {error}"))
            })?;
            if release != [1] {
                return Err(CoreError::Storage(
                    "slow disk child gate released with wrong token".into(),
                ));
            }
            Some(gate)
        } else {
            None
        };
        let bytes = self.inner.track_disk_bytes(budget)?;
        if let Some(gate) = gate.as_mut() {
            gate.write_all(&[2]).map_err(|error| {
                CoreError::Storage(format!("slow disk real adapter completion: {error}"))
            })?;
        }
        Ok(bytes)
    }
}

struct OwnedChild(Option<Child>);

impl OwnedChild {
    fn kill_and_reap(&mut self) -> TestResult {
        if let Some(mut child) = self.0.take() {
            if child.try_wait()?.is_none() {
                child.kill()?;
            }
            let _status = child.wait()?;
        }
        Ok(())
    }

    fn await_clean_exit(&mut self, bound: Duration) -> TestResult {
        let started = Instant::now();
        loop {
            let child = self.0.as_mut().ok_or("slow disk child already reaped")?;
            if let Some(status) = child.try_wait()? {
                let _reaped_child = self.0.take();
                if status.success() {
                    return Ok(());
                }
                return Err(
                    format!("slow disk runtime child exited unsuccessfully: {status}").into(),
                );
            }
            if started.elapsed() >= bound {
                return Err("slow disk runtime child did not stop and join within bound".into());
            }
            thread::sleep(Duration::from_millis(10));
        }
    }
}

impl Drop for OwnedChild {
    fn drop(&mut self) {
        let _cleanup = self.kill_and_reap();
    }
}

fn counter(snapshot: &MetricsSnapshotV1, name: &str) -> Result<u64, Box<dyn Error>> {
    snapshot
        .counters
        .iter()
        .find(|point| point.name == name)
        .map(|point| point.value)
        .ok_or_else(|| format!("required maintenance counter absent: {name}").into())
}

fn active_batch() -> Result<SearchCorpusBatch, Box<dyn Error>> {
    let repo = RepoId::new("repo-slow-disk")?;
    let revision = RevisionId::new("revision-slow-disk")?;
    let path = RepoRelativePath::new("src/needle.rs");
    let text = "fn needle() {}";
    let scope = fixture_source_scope_v1(
        SourceFileKey {
            source_repo_id: repo.clone(),
            repo_relative_path: path.clone(),
        },
        revision.clone(),
        vec![ChunkRecord {
            chunk_id: ChunkId::new("slow-disk-g1-chunk".to_string()),
            repo_relative_path: path,
            language: LanguageCode::new("rust")?,
            start_byte: 0,
            end_byte: u32::try_from(text.len())?,
            start_line: 1,
            end_line: 1,
            text: text.to_owned().into_boxed_str(),
            structural: None,
            parent_chunk_id: None,
            source_repo_id: Some(repo.clone()),
        }],
        Vec::new(),
    )?;
    Ok(SearchCorpusBatch::replace_generation(
        repo,
        revision,
        ManifestGeneration::new(1),
        "manifest:slow-disk-g1",
    )
    .source_event(SourcePublicationEvent {
        stream_id: "fixture:slow-disk".to_string(),
        event_id: "fixture:slow-disk:g1".to_string(),
        expected_base_event_id: None,
        payload_sha256: [0; 32],
    })
    .replace_scope(
        scope.coverage,
        scope.source_bytes,
        scope.chunks,
        scope.symbols,
    ))
}

fn wait_for_control(root: &Path, child: &mut OwnedChild) -> Result<QuantaIndex, Box<dyn Error>> {
    let started = Instant::now();
    loop {
        if let Ok(client) = QuantaIndex::connect(ConnectOptions::from_state_root(root))
            && client.observability().metrics_snapshot().is_ok()
        {
            return Ok(client);
        }
        if let Some(status) = child
            .0
            .as_mut()
            .ok_or("slow disk child already reaped")?
            .try_wait()?
        {
            return Err(format!("slow disk child exited before control UDS: {status}").into());
        }
        if started.elapsed() >= WAIT {
            return Err("slow disk child control UDS did not become ready".into());
        }
        thread::sleep(Duration::from_millis(10));
    }
}

fn wait_for_gate(
    listener: &UnixListener,
    child: &mut OwnedChild,
) -> Result<UnixStream, Box<dyn Error>> {
    listener.set_nonblocking(true)?;
    let started = Instant::now();
    loop {
        match listener.accept() {
            Ok((mut gate, _address)) => {
                gate.set_nonblocking(false)
                    .map_err(|error| format!("slow disk parent gate blocking mode: {error}"))?;
                gate.set_read_timeout(Some(WAIT))
                    .map_err(|error| format!("slow disk parent gate read deadline: {error}"))?;
                let mut entered = [0];
                gate.read_exact(&mut entered)
                    .map_err(|error| format!("slow disk parent gate entry read: {error}"))?;
                if entered != [1] {
                    return Err("slow disk child gate entered with wrong token".into());
                }
                return Ok(gate);
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(error) => return Err(error.into()),
        }
        if let Some(status) = child
            .0
            .as_mut()
            .ok_or("slow disk child already reaped")?
            .try_wait()?
        {
            return Err(format!("slow disk child exited before meter gate: {status}").into());
        }
        if started.elapsed() >= WAIT {
            return Err("slow disk meter never reached the controlled port".into());
        }
        thread::sleep(Duration::from_millis(10));
    }
}

fn run_child() -> TestResult {
    let root = PathBuf::from(std::env::var_os(CHILD_ROOT_ENV).ok_or("child root missing")?);
    let gate = PathBuf::from(std::env::var_os(CHILD_GATE_ENV).ok_or("child gate missing")?);
    let arm = PathBuf::from(std::env::var_os(CHILD_ARM_ENV).ok_or("child arm missing")?);
    let stop = PathBuf::from(std::env::var_os(CHILD_STOP_ENV).ok_or("child stop missing")?);
    let config = SearchdConfig::from_test_state_root(root)
        .with_maintenance_policy(MaintenancePolicy::new(CADENCE)?)
        .try_with_search_corpus_history_retention_limits(
            8,
            16 * 1024 * 1024,
            128,
            256 * 1024 * 1024,
        )?;
    let runtime = build_runtime_with_parts(
        config,
        Arc::new(KernelResidentMemoryProbe),
        |builder| builder,
        move |parts| {
            let real_disk = Arc::clone(&parts.lexical_disk_usage);
            parts.lexical_disk_usage = Arc::new(PausedActualDiskUsage {
                inner: real_disk,
                gate,
                arm,
                entered: AtomicBool::new(false),
            });
        },
    )?;
    let shutdown = Arc::new(AtomicBool::new(false));
    let signal = Arc::clone(&shutdown);
    let monitor = thread::spawn(move || -> Result<(), String> {
        let started = Instant::now();
        while !stop.exists() {
            if started.elapsed() >= Duration::from_secs(20) {
                signal.store(true, Ordering::Release);
                return Err("parent did not release and stop the slow disk child".into());
            }
            thread::sleep(Duration::from_millis(10));
        }
        signal.store(true, Ordering::Release);
        Ok(())
    });
    let result = quanta_index_searchd::drive(runtime, &shutdown);
    shutdown.store(true, Ordering::Release);
    monitor
        .join()
        .map_err(|panic| format!("slow disk stop monitor panicked: {panic:?}"))??;
    result?;
    Ok(())
}

/// The meter is held for at least five 100ms health ticks in a separate OS
/// process.
///
/// Control UDS must keep answering that the active physical backend
/// and maintenance heartbeat are ready; the meter must record skipped work.
#[test]
fn os_child_slow_disk_port_does_not_stale_active_readiness() -> TestResult {
    if std::env::var_os(CHILD_ROOT_ENV).is_some() {
        return run_child();
    }
    let parent = private_tempdir()?;
    let state_root = parent.path().join("state");
    let gate_path = parent.path().join("g.sock");
    let arm_path = parent.path().join("arm");
    let stop_path = parent.path().join("stop");
    let listener = UnixListener::bind(&gate_path)?;
    let child = Command::new(std::env::current_exe()?)
        .arg("--exact")
        .arg("process_slow_disk_tests::os_child_slow_disk_port_does_not_stale_active_readiness")
        .arg("--nocapture")
        .env(CHILD_ROOT_ENV, &state_root)
        .env(CHILD_GATE_ENV, &gate_path)
        .env(CHILD_ARM_ENV, &arm_path)
        .env(CHILD_STOP_ENV, &stop_path)
        .spawn()?;
    let mut child = OwnedChild(Some(child));
    let client = wait_for_control(&state_root, &mut child)?;
    let (_publish, activation) = client
        .search_corpus()
        .publish_and_activate(&active_batch()?, None)?;
    if activation.active.generation.lexical.manifest_generation != ManifestGeneration::new(1)
        || activation.active.generation.semantic.manifest_generation != ManifestGeneration::new(1)
    {
        return Err(format!("slow disk G1 did not activate both tracks: {activation:?}").into());
    }
    std::fs::write(&arm_path, b"arm")?;
    let mut gate = wait_for_gate(&listener, &mut child)?;
    let baseline = client.observability().metrics_snapshot()?;
    let baseline_ticks = counter(&baseline, "maintenance_ticks_total")?;
    let baseline_refreshes = counter(&baseline, "maintenance_disk_refreshes_total")?;
    let baseline_failures = counter(&baseline, "maintenance_disk_refresh_failures_total")?;
    let baseline_skipped = counter(&baseline, "maintenance_disk_refresh_skipped_total")?;
    let started = Instant::now();
    loop {
        let snapshot = client.observability().metrics_snapshot()?;
        let ticks = counter(&snapshot, "maintenance_ticks_total")?;
        let refreshes = counter(&snapshot, "maintenance_disk_refreshes_total")?;
        let skipped = counter(&snapshot, "maintenance_disk_refresh_skipped_total")?;
        if refreshes != baseline_refreshes {
            return Err("controlled meter completed a disk refresh before release".into());
        }
        if ticks >= baseline_ticks + 5 && skipped > baseline_skipped {
            let readiness = client.observability().process_readiness()?;
            if !readiness.ready
                || readiness.active_repositories != 1
                || !readiness.components.maintenance_heartbeat
                || readiness.active_candidate_integrity != Some(true)
            {
                return Err(
                    format!("slow disk port made active process unready: {readiness:?}").into(),
                );
            }
            break;
        }
        if started.elapsed() >= Duration::from_secs(5) {
            return Err(format!(
                "five maintenance ticks or skipped meter work absent: ticks={ticks} skipped={skipped}"
            ).into());
        }
        thread::sleep(Duration::from_millis(10));
    }
    gate.write_all(&[1])
        .map_err(|error| format!("slow disk parent gate release write: {error}"))?;
    let mut actual_adapter_completed = [0];
    gate.read_exact(&mut actual_adapter_completed)
        .map_err(|error| format!("slow disk parent actual adapter completion read: {error}"))?;
    if actual_adapter_completed != [2] {
        return Err("slow disk child never completed the real adapter walk".into());
    }
    let started = Instant::now();
    loop {
        let snapshot = client.observability().metrics_snapshot()?;
        if counter(&snapshot, "maintenance_disk_refresh_failures_total")? != baseline_failures {
            return Err("real disk adapter failed after the controlled delay".into());
        }
        if counter(&snapshot, "maintenance_disk_refreshes_total")? > baseline_refreshes {
            break;
        }
        if started.elapsed() >= WAIT {
            return Err("real disk adapter did not finish after meter gate release".into());
        }
        thread::sleep(Duration::from_millis(10));
    }
    std::fs::write(&stop_path, b"stop")?;
    child.await_clean_exit(Duration::from_secs(5))
}
