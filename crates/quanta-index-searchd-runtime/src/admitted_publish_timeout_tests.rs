//! Real UDS timeout after ingest admission, followed by durable journal replay.
//!
//! The gate wraps only this crate's lexical build port in a unit-test runtime.
//! Production composition, protocol, and daemon environment have no gate.

use std::error::Error;
use std::io::{Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use quanta_index_catalog::SqliteCatalog;
use quanta_index_contract::{
    BatchIngestMode, IngestOperationKindV1, SearchCorpusIngestBatch, SearchPlaneErrorCodeV2,
    SearchPlaneIngestIpcRequest, SearchPlaneIngestIpcRequestEnvelope, SearchPlaneIngestIpcResponse,
    SearchPlaneIngestIpcResponseEnvelope,
};
use quanta_index_core::{
    CoreError, IdempotencyCatalogPort as _, IdempotencyKeyV1, OperationInspectV1,
    SearchCorpusBatchBuildPort, SearchCorpusPreflightPhaseV1,
};
use quanta_index_ipc::{
    ClientIoPolicy, IpcError, IpcIoOperation, send_request, stamp_batch_digest_v1,
};
use quanta_index_sdk::{ConnectOptions, QuantaIndex, SdkError, SearchCorpusBatch};
use quanta_index_searchd::app::KernelResidentMemoryProbe;
use quanta_index_searchd_harness::{E2eRuntime, private_tempdir};

use super::{SearchdConfig, build_runtime_with_lexical_builder};

type TestResult = Result<(), Box<dyn Error>>;

// Keep the original thread panic payload in the test error. String payloads
// remain readable; other payloads retain their concrete type for inspection.
struct ThreadPanic {
    worker: &'static str,
    payload: Box<dyn std::any::Any + Send + 'static>,
}

impl std::fmt::Display for ThreadPanic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(message) = self
            .payload
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| self.payload.downcast_ref::<&str>().copied())
        {
            write!(f, "{} panicked: {message}", self.worker)
        } else {
            write!(
                f,
                "{} panicked with non-string payload of type {:?}",
                self.worker,
                self.payload.as_ref().type_id()
            )
        }
    }
}

impl std::fmt::Debug for ThreadPanic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(self, f)
    }
}

impl Error for ThreadPanic {}
const WAIT: Duration = Duration::from_secs(10);
const CLIENT_TIMEOUT: Duration = Duration::from_secs(2);
const CHILD_GATE_TIMEOUT: Duration = Duration::from_secs(45);
const OS_CHILD_ROOT_ENV: &str = "QI_E3_05_ADMITTED_CHILD_ROOT";
const OS_CHILD_GATE_ENV: &str = "QI_E3_05_ADMITTED_CHILD_GATE";

/// The child runs the same runtime composition over actual lexical and
/// semantic disk adapters. Only its lexical build port is wrapped. The gate
/// is reached after the operation journal admits the publish.
struct ProcessPausedLexicalBuild {
    inner: Arc<dyn SearchCorpusBatchBuildPort + Send + Sync>,
    gate: PathBuf,
}

impl SearchCorpusBatchBuildPort for ProcessPausedLexicalBuild {
    fn preflight_batch(
        &self,
        batch: &SearchCorpusIngestBatch,
        phase: SearchCorpusPreflightPhaseV1,
    ) -> Result<(), CoreError> {
        self.inner.preflight_batch(batch, phase)
    }

    fn build_batch(
        &self,
        batch: &SearchCorpusIngestBatch,
    ) -> Result<Option<quanta_index_contract::LexicalBuildStageDurationsV1>, CoreError> {
        let mut gate = UnixStream::connect(&self.gate)
            .map_err(|error| CoreError::Storage(format!("OS child build gate connect: {error}")))?;
        gate.set_read_timeout(Some(CHILD_GATE_TIMEOUT))
            .map_err(|error| {
                CoreError::Storage(format!("OS child build gate deadline: {error}"))
            })?;
        gate.write_all(&[1])
            .map_err(|error| CoreError::Storage(format!("OS child build gate entered: {error}")))?;
        let mut release = [0];
        gate.read_exact(&mut release)
            .map_err(|error| CoreError::Storage(format!("OS child build gate release: {error}")))?;
        if release != [1] {
            return Err(CoreError::Storage(
                "OS child build gate released with wrong token".into(),
            ));
        }
        self.inner.build_batch(batch)
    }
}

struct OwnedTestChild(Option<Child>);

