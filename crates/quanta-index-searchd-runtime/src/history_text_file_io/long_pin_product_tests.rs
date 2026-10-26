//! A real history query pins its Tantivy epoch through retention and physical GC.

use std::collections::BTreeSet;
use std::error::Error;
use std::fs;
use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock, mpsc};
use std::time::Duration;

use quanta_index_catalog::{CatalogIoAdmission, SqliteCatalog};
use quanta_index_contract::lex::{CommitRecord, CommitSha};
use quanta_index_contract::{
    AuxEpochV1, GenerationSnapshot, HistoryIngestBatch, HistoryOrderV1, HistoryQueryRequest,
    ManifestGeneration, QueryConstraintSetV1, RepoId, RevisionId, SearchPlaneQueryIpcRequest,
    SearchPlaneQueryIpcResponse, SearchPlaneTrackKind, TextQueryRequest, TextQuerySyntax,
};
use quanta_index_core::domains::structural::{
    StructuralError, StructuralQueryRequest, StructuralReadiness,
};
use quanta_index_core::{
    AuxiliaryGenerationKeyV1, CoreError, DiagnosticContextV1, GenerationStorageKeyV1,
    HistoryTextBuildV1, HistoryTextDiscardOutcomeV1, HistoryTextEpochReceiptV1,
    HistoryTextEpochStatusV1, HistoryTextHitV1, HistoryTextIndexPort, HistoryTextPageV1,
    HistoryTextQueryV1, HistoryTextSearcher, LexicalIndexOpenPort, LexicalSearcher,
    PinnedRepoMapSnapshot, RepoMapSnapshotAcquirePort, RepoMapSnapshotAcquireV1, RequestBudgetV1,
    SemanticIndexOpenPort, SemanticSearcher, StructuralMatchCandidate, StructuralProducerPort,
    ValidatedLexicalPlan,
};
use quanta_index_lexical::history_text_index::HistoryTextIndexAdapter;
use quanta_index_search_plane::{
    ActivationCatalog, AuxiliaryMaterializerParts, AuxiliaryMutationCoordinator,
    DirectHistoryMaterializer, HistoryIngestPort as _, HistoryTextIndexParts, Ledger,
    SearchPlaneDispatcher, make_pin,
};
use quanta_index_searchd::app::config::TEST_STATE_ROOT_PHYSICAL_POLICY;
use quanta_index_searchd::app::runtime::{RuntimeRetirementPort as _, StateRootLease};

use super::product_tests::disk_census;
use crate::legacy_physical_assembly::LegacyPhysicalAssembly;
use crate::legacy_retirement::LegacyRuntimeRetirement;

type TestResult = Result<(), Box<dyn Error>>;
type QueryPage = quanta_index_contract::SearchPlaneHistoryQueryResponse;
type QueryThread = std::thread::JoinHandle<Result<QueryPage, String>>;

#[derive(Debug)]
struct UnrelatedPort;

impl quanta_index_search_plane::SearchCorpusIndexBytesPort for UnrelatedPort {
    fn measurement_policy(
        &self,
    ) -> quanta_index_core::domains::generation::RetentionMeasurementPolicy {
        quanta_index_core::domains::generation::RetentionMeasurementPolicy::DEFAULT
    }

    fn measure_index_bytes(
        &self,
        _repo_id: &RepoId,
        _revision_id: &RevisionId,
        _generations: &BTreeSet<ManifestGeneration>,
        _budget: &RequestBudgetV1,
        _diagnostic: Option<&DiagnosticContextV1>,
    ) -> Result<quanta_index_core::SealedGenerationBytesV1, CoreError> {
        Err(CoreError::NotImplemented(
            "history fixture has no corpus-retention measurement route".into(),
        ))
    }
}

impl LexicalIndexOpenPort for UnrelatedPort {
    fn preflight_query_primitives(
        &self,
        _plan: &ValidatedLexicalPlan,
        _budget: &RequestBudgetV1,
    ) -> Result<(), CoreError> {
        Err(CoreError::NotImplemented(
            "history test has no lexical route".into(),
        ))
    }

