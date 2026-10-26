//! Real managed ANN last-pin reclaim and physical seal-write fault rails.

use super::*;
use std::{
    collections::{BTreeMap, BTreeSet},
    error::Error,
    fs::{self, OpenOptions},
    io::{Read as _, Write as _},
    os::unix::fs::MetadataExt as _,
    sync::{
        Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Duration,
};

use quanta_index_catalog::SqliteCatalog;
use quanta_index_contract::{
    EmbeddingNormalization, QueryConstraintSetV1, SemanticQueryRequest,
    index::{
        CoverageBinding, CoverageStatus, Digest, GroupActivation, IndexInstanceId, IndexKind,
        IndexProfile, InputDomain, InstanceSpec, OperationKey, OperationStatus, OwnerKey,
        OwnerMutation, ProducerId, PublicationGroupId, SchemaId, SlotId, SourceEvent,
        SourceTextAdmission, SourceTransition, StableOwnerId, StorageFormatId, StreamId,
        SubmitOutcome, TenantId, UpdateKind, UpdateRequest, ViewRef, ViewSelection, WorkspaceKey,
        WorktreeId,
    },
    native_protocol::NativeSemanticTraversal,
};
use quanta_index_core::{CoreError, RequestBudgetV1, TextEmbeddingBatch, TextEmbeddingProvider};
use quanta_index_core::{
    diagnostic_trace::{
        DiagnosticCaptureIdV1, DiagnosticClockV1, DiagnosticContextV1, DiagnosticEmitV1,
        DiagnosticEventKindV1, DiagnosticEventSinkV1, DiagnosticLaneV1, DiagnosticRootKindV1,
        DiagnosticSpanIdV1, DiagnosticStageV1, DiagnosticTerminalV1,
    },
    index::{
        IndexError, ObjectReadIoPort, ObjectRef, ObjectWriteIoPort, ObjectWriteTicket,
        QueryCoverage, WorkBudget, WorkLimits, WorkScope,
        effort::{ReservationGuard, ResourceNeed, ResourceScheduler},
        owner_map_key,
        ports::{
            CatalogCustodyMaintenancePort, CatalogExactHistoryPort, CatalogReclaimPort,
            CatalogTerminalWorkRetentionPort, ImmutableObjectPort, ManagedObjectReclaimPort,
            PersistentMapPort,
        },
    },
};
#[cfg(target_os = "linux")]
use quanta_index_kernel::ReadTicket;
use quanta_index_kernel::{
    AdapterDescriptor, AdapterRegistry, Kernel, KernelServices, QueryAdapterRegistry,
};
use quanta_index_searchd::app::runtime::StateRootLease;
use quanta_index_semantic::native::{
    NativeSemantic, NativeSemanticReadConfig, NativeSemanticReader, SearchResult,
    SemanticReadRequest, TextEmbeddingBridge,
};
use quanta_index_storage::{
    FileObjectStore, FileScratchSpace, ManagedObjectIo, ObjectIoObservation,
};

use crate::{
    native_object_read_io::NativeObjectReadIo,
    native_object_reclaim_io::NativeObjectReclaimIo,
    native_scratch_io::NativeScratchIo,
    publication_inventory::{PublicationBootInventoryLimits, seed_publication_capacity},
};

#[path = "../../tests/common/native_object_files.rs"]
mod native_object_files;
#[path = "../../tests/common/physical_enospc.rs"]
mod physical_enospc;

type TestResult<T = ()> = Result<T, Box<dyn Error>>;
const INSTANCE: IndexInstanceId = IndexInstanceId([53; 16]);
const PRODUCER: ProducerId = ProducerId([51; 16]);
const POLICY: Digest = Digest([54; 32]);
const COLD_CHILD: &str = "QUANTA_INDEX_ANN_ENOSPC_COLD_CHILD";
const COLD_DIRECTORY: &str = "QUANTA_INDEX_ANN_ENOSPC_COLD_DIRECTORY";
const COLD_VIEW: &str = "QUANTA_INDEX_ANN_ENOSPC_COLD_VIEW";
const COLD_SOURCE: &str = "QUANTA_INDEX_ANN_ENOSPC_COLD_SOURCE";
const COLD_COVERAGE: &str = "QUANTA_INDEX_ANN_ENOSPC_COLD_COVERAGE";
#[cfg(target_os = "linux")]
const CLOSE_PHASE: &str = "QUANTA_INDEX_ANN_CLOSE_PHASE";
#[cfg(target_os = "linux")]
const CLOSE_DIRECTORY: &str = "QUANTA_INDEX_ANN_CLOSE_DIRECTORY";
#[cfg(target_os = "linux")]
const CLOSE_EVIDENCE: &str = "QUANTA_INDEX_ANN_CLOSE_EVIDENCE";
#[cfg(target_os = "linux")]
const EXACT_CLOSE_TEST: &str = "native_object_write_io::ann_enospc::real_ann_seal_checked_close_eio_recovers_only_after_process_exit";
#[cfg(target_os = "linux")]
const EXACT_FSYNC_TEST: &str = "native_object_write_io::ann_enospc::real_ann_seal_file_fsync_eio_preserves_selected_views_and_recovers_same_operation";
const EXACT_TEST: &str = "native_object_write_io::ann_enospc::real_ann_seal_write_enospc_preserves_selected_views_and_recovers_same_operation";

fn require(value: bool, message: &'static str) -> TestResult {
    if value { Ok(()) } else { Err(message.into()) }
}

fn workspace() -> WorkspaceKey {
    WorkspaceKey {
        tenant: TenantId([1; 16]),
        collection: quanta_index_contract::index::CollectionId([2; 16]),
        worktree: WorktreeId([3; 16]),
        slot: SlotId([4; 16]),
    }
}

fn operation(sequence: u64) -> OperationKey {
    OperationKey {
        workspace: workspace(),
        epoch: 1,
        producer: PRODUCER,
        stream: StreamId([5; 16]),
        stream_epoch: 1,
        sequence,
    }
}

fn owner(id: u8) -> OwnerKey {
    OwnerKey {
        producer: PRODUCER,
        stable_id: StableOwnerId([id; 16]),
    }
}

fn work() -> WorkBudget {
    WorkBudget::with_request(
        WorkLimits {
            reads: 1_000_000,
            writes: 1_000_000,
            bytes: 1_500_000_000,
            mutations: 1_000_000,
        },
        RequestBudgetV1::for_duration(Duration::from_secs(120)),
    )
}

fn published(outcome: &SubmitOutcome) -> TestResult<ViewRef> {
    match outcome {
        SubmitOutcome::Terminal(OperationStatus::Published(view)) => Ok(*view),
        SubmitOutcome::Accepted(_)
        | SubmitOutcome::Terminal(
            OperationStatus::Unknown | OperationStatus::Pending(_) | OperationStatus::Refused(_),
        )
        | SubmitOutcome::Unknown(_) => {
            Err(format!("ANN fixture publication did not succeed: {outcome:?}").into())
        }
    }
}

struct FixedProvider;
impl TextEmbeddingProvider for FixedProvider {
    fn embed_batch(&self, texts: &[&str]) -> Result<TextEmbeddingBatch, CoreError> {
        let vectors = texts
            .iter()
            .map(|text| match *text {
                "alpha" => Ok(vec![1.0, 0.0]),
                "beta" => Ok(vec![0.0, 1.0]),
                "gamma" => Ok(vec![-1.0, 0.0]),
                _ => Err(CoreError::InvalidContract(
                    "unexpected ANN fixture text".into(),
                )),
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(TextEmbeddingBatch::unscoped(vectors))
    }
    fn model_id(&self) -> &'static str {
        "physical-ann-fixed"
    }
    fn model_revision(&self) -> &'static str {
        "revision-1"
    }
    fn dimension(&self) -> usize {
        2
    }
    fn normalization(&self) -> EmbeddingNormalization {
        EmbeddingNormalization::L2Unit
    }
}

#[derive(Debug, Default)]
struct SealEvents {
    active: Mutex<Option<DiagnosticSpanIdV1>>,
    begins: AtomicU64,
    errors: AtomicU64,
    dropped: AtomicBool,
}

impl SealEvents {
    fn is_active(&self) -> bool {
        self.active.lock().is_ok_and(|active| active.is_some())
    }
}

impl DiagnosticEventSinkV1 for SealEvents {
    fn try_emit(&self, event: DiagnosticEmitV1) -> bool {
        let Ok(mut active) = self.active.lock() else {
            self.dropped.store(true, Ordering::SeqCst);
            return false;
        };
        match event.kind {
            DiagnosticEventKindV1::SpanBegin {
                span_id,
                stage: DiagnosticStageV1::SemanticSeal,
                ..
            } => {
                if active.replace(span_id).is_some() {
                    self.dropped.store(true, Ordering::SeqCst);
                    return false;
                }
                let _previous = self.begins.fetch_add(1, Ordering::SeqCst);
            }
            DiagnosticEventKindV1::SpanEnd { span_id, outcome } if *active == Some(span_id) => {
                *active = None;
                if matches!(outcome, DiagnosticTerminalV1::Error { .. }) {
                    let _previous = self.errors.fetch_add(1, Ordering::SeqCst);
                }
            }
            DiagnosticEventKindV1::Health { .. } => {
                self.dropped.store(true, Ordering::SeqCst);
            }
            DiagnosticEventKindV1::SpanBegin { .. }
            | DiagnosticEventKindV1::SpanEnd { .. }
            | DiagnosticEventKindV1::RequestIdentity { .. }
            | DiagnosticEventKindV1::PhaseIdentity { .. }
            | DiagnosticEventKindV1::Counter { .. }
            | DiagnosticEventKindV1::AnnRegionObservation { .. }
            | DiagnosticEventKindV1::SqliteSyncObservation { .. }
            | DiagnosticEventKindV1::SyncAggregate { .. }
            | DiagnosticEventKindV1::ObjectKindAggregate { .. }
            | DiagnosticEventKindV1::AdoptionGroupAggregate { .. }
            | DiagnosticEventKindV1::Bm25CensusAggregate { .. }
            | DiagnosticEventKindV1::Bm25BoundaryAggregate { .. }
            | DiagnosticEventKindV1::Bm25LiveBuildAggregate { .. }
            | DiagnosticEventKindV1::Link { .. } => {}
        }
        true
    }
}

#[derive(Debug)]
struct Filler {
    file: Option<File>,
    random: File,
    path: PathBuf,
    device: u64,
    triggered: bool,
}

struct AnnFixture {
    kernel: Kernel<SqliteCatalog>,
    catalog: Arc<SqliteCatalog>,
    objects: Arc<FileObjectStore>,
    object_root: PathBuf,
    semantic: QueryAdapterRegistry<SemanticReadRequest, SearchResult>,
    bridge: Arc<TextEmbeddingBridge<FixedProvider>>,
    recipe: quanta_index_semantic::native::EmbeddingRecipe,
    observer: Arc<ObjectIoObservation>,
    physical: Arc<PublicationFileCapacity>,
    runtime: Arc<tokio::runtime::Runtime>,
}

#[derive(Debug, Default)]
struct CancelEvidence {
    count: u64,
    first: Option<Result<(), String>>,
}

fn semantic_spec(recipe: &quanta_index_semantic::native::EmbeddingRecipe) -> InstanceSpec {
    InstanceSpec {
        instance: INSTANCE,
        kind: IndexKind([71; 16]),
        group: PublicationGroupId([72; 16]),
        activation: GroupActivation::AsyncCoverage,
        input_domain: InputDomain::SourceLocal,
        schema: SchemaId([73; 16]),
        policy: recipe.fingerprint(),
        format: StorageFormatId([74; 16]),
    }
}

#[derive(Debug, Clone)]
enum FaultMode {
    Pressure {
        seal: Arc<SealEvents>,
        filler: Arc<Mutex<Filler>>,
        cancel: Arc<Mutex<CancelEvidence>>,
    },
    #[cfg(target_os = "linux")]
    Fuse(Arc<FuseFault>),
}

fn open_fixture(
    directory: &Path,
    runtime: Arc<tokio::runtime::Runtime>,
    fault: Option<FaultMode>,
) -> TestResult<AnnFixture> {
    let lease = StateRootLease::acquire(&directory.join("state"))?;
    let root = lease.state_root_identity_v1().to_path_buf();
    let inventory = PublicationBootInventoryLimits {
        max_bytes: 512 * 1024 * 1024,
        max_files: 8192,
        max_pending: 1024,
        max_entries: 16384,
        max_depth: 8,
        // Boot queues sibling directories, including up to 256 object shards.
        // Match the existing Native physical profile's bounded scan width.
        max_pending_frames: 1024,
    };
    // The held A view and current B view retain more than 256 distinct
    // source/artifact inodes before the intended fault. Use this fixture's
    // inventoried entry bound for tracking rather than the scalar test cap.
    let max_tracked_entries = usize::try_from(inventory.max_entries)?;
    let capacity =
        seed_publication_capacity(&lease, &root, inventory, &[], &RequestBudgetV1::unbounded())?;
    let physical = PublicationFileCapacity::new(lease, capacity, 32, max_tracked_entries, 256)?;
    let io = NativeScratchIo::register(
        Arc::clone(&physical),
        std::ffi::CString::new("quanta-publication-capacity")?,
        512,
    )?;
    let scratch = Arc::new(FileScratchSpace::open_managed_with_io(
        root.join("scratch"),
        16 * 1024 * 1024,
        io.clone(),
    )?);
    let catalog = Arc::new(SqliteCatalog::open_with_io_admission(
        &root,
        Duration::from_secs(2),
        io.clone(),
    )?);
    let object_root = root.join("object-store");
    let lock = object_root.join(".object-namespace.lease");
    io.prepare_lock(&lock)?;
    let read = Arc::new(NativeObjectReadIo::new(Arc::clone(&physical)));
    let write: Arc<dyn ObjectWriteIoPort> = match fault {
        Some(mode) => Arc::new(FaultWriteIo {
            inner: NativeObjectWriteIo::new(Arc::clone(&physical)),
            mode,
        }),
        None => Arc::new(NativeObjectWriteIo::new(Arc::clone(&physical))),
    };
    let observer = Arc::new(ObjectIoObservation::default());
    let objects = Arc::new(
        FileObjectStore::open_managed_with_io(
            object_root,
            scratch.clone(),
            catalog.clone(),
            ManagedObjectIo {
                namespace_lock: read.open(&lock, &mut work())?,
                read,
                write,
                reclaim: Arc::new(NativeObjectReclaimIo::new(Arc::clone(&physical))),
            },
        )?
        .with_io_observation(Arc::clone(&observer)),
    );
    let bridge = Arc::new(TextEmbeddingBridge::new(FixedProvider)?);
    let recipe = bridge.recipe(1024)?;
    let spec = semantic_spec(&recipe);
    let descriptor = AdapterDescriptor {
        kind: spec.kind,
        schema: spec.schema,
        format: spec.format,
    };
    let mut builders = AdapterRegistry::new();
    builders.register_owner(
        descriptor,
        NativeSemantic::new(Arc::clone(&bridge), recipe.clone())?.with_ann(),
    )?;
    let semantic = semantic_registry(Arc::clone(&bridge), &recipe, Arc::clone(&runtime))?;
    let kernel = Kernel::new(
        KernelServices::new(
            objects.clone(),
            objects.clone(),
            catalog.clone(),
            scratch,
            Duration::from_secs(120),
        )?,
        builders,
    );
    Ok(AnnFixture {
        kernel,
        catalog,
        objects,
        object_root: root.join("object-store"),
        semantic,
        bridge,
        recipe,
        observer,
        physical,
        runtime,
    })
}

fn semantic_registry(
    bridge: Arc<TextEmbeddingBridge<FixedProvider>>,
    recipe: &quanta_index_semantic::native::EmbeddingRecipe,
    runtime: Arc<tokio::runtime::Runtime>,
) -> TestResult<QueryAdapterRegistry<SemanticReadRequest, SearchResult>> {
    let spec = semantic_spec(recipe);
    let descriptor = AdapterDescriptor {
        kind: spec.kind,
        schema: spec.schema,
        format: spec.format,
    };
    let need = ResourceNeed {
        cpu_units: 1,
        io_bytes: 64 * 1024 * 1024,
        memory_bytes: 64 * 1024 * 1024,
        provider_units: 1,
    };
    let mut semantic = QueryAdapterRegistry::new();
    semantic.register(
        descriptor,
        need,
        NativeSemanticReader::new(
            bridge,
            recipe.clone(),
            NativeSemanticReadConfig {
                page_size: 8,
                max_owners: 128,
                max_query_bytes: 1024,
                need,
            },
        )?
        .with_ann(runtime),
    )?;
    Ok(semantic)
}

impl Filler {
    fn new(directory: &Path) -> TestResult<Self> {
        let path = directory.join("ann-bounded-pressure.bin");
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)?;
        let device = file.metadata()?.dev();
        Ok(Self {
            file: Some(file),
            random: File::open("/dev/urandom")?,
            path,
            device,
            triggered: false,
        })
    }

    fn fill_until_kernel_enospc(&mut self) -> TestResult {
        let file = self
            .file
            .as_mut()
            .ok_or("ANN pressure file already retired")?;
        let mut chunk = vec![0_u8; 1024 * 1024];
        let fragment = rustix::fs::fstatvfs(&*file)?.f_frsize;
        require(
            fragment > 0 && fragment <= u64::try_from(chunk.len())?,
            "ANN pressure requires a bounded nonzero filesystem fragment",
        )?;
        let attempts = 256_usize
            .checked_add(chunk.len().div_ceil(usize::try_from(fragment)?))
            .ok_or("ANN pressure attempt bound overflow")?;
        let mut maximum_write = u64::try_from(chunk.len())?;
        for _ in 0..attempts {
            let stat = rustix::fs::fstatvfs(&*file)?;
            let total = stat
                .f_blocks
                .checked_mul(stat.f_frsize)
                .ok_or("ANN volume size overflow")?;
            let available = stat
                .f_bavail
                .checked_mul(stat.f_frsize)
                .ok_or("ANN volume available-space overflow")?;
            require(
                total <= 128 * 1024 * 1024
                    && stat.f_frsize == fragment
                    && file.metadata()?.dev() == self.device,
                "ANN pressure escaped bounded volume",
            )?;
            // A large Write's ENOSPC can leave room for a small ANN object.
            // Consume the reported tail and require a fragment Write to fail.
            let amount = usize::try_from(available.min(maximum_write).max(fragment))?;
            let bytes = chunk
                .get_mut(..amount)
                .ok_or("ANN pressure exceeded its bounded buffer")?;
            self.random.read_exact(bytes)?;
            match file.write_all(bytes) {
                Ok(()) => file
                    .sync_all()
                    .map_err(|error| format!("ANN pressure sync failed after Write: {error}"))?,
                Err(error)
                    if error.raw_os_error() == Some(rustix::io::Errno::NOSPC.raw_os_error()) =>
                {
                    file.sync_all().map_err(|error| {
                        format!("ANN pressure sync failed after Write ENOSPC: {error}")
                    })?;
                    if u64::try_from(amount)? == fragment {
                        let remaining = rustix::fs::fstatvfs(&*file)?.f_bavail;
                        if remaining != 0 {
                            return Err(format!(
                                "ANN fragment Write ENOSPC left {remaining} available blocks"
                            )
                            .into());
                        }
                        return Ok(());
                    }
                    maximum_write = fragment;
                }
                Err(error) => {
                    return Err(format!("ANN pressure Write failed without ENOSPC: {error}").into());
                }
            }
        }
        Err("ANN pressure did not exhaust available blocks within its attempt bound".into())
    }

    fn remove(&mut self) -> TestResult {
        drop(self.file.take());
        fs::remove_file(&self.path)?;
        let parent = self.path.parent().ok_or("ANN pressure parent absent")?;
        File::open(parent)?.sync_all()?;
        Ok(())
    }
}