impl OwnedTestChild {
    fn stop(&mut self) -> TestResult {
        if let Some(mut child) = self.0.take() {
            if child.try_wait()?.is_none() {
                child.kill()?;
            }
            let _status = child.wait()?;
        }
        Ok(())
    }
}

impl Drop for OwnedTestChild {
    fn drop(&mut self) {
        let _cleanup = self.stop();
    }
}

struct PausedLexicalBuild {
    inner: Arc<dyn SearchCorpusBatchBuildPort + Send + Sync>,
    entered: mpsc::Sender<()>,
    release: Mutex<mpsc::Receiver<()>>,
    builds: Arc<AtomicUsize>,
}

impl SearchCorpusBatchBuildPort for PausedLexicalBuild {
    fn preflight_batch(
        &self,
        batch: &SearchCorpusIngestBatch,
        phase: SearchCorpusPreflightPhaseV1,
    ) -> Result<(), CoreError> {
        self.inner.preflight_batch(batch, phase)
    }

    fn build_batch(
        &self,
        batch: &SearchCorpusIngestBatch,
    ) -> Result<Option<quanta_index_contract::LexicalBuildStageDurationsV1>, CoreError> {
        let _previous = self.builds.fetch_add(1, Ordering::SeqCst);
        self.entered.send(()).map_err(|error| {
            CoreError::Storage(format!("admitted build observer lost: {error}"))
        })?;
        self.release
            .lock()
            .map_err(|error| CoreError::Storage(format!("admitted build gate poisoned: {error}")))?
            .recv_timeout(WAIT)
            .map_err(|error| CoreError::Storage(format!("admitted build gate expired: {error}")))?;
        self.inner.build_batch(batch)
    }
}

struct CountingLexicalBuild {
    inner: Arc<dyn SearchCorpusBatchBuildPort + Send + Sync>,
    builds: Arc<AtomicUsize>,
}

impl SearchCorpusBatchBuildPort for CountingLexicalBuild {
    fn preflight_batch(
        &self,
        batch: &SearchCorpusIngestBatch,
        phase: SearchCorpusPreflightPhaseV1,
    ) -> Result<(), CoreError> {
        self.inner.preflight_batch(batch, phase)
    }

    fn build_batch(
        &self,
        batch: &SearchCorpusIngestBatch,
    ) -> Result<Option<quanta_index_contract::LexicalBuildStageDurationsV1>, CoreError> {
        let _previous = self.builds.fetch_add(1, Ordering::SeqCst);
        self.inner.build_batch(batch)
    }
}

struct RunningRuntime {
    shutdown: Arc<AtomicBool>,
    join: Option<JoinHandle<anyhow::Result<()>>>,
}

impl RunningRuntime {
    fn start(runtime: quanta_index_searchd::SearchdRuntime) -> Self {
        let shutdown = Arc::new(AtomicBool::new(false));
        let signal = Arc::clone(&shutdown);
        let join = thread::spawn(move || quanta_index_searchd::drive(runtime, &signal));
        Self {
            shutdown,
            join: Some(join),
        }
    }

    fn stop(mut self) -> TestResult {
        self.shutdown.store(true, Ordering::Release);
        let join = self.join.take().ok_or("runtime driver already joined")?;
        join.join().map_err(|payload| ThreadPanic {
            worker: "runtime driver",
            payload,
        })??;
        Ok(())
    }
}

impl Drop for RunningRuntime {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Release);
        if let Some(join) = self.join.take() {
            let _outcome = join.join();
        }
    }
}

fn operation_key(batch: &SearchCorpusIngestBatch) -> IdempotencyKeyV1 {
    IdempotencyKeyV1 {
        kind: IngestOperationKindV1::SearchCorpus,
        repo_id: batch.repo_id.clone(),
        revision_id: batch.revision_id.clone(),
        generation: batch.generation,
        batch_digest: batch.batch_digest.clone(),
    }
}

fn ingest(
    socket: &std::path::Path,
    request_id: u64,
    batch: SearchCorpusIngestBatch,
    policy: ClientIoPolicy,
) -> Result<SearchPlaneIngestIpcResponse, IpcError> {
    let request = SearchPlaneIngestIpcRequestEnvelope {
        request_id,
        payload: SearchPlaneIngestIpcRequest::PublishSearchCorpusBatch(batch),
    };
    let response: SearchPlaneIngestIpcResponseEnvelope = send_request(socket, &request, policy)?;
    Ok(response.payload)
}