    fn open(
        &self,
        _repo: &RepoId,
        _revision: &RevisionId,
        _generation: ManifestGeneration,
        _budget: &RequestBudgetV1,
    ) -> Result<Box<dyn LexicalSearcher>, CoreError> {
        Err(CoreError::NotImplemented(
            "history test has no lexical route".into(),
        ))
    }

    fn open_proven(
        &self,
        _candidate: &GenerationSnapshot,
    ) -> Result<Box<dyn LexicalSearcher>, CoreError> {
        Err(CoreError::NotImplemented(
            "history test has no lexical route".into(),
        ))
    }
}

impl SemanticIndexOpenPort for UnrelatedPort {
    fn open(
        &self,
        _repo: &RepoId,
        _revision: &RevisionId,
        _generation: ManifestGeneration,
    ) -> Result<Box<dyn SemanticSearcher>, CoreError> {
        Err(CoreError::NotImplemented(
            "history test has no semantic route".into(),
        ))
    }

    fn open_proven(
        &self,
        _candidate: &GenerationSnapshot,
    ) -> Result<Box<dyn SemanticSearcher>, CoreError> {
        Err(CoreError::NotImplemented(
            "history test has no semantic route".into(),
        ))
    }
}

impl RepoMapSnapshotAcquirePort for UnrelatedPort {
    fn acquire(
        &self,
        _acquire: RepoMapSnapshotAcquireV1,
    ) -> Result<Box<dyn PinnedRepoMapSnapshot>, CoreError> {
        Err(CoreError::NotImplemented(
            "history test has no RepoMap route".into(),
        ))
    }
}

impl StructuralProducerPort for UnrelatedPort {
    fn readiness(&self, _request: &StructuralQueryRequest) -> StructuralReadiness {
        StructuralReadiness::ParseTreeProducerUnavailable
    }

    fn execute(
        &self,
        _request: &StructuralQueryRequest,
        _budget: &RequestBudgetV1,
    ) -> Result<Vec<StructuralMatchCandidate>, StructuralError> {
        Err(StructuralError::ProducerExecution(
            "history test has no structural route".into(),
        ))
    }
}

struct QueryGate {
    armed: AtomicBool,
    entered: Mutex<Option<mpsc::Sender<()>>>,
    resume: Mutex<mpsc::Receiver<()>>,
}

struct GatedSearcher {
    inner: Box<dyn HistoryTextSearcher>,
    gate: Arc<QueryGate>,
}

impl HistoryTextSearcher for GatedSearcher {
    fn search(
        &self,
        query: &HistoryTextQueryV1,
        after: Option<&HistoryTextHitV1>,
        limit: usize,
        admit: Arc<quanta_index_core::HistoryTextAdmitFn>,
        budget: &RequestBudgetV1,
    ) -> Result<HistoryTextPageV1, CoreError> {
        if self.gate.armed.swap(false, Ordering::AcqRel) {
            let sender = self
                .gate
                .entered
                .lock()
                .map_err(|error| CoreError::Storage(format!("query gate poisoned: {error}")))?
                .take()
                .ok_or_else(|| CoreError::Storage("query gate sender missing".into()))?;
            sender.send(()).map_err(|error| {
                CoreError::Storage(format!("query gate signal failed: {error}"))
            })?;
            self.gate
                .resume
                .lock()
                .map_err(|error| CoreError::Storage(format!("query gate poisoned: {error}")))?
                .recv()
                .map_err(|error| {
                    CoreError::Storage(format!("query gate resume failed: {error}"))
                })?;
        }
        self.inner.search(query, after, limit, admit, budget)
    }

    fn resident_bytes_estimate(&self) -> u64 {
        self.inner.resident_bytes_estimate()
    }
}

struct GatedHistoryPort {
    inner: Arc<HistoryTextIndexAdapter>,
    gate: Arc<QueryGate>,
}

impl HistoryTextIndexPort for GatedHistoryPort {
    fn epoch_status(
        &self,
        generation: &AuxiliaryGenerationKeyV1,
        epoch: AuxEpochV1,
    ) -> Result<HistoryTextEpochStatusV1, CoreError> {
        self.inner.epoch_status(generation, epoch)
    }