#[derive(Debug)]
struct FaultWriteIo {
    inner: NativeObjectWriteIo,
    mode: FaultMode,
}

struct FaultTicket {
    inner: Box<dyn ObjectWriteTicket>,
    mode: FaultMode,
    #[cfg(target_os = "linux")]
    target: PathBuf,
    #[cfg(target_os = "linux")]
    temporary: PathBuf,
}

impl ObjectWriteIoPort for FaultWriteIo {
    fn recover_retired_visible_create(
        &self,
        target: &Path,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        self.inner.recover_retired_visible_create(target, budget)
    }

    fn begin(
        &self,
        target: &Path,
        temporary: &Path,
        max_bytes: u64,
        budget: &mut WorkBudget,
    ) -> Result<Box<dyn ObjectWriteTicket>, IndexError> {
        let inner = self.inner.begin(target, temporary, max_bytes, budget)?;
        Ok(Box::new(FaultTicket {
            inner,
            mode: self.mode.clone(),
            #[cfg(target_os = "linux")]
            target: target.to_path_buf(),
            #[cfg(target_os = "linux")]
            temporary: temporary.to_path_buf(),
        }))
    }
}

impl ObjectWriteTicket for FaultTicket {
    fn creation_started(&mut self) -> Result<(), IndexError> {
        self.inner.creation_started()
    }
    fn created(&mut self, writer: &File) -> Result<(), IndexError> {
        self.inner.created(writer)?;
        match &self.mode {
            FaultMode::Pressure { seal, filler, .. } if seal.is_active() => {
                let mut filler = filler
                    .lock()
                    .map_err(|_poison| IndexError::Storage("ANN filler lock poisoned".into()))?;
                if !filler.triggered {
                    let device = writer
                        .metadata()
                        .map_err(|error| IndexError::Storage(format!("ANN writer stat: {error}")))?
                        .dev();
                    if device != filler.device {
                        return Err(IndexError::Storage(
                            "ANN writer escaped bounded volume".into(),
                        ));
                    }
                    filler.triggered = true;
                    filler.fill_until_kernel_enospc().map_err(|error| {
                        IndexError::Storage(format!("ANN physical pressure setup: {error}"))
                    })?;
                }
            }
            #[cfg(target_os = "linux")]
            FaultMode::Fuse(fault) if fault.kind == FuseFaultKind::Fsync => {
                fault.arm(writer, &self.temporary)?;
            }
            _ => {}
        }
        Ok(())
    }
    fn uncreated(&mut self) -> Result<(), IndexError> {
        self.inner.uncreated()
    }
    fn retire_writer(&mut self, writer: File) -> Result<(), IndexError> {
        #[cfg(target_os = "linux")]
        if let FaultMode::Fuse(fault) = &self.mode
            && fault.kind == FuseFaultKind::Close
        {
            // Even if control setup fails, the original physical owner must
            // consume the writer FD. Never bypass its checked close.
            let armed = fault.arm(&writer, &self.target);
            let retired = self.inner.retire_writer(writer);
            if let Err(error) = &retired {
                if let Ok(mut observed) = fault.retire_error.lock() {
                    *observed = Some(error.to_string());
                }
            }
            armed?;
            return retired;
        }
        self.inner.retire_writer(writer)
    }
    fn cancel(&mut self, writer: File) -> Result<(), IndexError> {
        let result = self.inner.cancel(writer);
        let cancel = match &self.mode {
            FaultMode::Pressure { cancel, .. } => cancel.as_ref(),
            #[cfg(target_os = "linux")]
            FaultMode::Fuse(fault) => &fault.cancel,
        };
        if let Ok(mut outcomes) = cancel.lock() {
            outcomes.count = outcomes.count.saturating_add(1);
            if outcomes.first.is_none() {
                outcomes.first = Some(match &result {
                    Ok(()) => Ok(()),
                    Err(error) => Err(error.to_string()),
                });
            }
        }
        result
    }
    fn synced(self: Box<Self>) -> Result<(), IndexError> {
        self.inner.synced()
    }
}

