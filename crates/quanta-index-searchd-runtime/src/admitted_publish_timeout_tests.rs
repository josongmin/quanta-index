//! Real UDS timeout after ingest admission, followed by durable journal replay.
//!
//! The gate wraps only this crate's lexical build port in a unit-test runtime.
//! Production composition, protocol, and daemon environment have no gate.

use std::error::Error;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use quanta_index_catalog::SqliteCatalog;
use quanta_index_contract::{
    IngestOperationKindV1, SearchCorpusIngestBatch, SearchPlaneErrorCodeV2,
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
use quanta_index_sdk::{ConnectOptions, QuantaIndex};
use quanta_index_searchd::app::KernelResidentMemoryProbe;
use quanta_index_searchd_harness::E2eRuntime;

use super::{SearchdConfig, build_runtime_with_lexical_builder};

type TestResult = Result<(), Box<dyn Error>>;
const WAIT: Duration = Duration::from_secs(10);
const CLIENT_TIMEOUT: Duration = Duration::from_secs(2);

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
        join.join().map_err(|_| "runtime driver panicked")??;
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

#[test]
fn timed_out_uds_peer_does_not_cancel_admitted_publish_or_replay_after_runtime_reassembly()
-> TestResult {
    let root = tempfile::tempdir()?;
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
    let timed_out = caller.join().map_err(|_| "UDS caller panicked")?;
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
    conflicting.source_event.expected_base_event_id =
        batch.source_event.expected_base_event_id.clone();
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