    fn publish_epoch(
        &self,
        generation: &AuxiliaryGenerationKeyV1,
        epoch: AuxEpochV1,
        build: HistoryTextBuildV1,
        budget: &RequestBudgetV1,
    ) -> Result<HistoryTextEpochReceiptV1, CoreError> {
        self.inner.publish_epoch(generation, epoch, build, budget)
    }

    fn open_epoch(
        &self,
        generation: &AuxiliaryGenerationKeyV1,
        epoch: AuxEpochV1,
    ) -> Result<Box<dyn HistoryTextSearcher>, CoreError> {
        let inner = self.inner.open_epoch(generation, epoch)?;
        if epoch == AuxEpochV1::new(1) {
            Ok(Box::new(GatedSearcher {
                inner,
                gate: Arc::clone(&self.gate),
            }))
        } else {
            Ok(inner)
        }
    }

    fn durable_epochs(
        &self,
        generation: &AuxiliaryGenerationKeyV1,
    ) -> Result<Vec<AuxEpochV1>, CoreError> {
        self.inner.durable_epochs(generation)
    }

    fn durable_generations(
        &self,
        repo: &RepoId,
        revision: &RevisionId,
    ) -> Result<Vec<ManifestGeneration>, CoreError> {
        self.inner.durable_generations(repo, revision)
    }

    fn discard_epoch(
        &self,
        generation: &AuxiliaryGenerationKeyV1,
        epoch: AuxEpochV1,
        diagnostic: Option<&DiagnosticContextV1>,
    ) -> Result<HistoryTextDiscardOutcomeV1, CoreError> {
        self.inner.discard_epoch(generation, epoch, diagnostic)
    }

    fn discard_generation(
        &self,
        generation: &AuxiliaryGenerationKeyV1,
        diagnostic: Option<&DiagnosticContextV1>,
    ) -> Result<HistoryTextDiscardOutcomeV1, CoreError> {
        self.inner.discard_generation(generation, diagnostic)
    }
}

fn commit(byte: u8) -> CommitRecord {
    let sha = CommitSha::from_bytes([byte; 20]);
    CommitRecord {
        wire_version: 1,
        sha,
        parents: Vec::new(),
        author_time_ms: u64::from(byte),
        committer_time_ms: u64::from(byte),
        applied_at_ms: u64::from(byte),
        author: "alice".into(),
        author_name: None,
        author_email: None,
        committer: "alice".into(),
        committer_name: None,
        committer_email: None,
        message: "needle".into(),
        is_merge: false,
        tags: Vec::new(),
    }
}

fn batch(pin: &quanta_index_contract::GenerationPin, byte: u8) -> HistoryIngestBatch {
    HistoryIngestBatch {
        repo_id: pin.repo_id.clone(),
        revision_id: pin.revision_id.clone(),
        generation: pin.manifest_generation,
        manifest_digest: None,
        batch_digest: format!("history-long-pin-{byte}"),
        commits: vec![commit(byte)],
        refs: Vec::new(),
        tags: Vec::new(),
        diff_hunks: Vec::new(),
    }
}

fn query(pin: &quanta_index_contract::GenerationPin) -> SearchPlaneQueryIpcRequest {
    SearchPlaneQueryIpcRequest::History(HistoryQueryRequest {
        text_query: TextQueryRequest {
            syntax: TextQuerySyntax::Native,
            query_text: "type:commit needle".into(),
            constraints: QueryConstraintSetV1::unconstrained(),
            generation: Some(pin.clone()),
            generation_selector: None,
            top_k: 16,
            cursor: None,
        },
        order: HistoryOrderV1::Relevance,
        cursor: None,
    })
}

fn history_page(
    dispatcher: &SearchPlaneDispatcher,
    pin: &quanta_index_contract::GenerationPin,
) -> Result<QueryPage, Box<dyn Error>> {
    let response = dispatcher.dispatch(query(pin), &RequestBudgetV1::unbounded());
    let SearchPlaneQueryIpcResponse::History(page) = response else {
        return Err(format!("history query did not return a page: {response:?}").into());
    };
    Ok(page)
}