fn wait_for_control(root: &std::path::Path) -> Result<QuantaIndex, Box<dyn Error>> {
    let started = Instant::now();
    loop {
        if let Ok(client) = QuantaIndex::connect(ConnectOptions::from_state_root(root))
            && client.observability().metrics_snapshot().is_ok()
        {
            return Ok(client);
        }
        if started.elapsed() >= WAIT {
            return Err("runtime control UDS did not become ready".into());
        }
        thread::sleep(Duration::from_millis(10));
    }
}

fn wait_for_peer_hangup(client: &QuantaIndex) -> TestResult {
    let started = Instant::now();
    loop {
        let snapshot = client.observability().metrics_snapshot()?;
        if snapshot
            .counters
            .iter()
            .any(|point| point.name == "ipc_ingest_peer_hangup_detected_total" && point.value > 0)
        {
            return Ok(());
        }
        if started.elapsed() >= WAIT {
            return Err("peer watch did not observe the timed-out ingest client's hangup".into());
        }
        thread::sleep(Duration::from_millis(10));
    }
}

fn wait_for_committed(
    catalog: &SqliteCatalog,
    key: &IdempotencyKeyV1,
) -> Result<quanta_index_contract::BatchPublishReceipt, Box<dyn Error>> {
    let started = Instant::now();
    loop {
        match catalog.inspect(key)? {
            OperationInspectV1::Committed {
                receipt,
                durable_sequence,
            } => return Ok(receipt.recorded_at(durable_sequence)),
            state @ (OperationInspectV1::Absent
            | OperationInspectV1::CommittedRepoMap { .. }
            | OperationInspectV1::Refused { .. }
            | OperationInspectV1::Uncertain { .. }) => {
                return Err(format!("admitted publish did not commit: {state:?}").into());
            }
            OperationInspectV1::InFlight { .. } => {}
        }
        if started.elapsed() >= WAIT {
            return Err("admitted publish did not settle before the journal wait bound".into());
        }
        thread::sleep(Duration::from_millis(10));
    }
}

fn test_config(root: &std::path::Path) -> Result<SearchdConfig, Box<dyn Error>> {
    Ok(SearchdConfig::from_test_state_root(root.to_path_buf())
        .try_with_search_corpus_history_retention_limits(
            8,
            16 * 1024 * 1024,
            128,
            256 * 1024 * 1024,
        )?)
}

fn sdk_batch_for_wire(
    batch: &SearchCorpusIngestBatch,
) -> Result<SearchCorpusBatch, Box<dyn Error>> {
    if !batch.seal || batch.bundle_payload.is_some() {
        return Err(
            "default-deadline fixture requires a sealed source batch without a bundle".into(),
        );
    }
    let mut sdk = match batch.mode {
        BatchIngestMode::ReplaceGeneration if batch.base_generation.is_none() => {
            SearchCorpusBatch::replace_generation(
                batch.repo_id.clone(),
                batch.revision_id.clone(),
                batch.generation,
                batch.manifest_digest.clone(),
            )
        }
        BatchIngestMode::Delta => SearchCorpusBatch::delta(
            batch.repo_id.clone(),
            batch.revision_id.clone(),
            batch.generation,
            batch
                .base_generation
                .ok_or("delta batch has no base generation")?,
            batch.manifest_digest.clone(),
        ),
        BatchIngestMode::ReplaceGeneration => {
            return Err("replace generation batch unexpectedly names a base".into());
        }
    }
    .source_event(batch.source_event.clone());
    for surface in &batch.clear_surfaces {
        sdk = sdk.clear_surface(*surface);
    }
    for scope in &batch.replace_scopes {
        sdk = sdk.replace_scope(
            scope.coverage.clone(),
            scope.source_bytes.clone(),
            scope.chunks.clone(),
            scope.symbols.clone(),
        );
    }
    for scope in &batch.tombstone_scopes {
        sdk = sdk.tombstone_scope(scope.file.clone());
    }
    for scope in &batch.semantic_replace_scopes {
        sdk = sdk.replace_semantic_scope(
            scope.scope.clone(),
            scope.scope_digest.clone(),
            scope.sources.clone(),
            scope.cluster_memberships.clone(),
        );
    }
    for scope in &batch.semantic_tombstone_scopes {
        sdk = sdk.tombstone_semantic_scope(scope.clone());
    }
    if sdk.batch_digest()? != batch.batch_digest {
        return Err("SDK builder changed the frozen wire fixture's canonical digest".into());
    }
    Ok(sdk)
}

