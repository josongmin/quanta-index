//! Real runtime assembly of one publication admission authority at both gates.
//!
//! The scripted port tests custody and call order. It does not estimate or
//! qualify process memory, I/O, or physical disk capacity.

use std::collections::BTreeMap;
use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use quanta_index_catalog::{CATALOG_FILE_NAME, catalog_dir};
use quanta_index_contract::{
    IngestOperationKindV1, SearchCorpusIngestBatch, SearchPlaneErrorCodeV2,
    SearchPlaneIngestIpcRequest, SearchPlaneIngestIpcRequestEnvelope, SearchPlaneIngestIpcResponse,
    SearchPlaneIngestIpcResponseEnvelope,
};
use quanta_index_core::publication_admission::{
    PublicationAdmissionRequest, PublicationOutputPermit, PublicationOutputReservation,
    PublicationResourceAdmissionPort, PublicationResourcePermit,
};
use quanta_index_core::{
    CoreError, DiagnosticContextV1, IdempotencyKeyV1, OperationInspectV1,
    PublicationValidationOwner, RequestBudgetV1, SearchCorpusBatchBuildPort,
    SearchCorpusPreflightPhaseV1,
};
use quanta_index_ipc::{ClientIoPolicy, send_request};
use quanta_index_sdk::{ConnectOptions, QuantaIndex};
use quanta_index_searchd::app::KernelResidentMemoryProbe;
use quanta_index_searchd_harness::{E2eRuntime, private_tempdir};

use super::{SearchdConfig, build_runtime_with_parts};

type TestResult = Result<(), Box<dyn Error>>;
const WAIT: Duration = Duration::from_secs(10);

#[derive(Clone, Copy)]
enum RefusalPoint {
    Control,
    Work,
}

#[derive(Default)]
struct Calls {
    control: AtomicUsize,
    work: AtomicUsize,
    live_control: AtomicUsize,
    dropped_control: AtomicUsize,
    preflight: AtomicUsize,
    build: AtomicUsize,
}

struct ControlPermit {
    calls: Arc<Calls>,
    inner: Box<dyn PublicationResourcePermit>,
}

impl PublicationResourcePermit for ControlPermit {
    fn held_file_allowance(
        &self,
    ) -> Result<Option<Arc<quanta_index_core::publication_capacity::PublicationAllowance>>, CoreError>
    {
        self.inner.held_file_allowance()
    }

    fn reserve_output(
        &self,
        request: PublicationOutputReservation,
    ) -> Result<Box<dyn PublicationOutputPermit>, CoreError> {
        self.inner.reserve_output(request)
    }
}

impl Drop for ControlPermit {
    fn drop(&mut self) {
        let _previous = self.calls.live_control.fetch_sub(1, Ordering::SeqCst);
        let _previous = self.calls.dropped_control.fetch_add(1, Ordering::SeqCst);
    }
}

struct RefusingAdmission {
    point: RefusalPoint,
    calls: Arc<Calls>,
    inner: Arc<dyn PublicationResourceAdmissionPort>,
}

fn refusal() -> CoreError {
    CoreError::Typed {
        code: SearchPlaneErrorCodeV2::IngestResourceBudgetExceeded,
        message: "scripted publication admission refusal".into(),
    }
}

impl PublicationResourceAdmissionPort for RefusingAdmission {
    fn try_acquire_control(
        &self,
        batch: &SearchCorpusIngestBatch,
        budget: &RequestBudgetV1,
    ) -> Result<Box<dyn PublicationResourcePermit>, CoreError> {
        let _previous = self.calls.control.fetch_add(1, Ordering::SeqCst);
        if matches!(self.point, RefusalPoint::Control) {
            return Err(refusal());
        }
        let inner = self.inner.try_acquire_control(batch, budget)?;
        if self
            .calls
            .live_control
            .compare_exchange(0, 1, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return Err(CoreError::InvalidContract(
                "control permit was already live".into(),
            ));
        }
        Ok(Box::new(ControlPermit {
            calls: Arc::clone(&self.calls),
            inner,
        }))
    }

    fn try_acquire(
        &self,
        _request: &PublicationAdmissionRequest<'_>,
        _budget: &RequestBudgetV1,
    ) -> Result<Box<dyn PublicationResourcePermit>, CoreError> {
        let _previous = self.calls.work.fetch_add(1, Ordering::SeqCst);
        if self.calls.live_control.load(Ordering::SeqCst) != 1 {
            return Err(CoreError::InvalidContract(
                "work gate did not share the held control authority".into(),
            ));
        }
        Err(refusal())
    }
}