fn dispatcher(
    ledger: Arc<RwLock<Ledger>>,
    activation: Arc<ActivationCatalog>,
    history: HistoryTextIndexParts,
) -> SearchPlaneDispatcher {
    let unused = Arc::new(UnrelatedPort);
    SearchPlaneDispatcher::new(
        unused.clone(),
        unused.clone(),
        unused.clone(),
        unused,
        ledger,
        activation,
    )
    .with_history_text(history)
}

struct PausedQuery {
    resume: Option<mpsc::Sender<()>>,
    thread: Option<QueryThread>,
}

impl PausedQuery {
    fn finish(mut self) -> Result<QueryPage, Box<dyn Error>> {
        let resume = self.resume.take().ok_or("paused query resume missing")?;
        let sent = resume.send(());
        let thread = self.thread.take().ok_or("paused query thread missing")?;
        let query = thread.join().map_err(|payload| {
            payload.downcast_ref::<String>().map_or_else(
                || {
                    format!(
                        "history query thread panicked with {:?}",
                        payload.as_ref().type_id()
                    )
                },
                |message| format!("history query thread panicked: {message}"),
            )
        })?;
        match (sent, query) {
            (Ok(()), Ok(page)) => Ok(page),
            (Ok(()), Err(query_error)) => Err(query_error.into()),
            (Err(send_error), Err(query_error)) => {
                Err(format!("query resume failed: {send_error}; query: {query_error}").into())
            }
            (Err(send_error), Ok(_page)) => {
                Err(format!("query resume failed after response: {send_error}").into())
            }
        }
    }
}

impl Drop for PausedQuery {
    fn drop(&mut self) {
        if let Some(resume) = self.resume.take() {
            let _released = resume.send(());
        }
        if let Some(thread) = self.thread.take() {
            let _joined = thread.join();
        }
    }
}