fn wait_for_os_child_build_gate(
    listener: &UnixListener,
    child: &mut OwnedTestChild,
) -> Result<UnixStream, Box<dyn Error>> {
    listener.set_nonblocking(true)?;
    let started = Instant::now();
    loop {
        match listener.accept() {
            Ok((mut gate, _address)) => {
                gate.set_nonblocking(false)
                    .map_err(|error| format!("OS child parent gate blocking mode: {error}"))?;
                gate.set_read_timeout(Some(WAIT))
                    .map_err(|error| format!("OS child parent gate read deadline: {error}"))?;
                let mut entered = [0];
                gate.read_exact(&mut entered)
                    .map_err(|error| format!("OS child parent gate entry read: {error}"))?;
                if entered != [1] {
                    return Err("OS child reached the build gate with wrong token".into());
                }
                return Ok(gate);
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(error) => return Err(error.into()),
        }
        if let Some(status) = child
            .0
            .as_mut()
            .ok_or("OS child already reaped")?
            .try_wait()?
        {
            return Err(format!("OS child exited before admitted build gate: {status}").into());
        }
        if started.elapsed() >= WAIT {
            return Err("OS child did not reach admitted build gate".into());
        }
        thread::sleep(Duration::from_millis(10));
    }
}

fn run_os_child_build_gate() -> TestResult {
    let root =
        PathBuf::from(std::env::var_os(OS_CHILD_ROOT_ENV).ok_or("child state root missing")?);
    let gate =
        PathBuf::from(std::env::var_os(OS_CHILD_GATE_ENV).ok_or("child build gate missing")?);
    let runtime = build_runtime_with_lexical_builder(
        test_config(&root)?,
        Arc::new(KernelResidentMemoryProbe),
        move |inner| Arc::new(ProcessPausedLexicalBuild { inner, gate }),
    )?;
    let shutdown = Arc::new(AtomicBool::new(false));
    quanta_index_searchd::drive(runtime, &shutdown)?;
    Err("OS child runtime returned before parent terminated it".into())
}

fn remove_dead_child_sockets(root: &Path) -> TestResult {
    for name in ["query.sock", "control.sock", "ingest.sock"] {
        let socket = root.join("search-plane").join(name);
        match std::fs::remove_file(socket) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

/// The same admission and durable replay contract across an actual OS
/// process boundary, using the SDK's unchanged 30-second client deadline.
///
/// The child owns the real catalog and disk adapters; its test-only build
/// wrapper supplies only the deterministic admitted-work barrier.
#[test]
fn default_sdk_timeout_in_os_child_still_commits_and_replays_without_rebuild() -> TestResult {
    if std::env::var_os(OS_CHILD_ROOT_ENV).is_some() {
        return run_os_child_build_gate();
    }
    let parent = private_tempdir()?;
    let fixture_root = parent.path().join("fixture");
    let state_root = parent.path().join("state");
    let fixture = E2eRuntime::boot_in(&fixture_root)?;
    let batch = fixture.text_search_corpus_batch(
        "src/process_timeout.rs",
        "fn right_token() { admitted_os_child_timeout_replay }",
    )?;
    let sdk_batch = sdk_batch_for_wire(&batch)?;
    let key = operation_key(&batch);
    let gate_path = parent.path().join("g.sock");
    let listener = UnixListener::bind(&gate_path)?;
    let current_exe = std::env::current_exe()?;
    let child = Command::new(current_exe)
        .arg("--exact")
        .arg("admitted_publish_timeout_tests::default_sdk_timeout_in_os_child_still_commits_and_replays_without_rebuild")
        .arg("--nocapture")
        .env(OS_CHILD_ROOT_ENV, &state_root)
        .env(OS_CHILD_GATE_ENV, &gate_path)
        .spawn()?;
    let mut child = OwnedTestChild(Some(child));
    let initial_control = wait_for_control(&state_root)?;
    let catalog = SqliteCatalog::open(&state_root, Duration::from_secs(2))?;
    let caller_root = state_root.clone();
    let caller = thread::spawn(move || -> Result<_, SdkError> {
        let client = QuantaIndex::connect(ConnectOptions::from_state_root(caller_root))?;
        client.search_corpus().publish(&sdk_batch)
    });
    let mut gate = wait_for_os_child_build_gate(&listener, &mut child)?;
    if !matches!(catalog.inspect(&key)?, OperationInspectV1::InFlight { .. }) {
        return Err("OS child build gate was not inside an admitted journal operation".into());
    }
    let timed_out = caller.join().map_err(|payload| ThreadPanic {
        worker: "default SDK caller",
        payload,
    })?;
    if !matches!(
        &timed_out,
        Err(SdkError::Transport(IpcError::Timeout {
            operation: IpcIoOperation::Read,
            timeout,
        })) if *timeout == quanta_index_ipc::DEFAULT_CLIENT_IO_TIMEOUT
    ) {
        return Err(format!(
            "default SDK deadline did not return typed read timeout: {timed_out:?}"
        )
        .into());
    }
    let control = wait_for_control(&state_root)?;
    wait_for_peer_hangup(&control)?;
    gate.write_all(&[1])
        .map_err(|error| format!("OS child parent gate release write: {error}"))?;
    let original = wait_for_committed(&catalog, &key)?;
    if !original.applied || original.durable_sequence == 0 {
        return Err(format!("timed-out OS child publish did not apply: {original:?}").into());
    }
    drop(control);
    drop(initial_control);
    drop(catalog);
    child.stop()?;
    remove_dead_child_sockets(&state_root)?;

    let reopened_builds = Arc::new(AtomicUsize::new(0));
    let counted_builds = Arc::clone(&reopened_builds);
    let second_runtime = build_runtime_with_lexical_builder(
        test_config(&state_root)?,
        Arc::new(KernelResidentMemoryProbe),
        move |inner| {
            Arc::new(CountingLexicalBuild {
                inner,
                builds: counted_builds,
            })
        },
    )?;
    let second_driver = RunningRuntime::start(second_runtime);
    let replay_client = wait_for_control(&state_root)?;
    let reopened_catalog = SqliteCatalog::open(&state_root, Duration::from_secs(2))?;
    if wait_for_committed(&reopened_catalog, &key)? != original {
        return Err("OS process termination changed the committed receipt".into());
    }
    let replay = replay_client
        .search_corpus()
        .publish(&sdk_batch_for_wire(&batch)?)?;
    if replay != original.clone().replayed() || reopened_builds.load(Ordering::SeqCst) != 0 {
        return Err(
            format!("SDK replay rebuilt or changed the committed result: {replay:?}").into(),
        );
    }
    let mut conflicting = fixture.text_search_corpus_batch(
        "src/process_timeout.rs",
        "fn wrong_token() { admitted_os_child_timeout_replay }",
    )?;
    conflicting.source_event.stream_id = batch.source_event.stream_id.clone();
    conflicting.source_event.event_id = batch.source_event.event_id.clone();
    conflicting.source_event.expected_base_event_id = batch.source_event.expected_base_event_id;
    stamp_batch_digest_v1(&mut conflicting)?;
    let conflict_key = operation_key(&conflicting);
    if conflict_key.batch_digest == key.batch_digest {
        return Err("OS-child conflict fixture did not change its canonical digest".into());
    }
    let socket = state_root.join("search-plane/ingest.sock");
    let refusal = ingest(&socket, 0xe305, conflicting, ClientIoPolicy::default())?;
    if !matches!(
        &refusal,
        SearchPlaneIngestIpcResponse::Error(error)
            if error.code == SearchPlaneErrorCodeV2::BatchDigestConflict
    ) {
        return Err(format!("OS-child replay conflict was not typed: {refusal:?}").into());
    }
    if !matches!(
        reopened_catalog.inspect(&conflict_key)?,
        OperationInspectV1::Absent
    ) || wait_for_committed(&reopened_catalog, &key)? != original
        || reopened_builds.load(Ordering::SeqCst) != 0
    {
        return Err("conflicting source event mutated the durable journal or rebuilt".into());
    }
    second_driver.stop()
}

#[test]
fn timed_out_uds_peer_does_not_cancel_admitted_publish_or_replay_after_runtime_reassembly()
-> TestResult {
    let root = private_tempdir()?;
    let fixture = E2eRuntime::boot_in(root.path())?;
    let batch = fixture.text_search_corpus_batch(
        "src/timeout.rs",
        "fn right_token() { admitted_timeout_replay }",
    )?;
    let key = operation_key(&batch);
    let socket = root.path().join("search-plane/ingest.sock");
    let catalog = SqliteCatalog::open(root.path(), Duration::from_secs(2))?;
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let builds = Arc::new(AtomicUsize::new(0));
    let gated_builds = Arc::clone(&builds);
    let first_runtime = build_runtime_with_lexical_builder(
        test_config(root.path())?,
        Arc::new(KernelResidentMemoryProbe),
        move |inner| {
            Arc::new(PausedLexicalBuild {
                inner,
                entered: entered_tx,
                release: Mutex::new(release_rx),
                builds: gated_builds,
            })
        },
    )?;
    let first_driver = RunningRuntime::start(first_runtime);
    let control = wait_for_control(root.path())?;
    let timed_out_socket = socket.clone();
    let timed_out_batch = batch.clone();
    let short_policy = ClientIoPolicy::try_new(CLIENT_TIMEOUT)?;
    let caller =
        thread::spawn(move || ingest(&timed_out_socket, 101, timed_out_batch, short_policy));
    entered_rx.recv_timeout(WAIT)?;
    if !matches!(catalog.inspect(&key)?, OperationInspectV1::InFlight { .. }) {
        return Err("lexical gate did not hold an applying journal operation".into());
    }
    let timed_out = caller.join().map_err(|payload| ThreadPanic {
        worker: "UDS caller",
        payload,
    })?;
    if !matches!(
        &timed_out,
        Err(IpcError::Timeout {
            operation: IpcIoOperation::Read,
            timeout,
        }) if *timeout == CLIENT_TIMEOUT
    ) {
        return Err(format!("expected typed UDS read timeout, got {timed_out:?}").into());
    }
    wait_for_peer_hangup(&control)?;
    release_tx.send(())?;
    let original = wait_for_committed(&catalog, &key)?;
    if !original.applied || original.durable_sequence == 0 || builds.load(Ordering::SeqCst) != 1 {
        return Err(format!("timed-out original did not apply exactly once: {original:?}").into());
    }
    first_driver.stop()?;
    drop(control);
    drop(catalog);

    let reopened_builds = Arc::new(AtomicUsize::new(0));
    let counted_builds = Arc::clone(&reopened_builds);
    let second_runtime = build_runtime_with_lexical_builder(
        test_config(root.path())?,
        Arc::new(KernelResidentMemoryProbe),
        move |inner| {
            Arc::new(CountingLexicalBuild {
                inner,
                builds: counted_builds,
            })
        },
    )?;
    let second_driver = RunningRuntime::start(second_runtime);
    let _control = wait_for_control(root.path())?;
    let reopened_catalog = SqliteCatalog::open(root.path(), Duration::from_secs(2))?;
    if wait_for_committed(&reopened_catalog, &key)? != original {
        return Err("runtime reassembly changed the committed operation receipt".into());
    }
    let replay = ingest(&socket, 102, batch.clone(), ClientIoPolicy::default())?;
    let SearchPlaneIngestIpcResponse::SearchCorpusReceipt(replay) = replay else {
        return Err(format!("exact replay was not acknowledged: {replay:?}").into());
    };
    if replay.receipt != original.clone().replayed()
        || builds.load(Ordering::SeqCst) != 1
        || reopened_builds.load(Ordering::SeqCst) != 0
    {
        return Err(format!("exact replay changed the original receipt: {replay:?}").into());
    }

    // Reuse the source event identity with a different *canonical* payload,
    // rather than a malformed carried digest. The source authority must
    // reject it before another journal row or build can be created.
    let mut conflicting = fixture.text_search_corpus_batch(
        "src/timeout.rs",
        "fn wrong_token() { admitted_timeout_replay }",
    )?;
    conflicting.source_event.stream_id = batch.source_event.stream_id.clone();
    conflicting.source_event.event_id = batch.source_event.event_id.clone();
    conflicting.source_event.expected_base_event_id = batch.source_event.expected_base_event_id;
    stamp_batch_digest_v1(&mut conflicting)?;
    let conflict_key = operation_key(&conflicting);
    if conflict_key.batch_digest == key.batch_digest {
        return Err("conflicting payload did not change its canonical digest".into());
    }
    let conflict = ingest(&socket, 103, conflicting, ClientIoPolicy::default())?;
    if !matches!(
        &conflict,
        SearchPlaneIngestIpcResponse::Error(error)
            if error.code == SearchPlaneErrorCodeV2::BatchDigestConflict
    ) {
        return Err(format!("source-event digest conflict was not typed: {conflict:?}").into());
    }
    if !matches!(
        reopened_catalog.inspect(&conflict_key)?,
        OperationInspectV1::Absent
    ) || wait_for_committed(&reopened_catalog, &key)? != original
        || builds.load(Ordering::SeqCst) != 1
        || reopened_builds.load(Ordering::SeqCst) != 0
    {
        return Err("digest conflict mutated the journal or re-applied the original".into());
    }
    second_driver.stop()
}