struct CountingBuild {
    inner: Arc<dyn SearchCorpusBatchBuildPort + Send + Sync>,
    calls: Arc<Calls>,
}

impl SearchCorpusBatchBuildPort for CountingBuild {
    fn prepare_hard_recovery_scan_with_owner(
        &self,
        batch: &SearchCorpusIngestBatch,
        owner: &mut PublicationValidationOwner,
        budget: &RequestBudgetV1,
        diagnostic: Option<&DiagnosticContextV1>,
    ) -> Result<(), CoreError> {
        self.inner
            .prepare_hard_recovery_scan_with_owner(batch, owner, budget, diagnostic)
    }

    fn preflight_batch(
        &self,
        batch: &SearchCorpusIngestBatch,
        phase: SearchCorpusPreflightPhaseV1,
        diagnostic: Option<&DiagnosticContextV1>,
    ) -> Result<(), CoreError> {
        let _previous = self.calls.preflight.fetch_add(1, Ordering::SeqCst);
        self.inner.preflight_batch(batch, phase, diagnostic)
    }

    fn preflight_batch_with_owner(
        &self,
        batch: &SearchCorpusIngestBatch,
        phase: SearchCorpusPreflightPhaseV1,
        owner: &mut PublicationValidationOwner,
        diagnostic: Option<&DiagnosticContextV1>,
    ) -> Result<(), CoreError> {
        let _previous = self.calls.preflight.fetch_add(1, Ordering::SeqCst);
        self.inner
            .preflight_batch_with_owner(batch, phase, owner, diagnostic)
    }

    fn build_batch(
        &self,
        batch: &SearchCorpusIngestBatch,
        diagnostic: Option<&DiagnosticContextV1>,
    ) -> Result<Option<quanta_index_contract::LexicalBuildStageDurationsV1>, CoreError> {
        let _previous = self.calls.build.fetch_add(1, Ordering::SeqCst);
        self.inner.build_batch(batch, diagnostic)
    }

    fn build_batch_with_owner(
        &self,
        batch: &SearchCorpusIngestBatch,
        owner: &mut PublicationValidationOwner,
        diagnostic: Option<&DiagnosticContextV1>,
    ) -> Result<Option<quanta_index_contract::LexicalBuildStageDurationsV1>, CoreError> {
        let _previous = self.calls.build.fetch_add(1, Ordering::SeqCst);
        self.inner.build_batch_with_owner(batch, owner, diagnostic)
    }
}

struct RunningRuntime {
    stop: Arc<AtomicBool>,
    driver: Option<JoinHandle<anyhow::Result<()>>>,
}

impl RunningRuntime {
    fn start(runtime: quanta_index_searchd::SearchdRuntime) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let signal = Arc::clone(&stop);
        let driver = thread::spawn(move || quanta_index_searchd::drive(runtime, &signal));
        Self {
            stop,
            driver: Some(driver),
        }
    }

    fn stop(mut self) -> TestResult {
        self.stop.store(true, Ordering::Release);
        let driver = self.driver.take().ok_or("runtime driver already joined")?;
        driver
            .join()
            .map_err(|_panic| "runtime driver panicked")??;
        Ok(())
    }
}

impl Drop for RunningRuntime {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(driver) = self.driver.take() {
            let _joined = driver.join();
        }
    }
}