#[test]
fn actual_history_query_pin_defers_pruned_epoch_gc_until_last_reader_leaves() -> TestResult {
    let directory = tempfile::tempdir()?;
    let requested = directory.path().join("state");
    fs::create_dir(&requested)?;
    fs::set_permissions(&requested, fs::Permissions::from_mode(0o700))?;
    let lease = StateRootLease::acquire(&requested)?;
    let root = lease.state_root_identity_v1().to_path_buf();
    let physical = LegacyPhysicalAssembly::new(
        lease,
        TEST_STATE_ROOT_PHYSICAL_POLICY,
        4096,
        &RequestBudgetV1::unbounded(),
        &[],
    )?;
    let LegacyPhysicalAssembly {
        state_root_lease,
        publication,
        catalog_io,
        catalog_funding,
        history: history_factory,
        durable_control,
        source_upload,
        repomap,
        semantic,
        semantic_maintenance,
    } = physical;
    drop((
        durable_control,
        source_upload,
        repomap,
        semantic,
        semantic_maintenance,
    ));
    let deletions = publication.file_deletions();
    let binder = deletions.binder();
    let catalog_admission: Arc<dyn CatalogIoAdmission> = catalog_io.clone();
    let catalog = Arc::new(SqliteCatalog::open_with_io_admission(
        &root,
        Duration::from_secs(1),
        catalog_admission,
    )?);
    let history_root = root.join("authorities").join("history-text");
    let index = Arc::new(HistoryTextIndexAdapter::with_file_admission(
        history_root.clone(),
        history_factory.clone(),
    )?);
    let (entered_tx, entered_rx) = mpsc::channel();
    let (resume_tx, resume_rx) = mpsc::channel();
    let gate = Arc::new(QueryGate {
        armed: AtomicBool::new(true),
        entered: Mutex::new(Some(entered_tx)),
        resume: Mutex::new(resume_rx),
    });
    let port = Arc::new(GatedHistoryPort {
        inner: Arc::clone(&index),
        gate,
    });
    let history = HistoryTextIndexParts::new(port.clone());
    let mut ready = Ledger::default();
    let pin = make_pin(
        RepoId::new("history-pin")?,
        RevisionId::new("revision-pin")?,
        ManifestGeneration::new(9),
    );
    ready.lexical_seal(pin.manifest_generation);
    ready.semantic_seal_with_digest(pin.manifest_generation, "manifest-digest-9");
    ready.record_track_materialized(
        &pin.repo_id,
        &pin.revision_id,
        SearchPlaneTrackKind::Lexical,
        pin.manifest_generation,
        None,
    );
    ready.record_track_seal(
        &pin.repo_id,
        &pin.revision_id,
        SearchPlaneTrackKind::Lexical,
        pin.manifest_generation,
    );
    ready.record_track_materialized(
        &pin.repo_id,
        &pin.revision_id,
        SearchPlaneTrackKind::Semantic,
        pin.manifest_generation,
        Some("manifest-digest-9"),
    );
    ready.record_track_seal_with_digest(
        &pin.repo_id,
        &pin.revision_id,
        SearchPlaneTrackKind::Semantic,
        pin.manifest_generation,
        "manifest-digest-9",
    );
    ready.record_historically_sealed_search_corpus(
        &pin.repo_id,
        &pin.revision_id,
        pin.manifest_generation,
        "manifest-digest-9",
    );
    let ledger = Arc::new(RwLock::new(ready));
    let materializer = DirectHistoryMaterializer::new(AuxiliaryMaterializerParts {
        catalog: catalog.clone(),
        coordinator: AuxiliaryMutationCoordinator::shared(),
        ledger: Arc::clone(&ledger),
    })
    .with_history_text(history.clone());
    // The explicit query pin only reads activation selection. Keep this
    // unrelated fixture outside the leased physical root's disk census.
    let activation_fixture = tempfile::tempdir()?;
    let activation_owner = quanta_index_search_plane::SearchCorpusLifecycleOwner::open(
        activation_fixture.path(),
        quanta_index_search_plane::readiness::SearchCorpusHistoryRetentionPolicyV1::new(
            2,
            1024 * 1024,
            8,
            8 * 1024 * 1024,
        )?,
        Arc::new(UnrelatedPort),
    )?;
    let activation = activation_owner.activation_catalog();
    let live_dispatcher = Arc::new(dispatcher(
        Arc::clone(&ledger),
        Arc::clone(&activation),
        history.clone(),
    ));
    let _first = materializer.publish_batch(&batch(&pin, 1), &RequestBudgetV1::unbounded())?;
    let generation = AuxiliaryGenerationKeyV1 {
        repo_id: pin.repo_id.clone(),
        revision_id: pin.revision_id.clone(),
        generation: pin.manifest_generation,
    };
    let old_epoch = GenerationStorageKeyV1::for_repo_revision(&pin.repo_id, &pin.revision_id)
        .generation_dir(&history_root, pin.manifest_generation)
        .join("e1");
    let old_manifest = old_epoch.join("history-text-manifest.cbor");
    let old_manifest_metadata = fs::metadata(&old_manifest)?;
    if old_manifest_metadata.len() == 0 || old_manifest_metadata.nlink() != 1 {
        return Err("first epoch lacks an independent single-link manifest inode".into());
    }
    let query_dispatcher = Arc::clone(&live_dispatcher);
    let query_pin = pin.clone();
    let paused = PausedQuery {
        resume: Some(resume_tx),
        thread: Some(std::thread::spawn(move || {
            history_page(&query_dispatcher, &query_pin).map_err(|error| error.to_string())
        })),
    };
    if let Err(wait_error) = entered_rx.recv_timeout(Duration::from_secs(30)) {
        let query_outcome = paused.finish();
        return Err(format!(
            "history query did not reach its opened Tantivy searcher: {wait_error}; query outcome: {query_outcome:?}"
        )
        .into());
    }
    for byte in 2..=10 {
        let _receipt =
            materializer.publish_batch(&batch(&pin, byte), &RequestBudgetV1::unbounded())?;
    }
    let retained = ledger
        .read()
        .map_err(|error| format!("history ledger poisoned: {error}"))?
        .history_retained_epochs(&pin.repo_id, &pin.revision_id, pin.manifest_generation)
        .ok_or("history generation vanished")?;
    // Eight superseded epochs plus the current epoch are retained.
    if retained.len() != 9 || retained.contains(&AuxEpochV1::new(1)) {
        return Err("tenth epoch did not prune the first from the logical ledger".into());
    }
    let pinned_manifest = fs::metadata(&old_manifest)?;
    if pinned_manifest.ino() != old_manifest_metadata.ino()
        || pinned_manifest.dev() != old_manifest_metadata.dev()
    {
        return Err("pruned first epoch was replaced while its query held the handle".into());
    }
    let pinned_snapshot = binder.capacity_snapshot()?;
    let pinned_disk = disk_census(&root)?;
    let deferred = history.reconcile_generation(&generation, &retained, None)?;
    if deferred != vec![AuxEpochV1::new(1)]
        || binder.capacity_snapshot()? != pinned_snapshot
        || disk_census(&root)? != pinned_disk
    {
        return Err("retention reclaimed or refunded an epoch held by a real query".into());
    }
    let old_page = paused.finish()?;
    if old_page.read_epoch != AuxEpochV1::new(1)
        || old_page.commits.len() != 1
        || old_page.commits.first().map(|row| row.sha) != Some(CommitSha::from_bytes([1; 20]))
    {
        return Err("old query did not finish against its fixed first-epoch row".into());
    }
    let reclaimed = history.reconcile_generation(&generation, &retained, None)?;
    if !reclaimed.is_empty() || old_epoch.exists() {
        return Err("first epoch was not reclaimed after its final query left".into());
    }
    let after_gc = binder.capacity_snapshot()?;
    let after_gc_disk = disk_census(&root)?;
    let refunded_bytes = pinned_snapshot
        .used_bytes
        .checked_sub(after_gc.used_bytes)
        .ok_or("history charge increased during old epoch retirement")?;
    if refunded_bytes < old_manifest_metadata.len()
        || after_gc.used_files >= pinned_snapshot.used_files
        || pinned_disk
            .0
            .checked_sub(after_gc_disk.0)
            .ok_or("history disk occupancy increased during old epoch retirement")?
            < old_manifest_metadata.len()
        || after_gc_disk.1 >= pinned_disk.1
    {
        return Err("old manifest inode and ledger charge survived final reader retirement".into());
    }
    drop(materializer);
    drop(live_dispatcher);
    drop(history);
    drop(port);
    drop(index);
    let cold = Arc::new(HistoryTextIndexAdapter::with_file_admission(
        history_root,
        history_factory.clone(),
    )?);
    let cold_parts = HistoryTextIndexParts::new(cold.clone());
    let cold_dispatcher = dispatcher(Arc::clone(&ledger), activation, cold_parts.clone());
    let latest = history_page(&cold_dispatcher, &pin)?;
    let expected: BTreeSet<_> = (1_u8..=10)
        .map(|byte| CommitSha::from_bytes([byte; 20]))
        .collect();
    let actual: BTreeSet<_> = latest.commits.iter().map(|row| row.sha).collect();
    if latest.read_epoch != AuxEpochV1::new(10) || actual != expected || latest.commits.len() != 10
    {
        return Err("cold reopened real history query lost fixed committed rows".into());
    }
    drop(cold_dispatcher);
    drop(ledger);
    if !matches!(
        cold_parts.retire_and_discard_generation(&generation, None)?,
        Some(HistoryTextDiscardOutcomeV1::Discarded { bytes }) if bytes > 0
    ) {
        return Err("final generation retirement did not remove its index".into());
    }
    drop(cold_parts);
    drop(cold);
    drop(history_factory);
    let retirement =
        LegacyRuntimeRetirement::new(catalog.clone(), catalog_io.clone(), deletions.clone());
    drop(catalog);
    drop(catalog_io);
    retirement.retire()?;
    let settled = binder.capacity_snapshot()?;
    let (disk_bytes, disk_files) = disk_census(&root)?;
    if settled.used_bytes != disk_bytes
        || settled.used_files != disk_files
        || settled.active_allowances != 0
        || settled.active_tickets != 0
        || settled.uncertain_pending != 0
        || settled.poisoned
        || !binder.is_quiescent()?
    {
        return Err(
            "history and Catalog retirement did not match independent disk inventory".into(),
        );
    }
    drop(retirement);
    drop(deletions);
    drop(publication);
    drop(catalog_funding);
    drop(binder);
    drop(state_root_lease);
    let reacquired = StateRootLease::acquire_existing(&root)?;
    drop(reacquired);
    Ok(())
}