#[cfg(target_os = "linux")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FuseFaultKind {
    Fsync,
    Close,
}

#[cfg(target_os = "linux")]
#[derive(Debug)]
struct FuseFault {
    kind: FuseFaultKind,
    seal: Arc<SealEvents>,
    control: PathBuf,
    device: u64,
    armed: AtomicBool,
    cancel: Mutex<CancelEvidence>,
    retire_error: Mutex<Option<String>>,
}

#[cfg(target_os = "linux")]
impl FuseFault {
    fn clear(&self) -> TestResult {
        fs::write(&self.control, b"off none\n")?;
        require(
            fs::read(&self.control)? == b"off none\n",
            "ANN FUSE fault switch did not clear",
        )
    }

    fn arm(&self, writer: &File, path: &Path) -> Result<(), IndexError> {
        if !self.seal.is_active() || self.armed.swap(true, Ordering::SeqCst) {
            return Ok(());
        }
        let observed = writer
            .metadata()
            .map_err(|error| IndexError::Storage(format!("ANN FUSE writer stat: {error}")))?;
        let linked = fs::symlink_metadata(path)
            .map_err(|error| IndexError::Storage(format!("ANN FUSE linked path stat: {error}")))?;
        if observed.dev() != self.device
            || linked.dev() != observed.dev()
            || linked.ino() != observed.ino()
            || !linked.file_type().is_file()
        {
            return Err(IndexError::Storage(
                "ANN FUSE linked inode escaped fault mount".into(),
            ));
        }
        let name = path
            .file_name()
            .and_then(std::ffi::OsStr::to_str)
            .ok_or_else(|| IndexError::Storage("ANN FUSE basename absent".into()))?;
        if name.chars().any(char::is_whitespace) {
            return Err(IndexError::Storage(
                "ANN FUSE basename has whitespace".into(),
            ));
        }
        let operation = match self.kind {
            FuseFaultKind::Fsync => "fsync",
            FuseFaultKind::Close => "flush",
        };
        fs::write(&self.control, format!("{operation} {name}\n"))
            .map_err(|error| IndexError::Storage(format!("ANN FUSE arm {operation}: {error}")))
    }
}

#[derive(Debug, Default)]
struct StepClock(AtomicU64);

impl DiagnosticClockV1 for StepClock {
    fn now_ns(&self) -> u64 {
        self.0.fetch_add(1, Ordering::Relaxed)
    }
}

struct TestScheduler;

impl ResourceScheduler for TestScheduler {
    type Token = ();

    fn try_reserve(&self, _need: ResourceNeed) -> Option<Self::Token> {
        Some(())
    }
    fn release(&self, _token: Self::Token) {}
}

fn source(
    fixture: &AnnFixture,
    view: ViewRef,
) -> TestResult<quanta_index_contract::index::SourceCut> {
    let ticket = fixture
        .kernel
        .pin_view(ViewSelection::Exact(view), &work())?;
    let cut = ticket.view().source;
    ticket.release()?;
    Ok(cut)
}

fn configure(fixture: &AnnFixture) -> TestResult<ViewRef> {
    let genesis = fixture
        .kernel
        .create_workspace(workspace(), 1, &mut work())?;
    let ingress = fixture.kernel.begin_input(operation(1))?;
    let profile = fixture.kernel.stage_profile(
        ingress,
        &IndexProfile {
            instances: vec![semantic_spec(&fixture.recipe)],
        },
        &mut work(),
    )?;
    published(&block_on(
        fixture.runtime.as_ref(),
        fixture.kernel.submit(
            UpdateRequest {
                operation: operation(1),
                expected_view: genesis,
                kind: UpdateKind::Reconfigure { profile },
            },
            &mut work(),
        ),
    )?)
}

#[expect(
    clippy::disallowed_methods,
    reason = "test-only driver executes the real Kernel future"
)]
fn block_on<F: std::future::Future>(runtime: &tokio::runtime::Runtime, future: F) -> F::Output {
    runtime.block_on(future)
}

fn stage_sources(
    fixture: &AnnFixture,
    base: ViewRef,
    first_id: u8,
    last_id: u8,
    operation_sequence: u64,
    event_sequence: u64,
) -> TestResult<ViewRef> {
    let ingress = fixture.kernel.begin_input(operation(operation_sequence))?;
    let mut staged = Vec::new();
    staged.try_reserve_exact(64)?;
    for id in first_id..=last_id {
        let text: &[u8] = match id {
            1 => b"alpha",
            65 => b"gamma",
            _ => b"beta",
        };
        let reference = fixture.kernel.stage_source_text(
            ingress,
            &format!("src/{id}.rs"),
            text,
            SourceTextAdmission::Utf8 { policy: POLICY },
            &mut work(),
        )?;
        staged.push(Ok(OwnerMutation {
            owner: owner(id),
            expected: None,
            next: Some(reference),
        }));
    }
    let owners = fixture
        .kernel
        .stage_mutations(ingress, staged, &mut work())?;
    let mut transition = SourceTransition {
        expected_source: source(fixture, base)?,
        owners,
        events: Vec::new(),
    };
    transition.events.push(SourceEvent {
        producer: PRODUCER,
        stream: StreamId([5; 16]),
        stream_epoch: 1,
        predecessor: event_sequence
            .checked_sub(1)
            .ok_or("ANN event starts at zero")?,
        sequence: event_sequence,
        payload: transition.owner_digest()?,
    });
    published(&block_on(
        fixture.runtime.as_ref(),
        fixture.kernel.submit(
            UpdateRequest {
                operation: operation(operation_sequence),
                expected_view: base,
                kind: UpdateKind::SourceAdvance(transition),
            },
            &mut work(),
        ),
    )?)
}

fn materialize(
    fixture: &AnnFixture,
    view: ViewRef,
    sequence: u64,
    preferred: Option<OwnerKey>,
) -> TestResult<ViewRef> {
    let ready = fixture
        .kernel
        .ready_work(workspace(), 1, 128, &mut work())?;
    let intent = preferred
        .map_or_else(
            || ready.first(),
            |preferred| {
                ready.iter().find(|intent| {
                    matches!(
                        intent.scope, WorkScope::Owner { owner: candidate, .. } if candidate == preferred
                    )
                })
            },
        )
    .ok_or("ANN fixture has no expected materialization work")?;
    let claim = fixture
        .kernel
        .claim_work(intent, Duration::from_secs(120), &work())?;
    published(&block_on(
        fixture.runtime.as_ref(),
        fixture.kernel.submit(
            UpdateRequest {
                operation: operation(sequence),
                expected_view: view,
                kind: UpdateKind::Materialize { claim },
            },
            &mut work(),
        ),
    )?)
}

fn semantic_hit(
    fixture: &AnnFixture,
    view: ViewRef,
    query: &str,
) -> TestResult<(OwnerKey, f64, CoverageBinding)> {
    semantic_hit_with(&fixture.kernel, &fixture.semantic, view, query)
}

fn semantic_hit_with(
    kernel: &Kernel<SqliteCatalog>,
    semantic: &QueryAdapterRegistry<SemanticReadRequest, SearchResult>,
    view: ViewRef,
    query: &str,
) -> TestResult<(OwnerKey, f64, CoverageBinding)> {
    let (owner, cosine, coverage, _traversal) =
        semantic_hit_observed_with(kernel, semantic, view, query)?;
    Ok((owner, cosine, coverage))
}