fn ready(root: &Path) -> Result<QuantaIndex, Box<dyn Error>> {
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

fn journal_key(batch: &SearchCorpusIngestBatch) -> IdempotencyKeyV1 {
    IdempotencyKeyV1 {
        kind: IngestOperationKindV1::SearchCorpus,
        repo_id: batch.repo_id.clone(),
        revision_id: batch.revision_id.clone(),
        generation: batch.generation,
        batch_digest: batch.batch_digest.clone(),
    }
}

fn journal_rows(root: &Path) -> Result<i64, Box<dyn Error>> {
    // Independent observation must not checkpoint or initialize the live DB
    // outside its original funded VFS.
    let connection = rusqlite::Connection::open_with_flags(
        catalog_dir(root).join(CATALOG_FILE_NAME),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )?;
    Ok(connection.query_row("SELECT COUNT(*) FROM idempotency_v2", [], |row| row.get(0))?)
}

fn activation_files(root: &Path) -> Result<BTreeMap<PathBuf, Vec<u8>>, Box<dyn Error>> {
    fn visit(
        base: &Path,
        path: &Path,
        files: &mut BTreeMap<PathBuf, Vec<u8>>,
    ) -> Result<(), Box<dyn Error>> {
        for entry in fs::read_dir(path)? {
            let entry = entry?;
            let kind = entry.file_type()?;
            if kind.is_dir() {
                visit(base, &entry.path(), files)?;
            } else if kind.is_file() {
                let relative = entry.path().strip_prefix(base)?.to_path_buf();
                let _previous = files.insert(relative, fs::read(entry.path())?);
            } else {
                return Err("unexpected activation catalog entry kind".into());
            }
        }
        Ok(())
    }
    let base = root.join("activations");
    let mut files = BTreeMap::new();
    visit(&base, &base, &mut files)?;
    Ok(files)
}

fn run_refusal(point: RefusalPoint) -> TestResult {
    let directory = private_tempdir()?;
    let fixture_root = directory.path().join("fixture");
    let state_root = directory.path().join("state");
    let fixture = E2eRuntime::boot_in(&fixture_root)?;
    let batch = fixture.text_search_corpus_batch(
        "src/admission.rs",
        "fn admission_fixture() { truthful_publication_boundary }",
    )?;
    let key = journal_key(&batch);
    let calls = Arc::new(Calls::default());
    let admission_calls = Arc::clone(&calls);
    let build_calls = Arc::clone(&calls);
    let mut catalog = None;
    let runtime = build_runtime_with_parts(
        SearchdConfig::from_test_state_root(state_root.clone())
            .try_with_search_corpus_history_retention_limits(
                8,
                16 * 1024 * 1024,
                128,
                256 * 1024 * 1024,
            )?,
        Arc::new(KernelResidentMemoryProbe),
        move |inner| {
            Arc::new(CountingBuild {
                inner,
                calls: build_calls,
            })
        },
        |parts| {
            catalog = Some(Arc::clone(&parts.idempotency));
            parts.publication_admission = Arc::new(RefusingAdmission {
                point,
                calls: admission_calls,
                inner: Arc::clone(&parts.publication_admission),
            });
        },
    )?;
    let driver = RunningRuntime::start(runtime);
    let _client = ready(&state_root)?;
    let catalog = catalog.ok_or("runtime did not lend its journal interface")?;
    let rows_before = journal_rows(&state_root)?;
    let activation_before = activation_files(&state_root)?;
    let request = SearchPlaneIngestIpcRequestEnvelope {
        diagnostic_attempt: quanta_index_contract::DiagnosticAttemptFieldV1::Absent,
        request_id: 0xa11d,
        payload: SearchPlaneIngestIpcRequest::PublishSearchCorpusBatch(batch),
    };
    let response: SearchPlaneIngestIpcResponseEnvelope = send_request(
        &state_root.join("search-plane/ingest.sock"),
        &request,
        ClientIoPolicy::default(),
    )?;
    if !matches!(
        &response.payload,
        SearchPlaneIngestIpcResponse::Error(error)
            if error.code == SearchPlaneErrorCodeV2::IngestResourceBudgetExceeded
    ) {
        return Err(format!(
            "admission refusal did not remain typed: {:?}",
            response.payload
        )
        .into());
    }
    if activation_files(&state_root)? != activation_before
        || calls.build.load(Ordering::SeqCst) != 0
        || calls.live_control.load(Ordering::SeqCst) != 0
    {
        return Err(
            "refusal changed source/version authority, built an index, or leaked permit".into(),
        );
    }
    match point {
        RefusalPoint::Control => {
            if journal_rows(&state_root)? != rows_before
                || !matches!(catalog.inspect(&key)?, OperationInspectV1::Absent)
                || calls.control.load(Ordering::SeqCst) != 1
                || calls.work.load(Ordering::SeqCst) != 0
                || calls.preflight.load(Ordering::SeqCst) != 0
                || calls.dropped_control.load(Ordering::SeqCst) != 0
            {
                return Err("control refusal crossed journal or preflight boundary".into());
            }
        }
        RefusalPoint::Work => {
            if journal_rows(&state_root)?
                != rows_before
                    .checked_add(1)
                    .ok_or("journal row count overflow")?
                || !matches!(catalog.inspect(&key)?, OperationInspectV1::Uncertain { .. })
                || calls.control.load(Ordering::SeqCst) != 1
                || calls.work.load(Ordering::SeqCst) != 1
                || calls.dropped_control.load(Ordering::SeqCst) != 1
                || calls.preflight.load(Ordering::SeqCst) == 0
            {
                return Err(
                    "work refusal did not settle original journal and control permit".into(),
                );
            }
        }
    }
    drop(catalog);
    driver.stop()
}

#[test]
fn control_refusal_precedes_journal_and_materialization_in_real_runtime() -> TestResult {
    run_refusal(RefusalPoint::Control)
}

#[test]
fn work_refusal_uses_same_authority_and_settles_uncertain_journal() -> TestResult {
    run_refusal(RefusalPoint::Work)
}