fn semantic_hit_observed_with(
    kernel: &Kernel<SqliteCatalog>,
    semantic: &QueryAdapterRegistry<SemanticReadRequest, SearchResult>,
    view: ViewRef,
    query: &str,
) -> TestResult<(OwnerKey, f64, CoverageBinding, NativeSemanticTraversal)> {
    let mut budget = work();
    let mut session = kernel.begin_query_session(ViewSelection::Exact(view), &mut budget)?;
    let stage = session.prepare_stage(
        semantic,
        INSTANCE,
        SemanticQueryRequest {
            query_text: query.into(),
            constraints: QueryConstraintSetV1::unconstrained(),
            generation: None,
            generation_selector: None,
            lexical_scope: None,
            top_k: 1,
        }
        .into(),
    )?;
    let need = stage.required_resources();
    let guard = ReservationGuard::try_acquire(Arc::new(TestScheduler), need)
        .ok_or("ANN test scheduler refused reservation")?;
    let outcome = stage.execute(&guard)?;
    require(outcome.view() == view, "ANN query selected the wrong view")?;
    let [hit] = outcome.result().hits.as_slice() else {
        return Err("ANN query did not return one selected hit".into());
    };
    let coverage = match outcome.coverage() {
        QueryCoverage::Async(binding) => **binding,
        QueryCoverage::Core => return Err("ANN query returned core coverage".into()),
    };
    let result = (hit.owner, hit.cosine, coverage, outcome.result().traversal);
    session.close()?;
    Ok(result)
}

fn pin_count(catalog: &SqliteCatalog) -> TestResult<i64> {
    Ok(rusqlite::Connection::open_with_flags(
        catalog.path(),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )?
    .query_row("SELECT COUNT(*) FROM native_pin_v1", [], |row| row.get(0))?)
}

fn cold_child(mount: &Path) -> TestResult {
    let directory =
        PathBuf::from(std::env::var_os(COLD_DIRECTORY).ok_or("ANN cold child root absent")?);
    let name = directory
        .file_name()
        .and_then(std::ffi::OsStr::to_str)
        .ok_or("ANN cold child root name absent")?;
    require(
        directory.parent() == Some(mount)
            && name.starts_with("qidx-ann-enospc-")
            && fs::symlink_metadata(&directory)?.file_type().is_dir()
            && fs::canonicalize(&directory)? == directory
            && fs::metadata(&directory)?.dev() == fs::metadata(mount)?.dev()
            && fs::metadata(std::env::current_exe()?)?.dev() != fs::metadata(mount)?.dev(),
        "ANN cold child escaped its private bounded mount",
    )?;
    let expected_view: ViewRef = serde_json::from_str(&std::env::var(COLD_VIEW)?)?;
    let expected_source: quanta_index_contract::index::SourceCut =
        serde_json::from_str(&std::env::var(COLD_SOURCE)?)?;
    let expected_coverage: CoverageBinding = serde_json::from_str(&std::env::var(COLD_COVERAGE)?)?;
    let runtime = Arc::new(
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?,
    );
    let reopened = open_fixture(&directory, runtime, None)?;
    require(
        latest_view(&reopened)? == expected_view,
        "cold ANN child reopened a different current root",
    )?;
    require(
        source(&reopened, expected_view)? == expected_source,
        "cold ANN child reopened a different B source cut",
    )?;
    let (inherited_winner, inherited_score, inherited_coverage) =
        semantic_hit(&reopened, expected_view, "alpha")?;
    require(
        inherited_winner == owner(1)
            && inherited_score.to_bits() == 1.0f64.to_bits()
            && inherited_coverage == expected_coverage,
        "cold ANN child lost the inherited A region in the current B view",
    )?;
    let (winner, score, coverage) = semantic_hit(&reopened, expected_view, "gamma")?;
    require(
        winner == owner(65) && score.to_bits() == 1.0f64.to_bits() && coverage == expected_coverage,
        "cold ANN child lost the recovered B region",
    )?;
    require(
        pin_count(&reopened.catalog)? == 0,
        "cold ANN child retained a selected-view pin",
    )?;
    // An open managed store retains its namespace reader, and the live
    // Catalog retains VFS handles. Check retirement after those owners close;
    // retaining this observer reference does not create a reader or mutation.
    let physical = Arc::clone(&reopened.physical);
    drop(reopened);
    require(
        physical.is_quiescent()?,
        "cold ANN child retained physical custody after owner shutdown",
    )?;
    Ok(())
}

fn latest_view(fixture: &AnnFixture) -> TestResult<ViewRef> {
    let ticket = fixture.kernel.pin_view(
        ViewSelection::Latest {
            workspace: workspace(),
            epoch: 1,
        },
        &work(),
    )?;
    let view = ticket.view().identity;
    ticket.release()?;
    Ok(view)
}

// Traverse registered immutable edges, not ANN codec fields or child positions.
// The selected searchable root owns its region-map, descriptor, file-set and
// chunk descendants through the original install registrations.
fn object_closure(
    objects: &FileObjectStore,
    roots: impl IntoIterator<Item = ObjectRef>,
) -> TestResult<BTreeSet<ObjectRef>> {
    let mut pending: Vec<_> = roots.into_iter().collect();
    let mut seen = BTreeSet::new();
    let mut budget = work();
    while let Some(reference) = pending.pop() {
        if !seen.insert(reference) {
            continue;
        }
        require(
            seen.len() <= 16_384,
            "ANN object closure exceeded fixture bound",
        )?;
        let object = objects.get(reference, &mut budget)?;
        pending.extend(object.registration.children.as_slice().iter().copied());
    }
    Ok(seen)
}

fn delete_first_region_sources(fixture: &AnnFixture, base: ViewRef) -> TestResult<ViewRef> {
    let prior = source(fixture, base)?;
    let ingress = fixture.kernel.begin_input(operation(67))?;
    let mut mutations = Vec::new();
    mutations.try_reserve_exact(64)?;
    let mut budget = work();
    for id in 1..=64 {
        let source = fixture
            .objects
            .lookup_object(prior.owners, &owner_map_key(owner(id)), &mut budget)?
            .ok_or("old ANN source owner absent before deletion")?;
        mutations.push(Ok(OwnerMutation {
            owner: owner(id),
            expected: Some(source),
            next: None,
        }));
    }
    let owners = fixture
        .kernel
        .stage_mutations(ingress, mutations, &mut work())?;
    let mut transition = SourceTransition {
        expected_source: prior,
        owners,
        events: Vec::new(),
    };
    transition.events.push(SourceEvent {
        producer: PRODUCER,
        stream: StreamId([5; 16]),
        stream_epoch: 1,
        predecessor: 1,
        sequence: 2,
        payload: transition.owner_digest()?,
    });
    published(&block_on(
        fixture.runtime.as_ref(),
        fixture.kernel.submit(
            UpdateRequest {
                operation: operation(67),
                expected_view: base,
                kind: UpdateKind::SourceAdvance(transition),
            },
            &mut work(),
        ),
    )?)
}

fn release_ann_terminal_custody(catalog: &SqliteCatalog) -> TestResult {
    for sequence in 1..=69 {
        let mut complete = false;
        for _ in 0..64 {
            if !catalog
                .release_terminal_custody(operation(sequence), 256, &mut work())?
                .has_more
            {
                complete = true;
                break;
            }
        }
        require(complete, "ANN terminal custody exceeded bounded release")?;
    }
    Ok(())
}

fn retire_ann_terminal_work(catalog: &SqliteCatalog) -> TestResult<u32> {
    let mut after = None;
    let mut blocked = 0_u32;
    for _ in 0..8 {
        let page = catalog.retire_terminal_work_page(workspace(), 1, after, 128, &mut work())?;
        blocked = blocked
            .checked_add(page.blocked)
            .ok_or("ANN terminal work blocked count overflow")?;
        if !page.has_more {
            return Ok(blocked);
        }
        require(
            page.examined > 0 && page.after.is_some(),
            "ANN terminal work retirement did not advance",
        )?;
        after = page.after;
    }
    Err("ANN terminal work exceeded bounded retirement".into())
}

fn reclaim_old_closure(fixture: &AnnFixture, paths: &BTreeMap<ObjectRef, PathBuf>) -> TestResult {
    let mut remaining = paths.clone();
    let mut after = None;
    for _ in 0..8192 {
        // Match the prior eight-candidate page bound using production steps.
        for _ in 0..8 {
            if remaining.is_empty() {
                return Ok(());
            }
            // An unfinished child drain keeps the prior cursor; an exhausted
            // forward sweep returns to the queue head.
            let step = crate::native_reclaim_driver::reclaim_one_step(
                &fixture.catalog,
                &fixture.objects,
                after,
                32,
                &mut work(),
            )?;
            if let Some(reference) = step.examined {
                if let Some(path) = remaining.get(&reference)
                    && native_object_files::absent_nofollow(path)?
                {
                    let _removed = remaining.remove(&reference);
                }
                after = if step.has_more {
                    step.resume_after
                } else {
                    None
                };
                continue;
            }
            if after.is_some() {
                // has_more described the queue before the previous drain. A
                // drain may have queued lower children and removed that tail.
                after = None;
                continue;
            }
            let mut present = 0_usize;
            for path in remaining.values() {
                if !native_object_files::absent_nofollow(path)? {
                    present = present.checked_add(1).ok_or("ANN inode count overflow")?;
                }
            }
            if present == 0 {
                return Ok(());
            }
            return Err(format!(
                "old ANN closure has {present} physical inodes after reclaim queue head exhaustion"
            )
            .into());
        }
    }
    Err(format!(
        "old ANN closure still has {} candidate references after bounded reclaim",
        remaining.len()
    )
    .into())
}

#[test]
fn real_managed_ann_last_pin_reclaims_old_searchable_closure() -> TestResult {
    let directory = tempfile::tempdir()?;
    let runtime = Arc::new(
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?,
    );
    let fixture = open_fixture(directory.path(), runtime, None)?;
    let configured = configure(&fixture)?;
    let mut sealed = stage_sources(&fixture, configured, 1, 64, 2, 1)?;
    for sequence in 3..=66 {
        sealed = materialize(
            &fixture,
            sealed,
            sequence,
            (sequence == 3).then(|| owner(1)),
        )?;
    }
    let (old_winner, old_score, old_coverage, old_traversal) =
        semantic_hit_observed_with(&fixture.kernel, &fixture.semantic, sealed, "alpha")?;
    require(
        old_winner == owner(1)
            && old_score.to_bits() == 1.0_f64.to_bits()
            && old_coverage.status == CoverageStatus::Complete
            && matches!(old_traversal, NativeSemanticTraversal::Approximate { .. }),
        "old sealed ANN query failed its fixed alpha oracle",
    )?;
    let old_root = old_coverage
        .searchable
        .ok_or("sealed ANN searchable root absent")?;
    let old_closure = object_closure(fixture.objects.as_ref(), [old_root])?;
    let mut old_pin = fixture
        .kernel
        .pin_view(ViewSelection::Exact(sealed), &work())?;
    let deleted = delete_first_region_sources(&fixture, sealed)?;
    old_pin.renew()?;
    let added = stage_sources(&fixture, deleted, 65, 65, 68, 3)?;
    let latest = materialize(&fixture, added, 69, Some(owner(65)))?;
    let (current_winner, current_score, current_coverage) =
        semantic_hit(&fixture, latest, "gamma")?;
    require(
        current_winner == owner(65)
            && current_score.to_bits() == 1.0_f64.to_bits()
            && current_coverage.status == CoverageStatus::Complete,
        "current ANN query failed its independent gamma oracle",
    )?;
    let (held_winner, held_score, held_coverage, held_traversal) =
        semantic_hit_observed_with(&fixture.kernel, &fixture.semantic, sealed, "alpha")?;
    require(
        (held_winner, held_score.to_bits(), held_coverage)
            == (old_winner, old_score.to_bits(), old_coverage)
            && matches!(held_traversal, NativeSemanticTraversal::Approximate { .. }),
        "held old ANN view changed after all old members were removed",
    )?;
    let current_ticket = fixture
        .kernel
        .pin_view(ViewSelection::Exact(latest), &work())?;
    let current = current_ticket.view();
    let current_roots = [
        Some(current.profile),
        current.source.owners.0,
        current.source.paths.0,
        current.source.frontiers.0,
        current.projections.0,
        current.members.0,
        current.coverage.0,
        current_coverage.searchable,
    ];
    let current_closure = object_closure(
        fixture.objects.as_ref(),
        current_roots.into_iter().flatten(),
    )?;
    current_ticket.release()?;
    require(
        pin_count(&fixture.catalog)? == 1,
        "old ANN pin is not the last retained view",
    )?;
    let unique: Vec<_> = old_closure.difference(&current_closure).copied().collect();
    require(
        unique.contains(&old_root) && unique.len() >= 5,
        "old sealed ANN region has no independent searchable closure",
    )?;
    let mut physical = BTreeMap::new();
    let mut read_work = work();
    for reference in unique {
        let path = native_object_files::object_file(&fixture.object_root, reference)?;
        let metadata = fs::symlink_metadata(&path)?;
        require(
            metadata.file_type().is_file(),
            "old ANN object is not a regular inode",
        )?;
        let object = fixture.objects.get(reference, &mut read_work)?;
        require(
            object.registration.reference == reference,
            "old ANN closure object identity changed",
        )?;
        let _previous = physical.insert(
            reference,
            (path, metadata.dev(), metadata.ino(), metadata.len()),
        );
    }
    release_ann_terminal_custody(&fixture.catalog)?;
    old_pin.renew()?;
    let _floor = fixture.catalog.advance_exact_history_floor(
        workspace(),
        1,
        latest.sequence,
        &mut work(),
    )?;
    let _blocked_work_while_pinned = retire_ann_terminal_work(&fixture.catalog)?;
    let retirement =
        fixture
            .catalog
            .retire_exact_history_page(workspace(), 1, None, 128, &mut work())?;
    require(
        retirement.blocked >= 1 && !retirement.has_more,
        "held old ANN view was not the bounded retirement blocker",
    )?;
    // Exercise real reclaim while the old ticket still owns its root.
    let queued = fixture.catalog.reclaim_work_page(None, 8, &mut work())?;
    for item in queued.items {
        let _progress = fixture.objects.reclaim_one(item, 32, &mut work())?;
    }
    old_pin.validate()?;
    let mut retained_read = work();
    for (reference, (path, device, inode, bytes)) in &physical {
        let retained = fs::symlink_metadata(path)?;
        require(
            retained.file_type().is_file()
                && (retained.dev(), retained.ino(), retained.len()) == (*device, *inode, *bytes),
            "old ANN object inode changed while its last view was pinned",
        )?;
        let object = fixture.objects.get(*reference, &mut retained_read)?;
        require(
            object.registration.reference == *reference,
            "old ANN object nofollow read failed while pinned",
        )?;
    }
    old_pin.release()?;
    require(
        pin_count(&fixture.catalog)? == 0,
        "old ANN pin did not release",
    )?;
    require(
        retire_ann_terminal_work(&fixture.catalog)? == 0,
        "terminal ANN work still blocks exact view retirement",
    )?;
    let retirement =
        fixture
            .catalog
            .retire_exact_history_page(workspace(), 1, None, 128, &mut work())?;
    require(
        retirement.retired >= 1 && !retirement.has_more,
        "released old ANN view did not retire",
    )?;
    let paths = physical
        .into_iter()
        .map(|(reference, (path, _, _, _))| (reference, path))
        .collect();
    reclaim_old_closure(&fixture, &paths)?;
    let (winner, score, coverage) = semantic_hit(&fixture, latest, "gamma")?;
    require(
        (winner, score.to_bits(), coverage)
            == (current_winner, current_score.to_bits(), current_coverage),
        "current ANN query changed after reclaiming the last old pin",
    )
}

/// Run only under tools/testing/physical-enospc.py with a disposable bounded volume.
///
/// The source and original real Runtime publication owner live there;
/// the external filler is a second, test-owned regular file on that volume.
#[test]
#[ignore = "requires QUANTA_INDEX_PHYSICAL_ENOSPC_ROOT on an isolated bounded filesystem"]
fn real_ann_seal_write_enospc_preserves_selected_views_and_recovers_same_operation() -> TestResult {
    let mount = physical_enospc::bounded_volume_root()?;
    if std::env::var_os(COLD_CHILD).as_deref() == Some(std::ffi::OsStr::new("1")) {
        return cold_child(&mount);
    }
    let directory = tempfile::Builder::new()
        .prefix("qidx-ann-enospc-")
        .tempdir_in(mount)?;
    let filler = Arc::new(Mutex::new(Filler::new(directory.path())?));
    let seal = Arc::new(SealEvents::default());
    let cancel = Arc::new(Mutex::new(CancelEvidence::default()));
    let runtime = Arc::new(
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?,
    );
    let fixture = open_fixture(
        directory.path(),
        Arc::clone(&runtime),
        Some(FaultMode::Pressure {
            seal: Arc::clone(&seal),
            filler: Arc::clone(&filler),
            cancel: Arc::clone(&cancel),
        }),
    )?;
    let configured = configure(&fixture)?;
    let mut current = stage_sources(&fixture, configured, 1, 64, 2, 1)?;
    let a_source = source(&fixture, current)?;
    for sequence in 3..=66 {
        current = materialize(
            &fixture,
            current,
            sequence,
            (sequence == 3).then(|| owner(1)),
        )?;
    }
    let old_view = current;
    let (old_winner, old_score, old_coverage) = semantic_hit(&fixture, old_view, "alpha")?;
    require(
        old_winner == owner(1),
        "first ANN region lost the alpha owner",
    )?;
    require(
        old_score.to_bits() == 1.0f64.to_bits(),
        "first ANN region changed fixed score",
    )?;
    require(
        old_coverage.status == CoverageStatus::Complete
            && old_coverage.covered == Some(old_coverage.desired),
        "first ANN region lacks complete A coverage",
    )?;
    require(
        source(&fixture, old_view)? == a_source,
        "A materialization changed the source cut",
    )?;
    let mut old_pin = fixture
        .kernel
        .pin_view(ViewSelection::Exact(old_view), &work())?;
    old_pin.validate()?;
    current = stage_sources(&fixture, old_view, 65, 128, 67, 2)?;
    let b_source = source(&fixture, current)?;
    require(
        b_source != a_source,
        "B SourceAdvance did not create a distinct source cut",
    )?;
    for sequence in 68..=130 {
        old_pin.renew()?;
        current = materialize(
            &fixture,
            current,
            sequence,
            (sequence == 68).then(|| owner(65)),
        )?;
    }
    let before_view = current;
    let before_source = source(&fixture, before_view)?;
    require(
        before_source == b_source,
        "B materialization changed the source cut",
    )?;
    let (before_winner, before_score, before_coverage) =
        semantic_hit(&fixture, before_view, "alpha")?;
    require(
        before_winner == owner(1),
        "pre-fault ANN query lost the alpha owner",
    )?;
    require(
        before_score.to_bits() == 1.0f64.to_bits(),
        "pre-fault ANN score changed",
    )?;
    require(
        matches!(before_coverage.status, CoverageStatus::Partial { .. }),
        "pre-fault ANN coverage is not partial",
    )?;
    let remaining = fixture
        .kernel
        .ready_work(workspace(), 1, 128, &mut work())?;
    let [last] = remaining.as_slice() else {
        return Err("ANN fault boundary is not the last owner".into());
    };
    let recovery_started = std::time::Instant::now();
    let claim = fixture
        .kernel
        .claim_work(last, Duration::from_secs(120), &work())?;
    let before_io = fixture.observer.snapshot()?;

    let context = DiagnosticContextV1::new(
        DiagnosticCaptureIdV1([88; 16]),
        Arc::new(StepClock::default()),
        seal.clone(),
    );
    let trace_root = context.enter_root(DiagnosticRootKindV1::Operation, DiagnosticLaneV1(1))?;
    let traced = RequestBudgetV1::for_duration(Duration::from_secs(120))
        .with_diagnostic_context(context.child_context(&trace_root));
    let mut final_work = WorkBudget::with_request(
        WorkLimits {
            reads: 1_000_000,
            writes: 1_000_000,
            bytes: 1_500_000_000,
            mutations: 1_000_000,
        },
        traced,
    );
    let key = operation(131);
    let outcome = block_on(
        fixture.runtime.as_ref(),
        fixture.kernel.submit(
            UpdateRequest {
                operation: key,
                expected_view: before_view,
                kind: UpdateKind::Materialize { claim },
            },
            &mut final_work,
        ),
    );
    trace_root.finish(DiagnosticTerminalV1::from_result(&outcome))?;
    if !matches!(&outcome, Ok(SubmitOutcome::Unknown(found)) if *found == key) {
        let elapsed = recovery_started.elapsed();
        let remaining = final_work.request().remaining();
        let interruption = final_work.request().interruption();
        let io_delta = fixture
            .observer
            .snapshot()
            .and_then(|sample| sample.checked_since(&before_io));
        let seal_state = (
            seal.begins.load(Ordering::SeqCst),
            seal.errors.load(Ordering::SeqCst),
            seal.dropped.load(Ordering::SeqCst),
            seal.is_active(),
        );
        let pressure = filler
            .lock()
            .map(|state| state.triggered)
            .map_err(|_poison| "ANN filler lock poisoned");
        let cancellation = cancel
            .lock()
            .map(|state| format!("{state:?}"))
            .map_err(|_poison| "ANN cancel evidence lock poisoned");
        let status = fixture.kernel.status(
            key,
            quanta_index_core::index::ports::OperationInspectAccess::Request(&work()),
        );
        return Err(format!(
            "actual ANN write failure did not retain the original unknown operation: \
             outcome={outcome:?}; status={status:?}; work={final_work:?}; \
             elapsed={elapsed:?}; remaining={remaining:?}; interruption={interruption:?}; \
             seal_begin_error_drop_active={seal_state:?}; io={io_delta:?}; \
             pressure_triggered={pressure:?}; cancel={cancellation:?}"
        )
        .into());
    }
    require(
        matches!(
            fixture.kernel.status(
                key,
                quanta_index_core::index::ports::OperationInspectAccess::Request(&work()),
            )?,
            OperationStatus::Pending(_)
        ),
        "failed ANN operation is not pending for same-key recovery",
    )?;
    require(
        seal.begins.load(Ordering::SeqCst) == 1
            && seal.errors.load(Ordering::SeqCst) == 1
            && !seal.dropped.load(Ordering::SeqCst)
            && !seal.is_active(),
        "ANN fault lacks one closed SemanticSeal error span",
    )?;
    let delta = fixture.observer.snapshot()?.checked_since(&before_io)?;
    require(
        delta.enospc_writes == 1 && delta.failed_writes == 1,
        "ANN file-set did not reach one actual observed kernel ENOSPC write",
    )?;
    require(
        filler
            .lock()
            .map_err(|_poison| "ANN filler lock poisoned")?
            .triggered,
        "ANN pressure did not start after created()",
    )?;
    let cancelled = cancel
        .lock()
        .map_err(|_poison| "ANN cancel evidence lock poisoned")?;
    if cancelled.count != 1 || !matches!(cancelled.first.as_ref(), Some(Ok(()))) {
        return Err(format!(
            "actual temporary writer cancellation was not witnessed: {cancelled:?}"
        )
        .into());
    }
    drop(cancelled);

    // Free only the external pressure file after the original writer's real
    // cancellation has been witnessed. New query tickets write Catalog rows,
    // so acquire them only after space is available, before any resume. No
    // claim or operation is regenerated by removing the external filler.
    filler
        .lock()
        .map_err(|_poison| "ANN filler lock poisoned")?
        .remove()?;
    require(
        latest_view(&fixture)? == before_view,
        "failed ANN seal published a new current root",
    )?;
    require(
        source(&fixture, before_view)? == before_source,
        "failed ANN seal changed the source cut",
    )?;
    old_pin.validate()?;
    let (failed_winner, failed_score, failed_coverage) =
        semantic_hit(&fixture, before_view, "alpha")?;
    require(
        (failed_winner, failed_score.to_bits(), failed_coverage)
            == (before_winner, before_score.to_bits(), before_coverage),
        "failed ANN seal changed the current query or coverage",
    )?;
    let (pinned_winner, pinned_score, pinned_coverage) = semantic_hit(&fixture, old_view, "alpha")?;
    require(
        (pinned_winner, pinned_score.to_bits(), pinned_coverage)
            == (old_winner, old_score.to_bits(), old_coverage),
        "failed ANN seal changed the held old-view query",
    )?;
    let final_view = published(&block_on(
        fixture.runtime.as_ref(),
        fixture.kernel.resume(key, &mut work()),
    )?)?;
    require(
        recovery_started.elapsed() < Duration::from_secs(120),
        "same-key ANN recovery exceeded the original claim window",
    )?;
    require(
        source(&fixture, final_view)? == before_source,
        "same-key ANN recovery changed the source cut",
    )?;
    let (final_winner, final_score, final_coverage) = semantic_hit(&fixture, final_view, "alpha")?;
    require(
        final_winner == owner(1)
            && final_score.to_bits() == 1.0f64.to_bits()
            && final_coverage.status == CoverageStatus::Complete,
        "recovered ANN view lacks complete alpha coverage",
    )?;
    let (beta_winner, beta_score, beta_coverage) = semantic_hit(&fixture, final_view, "gamma")?;
    require(
        beta_winner == owner(65)
            && beta_score.to_bits() == 1.0f64.to_bits()
            && beta_coverage == final_coverage,
        "recovered ANN view lacks the independent gamma owner",
    )?;
    old_pin.validate()?;
    // A fresh reader reloads the immutable ANN file set using the same live
    // managed Catalog/lease while the old selected view remains pinned.
    let warm_reader = semantic_registry(
        Arc::clone(&fixture.bridge),
        &fixture.recipe,
        Arc::clone(&fixture.runtime),
    )?;
    let (reopened_old, reopened_old_score, reopened_old_coverage) =
        semantic_hit_with(&fixture.kernel, &warm_reader, old_view, "alpha")?;
    require(
        (
            reopened_old,
            reopened_old_score.to_bits(),
            reopened_old_coverage,
        ) == (old_winner, old_score.to_bits(), old_coverage),
        "reopen changed the held old ANN view",
    )?;
    let (reopened_final, reopened_final_score, reopened_final_coverage) =
        semantic_hit_with(&fixture.kernel, &warm_reader, final_view, "alpha")?;
    require(
        (
            reopened_final,
            reopened_final_score.to_bits(),
            reopened_final_coverage,
        ) == (final_winner, final_score.to_bits(), final_coverage),
        "reopen changed the recovered ANN view",
    )?;
    drop(warm_reader);
    require(
        pin_count(&fixture.catalog)? == 1,
        "old selected-view pin disappeared early",
    )?;
    old_pin.release()?;
    require(
        pin_count(&fixture.catalog)? == 0,
        "old selected-view pin did not release",
    )?;
    drop(fixture);
    drop(runtime);
    // A separate process proves restart behavior after all original managed
    // owners drop. The released old view may be reclaimed; verify final B.
    let output = std::process::Command::new(std::env::current_exe()?)
        .args(["--exact", EXACT_TEST, "--ignored", "--test-threads=1"])
        .env(COLD_CHILD, "1")
        .env(COLD_DIRECTORY, directory.path())
        .env(COLD_VIEW, serde_json::to_string(&final_view)?)
        .env(COLD_SOURCE, serde_json::to_string(&before_source)?)
        .env(COLD_COVERAGE, serde_json::to_string(&final_coverage)?)
        .output()?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    if !output.status.success()
        || !stdout.contains("running 1 test")
        || !stdout.contains("test result: ok. 1 passed; 0 failed;")
    {
        return Err(format!(
            "ANN cold child did not pass its exact ignored test: status={}; stdout={}; stderr={}",
            output.status,
            stdout,
            String::from_utf8_lossy(&output.stderr),
        )
        .into());
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn fuse_fault_mount_and_control() -> TestResult<(PathBuf, PathBuf)> {
    let mount =
        PathBuf::from(std::env::var_os("QIDX_REAL_FAULT_MOUNT").ok_or("ANN FUSE mount absent")?);
    let control = PathBuf::from(
        std::env::var_os("QIDX_REAL_FAULT_CONTROL").ok_or("ANN FUSE control absent")?,
    );
    require(
        mount.is_absolute() && control.is_absolute() && fs::canonicalize(&mount)? == mount,
        "ANN FUSE fixture paths are not absolute and canonical",
    )?;
    require(
        fs::symlink_metadata(&control)?.file_type().is_file()
            && fs::metadata(&mount)?.dev() != fs::metadata(&control)?.dev(),
        "ANN FUSE control shares the fault filesystem",
    )?;
    let mount_name = mount.to_str().ok_or("ANN FUSE mount is not UTF-8")?;
    require(
        fs::read_to_string("/proc/self/mountinfo")?
            .lines()
            .any(|line| {
                line.split_whitespace().nth(4) == Some(mount_name)
                    && line.contains(" - fuse.passthrough-fault ")
            }),
        "ANN FUSE mount lacks the pinned backend identity",
    )?;
    let stat = rustix::fs::statvfs(&mount)?;
    let total = stat
        .f_blocks
        .checked_mul(stat.f_frsize)
        .ok_or("ANN FUSE bounded capacity overflow")?;
    require(
        (32 * 1024 * 1024..=128 * 1024 * 1024).contains(&total),
        "ANN FUSE fixture is not on a bounded filesystem",
    )?;
    fs::write(&control, b"off none\n")?;
    Ok((mount, control))
}

#[cfg(target_os = "linux")]
struct AnnFaultBefore {
    old_view: ViewRef,
    old_source: quanta_index_contract::index::SourceCut,
    old_coverage: CoverageBinding,
    old_pin: ReadTicket,
    current_view: ViewRef,
    current_source: quanta_index_contract::index::SourceCut,
    current_coverage: CoverageBinding,
}

#[cfg(target_os = "linux")]
fn prepare_ann_fault_before(fixture: &AnnFixture) -> TestResult<AnnFaultBefore> {
    let configured = configure(fixture)?;
    let mut current = stage_sources(fixture, configured, 1, 64, 2, 1)?;
    let old_source = source(fixture, current)?;
    for sequence in 3..=66 {
        current = materialize(
            fixture,
            current,
            sequence,
            (sequence == 3).then(|| owner(1)),
        )?;
    }
    let old_view = current;
    let (old_winner, old_score, old_coverage) = semantic_hit(fixture, old_view, "alpha")?;
    require(
        old_winner == owner(1)
            && old_score.to_bits() == 1.0f64.to_bits()
            && old_coverage.status == CoverageStatus::Complete
            && old_coverage.covered == Some(old_coverage.desired)
            && source(fixture, old_view)? == old_source,
        "ANN FUSE A view failed fixed complete-coverage oracle",
    )?;
    let mut old_pin = fixture
        .kernel
        .pin_view(ViewSelection::Exact(old_view), &work())?;
    old_pin.validate()?;
    current = stage_sources(fixture, old_view, 65, 128, 67, 2)?;
    let current_source = source(fixture, current)?;
    require(
        current_source != old_source,
        "ANN FUSE B source cut equals A",
    )?;
    for sequence in 68..=130 {
        old_pin.renew()?;
        current = materialize(
            fixture,
            current,
            sequence,
            (sequence == 68).then(|| owner(65)),
        )?;
    }
    let current_view = current;
    let (current_winner, current_score, current_coverage) =
        semantic_hit(fixture, current_view, "alpha")?;
    require(
        current_winner == owner(1)
            && current_score.to_bits() == 1.0f64.to_bits()
            && matches!(current_coverage.status, CoverageStatus::Partial { .. })
            && source(fixture, current_view)? == current_source,
        "ANN FUSE pre-fault B view failed fixed partial-coverage oracle",
    )?;
    Ok(AnnFaultBefore {
        old_view,
        old_source,
        old_coverage,
        old_pin,
        current_view,
        current_source,
        current_coverage,
    })
}

/// Run only under a private 32..=128 MiB fuse.passthrough-fault mount with
/// exact-basename fsync control outside the mount.
#[cfg(target_os = "linux")]
#[test]
#[ignore = "requires bounded FUSE fault backend, exact control and isolated mount"]
fn real_ann_seal_file_fsync_eio_preserves_selected_views_and_recovers_same_operation() -> TestResult
{
    let (mount, control) = fuse_fault_mount_and_control()?;
    if std::env::var_os(COLD_CHILD).as_deref() == Some(std::ffi::OsStr::new("1")) {
        return cold_child(&mount);
    }
    let directory = tempfile::Builder::new()
        .prefix("qidx-ann-enospc-")
        .tempdir_in(&mount)?;
    let seal = Arc::new(SealEvents::default());
    let fault = Arc::new(FuseFault {
        kind: FuseFaultKind::Fsync,
        seal: Arc::clone(&seal),
        control,
        device: fs::metadata(&mount)?.dev(),
        armed: AtomicBool::new(false),
        cancel: Mutex::new(CancelEvidence::default()),
        retire_error: Mutex::new(None),
    });
    let runtime = Arc::new(
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?,
    );
    let fixture = open_fixture(
        directory.path(),
        Arc::clone(&runtime),
        Some(FaultMode::Fuse(Arc::clone(&fault))),
    )?;
    let before = prepare_ann_fault_before(&fixture)?;
    let remaining = fixture
        .kernel
        .ready_work(workspace(), 1, 128, &mut work())?;
    let [last] = remaining.as_slice() else {
        return Err("ANN FUSE fault boundary is not the final owner".into());
    };
    let claim = fixture
        .kernel
        .claim_work(last, Duration::from_secs(120), &work())?;
    let before_io = fixture.observer.snapshot()?;
    let context = DiagnosticContextV1::new(
        DiagnosticCaptureIdV1([89; 16]),
        Arc::new(StepClock::default()),
        seal.clone(),
    );
    let trace_root = context.enter_root(DiagnosticRootKindV1::Operation, DiagnosticLaneV1(1))?;
    let traced = RequestBudgetV1::for_duration(Duration::from_secs(120))
        .with_diagnostic_context(context.child_context(&trace_root));
    let mut final_work = WorkBudget::with_request(
        WorkLimits {
            reads: 1_000_000,
            writes: 1_000_000,
            bytes: 1_500_000_000,
            mutations: 1_000_000,
        },
        traced,
    );
    let key = operation(131);
    let outcome = block_on(
        fixture.runtime.as_ref(),
        fixture.kernel.submit(
            UpdateRequest {
                operation: key,
                expected_view: before.current_view,
                kind: UpdateKind::Materialize { claim },
            },
            &mut final_work,
        ),
    );
    trace_root.finish(DiagnosticTerminalV1::from_result(&outcome))?;
    // Clear the external fault before any Catalog/query writes or retry.
    fault.clear()?;
    require(
        matches!(outcome, Ok(SubmitOutcome::Unknown(found)) if found == key),
        "ANN FUSE fsync failure did not preserve the same unknown operation",
    )?;
    require(
        matches!(
            fixture.kernel.status(
                key,
                quanta_index_core::index::ports::OperationInspectAccess::Request(&work()),
            )?,
            OperationStatus::Pending(_)
        ),
        "ANN FUSE fsync operation is not pending",
    )?;
    require(
        fault.armed.load(Ordering::SeqCst)
            && seal.begins.load(Ordering::SeqCst) == 1
            && seal.errors.load(Ordering::SeqCst) == 1
            && !seal.dropped.load(Ordering::SeqCst)
            && !seal.is_active(),
        "ANN FUSE fsync was not one closed SemanticSeal error",
    )?;
    let delta = fixture.observer.snapshot()?.checked_since(&before_io)?;
    require(
        delta.failed_file_syncs == 1,
        "ANN FUSE fsync was not observed as one failed file sync",
    )?;
    let cancelled = fault
        .cancel
        .lock()
        .map_err(|_| "ANN FUSE cancel lock poisoned")?;
    require(
        cancelled.count == 1 && matches!(cancelled.first.as_ref(), Some(Ok(()))),
        "ANN FUSE failed temporary writer was not safely cancelled",
    )?;
    drop(cancelled);
    require(
        latest_view(&fixture)? == before.current_view
            && source(&fixture, before.current_view)? == before.current_source,
        "ANN FUSE fsync published or changed the B source cut",
    )?;
    before.old_pin.validate()?;
    let (old_winner, old_score, old_coverage) = semantic_hit(&fixture, before.old_view, "alpha")?;
    require(
        old_winner == owner(1)
            && old_score.to_bits() == 1.0f64.to_bits()
            && old_coverage == before.old_coverage
            && source(&fixture, before.old_view)? == before.old_source,
        "ANN FUSE fsync changed the held A view",
    )?;
    let (current_winner, current_score, current_coverage) =
        semantic_hit(&fixture, before.current_view, "alpha")?;
    require(
        current_winner == owner(1)
            && current_score.to_bits() == 1.0f64.to_bits()
            && current_coverage == before.current_coverage,
        "ANN FUSE fsync changed the selected B view",
    )?;
    let final_view = published(&block_on(
        fixture.runtime.as_ref(),
        fixture.kernel.resume(key, &mut work()),
    )?)?;
    require(
        source(&fixture, final_view)? == before.current_source,
        "ANN FUSE same-key recovery changed B source",
    )?;
    let (winner, score, final_coverage) = semantic_hit(&fixture, final_view, "alpha")?;
    let (gamma_winner, gamma_score, gamma_coverage) = semantic_hit(&fixture, final_view, "gamma")?;
    require(
        winner == owner(1)
            && score.to_bits() == 1.0f64.to_bits()
            && gamma_winner == owner(65)
            && gamma_score.to_bits() == 1.0f64.to_bits()
            && final_coverage.status == CoverageStatus::Complete
            && final_coverage == gamma_coverage,
        "ANN FUSE same-key recovery lacks independently fixed A/B ANN gold",
    )?;
    let warm_reader = semantic_registry(
        Arc::clone(&fixture.bridge),
        &fixture.recipe,
        Arc::clone(&fixture.runtime),
    )?;
    let (warm_old, warm_old_score, warm_old_coverage) =
        semantic_hit_with(&fixture.kernel, &warm_reader, before.old_view, "alpha")?;
    let (warm_final, warm_final_score, warm_final_coverage) =
        semantic_hit_with(&fixture.kernel, &warm_reader, final_view, "gamma")?;
    require(
        warm_old == owner(1)
            && warm_old_score.to_bits() == 1.0f64.to_bits()
            && warm_old_coverage == before.old_coverage
            && warm_final == owner(65)
            && warm_final_score.to_bits() == 1.0f64.to_bits()
            && warm_final_coverage == final_coverage,
        "ANN FUSE warm reopen changed A/B ANN gold",
    )?;
    drop(warm_reader);
    before.old_pin.release()?;
    require(
        pin_count(&fixture.catalog)? == 0,
        "ANN FUSE old pin did not release",
    )?;
    drop(fixture);
    drop(runtime);
    let output = std::process::Command::new(std::env::current_exe()?)
        .args(["--exact", EXACT_FSYNC_TEST, "--ignored", "--test-threads=1"])
        .env(COLD_CHILD, "1")
        .env(COLD_DIRECTORY, directory.path())
        .env(COLD_VIEW, serde_json::to_string(&final_view)?)
        .env(COLD_SOURCE, serde_json::to_string(&before.current_source)?)
        .env(COLD_COVERAGE, serde_json::to_string(&final_coverage)?)
        .output()?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    require(
        output.status.success()
            && stdout.contains("running 1 test")
            && stdout.contains("test result: ok. 1 passed; 0 failed;"),
        "ANN FUSE cold child failed exact process-reopen oracle",
    )?;
    Ok(())
}

#[cfg(target_os = "linux")]
#[derive(serde::Serialize, serde::Deserialize)]
struct CloseExpected {
    old_pin_session: u64,
    old_pin_session_fence: u64,
    old_pin_fence: u64,
    old_pin_expires_at_ms: u64,
    old_view: ViewRef,
    old_source: quanta_index_contract::index::SourceCut,
    old_coverage: CoverageBinding,
    current_view: ViewRef,
    current_source: quanta_index_contract::index::SourceCut,
    current_coverage: CoverageBinding,
}

#[cfg(target_os = "linux")]
fn close_fault_child(mount: &Path, control: PathBuf) -> TestResult {
    let directory =
        PathBuf::from(std::env::var_os(CLOSE_DIRECTORY).ok_or("ANN close child directory absent")?);
    let evidence = PathBuf::from(
        std::env::var_os(CLOSE_EVIDENCE).ok_or("ANN close child evidence path absent")?,
    );
    let name = directory
        .file_name()
        .and_then(std::ffi::OsStr::to_str)
        .ok_or("ANN close child directory name absent")?;
    require(
        directory.parent() == Some(mount)
            && name.starts_with("qidx-ann-close-")
            && fs::canonicalize(&directory)? == directory
            && fs::metadata(&directory)?.dev() == fs::metadata(mount)?.dev()
            && evidence.parent() == control.parent()
            && fs::symlink_metadata(&evidence)?.file_type().is_file()
            && fs::metadata(&evidence)?.dev() != fs::metadata(mount)?.dev(),
        "ANN close child paths escaped owned fault mount or control parent",
    )?;
    let seal = Arc::new(SealEvents::default());
    let fault = Arc::new(FuseFault {
        kind: FuseFaultKind::Close,
        seal: Arc::clone(&seal),
        control,
        device: fs::metadata(mount)?.dev(),
        armed: AtomicBool::new(false),
        cancel: Mutex::new(CancelEvidence::default()),
        retire_error: Mutex::new(None),
    });
    let runtime = Arc::new(
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?,
    );
    let fixture = open_fixture(
        &directory,
        Arc::clone(&runtime),
        Some(FaultMode::Fuse(Arc::clone(&fault))),
    )?;
    let before = prepare_ann_fault_before(&fixture)?;
    let remaining = fixture
        .kernel
        .ready_work(workspace(), 1, 128, &mut work())?;
    let [last] = remaining.as_slice() else {
        return Err("ANN close fault boundary is not the final owner".into());
    };
    let claim = fixture
        .kernel
        .claim_work(last, Duration::from_secs(120), &work())?;
    let context = DiagnosticContextV1::new(
        DiagnosticCaptureIdV1([90; 16]),
        Arc::new(StepClock::default()),
        seal.clone(),
    );
    let trace_root = context.enter_root(DiagnosticRootKindV1::Operation, DiagnosticLaneV1(1))?;
    let traced = RequestBudgetV1::for_duration(Duration::from_secs(120))
        .with_diagnostic_context(context.child_context(&trace_root));
    let mut final_work = WorkBudget::with_request(
        WorkLimits {
            reads: 1_000_000,
            writes: 1_000_000,
            bytes: 1_500_000_000,
            mutations: 1_000_000,
        },
        traced,
    );
    let key = operation(131);
    let outcome = block_on(
        fixture.runtime.as_ref(),
        fixture.kernel.submit(
            UpdateRequest {
                operation: key,
                expected_view: before.current_view,
                kind: UpdateKind::Materialize { claim },
            },
            &mut final_work,
        ),
    );
    trace_root.finish(DiagnosticTerminalV1::from_result(&outcome))?;
    fault.clear()?;
    require(
        matches!(outcome, Ok(SubmitOutcome::Unknown(found)) if found == key),
        "ANN checked-close EIO did not retain original unknown operation",
    )?;
    let close_error = fault
        .retire_error
        .lock()
        .map_err(|_| "ANN checked-close evidence lock poisoned")?
        .clone()
        .ok_or("ANN checked-close result was not observed")?;
    require(
        fault.armed.load(Ordering::SeqCst)
            && close_error.contains("close actual writer FD")
            && close_error.contains("os error 5")
            && seal.begins.load(Ordering::SeqCst) == 1
            && seal.errors.load(Ordering::SeqCst) == 1
            && !seal.dropped.load(Ordering::SeqCst)
            && !seal.is_active(),
        "ANN checked-close EIO lacked original FD and closed SemanticSeal error",
    )?;
    // The ambiguous original close freezes further in-process physical
    // admission. The cold parent checks the journal and selected view after
    // this process exits; a same-process read here would violate that gate.
    // Keep the expected source/view identities outside the quarantined root.
    // The parent will not open a second authority until this process exits.
    let held = before.old_pin.record().lease;
    let expected = CloseExpected {
        old_pin_session: held.session.id,
        old_pin_session_fence: held.session.fence,
        old_pin_fence: held.fence,
        old_pin_expires_at_ms: held.expires_at_ms,
        old_view: before.old_view,
        old_source: before.old_source,
        old_coverage: before.old_coverage,
        current_view: before.current_view,
        current_source: before.current_source,
        current_coverage: before.current_coverage,
    };
    fs::write(&evidence, serde_json::to_vec(&expected)?)?;
    File::open(&evidence)?.sync_all()?;
    File::open(
        evidence
            .parent()
            .ok_or("ANN close evidence parent absent")?,
    )?
    .sync_all()?;
    // Preserve the actual child-owned pin until process death. ReadTicket's
    // Drop calls Catalog.release_ticket; that would be a forbidden in-process
    // admission after the ambiguous close and would erase the stale-pin oracle.
    // The OS closes this process's read-session lock on exit. The cold owner
    // must retire the dead session using the original Catalog recovery port.
    std::mem::forget(before.old_pin);
    Ok(())
}

/// Fault child retains quarantine until exit. Its parent then proves restart
/// inventory, no false publication, fixed A/B gold, and same-operation replay.
#[cfg(target_os = "linux")]
#[test]
#[ignore = "requires bounded FUSE fault backend, checked-close EIO and cold process reopen"]
fn real_ann_seal_checked_close_eio_recovers_only_after_process_exit() -> TestResult {
    let (mount, control) = fuse_fault_mount_and_control()?;
    if std::env::var_os(CLOSE_PHASE).as_deref() == Some(std::ffi::OsStr::new("fault")) {
        return close_fault_child(&mount, control);
    }
    let directory = tempfile::Builder::new()
        .prefix("qidx-ann-close-")
        .tempdir_in(&mount)?;
    let evidence_parent = control.parent().ok_or("ANN close control parent absent")?;
    let evidence = tempfile::Builder::new()
        .prefix("qidx-ann-close-expected-")
        .tempfile_in(evidence_parent)?;
    require(
        fs::metadata(evidence.path())?.dev() != fs::metadata(&mount)?.dev(),
        "ANN close expected result shares fault mount",
    )?;
    let output = std::process::Command::new(std::env::current_exe()?)
        .args(["--exact", EXACT_CLOSE_TEST, "--ignored", "--test-threads=1"])
        .env(CLOSE_PHASE, "fault")
        .env(CLOSE_DIRECTORY, directory.path())
        .env(CLOSE_EVIDENCE, evidence.path())
        .output()?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    if !output.status.success()
        || !stdout.contains("running 1 test")
        || !stdout.contains("test result: ok. 1 passed; 0 failed;")
    {
        return Err(format!(
            "ANN checked-close fault child failed: status={}; stdout={}; stderr={}",
            output.status,
            stdout,
            String::from_utf8_lossy(&output.stderr),
        )
        .into());
    }
    fs::write(&control, b"off none\n")?;
    let expected: CloseExpected = serde_json::from_slice(&fs::read(evidence.path())?)?;
    let runtime = Arc::new(
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?,
    );
    let reopened = open_fixture(directory.path(), Arc::clone(&runtime), None)?;
    // The failed materialization also owns an internal read ticket. Its
    // release can fail inside the quarantined process. Authenticate the held
    // A pin and every retained pin's dead session/view instead of assuming
    // that exactly one pin survives. Expiry alone is not release authority.
    require(
        expected.old_view.workspace == expected.current_view.workspace
            && expected.old_view.epoch == expected.current_view.epoch,
        "ANN checked-close expected A/B scopes differ",
    )?;
    let connection = rusqlite::Connection::open_with_flags(
        reopened.catalog.path(),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )?;
    let (stale_pins, child_pins, held_pins, selected_pins): (i64, i64, i64, i64) = connection
        .query_row(
            "SELECT COUNT(*),
             COUNT(*) FILTER (WHERE session_id=?1 AND session_fence=?2),
             COUNT(*) FILTER (WHERE session_id=?1 AND session_fence=?2
                 AND fence=?3 AND expires_at_ms=?4
                 AND workspace=?5 AND epoch=?6 AND sequence=?7),
             COUNT(*) FILTER (WHERE workspace=?5 AND epoch=?6
                 AND (sequence=?7 OR sequence=?8))
             FROM native_pin_v1",
            rusqlite::params![
                i64::try_from(expected.old_pin_session)?,
                i64::try_from(expected.old_pin_session_fence)?,
                i64::try_from(expected.old_pin_fence)?,
                i64::try_from(expected.old_pin_expires_at_ms)?,
                serde_json::to_vec(&expected.old_view.workspace)?,
                i64::try_from(expected.old_view.epoch)?,
                i64::try_from(expected.old_view.sequence)?,
                i64::try_from(expected.current_view.sequence)?,
            ],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )?;
    drop(connection);
    if stale_pins < 1 || child_pins != stale_pins || held_pins != 1 || selected_pins != stale_pins {
        return Err(format!(
            "ANN checked-close cold custody changed: stale={stale_pins}, \
             child={child_pins}, held={held_pins}, selected={selected_pins}"
        )
        .into());
    }
    let recovered = reopened
        .kernel
        .recover_read_sessions(None, 16, &mut work())?;
    let remaining_pins = pin_count(&reopened.catalog)?;
    if recovered.recovered != 1
        || i64::from(recovered.pins_released) != stale_pins
        || recovered.has_more
        || remaining_pins != 0
    {
        return Err(format!(
            "ANN checked-close dead-session recovery incomplete: \
             before={stale_pins}, recovered={recovered:?}, after={remaining_pins}"
        )
        .into());
    }
    println!("ANN close cold recovery: dead_sessions=1, pins_released={stale_pins}, remaining=0");
    require(
        latest_view(&reopened)? == expected.current_view
            && source(&reopened, expected.current_view)? == expected.current_source,
        "ANN checked-close cold boot selected an uncommitted view",
    )?;
    require(
        matches!(
            reopened.kernel.status(
                operation(131),
                quanta_index_core::index::ports::OperationInspectAccess::Request(&work()),
            )?,
            OperationStatus::Pending(_)
        ),
        "ANN checked-close cold boot lost original pending operation",
    )?;
    let (old_winner, old_score, old_coverage) =
        semantic_hit(&reopened, expected.old_view, "alpha")?;
    let (current_winner, current_score, current_coverage) =
        semantic_hit(&reopened, expected.current_view, "alpha")?;
    require(
        old_winner == owner(1)
            && old_score.to_bits() == 1.0f64.to_bits()
            && old_coverage == expected.old_coverage
            && source(&reopened, expected.old_view)? == expected.old_source
            && current_winner == owner(1)
            && current_score.to_bits() == 1.0f64.to_bits()
            && current_coverage == expected.current_coverage,
        "ANN checked-close cold boot changed held A or selected B gold",
    )?;
    let final_view = published(&block_on(
        reopened.runtime.as_ref(),
        reopened.kernel.resume(operation(131), &mut work()),
    )?)?;
    require(
        source(&reopened, final_view)? == expected.current_source,
        "ANN checked-close same-key recovery changed B source",
    )?;
    let (alpha_winner, alpha_score, coverage) = semantic_hit(&reopened, final_view, "alpha")?;
    let (gamma_winner, gamma_score, gamma_coverage) = semantic_hit(&reopened, final_view, "gamma")?;
    require(
        alpha_winner == owner(1)
            && alpha_score.to_bits() == 1.0f64.to_bits()
            && gamma_winner == owner(65)
            && gamma_score.to_bits() == 1.0f64.to_bits()
            && coverage.status == CoverageStatus::Complete
            && coverage == gamma_coverage,
        "ANN checked-close cold replay lacks fixed A/B ANN gold",
    )?;
    let warm_reader = semantic_registry(
        Arc::clone(&reopened.bridge),
        &reopened.recipe,
        Arc::clone(&reopened.runtime),
    )?;
    let (warm_winner, warm_score, warm_coverage) =
        semantic_hit_with(&reopened.kernel, &warm_reader, final_view, "gamma")?;
    require(
        warm_winner == owner(65)
            && warm_score.to_bits() == 1.0f64.to_bits()
            && warm_coverage == coverage,
        "ANN checked-close recovered B gold changed on fresh reader",
    )?;
    Ok(())
}
