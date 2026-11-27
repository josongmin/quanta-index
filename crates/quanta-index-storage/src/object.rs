use std::{
    fmt,
    fs::{self, File, OpenOptions},
    io::Write,
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, MutexGuard, TryLockError,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
    time::{SystemTime, UNIX_EPOCH},
};

use quanta_index_core::index::{
    AttemptRef, Digest, IndexError, MAX_RECLAIM_CHILDREN, ObjectBatchInput, ObjectInstallScope,
    ObjectRef, ObjectWriteSyncDirectories, RegisteredObject, ScratchSpacePort, StoredObject,
    WorkBudget, WorkUsage,
    ports::{ImmutableObjectPort, ManagedObjectAuthority, ObjectWriteSink},
};
use quanta_index_core::publication_capacity::InodeKey;
use quanta_index_core::{
    DiagnosticCounterV1, DiagnosticHealthCodeV1, DiagnosticLaneV1,
    DiagnosticManagedInstallCensusKindV1, DiagnosticObjectInstallPhaseV1 as InstallPhase,
    DiagnosticSpanGuardV1, DiagnosticStageV1, DiagnosticTerminalV1, DiagnosticTraceErrorV1,
};
use sha2::{Digest as _, Sha256};

use crate::scratch::FileScratchSpace;

mod batch_metadata;
mod io_observation;
#[cfg(test)]
mod managed_trace_tests;
mod namespace;
mod physical_observation;
mod prepared_leaf_run;
mod read_io;
mod read_only;
mod reclaim;
#[cfg(all(test, target_os = "macos"))]
mod retained_read_tests;
mod scalar_sync;
mod write_io;
mod write_preparation;

use batch_metadata::{
    BatchIndex, BatchReferences, BatchShards, ChildValidationSlot, RegistrationBatch,
    ValidatedExternalChildren,
};
use io_observation::{CustodyKind, SyncKind};
pub use io_observation::{ObjectIoObservation, ObjectIoSnapshot};
pub use read_only::FileObjectReader;
use scalar_sync::ScalarSync;

use namespace::NamespaceLease;
use physical_observation::{
    ChildCheckCounts, CrossGroupChildOverlap, PhysicalObservation, measure_physical,
};
#[cfg(target_os = "macos")]
use read_io::ObjectIoFile;
use read_io::ReadWire;
use write_io::ObjectWriter;

struct SyncedDirectory {
    _file: File,
    identity: InodeKey,
}

use quanta_index_core::index::{
    NATIVE_IMMUTABLE_OBJECT_HASH_DOMAIN_V1 as HASH_DOMAIN,
    NATIVE_IMMUTABLE_OBJECT_HEADER_BYTES_V1 as HEADER_LEN,
    NATIVE_IMMUTABLE_OBJECT_MAGIC_V1 as MAGIC,
};
const MAX_PAYLOAD: usize = 16 * 1024 * 1024;
const MAX_CHILDREN: usize = MAX_RECLAIM_CHILDREN;
const MAX_WIRE: usize = HEADER_LEN + MAX_CHILDREN * 32 + MAX_PAYLOAD;
const STANDALONE_SCRATCH_CAP: u64 = 16 * 1024 * 1024 * 1024;
// The adapter's caller input limit remains separate from child-first map DAG
// aggregation. Both paths retain the same aggregate wire-byte limit.
pub(crate) const MAX_BATCH_INPUTS: usize =
    quanta_index_core::index::ports::MAX_OBJECT_INSTALL_BATCH;
pub(crate) const MAX_INSTALL_BATCH_INPUTS: usize =
    quanta_index_core::index::ports::MAX_OBJECT_INSTALL_BATCH;
pub(crate) const MAX_BATCH_BYTES: u64 =
    quanta_index_core::index::MAX_NATIVE_IMMUTABLE_OBJECT_WIRE_BYTES_V1;
// Bound live packet backing and the number of installs under one gate union.
pub(crate) const MAX_PREPARED_INSTALL_GROUPS: usize = 4;
type InstallGateGuards<'a> = [Option<MutexGuard<'a, ()>>; 256];

/// Diagnostic-only evidence of physically validated reuse within one bounded
/// install group. Catalog custody still has to succeed before it is emitted.
#[derive(Clone, Copy, Default)]
struct CasReuseMask([u64; MAX_INSTALL_BATCH_INPUTS.div_ceil(64)]);

impl CasReuseMask {
    fn single() -> Self {
        let mut mask = Self::default();
        mask.mark(0);
        mask
    }

    fn mark(&mut self, index: usize) {
        if let Some(word) = self.0.get_mut(index / 64) {
            *word |= 1u64 << (index % 64);
        }
    }

    fn contains(&self, index: usize) -> bool {
        self.0
            .get(index / 64)
            .is_some_and(|word| word & (1u64 << (index % 64)) != 0)
    }

    fn count(&self) -> u64 {
        self.0.iter().map(|word| u64::from(word.count_ones())).sum()
    }
}

fn emit_confirmed_cas_reuse(
    objects: &[RegisteredObject],
    reuse: &CasReuseMask,
    budget: &WorkBudget,
) {
    let Some(context) = budget.request().diagnostic_context() else {
        return;
    };
    let kind = DiagnosticManagedInstallCensusKindV1::StorageCasReuse;
    context.managed_install_census_batch(kind, reuse.count());
    for (index, object) in objects.iter().enumerate() {
        if reuse.contains(index) {
            context.managed_install_census_entry(kind, object.reference, None);
        }
    }
}

#[cfg(target_os = "macos")]
fn batch_barrier(file: &File) -> Result<(), IndexError> {
    rustix::fs::fcntl_fullfsync(file)
        .map_err(|error| IndexError::Storage(format!("full sync immutable object batch: {error}")))
}

#[cfg(not(target_os = "macos"))]
fn batch_barrier(_file: &File) -> Result<(), IndexError> {
    Ok(())
}

pub(crate) use write_preparation::{AdmittedWire, PreparedObject};

fn batch_preparation_memory_bytes(count: usize) -> Result<u64, IndexError> {
    let bytes = count
        .checked_mul(
            std::mem::size_of::<PreparedObject>()
                .checked_add(std::mem::size_of::<&PreparedObject>())
                .ok_or(IndexError::ResourceLimited)?,
        )
        .ok_or(IndexError::ResourceLimited)?;
    u64::try_from(bytes).map_err(|_error| IndexError::ResourceLimited)
}

#[derive(Clone, Copy)]
enum BatchInstallPlan<'a> {
    Single,
    Run {
        attempt: AttemptRef,
        group: &'a PreparedInstallGroup,
        previous_groups: &'a [Option<PreparedInstallGroup>],
        gates: &'a InstallGateGuards<'a>,
        reserve: bool,
        confirm: bool,
    },
}

struct PreparedInstallGroup {
    references: BatchReferences,
    metadata: RegistrationBatch,
    shards: BatchShards,
}

/// A content-addressed object store. Every installed object is immutable and
/// synced before its reference is returned to a caller.
pub struct FileObjectStore {
    objects: PathBuf,
    scratch: Arc<dyn ScratchSpacePort>,
    temp_sequence: AtomicU64,
    namespace: NamespaceLease,
    authority: Option<Arc<dyn ManagedObjectAuthority>>,
    gates: [Mutex<()>; 256],
    io_observer: Option<Arc<ObjectIoObservation>>,
    reclaim_io: Option<Arc<dyn quanta_index_core::index::ObjectUnlinkPort>>,
    read_io: Option<Arc<dyn quanta_index_core::index::ObjectReadIoPort>>,
    write_io: Option<Arc<dyn quanta_index_core::index::ObjectWriteIoPort>>,
}

/// Required physical facets supplied before managed namespace construction.
///
/// The runtime builds every facet from one common capacity owner. The admitted
/// lock must already name this root's private `.object-namespace.lease` inode.
pub struct ManagedObjectIo {
    pub namespace_lock: Box<dyn quanta_index_core::index::ObjectReadFile>,
    pub read: Arc<dyn quanta_index_core::index::ObjectReadIoPort>,
    pub write: Arc<dyn quanta_index_core::index::ObjectWriteIoPort>,
    pub reclaim: Arc<dyn quanta_index_core::index::ObjectUnlinkPort>,
}

impl fmt::Debug for FileObjectStore {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("FileObjectStore")
            .field("objects", &self.objects)
            .finish_non_exhaustive()
    }
}

impl FileObjectStore {
    /// Attach before any writes. One owner controls scalar and batch outputs;
    /// the same managed Catalog still grants and confirms installation.
    pub fn with_write_io(
        mut self,
        io: Arc<dyn quanta_index_core::index::ObjectWriteIoPort>,
    ) -> Result<Self, IndexError> {
        if self.authority.is_none() {
            return Err(IndexError::BackendViolation(
                "physical write owner requires managed Catalog authority".into(),
            ));
        }
        self.write_io = Some(io);
        Ok(self)
    }

    /// Attach before any reads. Failure never falls back to a raw open.
    /// The existing managed Catalog and namespace remain the logical authority.
    pub fn with_read_io(
        mut self,
        io: Arc<dyn quanta_index_core::index::ObjectReadIoPort>,
    ) -> Result<Self, IndexError> {
        if self.authority.is_none() {
            return Err(IndexError::BackendViolation(
                "physical read owner requires managed Catalog authority".into(),
            ));
        }
        self.read_io = Some(io);
        Ok(self)
    }

    /// Install the physical unlink owner before managed reclaim work is issued.
    /// Its success still requires the original Catalog permit and absence checks.
    pub fn with_reclaim_io(
        mut self,
        io: Arc<dyn quanta_index_core::index::ObjectUnlinkPort>,
    ) -> Result<Self, IndexError> {
        if self.authority.is_none() {
            return Err(IndexError::BackendViolation(
                "physical reclaim owner requires managed Catalog authority".into(),
            ));
        }
        self.reclaim_io = Some(io);
        Ok(self)
    }

    /// Opt-in install-operation counters. Attach before issuing any work.
    /// Constructor/GC and Catalog/WAL IO counts are outside this observation.
    /// Custody call time includes the original authority/sink implementation.
    #[must_use]
    pub fn with_io_observation(mut self, observer: Arc<ObjectIoObservation>) -> Self {
        self.io_observer = Some(observer);
        self
    }

    fn write_observed(&self, file: &mut File, bytes: &[u8]) -> Result<(), IndexError> {
        let result = match &self.io_observer {
            Some(observer) => observer.write_all(file, bytes),
            None => file.write_all(bytes),
        };
        result.map_err(|error| storage_error(&error))
    }

    fn sync_observed(
        &self,
        kind: SyncKind,
        operation: impl FnOnce() -> Result<(), IndexError>,
    ) -> Result<(), IndexError> {
        match &self.io_observer {
            Some(observer) => observer.sync(kind, operation),
            None => operation(),
        }
    }

    fn sync_observed_witness(
        &self,
        operation: impl FnOnce() -> Result<SyncedDirectory, IndexError>,
    ) -> Result<SyncedDirectory, IndexError> {
        let mut witness = None;
        self.sync_observed(SyncKind::Directory, || {
            witness = Some(operation()?);
            Ok(())
        })?;
        witness.ok_or_else(|| {
            IndexError::BackendViolation("directory sync returned without an FD witness".into())
        })
    }

    fn sync_batch_directory_witness(
        &self,
        path: &Path,
        expected_device: Option<u64>,
    ) -> Result<SyncedDirectory, IndexError> {
        self.sync_observed_witness(|| {
            #[cfg(target_os = "macos")]
            if let Some(device) = expected_device {
                return sync_directory_plain_witness(path, device);
            }
            #[cfg(not(target_os = "macos"))]
            let _ = expected_device;
            sync_directory_witness(path)
        })
    }

    fn custody_observed(
        &self,
        kind: CustodyKind,
        objects: usize,
        operation: impl FnOnce() -> Result<(), IndexError>,
    ) -> Result<(), IndexError> {
        match &self.io_observer {
            Some(observer) => observer.custody(kind, objects, operation),
            None => operation(),
        }
    }

    fn begin_managed_custody_span(
        budget: &WorkBudget,
        stage: DiagnosticStageV1,
        inputs: usize,
    ) -> Option<DiagnosticSpanGuardV1> {
        let context = budget.request().diagnostic_context()?;
        match context.enter(stage, DiagnosticLaneV1(3)) {
            Ok(span) => {
                match u64::try_from(inputs) {
                    Ok(inputs) => {
                        context.counter(&span, DiagnosticCounterV1::ManagedObjectInputs, inputs);
                    }
                    Err(_error) => context.health(DiagnosticHealthCodeV1::CounterOverflow),
                }
                Some(span)
            }
            Err(error) => {
                context.health(match error {
                    DiagnosticTraceErrorV1::SpanIdentityExhausted => {
                        DiagnosticHealthCodeV1::CounterOverflow
                    }
                    DiagnosticTraceErrorV1::ClockRegressed => {
                        DiagnosticHealthCodeV1::ClockRegressed
                    }
                    DiagnosticTraceErrorV1::ForeignCaptureSpan => {
                        DiagnosticHealthCodeV1::ForeignCaptureSpan
                    }
                    DiagnosticTraceErrorV1::MissingParentSpan => {
                        DiagnosticHealthCodeV1::MissingParentSpan
                    }
                });
                None
            }
        }
    }

    fn finish_managed_custody_span<T, E>(
        span: Option<DiagnosticSpanGuardV1>,
        result: Result<T, E>,
    ) -> Result<T, E> {
        if let Some(span) = span {
            let _finished = span.finish(DiagnosticTerminalV1::from_result(&result));
        }
        result
    }

    fn with_managed_custody_span(
        budget: &mut WorkBudget,
        stage: DiagnosticStageV1,
        inputs: usize,
        operation: impl FnOnce(&mut WorkBudget) -> Result<(), IndexError>,
    ) -> Result<(), IndexError> {
        let span = Self::begin_managed_custody_span(budget, stage, inputs);
        let before = budget.usage();
        let child = span.as_ref().and_then(|span| {
            budget
                .request()
                .diagnostic_context()
                .map(|context| context.child_context(span))
        });
        let result = {
            let mut scoped = budget.scoped_diagnostic_context(child);
            operation(&mut scoped)
        };
        if let Some(span) = span.as_ref() {
            budget.record_diagnostic_usage_delta(before, span);
        }
        Self::finish_managed_custody_span(span, result)
    }

    /// Opens or creates a store. The caller must keep its directory private to
    /// this store and let the catalog govern object reclamation.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, IndexError> {
        let root = path.as_ref();
        Self::open_root(root)?;
        let namespace = NamespaceLease::shared(root)?;
        let objects = Self::open_objects(root)?;
        let scratch = Arc::new(FileScratchSpace::open_standalone(
            root.join("scratch"),
            STANDALONE_SCRATCH_CAP,
        )?);
        Ok(Self {
            objects,
            scratch,
            temp_sequence: AtomicU64::new(0),
            namespace,
            authority: None,
            gates: std::array::from_fn(|_index| Mutex::new(())),
            io_observer: None,
            reclaim_io: None,
            read_io: None,
            write_io: None,
        })
    }

    /// Opens a store with an explicitly owned shared scratch space. The
    /// service holds its namespace lease for every allocation lifetime.
    pub fn open_with_scratch(
        path: impl AsRef<Path>,
        scratch: Arc<dyn ScratchSpacePort>,
    ) -> Result<Self, IndexError> {
        let root = path.as_ref();
        Self::open_root(root)?;
        let namespace = NamespaceLease::shared(root)?;
        let objects = Self::open_objects(root)?;
        Ok(Self {
            objects,
            scratch,
            temp_sequence: AtomicU64::new(0),
            namespace,
            authority: None,
            gates: std::array::from_fn(|_index| Mutex::new(())),
            io_observer: None,
            reclaim_io: None,
            read_io: None,
            write_io: None,
        })
    }

    /// Opens one process-lifetime owner of the immutable object namespace.
    ///
    /// Standalone openers are excluded until this store and all `Arc` clones
    /// have been dropped. This constructor does not authorize physical GC.
    pub fn open_managed_with_scratch(
        path: impl AsRef<Path>,
        scratch: Arc<dyn ScratchSpacePort>,
        authority: Arc<dyn ManagedObjectAuthority>,
    ) -> Result<Self, IndexError> {
        Self::open_managed_inner(path.as_ref(), scratch, authority, None)
    }

    /// Construct with admitted namespace lifetime and all physical facets.
    /// A failed binding cannot select the unqualified lock/open path.
    pub fn open_managed_with_io(
        path: impl AsRef<Path>,
        scratch: Arc<dyn ScratchSpacePort>,
        authority: Arc<dyn ManagedObjectAuthority>,
        io: ManagedObjectIo,
    ) -> Result<Self, IndexError> {
        Self::open_managed_inner(path.as_ref(), scratch, authority, Some(io))
    }

    fn open_managed_inner(
        root: &Path,
        scratch: Arc<dyn ScratchSpacePort>,
        authority: Arc<dyn ManagedObjectAuthority>,
        io: Option<ManagedObjectIo>,
    ) -> Result<Self, IndexError> {
        Self::open_root(root)?;
        let (mut namespace, read_io, write_io, reclaim_io) = match io {
            Some(io) => (
                NamespaceLease::exclusive_with_io(root, io.namespace_lock)?,
                Some(io.read),
                Some(io.write),
                Some(io.reclaim),
            ),
            None => (NamespaceLease::exclusive(root)?, None, None, None),
        };
        let objects = Self::open_objects(root)?;
        namespace.bind_objects(&objects)?;
        authority.bind_namespace(namespace.validate_objects(&objects)?)?;
        Ok(Self {
            objects,
            scratch,
            temp_sequence: AtomicU64::new(0),
            namespace,
            authority: Some(authority),
            gates: std::array::from_fn(|_index| Mutex::new(())),
            io_observer: None,
            reclaim_io,
            read_io,
            write_io,
        })
    }

    fn open_root(root: &Path) -> Result<(), IndexError> {
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(root)
            .map_err(|error| storage_error(&error))?;
        validate_private_directory(root)?;
        if let Some(parent) = root.parent() {
            let parent = if parent.as_os_str().is_empty() {
                Path::new(".")
            } else {
                parent
            };
            sync_parent_directory(parent)?;
        }
        Ok(())
    }

    fn open_objects(root: &Path) -> Result<PathBuf, IndexError> {
        let objects = root.join("objects");
        match fs::DirBuilder::new().mode(0o700).create(&objects) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(storage_error(&error)),
        }
        validate_private_directory(&objects)?;
        sync_directory(root)?;
        Ok(objects)
    }

    pub(crate) fn path_for(&self, reference: ObjectRef) -> PathBuf {
        read_io::object_path(&self.objects, reference)
    }

    pub(crate) fn scratch_space(&self) -> &dyn ScratchSpacePort {
        self.scratch.as_ref()
    }

    fn gate_index(reference: ObjectRef) -> usize {
        usize::from((reference.0).0[0])
    }

    fn lock_gate(
        &self,
        index: usize,
        budget: &WorkBudget,
    ) -> Result<MutexGuard<'_, ()>, IndexError> {
        loop {
            budget.checkpoint()?;
            if let Some(guard) = self.try_lock_gate(index)? {
                return Ok(guard);
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    fn try_lock_gate(&self, index: usize) -> Result<Option<MutexGuard<'_, ()>>, IndexError> {
        let gate = self
            .gates
            .get(index)
            .ok_or_else(|| IndexError::BackendViolation("object gate index out of range".into()))?;
        match gate.try_lock() {
            Ok(guard) => Ok(Some(guard)),
            Err(TryLockError::WouldBlock) => Ok(None),
            Err(TryLockError::Poisoned(_)) => Err(IndexError::BackendViolation(
                "object mutation gate poisoned".into(),
            )),
        }
    }

    fn reserve_install(
        &self,
        sink: &dyn ObjectWriteSink,
        scope: Option<ObjectInstallScope>,
        objects: &[RegisteredObject],
        budget: &mut WorkBudget,
        batch: bool,
    ) -> Result<(), IndexError> {
        if let Some(authority) = &self.authority {
            let scope = scope.ok_or_else(|| {
                IndexError::BackendViolation("managed installation lacks catalog scope".into())
            })?;
            self.custody_observed(CustodyKind::Reserve, objects.len(), || {
                Self::with_managed_custody_span(
                    budget,
                    DiagnosticStageV1::ManagedObjectReserve,
                    objects.len(),
                    |budget| authority.reserve_managed(scope, objects, budget),
                )
            })
        } else if !batch {
            let object = objects.first().ok_or_else(|| {
                IndexError::InvalidInput("scalar object installation is empty".into())
            })?;
            self.custody_observed(CustodyKind::Reserve, 1, || sink.reserve(object))
        } else {
            self.custody_observed(CustodyKind::Reserve, objects.len(), || {
                sink.reserve_batch(objects)
            })
        }
    }

    fn confirm_install(
        &self,
        sink: &dyn ObjectWriteSink,
        scope: Option<ObjectInstallScope>,
        objects: &[RegisteredObject],
        budget: &mut WorkBudget,
        batch: bool,
        cas_reuse: &CasReuseMask,
    ) -> Result<(), IndexError> {
        if let Some(authority) = &self.authority {
            let scope = scope.ok_or_else(|| {
                IndexError::BackendViolation("managed installation lacks catalog scope".into())
            })?;
            self.custody_observed(CustodyKind::Confirm, objects.len(), || {
                Self::with_managed_custody_span(
                    budget,
                    DiagnosticStageV1::ManagedObjectConfirm,
                    objects.len(),
                    |budget| {
                        authority.confirm_managed(scope, objects, budget)?;
                        emit_confirmed_cas_reuse(objects, cas_reuse, budget);
                        Ok(())
                    },
                )
            })
        } else if !batch {
            let object = objects.first().ok_or_else(|| {
                IndexError::InvalidInput("scalar object installation is empty".into())
            })?;
            self.custody_observed(CustodyKind::Confirm, 1, || sink.installed(object))
        } else {
            self.custody_observed(CustodyKind::Confirm, objects.len(), || {
                sink.installed_batch(objects)
            })
        }
    }

    pub(crate) fn prepare_object(
        &self,
        payload: &[u8],
        children: &[ObjectRef],
        budget: &mut WorkBudget,
    ) -> Result<PreparedObject, IndexError> {
        write_preparation::prepare(payload, children, budget)
    }

    pub(crate) fn install_prepared_batch(
        &self,
        objects: &[&PreparedObject],
        sink: &dyn ObjectWriteSink,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        self.install_prepared_batch_with_barrier(objects, sink, budget, batch_barrier)
    }

    pub(crate) fn can_run_prepared_installs(&self, sink: &dyn ObjectWriteSink) -> bool {
        self.authority
            .as_ref()
            .is_some_and(|authority| authority.supports_atomic_attempt_advance())
            && matches!(sink.managed_scope(), Some(ObjectInstallScope::Attempt(_)))
    }

    pub(crate) fn install_prepared_run(
        &self,
        groups: &[&[&PreparedObject]],
        sink: &dyn ObjectWriteSink,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        if !(2..=MAX_PREPARED_INSTALL_GROUPS).contains(&groups.len()) {
            return Err(IndexError::ResourceLimited);
        }
        if !self.can_run_prepared_installs(sink) {
            return Err(IndexError::BackendViolation(
                "prepared install run requires atomic attempt advance".into(),
            ));
        }
        let mut total_bytes = 0u64;
        for group in groups {
            if group.is_empty() || group.len() > MAX_INSTALL_BATCH_INPUTS {
                return Err(IndexError::ResourceLimited);
            }
            for object in *group {
                total_bytes = total_bytes
                    .checked_add(object.registration.bytes)
                    .ok_or(IndexError::ResourceLimited)?;
            }
        }
        if total_bytes > MAX_BATCH_BYTES {
            return Err(IndexError::ResourceLimited);
        }
        let mut prepared: [Option<PreparedInstallGroup>; MAX_PREPARED_INSTALL_GROUPS] =
            std::array::from_fn(|_| None);
        for (index, group) in groups.iter().enumerate() {
            // Validate every packet before the first Catalog or filesystem effect.
            let references = BatchReferences::new(group, budget)?;
            let metadata = RegistrationBatch::new(group, budget)?;
            prepared[index] = Some(PreparedInstallGroup {
                references,
                shards: BatchShards::new(&self.objects, metadata.as_slice(), budget)?,
                metadata,
            });
        }
        self.install_admitted_run(groups, sink, budget, &prepared[..groups.len()])
    }

    fn install_admitted_run(
        &self,
        groups: &[&[&PreparedObject]],
        sink: &dyn ObjectWriteSink,
        budget: &mut WorkBudget,
        prepared: &[Option<PreparedInstallGroup>],
    ) -> Result<(), IndexError> {
        if groups.is_empty()
            || groups.len() > MAX_PREPARED_INSTALL_GROUPS
            || groups.len() != prepared.len()
        {
            return Err(IndexError::ResourceLimited);
        }
        let (authority, attempt) = match (self.authority.as_ref(), sink.managed_scope()) {
            (Some(authority), Some(ObjectInstallScope::Attempt(attempt)))
                if authority.supports_atomic_attempt_advance() =>
            {
                (authority, attempt)
            }
            _ => {
                return Err(IndexError::BackendViolation(
                    "admitted run scope differs".into(),
                ));
            }
        };
        let gates = self.lock_install_gates(groups, budget)?;
        let mut prior_cas_reuse = CasReuseMask::default();
        for (index, group) in groups.iter().enumerate() {
            let current = prepared[index].as_ref().ok_or_else(|| {
                IndexError::BackendViolation("prepared run metadata absent".into())
            })?;
            if index > 0 {
                let previous = prepared[index - 1].as_ref().ok_or_else(|| {
                    IndexError::BackendViolation("prepared run predecessor absent".into())
                })?;
                let confirmed = previous.metadata.as_slice();
                let next = current.metadata.as_slice();
                let transition_inputs = confirmed
                    .len()
                    .checked_add(next.len())
                    .ok_or(IndexError::ResourceLimited)?;
                let mut advance = || {
                    Self::with_managed_custody_span(
                        budget,
                        DiagnosticStageV1::ManagedObjectAdvance,
                        transition_inputs,
                        |budget| {
                            authority.advance_attempt_managed(attempt, confirmed, next, budget)?;
                            emit_confirmed_cas_reuse(confirmed, &prior_cas_reuse, budget);
                            Ok(())
                        },
                    )
                };
                match &self.io_observer {
                    Some(observer) => observer.advance(confirmed.len(), next.len(), advance),
                    None => advance(),
                }?;
            }
            let mut current_cas_reuse = CasReuseMask::default();
            self.install_prepared_batch_with_barrier_plan(
                group,
                sink,
                budget,
                batch_barrier,
                BatchInstallPlan::Run {
                    attempt,
                    group: current,
                    previous_groups: &prepared[..index],
                    gates: &gates,
                    reserve: index == 0,
                    confirm: index + 1 == groups.len(),
                },
                &mut current_cas_reuse,
            )?;
            prior_cas_reuse = current_cas_reuse;
        }
        Ok(())
    }

    fn lock_install_gates<'a>(
        &'a self,
        groups: &[&[&PreparedObject]],
        budget: &WorkBudget,
    ) -> Result<InstallGateGuards<'a>, IndexError> {
        let mut needed = [false; 256];
        for group in groups {
            for object in *group {
                needed[Self::gate_index(object.registration.reference)] = true;
            }
        }
        loop {
            budget.checkpoint()?;
            match self.try_lock_install_gate_union(&needed, budget)? {
                Ok(guards) => return Ok(guards),
                Err(contended) => {
                    // The failed attempt dropped its entire prefix. Wait for
                    // only the contended gate before rebuilding the union.
                    drop(self.lock_gate(contended, budget)?);
                }
            }
        }
    }

    /// Return the blocked gate index after releasing the acquired prefix.
    fn try_lock_install_gate_union<'a>(
        &'a self,
        needed: &[bool; 256],
        budget: &WorkBudget,
    ) -> Result<Result<InstallGateGuards<'a>, usize>, IndexError> {
        let mut guards = std::array::from_fn(|_| None);
        for (index, &required) in needed.iter().enumerate() {
            if required {
                budget.checkpoint()?;
                let Some(guard) = self.try_lock_gate(index)? else {
                    return Ok(Err(index));
                };
                guards[index] = Some(guard);
            }
        }
        Ok(Ok(guards))
    }

    fn install_prepared_batch_with_barrier(
        &self,
        objects: &[&PreparedObject],
        sink: &dyn ObjectWriteSink,
        budget: &mut WorkBudget,
        barrier: impl FnOnce(&File) -> Result<(), IndexError>,
    ) -> Result<(), IndexError> {
        let mut cas_reuse = CasReuseMask::default();
        self.install_prepared_batch_with_barrier_plan(
            objects,
            sink,
            budget,
            barrier,
            BatchInstallPlan::Single,
            &mut cas_reuse,
        )
    }

    fn install_prepared_batch_with_barrier_plan(
        &self,
        objects: &[&PreparedObject],
        sink: &dyn ObjectWriteSink,
        budget: &mut WorkBudget,
        barrier: impl FnOnce(&File) -> Result<(), IndexError>,
        plan: BatchInstallPlan<'_>,
        cas_reuse: &mut CasReuseMask,
    ) -> Result<(), IndexError> {
        if objects.is_empty() {
            return Ok(());
        }
        if objects.len() > MAX_INSTALL_BATCH_INPUTS {
            return Err(IndexError::ResourceLimited);
        }
        let mut total_bytes = 0u64;
        for object in objects {
            total_bytes = total_bytes
                .checked_add(object.registration.bytes)
                .ok_or(IndexError::ResourceLimited)?;
        }
        if total_bytes > MAX_BATCH_BYTES {
            return Err(IndexError::ResourceLimited);
        }
        let count = u64::try_from(objects.len())
            .map_err(|_conversion_error| IndexError::ResourceLimited)?;
        budget.charge(WorkUsage {
            writes: count,
            bytes: total_bytes
                .checked_add(count.checked_mul(256).ok_or(IndexError::ResourceLimited)?)
                .ok_or(IndexError::ResourceLimited)?,
            ..WorkUsage::default()
        })?;
        let owned_group = if matches!(plan, BatchInstallPlan::Single) {
            let references = BatchReferences::new(objects, budget)?;
            let metadata = RegistrationBatch::new(objects, budget)?;
            Some(PreparedInstallGroup {
                references,
                shards: BatchShards::new(&self.objects, metadata.as_slice(), budget)?,
                metadata,
            })
        } else {
            None
        };
        let group = match plan {
            BatchInstallPlan::Single => owned_group.as_ref().ok_or_else(|| {
                IndexError::BackendViolation("batch registration metadata absent".into())
            })?,
            BatchInstallPlan::Run { group, .. } => group,
        };
        let all_refs = &group.references;
        let registrations = group.metadata.as_slice();
        let referenced_shards = &group.shards;
        let _gate_guards = if matches!(plan, BatchInstallPlan::Single) {
            Some(self.lock_install_gates(&[objects], budget)?)
        } else {
            None
        };
        let scope = if self.authority.is_some() {
            Some(match plan {
                BatchInstallPlan::Single => sink.managed_scope().ok_or_else(|| {
                    IndexError::BackendViolation("managed installation lacks catalog scope".into())
                })?,
                BatchInstallPlan::Run { attempt, .. } => ObjectInstallScope::Attempt(attempt),
            })
        } else {
            None
        };
        if matches!(
            plan,
            BatchInstallPlan::Single | BatchInstallPlan::Run { reserve: true, .. }
        ) {
            self.reserve_install(sink, scope, registrations, budget, true)?;
        }
        let mut physical =
            PhysicalObservation::new(budget, self.authority.is_some(), objects.len());
        let track_cas_reuse =
            self.authority.is_some() && budget.request().diagnostic_context().is_some();
        let protected_gates = match plan {
            BatchInstallPlan::Single => None,
            BatchInstallPlan::Run { gates, .. } => Some(gates),
        };
        let mut child_checks = ChildCheckCounts::default();
        let observe_child_checks = physical.is_some();
        if let (Some(physical), Some(gates)) = (physical.as_ref(), protected_gates) {
            physical.observe_external_children(budget, registrations, all_refs, gates);
        }
        let result: Result<(), IndexError> = {
            let child = physical
                .as_ref()
                .and_then(PhysicalObservation::child_context);
            let mut scoped = budget.scoped_diagnostic_context(child);
            let budget = &mut *scoped;
            (|| {
                measure_physical(&mut physical, InstallPhase::ChildValidation, || {
                    self.validate_prepared_children(
                        objects,
                        group,
                        plan,
                        observe_child_checks,
                        &mut child_checks,
                        budget,
                    )
                })?;

                #[cfg(target_os = "macos")]
                let batch_device =
                    measure_physical(&mut physical, InstallPhase::NamespaceValidation, || {
                        self.validate_store_root()?;
                        Ok::<u64, IndexError>(
                            fs::symlink_metadata(&self.objects)
                                .map_err(|error| storage_error(&error))?
                                .dev(),
                        )
                    })?;
                #[cfg(target_os = "macos")]
                let expected_writer_device = Some(batch_device);
                #[cfg(not(target_os = "macos"))]
                let expected_writer_device = None;
                #[cfg(target_os = "macos")]
                let mut barrier_file: Option<ObjectIoFile> = None;
                #[cfg(target_os = "macos")]
                let mut barrier_position: Option<usize> = None;
                let mut pending_writes: [Option<
                    Box<dyn quanta_index_core::index::ObjectWriteTicket>,
                >; MAX_INSTALL_BATCH_INPUTS] = std::array::from_fn(|_| None);
                for (position, object) in objects.iter().enumerate() {
                    let (final_path, existing) = measure_physical(
                        &mut physical,
                        InstallPhase::NamespaceValidation,
                        || -> Result<_, IndexError> {
                            budget.checkpoint()?;
                            self.validate_store_root()?;
                            let final_path = self.path_for(object.registration.reference);
                            let shard = final_path.parent().ok_or_else(|| {
                                IndexError::Storage("object path has no parent".into())
                            })?;
                            #[cfg(target_os = "macos")]
                            ensure_same_device(
                                batch_device,
                                ensure_private_directory_metadata(shard)?.dev(),
                            )?;
                            #[cfg(not(target_os = "macos"))]
                            ensure_private_directory(shard)?;
                            let existing = final_path.exists();
                            Ok((final_path, existing))
                        },
                    )?;
                    let shard = final_path
                        .parent()
                        .ok_or_else(|| IndexError::Storage("object path has no parent".into()))?;
                    if existing {
                        #[cfg(target_os = "macos")]
                        {
                            let existing =
                                measure_physical(&mut physical, InstallPhase::Existing, || {
                                    self.sync_batch_existing(
                                        object.registration.reference,
                                        &final_path,
                                        &object.wire,
                                        batch_device,
                                        budget,
                                        "sync existing immutable object",
                                    )
                                })?;
                            if barrier_file.is_none() {
                                barrier_file = Some(existing);
                            }
                        }
                        #[cfg(not(target_os = "macos"))]
                        measure_physical(&mut physical, InstallPhase::Existing, || {
                            self.verify_existing(
                                object.registration.reference,
                                &final_path,
                                &object.wire,
                                budget,
                            )
                        })?;
                        if track_cas_reuse {
                            cas_reuse.mark(position);
                        }
                        continue;
                    }
                    let mut writer = self.create_writer_observed(
                        shard,
                        &final_path,
                        object.registration.bytes,
                        expected_writer_device,
                        budget,
                        &mut physical,
                    )?;
                    measure_physical(&mut physical, InstallPhase::Write, || {
                        self.write_observed(writer.file_mut()?, &object.wire)
                    })?;
                    #[cfg(target_os = "macos")]
                    measure_physical(&mut physical, InstallPhase::FileSync, || {
                        self.sync_observed(SyncKind::File, || {
                            rustix::fs::fsync(writer.file()?).map_err(|error| {
                                IndexError::Storage(format!("sync immutable object: {error}"))
                            })
                        })
                    })?;
                    #[cfg(not(target_os = "macos"))]
                    measure_physical(&mut physical, InstallPhase::FileSync, || {
                        self.sync_observed(SyncKind::File, || {
                            writer
                                .file()?
                                .sync_all()
                                .map_err(|error| storage_error(&error))
                        })
                    })?;
                    let published = measure_physical(
                        &mut physical,
                        InstallPhase::PublishAndRetire,
                        || -> Result<_, IndexError> {
                            let written = writer
                                .file()?
                                .metadata()
                                .map_err(|error| storage_error(&error))?;
                            validate_private_file(&written)?;
                            writer.publish(&final_path, &written)
                        },
                    );
                    match published {
                        Ok(true) => {
                            measure_physical(
                                &mut physical,
                                InstallPhase::PublishAndRetire,
                                || {
                                    // Publish already checked that the canonical name
                                    // is this writer's private, single-link inode.
                                    #[cfg(target_os = "macos")]
                                    {
                                        if barrier_file.is_none() {
                                            barrier_file = Some(ObjectIoFile::Writer(writer));
                                            barrier_position = Some(position);
                                        } else {
                                            *pending_writes
                                                .get_mut(position)
                                                .ok_or(IndexError::ResourceLimited)? =
                                                writer.retire()?;
                                        }
                                    }
                                    #[cfg(not(target_os = "macos"))]
                                    {
                                        *pending_writes
                                            .get_mut(position)
                                            .ok_or(IndexError::ResourceLimited)? =
                                            writer.retire()?;
                                    }
                                    Ok::<(), IndexError>(())
                                },
                            )?;
                        }
                        Ok(false) => {
                            measure_physical(
                                &mut physical,
                                InstallPhase::PublishAndRetire,
                                || {
                                    let _no_output = writer.retire()?;
                                    Ok::<(), IndexError>(())
                                },
                            )?;
                            #[cfg(target_os = "macos")]
                            {
                                let existing = measure_physical(
                                    &mut physical,
                                    InstallPhase::Existing,
                                    || {
                                        self.sync_batch_existing(
                                            object.registration.reference,
                                            &final_path,
                                            &object.wire,
                                            batch_device,
                                            budget,
                                            "sync concurrent immutable object",
                                        )
                                    },
                                )?;
                                if barrier_file.is_none() {
                                    barrier_file = Some(existing);
                                }
                            }
                            #[cfg(not(target_os = "macos"))]
                            measure_physical(&mut physical, InstallPhase::Existing, || {
                                self.verify_existing(
                                    object.registration.reference,
                                    &final_path,
                                    &object.wire,
                                    budget,
                                )
                            })?;
                            if track_cas_reuse {
                                cas_reuse.mark(position);
                            }
                        }
                        Err(error) => return Err(error),
                    }
                }
                // A concurrently installed matching object may not have had its directory
                // entry synced yet. Every referenced shard must be durable before custody
                // confirmation, including already-existing objects.
                let mut shard_witnesses = [None; 256];
                let objects_witness =
                    measure_physical(&mut physical, InstallPhase::DirectorySync, || {
                        let objects = self
                            .sync_batch_directory_witness(&self.objects, expected_writer_device)?;
                        for (prefix, shard) in referenced_shards.indexed_iter() {
                            let witnessed =
                                self.sync_batch_directory_witness(shard, expected_writer_device)?;
                            *shard_witnesses
                                .get_mut(prefix)
                                .ok_or(IndexError::ResourceLimited)? = Some(witnessed.identity);
                        }
                        Ok::<SyncedDirectory, IndexError>(objects)
                    })?;
                #[cfg(target_os = "macos")]
                {
                    let file = measure_physical(&mut physical, InstallPhase::Barrier, || {
                        budget.checkpoint()?;
                        let file = barrier_file.ok_or_else(|| {
                            IndexError::BackendViolation(
                                "nonempty object batch has no barrier file".into(),
                            )
                        })?;
                        self.sync_observed(SyncKind::Barrier, || barrier(file.file()?))?;
                        Ok::<_, IndexError>(file)
                    })?;
                    let barrier_ticket =
                        measure_physical(&mut physical, InstallPhase::PublishAndRetire, || {
                            if let ObjectIoFile::Writer(writer) = file {
                                writer.retire()
                            } else {
                                Ok(None)
                            }
                        })?;
                    measure_physical(&mut physical, InstallPhase::Settlement, || {
                        if let Some(ticket) = barrier_ticket {
                            let position = barrier_position.ok_or_else(|| {
                                IndexError::BackendViolation(
                                    "barrier writer position absent".into(),
                                )
                            })?;
                            let registration = registrations
                                .get(position)
                                .ok_or(IndexError::ResourceLimited)?;
                            ticket.synced(object_sync_directories(
                                registration,
                                objects_witness.identity,
                                &shard_witnesses,
                            )?)?;
                        }
                        Ok::<(), IndexError>(())
                    })?;
                }
                #[cfg(not(target_os = "macos"))]
                let _unused_barrier = barrier;
                measure_physical(&mut physical, InstallPhase::Settlement, || {
                    for (position, ticket) in pending_writes.into_iter().enumerate() {
                        if let Some(ticket) = ticket {
                            let registration = registrations
                                .get(position)
                                .ok_or(IndexError::ResourceLimited)?;
                            ticket.synced(object_sync_directories(
                                registration,
                                objects_witness.identity,
                                &shard_witnesses,
                            )?)?;
                        }
                    }
                    budget.checkpoint()?;
                    Ok::<(), IndexError>(())
                })?;
                drop(objects_witness);
                Ok(())
            })()
        };
        if let Some(physical) = physical.as_mut() {
            if protected_gates.is_some() {
                physical.observe_child_checks(&child_checks);
            }
            physical.finish(&result);
        }
        result?;
        if matches!(
            plan,
            BatchInstallPlan::Single | BatchInstallPlan::Run { confirm: true, .. }
        ) {
            self.confirm_install(sink, scope, registrations, budget, true, cas_reuse)
        } else {
            Ok(())
        }
    }

    pub(crate) fn put_inner(
        &self,
        payload: &[u8],
        children: &[ObjectRef],
        sink: Option<&dyn ObjectWriteSink>,
        budget: &mut WorkBudget,
    ) -> Result<(RegisteredObject, bool), IndexError> {
        #[cfg(target_os = "macos")]
        let barrier = |file: &File| {
            rustix::fs::fcntl_fullfsync(file).map_err(|error| {
                IndexError::Storage(format!("full sync immutable object: {error}"))
            })
        };
        #[cfg(not(target_os = "macos"))]
        let barrier = |_: &File| Ok(());
        self.put_inner_with_barrier(payload, children, sink, budget, barrier)
    }

    fn put_inner_with_barrier(
        &self,
        payload: &[u8],
        children: &[ObjectRef],
        sink: Option<&dyn ObjectWriteSink>,
        budget: &mut WorkBudget,
        barrier: impl FnOnce(&File) -> Result<(), IndexError>,
    ) -> Result<(RegisteredObject, bool), IndexError> {
        if self.authority.is_some() && sink.is_none() {
            return Err(IndexError::BackendViolation(
                "managed object installation requires catalog custody".into(),
            ));
        }
        let prepared = self.prepare_object(payload, children, budget)?;
        let registration = &prepared.registration;
        let wire = &prepared.wire;
        budget.charge(WorkUsage {
            writes: 1,
            bytes: registration.bytes,
            ..WorkUsage::default()
        })?;
        let _gate_guard = self.lock_gate(Self::gate_index(registration.reference), budget)?;
        let scope = if self.authority.is_some() {
            let sink = sink.ok_or_else(|| {
                IndexError::BackendViolation("managed installation lacks catalog scope".into())
            })?;
            Some(sink.managed_scope().ok_or_else(|| {
                IndexError::BackendViolation("managed installation lacks catalog scope".into())
            })?)
        } else {
            None
        };
        if let Some(sink) = sink {
            self.reserve_install(
                sink,
                scope,
                std::slice::from_ref(registration),
                budget,
                false,
            )?;
            budget.checkpoint()?;
        }
        for child in children {
            budget.charge(WorkUsage {
                reads: 1,
                ..WorkUsage::default()
            })?;
            self.validate_child_path(*child)?;
        }

        let reference = registration.reference;
        self.validate_store_root()?;
        let final_path = self.path_for(reference);
        let shard = final_path
            .parent()
            .ok_or_else(|| IndexError::Storage("object path has no parent".into()))?;
        ensure_private_directory(shard)?;
        if final_path.exists() {
            self.verify_existing(reference, &final_path, wire, budget)?;
            if let Some(sink) = sink {
                self.sync_observed(SyncKind::Directory, || sync_directory(&self.objects))?;
                self.sync_observed(SyncKind::Directory, || sync_directory(shard))?;
                self.confirm_install(
                    sink,
                    scope,
                    std::slice::from_ref(registration),
                    budget,
                    false,
                    &CasReuseMask::single(),
                )?;
            }
            return Ok((prepared.registration, false));
        }
        let mut sync = ScalarSync::new(self, shard)?;
        let mut objects_witness = sync.sync_objects(self)?;
        let mut writer =
            self.create_writer(shard, &final_path, registration.bytes, None, budget)?;
        sync.writer_ready(self, &writer, &mut objects_witness)?;
        self.write_observed(writer.file_mut()?, wire)?;
        sync.sync_file(self, &writer)?;
        let written = writer
            .file()?
            .metadata()
            .map_err(|error| storage_error(&error))?;
        validate_private_file(&written)?;
        match writer.publish(&final_path, &written) {
            Ok(true) => {
                let shard_witness = sync.sync_shard(self, shard)?;
                validate_private_file(
                    &writer
                        .file()?
                        .metadata()
                        .map_err(|error| storage_error(&error))?,
                )?;
                sync.finish_barrier(self, &writer, barrier)?;
                if let Some(ticket) = writer.retire()? {
                    ticket.synced(ObjectWriteSyncDirectories {
                        objects: objects_witness.identity,
                        shard: shard_witness,
                    })?;
                }
                drop(objects_witness);
                if let Some(sink) = sink {
                    self.confirm_install(
                        sink,
                        scope,
                        std::slice::from_ref(registration),
                        budget,
                        false,
                        &CasReuseMask::default(),
                    )?;
                }
                Ok((prepared.registration, true))
            }
            Ok(false) => {
                let _no_output = writer.retire()?;
                self.verify_existing(reference, &final_path, wire, budget)?;
                if let Some(sink) = sink {
                    self.sync_observed(SyncKind::Directory, || sync_directory(&self.objects))?;
                    self.sync_observed(SyncKind::Directory, || sync_directory(shard))?;
                    self.confirm_install(
                        sink,
                        scope,
                        std::slice::from_ref(registration),
                        budget,
                        false,
                        &CasReuseMask::single(),
                    )?;
                }
                Ok((prepared.registration, false))
            }
            Err(error) => Err(error),
        }
    }

    fn next_temp_path(&self, shard: &Path) -> Result<PathBuf, IndexError> {
        let sequence = self.temp_sequence.fetch_add(1, Ordering::Relaxed);
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|error| IndexError::Storage(format!("system clock before epoch: {error}")))?
            .as_nanos();
        Ok(shard.join(format!(".tmp-{}-{sequence}-{nonce}", std::process::id())))
    }

    fn create_writer(
        &self,
        shard: &Path,
        target: &Path,
        max_bytes: u64,
        expected_device: Option<u64>,
        budget: &mut WorkBudget,
    ) -> Result<ObjectWriter, IndexError> {
        self.create_writer_observed(shard, target, max_bytes, expected_device, budget, &mut None)
    }

    fn create_writer_observed(
        &self,
        shard: &Path,
        target: &Path,
        max_bytes: u64,
        expected_device: Option<u64>,
        budget: &mut WorkBudget,
        physical: &mut Option<PhysicalObservation>,
    ) -> Result<ObjectWriter, IndexError> {
        let Some(io) = &self.write_io else {
            return measure_physical(physical, InstallPhase::WriterCreate, || {
                let (file, path) = self.create_temp(shard)?;
                let writer = ObjectWriter::new(file, path, None);
                #[cfg(target_os = "macos")]
                if let Some(device) = expected_device {
                    ensure_same_device(
                        device,
                        writer
                            .file()?
                            .metadata()
                            .map_err(|error| storage_error(&error))?
                            .dev(),
                    )?;
                }
                #[cfg(not(target_os = "macos"))]
                let _ = expected_device;
                Ok(writer)
            });
        };
        let (path, mut ticket) =
            measure_physical(physical, InstallPhase::WriterTicketPrepare, || {
                validate_private_directory(shard)?;
                let path = self.next_temp_path(shard)?;
                let mut ticket = io.begin(target, &path, max_bytes, budget)?;
                ticket.creation_started()?;
                Ok::<_, IndexError>((path, ticket))
            })?;
        let file = measure_physical(physical, InstallPhase::WriterOpen, || {
            let opened = match ticket.open_declared_temp() {
                Ok(Some(file)) => Ok(file),
                Ok(None) => nofollow_flag().and_then(|flag| {
                    OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .mode(0o600)
                        .custom_flags(flag)
                        .open(&path)
                        .map_err(|error| storage_error(&error))
                }),
                Err(error) => Err(error),
            };
            match opened {
                Ok(file) => Ok(file),
                Err(primary) => {
                    ticket.uncreated()?;
                    Err(primary)
                }
            }
        })?;
        let mut writer = ObjectWriter::new(file, path, Some(ticket));
        measure_physical(physical, InstallPhase::WriterCreatedWitness, || {
            writer.created()?;
            let born = writer
                .file()?
                .metadata()
                .map_err(|error| storage_error(&error))?;
            validate_private_file(&born)?;
            #[cfg(target_os = "macos")]
            if let Some(device) = expected_device {
                ensure_same_device(device, born.dev())?;
            }
            #[cfg(not(target_os = "macos"))]
            let _ = expected_device;
            Ok::<(), IndexError>(())
        })?;
        Ok(writer)
    }

    fn create_temp(&self, shard: &Path) -> Result<(File, PathBuf), IndexError> {
        validate_private_directory(shard)?;
        for _ in 0..16 {
            let path = self.next_temp_path(shard)?;
            match OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .custom_flags(nofollow_flag()?)
                .open(&path)
            {
                Ok(file) => {
                    if let Err(error) = file
                        .metadata()
                        .map_err(|error| storage_error(&error))
                        .and_then(|metadata| validate_private_file(&metadata))
                    {
                        drop(file);
                        drop(fs::remove_file(&path));
                        return Err(error);
                    }
                    return Ok((file, path));
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(storage_error(&error)),
            }
        }
        Err(IndexError::Storage(
            "temporary object name exhausted".into(),
        ))
    }

    fn validate_prepared_children(
        &self,
        objects: &[&PreparedObject],
        group: &PreparedInstallGroup,
        plan: BatchInstallPlan<'_>,
        observe: bool,
        counts: &mut ChildCheckCounts,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        budget.checkpoint()?;
        let protected = match plan {
            BatchInstallPlan::Single => None,
            BatchInstallPlan::Run {
                gates,
                previous_groups,
                ..
            } => Some((gates, previous_groups)),
        };
        let mut validated = if protected.is_some() {
            let children = objects.iter().try_fold(0usize, |count, object| {
                count
                    .checked_add(object.registration.children.len())
                    .ok_or(IndexError::ResourceLimited)
            })?;
            ValidatedExternalChildren::try_new(children, budget)?
        } else {
            None
        };
        // Admit observation only after the normal validator's own optional
        // window, and release it before the group can reserve write memory.
        let overlap = if observe {
            protected.and_then(|(gates, previous)| {
                CrossGroupChildOverlap::measure(budget, previous, group, gates)
            })
        } else {
            None
        };
        counts.cross_group = overlap.as_ref().map(CrossGroupChildOverlap::counts);
        for object in objects {
            for child in &object.registration.children {
                if group.references.contains(*child)? {
                    continue;
                }
                let slot = if let (Some(validated), Some((gates, _))) = (&validated, protected) {
                    budget.charge(WorkUsage {
                        bytes: 1,
                        ..WorkUsage::default()
                    })?;
                    if gates[Self::gate_index(*child)].is_some() {
                        validated.locate(child, budget)?
                    } else {
                        ChildValidationSlot::Untracked
                    }
                } else {
                    ChildValidationSlot::Untracked
                };
                if matches!(slot, ChildValidationSlot::Validated) {
                    counts.reused += 1;
                    continue;
                }
                budget.charge(WorkUsage {
                    reads: 1,
                    ..WorkUsage::default()
                })?;
                self.validate_child_path(*child)?;
                if let (Some(overlap), Some(cross_group)) = (&overlap, &mut counts.cross_group) {
                    cross_group.path_checks += 1;
                    cross_group.repeated_path_checks += u64::from(overlap.contains(child));
                }
                if let (Some(validated), ChildValidationSlot::Vacant(index)) =
                    (&mut validated, slot)
                {
                    // The original gate stays held through the run. A
                    // physical group never borrows a prior group's witness.
                    validated.remember(index, child, budget)?;
                }
            }
        }
        Ok(())
    }

    fn validate_child_path(&self, reference: ObjectRef) -> Result<(), IndexError> {
        self.validate_store_root()?;
        let path = self.path_for(reference);
        let shard = path
            .parent()
            .ok_or_else(|| IndexError::Storage("object path has no parent".into()))?;
        validate_private_shard(
            shard,
            IndexError::InvalidInput("object references a missing child".into()),
        )?;
        let metadata = fs::symlink_metadata(&path).map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                IndexError::InvalidInput("object references a missing child".into())
            } else {
                storage_error(&error)
            }
        })?;
        validate_private_file(&metadata)
    }

    fn validate_store_root(&self) -> Result<(), IndexError> {
        let root = self
            .objects
            .parent()
            .ok_or_else(|| IndexError::Storage("object store has no root".into()))?;
        if self.authority.is_some() {
            let _binding = self.namespace.validate_objects(&self.objects)?;
        } else {
            validate_private_directory(root)?;
            validate_private_directory(&self.objects)?;
        }
        Ok(())
    }

    fn verify_existing(
        &self,
        reference: ObjectRef,
        final_path: &Path,
        expected: &[u8],
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        if let Some(io) = &self.write_io {
            // A failed batch may leave a renamed, retired safe create pending
            // before its shared barrier. Only the original physical authority
            // can settle that exact path before the admitted read below.
            io.recover_retired_visible_create(final_path, budget)?;
        }
        let stored = self.read_wire(reference, budget)?;
        if stored.bytes() != expected {
            return Err(IndexError::CorruptObject(
                "content-addressed path has different bytes".into(),
            ));
        }
        Ok(())
    }

    #[cfg(target_os = "macos")]
    fn verify_existing_for_sync(
        &self,
        reference: ObjectRef,
        final_path: &Path,
        expected: &[u8],
        budget: &mut WorkBudget,
    ) -> Result<ObjectIoFile, IndexError> {
        if let Some(io) = &self.write_io {
            io.recover_retired_visible_create(final_path, budget)?;
        }
        let (stored, owner) = read_io::read_wire_with_owner(
            &self.objects,
            self.authority.as_ref().map(|_authority| &self.namespace),
            self.read_io.as_deref(),
            reference,
            budget,
        )?;
        if stored.bytes() != expected {
            return Err(IndexError::CorruptObject(
                "content-addressed path has different bytes".into(),
            ));
        }
        // The full wire and its temporary memory lease end before durability
        // work, while the exact file validated by the read stays owned.
        drop(stored);
        Ok(owner)
    }

    #[cfg(target_os = "macos")]
    fn sync_batch_existing(
        &self,
        reference: ObjectRef,
        final_path: &Path,
        expected: &[u8],
        batch_device: u64,
        budget: &mut WorkBudget,
        context: &str,
    ) -> Result<ObjectIoFile, IndexError> {
        let owner = self.verify_existing_for_sync(reference, final_path, expected, budget)?;
        budget.checkpoint()?;
        // Acquire the exact admitted descriptor before checking the live name.
        // An owner may do work while handing out its file reference.
        let file = owner.file()?;
        let opened = file.metadata().map_err(|error| storage_error(&error))?;
        validate_private_file(&opened)?;
        ensure_same_device(batch_device, opened.dev())?;
        self.validate_store_root()?;
        let shard = final_path
            .parent()
            .ok_or_else(|| IndexError::Storage("object path has no parent".into()))?;
        let shard_metadata = private_shard_metadata(
            shard,
            IndexError::CorruptObject("referenced object is absent".into()),
        )?;
        ensure_same_device(batch_device, shard_metadata.dev())?;
        let linked = fs::symlink_metadata(final_path).map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                IndexError::CorruptObject("referenced object is absent".into())
            } else {
                storage_error(&error)
            }
        })?;
        validate_private_file(&linked)?;
        if opened.dev() != linked.dev() || opened.ino() != linked.ino() {
            return Err(IndexError::CorruptObject(
                "immutable object changed before batch sync".into(),
            ));
        }
        budget.checkpoint()?;
        self.sync_observed(SyncKind::File, || {
            rustix::fs::fsync(file)
                .map_err(|error| IndexError::Storage(format!("{context}: {error}")))
        })?;
        Ok(owner)
    }

    fn read_wire(
        &self,
        reference: ObjectRef,
        budget: &mut WorkBudget,
    ) -> Result<ReadWire, IndexError> {
        read_io::read_wire(
            &self.objects,
            self.authority.as_ref().map(|_authority| &self.namespace),
            self.read_io.as_deref(),
            reference,
            budget,
        )
    }

    /// Hold one admitted physical page FD through payload decoding. The
    /// caller must finish the stream before using any parsed snapshot data.
    pub fn open_protected_snapshot_page_v1(
        &self,
        reference: ObjectRef,
        budget: &mut WorkBudget,
    ) -> Result<Box<dyn quanta_index_core::index::NativeProtectedSnapshotPageV1 + '_>, IndexError>
    {
        read_io::open_protected_snapshot_page(
            &self.objects,
            self.authority.as_ref().map(|_authority| &self.namespace),
            self.read_io.as_deref(),
            reference,
            budget,
        )
    }

    /// One canonical map-node read inside a pre-admitted scan window. The
    /// caller supplies the map codec's payload and child bounds, rather than
    /// the generic object store's much larger accepted wire bounds.
    pub(crate) fn canonical_map_read_memory_credit_limits(
        &self,
        payload: usize,
        children: usize,
    ) -> Result<Option<quanta_index_core::index::MemoryCreditLimits>, IndexError> {
        let io = if let Some(io) = &self.read_io {
            let Some(bound) = io.read_owner_memory_bound()? else {
                return Ok(None);
            };
            bound
        } else {
            quanta_index_core::index::MemoryCreditLimits {
                temporary_data_bytes: 0,
                retained_data_bytes: 0,
                max_live_temporary_leases: 0,
                max_live_retained_leases: 0,
            }
        };
        let wire = HEADER_LEN
            .checked_add(payload)
            .and_then(|bytes| bytes.checked_add(children.checked_mul(32)?))
            .ok_or(IndexError::ResourceLimited)?;
        let wire = u64::try_from(wire).map_err(|_error| IndexError::ResourceLimited)?;
        Ok(Some(quanta_index_core::index::MemoryCreditLimits {
            temporary_data_bytes: read_io::path_memory_bytes(&self.objects)?
                .checked_add(wire)
                .and_then(|bytes| bytes.checked_add(io.temporary_data_bytes))
                .ok_or(IndexError::ResourceLimited)?,
            retained_data_bytes: StoredObject::allocation_bytes(payload, children)?
                .checked_add(io.retained_data_bytes)
                .ok_or(IndexError::ResourceLimited)?,
            max_live_temporary_leases: io
                .max_live_temporary_leases
                .checked_add(2)
                .ok_or(IndexError::ResourceLimited)?,
            max_live_retained_leases: io
                .max_live_retained_leases
                .checked_add(2)
                .ok_or(IndexError::ResourceLimited)?,
        }))
    }
}

fn local_install_batch_memory_bound(
    objects: &Path,
    shape: quanta_index_core::index::ports::ObjectInstallMemoryShape,
) -> Result<quanta_index_core::index::MemoryCreditLimits, IndexError> {
    shape.validate()?;
    if shape.maximum_payload_bytes > MAX_PAYLOAD
        || u64::try_from(shape.maximum_total_payload_bytes)
            .map_err(|_error| IndexError::ResourceLimited)?
            > MAX_BATCH_BYTES
        || shape.maximum_total_children
            > shape
                .maximum_inputs
                .checked_mul(MAX_CHILDREN)
                .ok_or(IndexError::ResourceLimited)?
    {
        return Err(IndexError::ResourceLimited);
    }
    let count = shape.maximum_inputs;
    let count_u64 = u64::try_from(count).map_err(|_error| IndexError::ResourceLimited)?;
    let children_u64 = u64::try_from(shape.maximum_total_children)
        .map_err(|_error| IndexError::ResourceLimited)?;
    let payload_u64 = u64::try_from(shape.maximum_total_payload_bytes)
        .map_err(|_error| IndexError::ResourceLimited)?;
    let wire_bytes = count_u64
        .checked_mul(u64::try_from(HEADER_LEN).map_err(|_error| IndexError::ResourceLimited)?)
        .and_then(|bytes| bytes.checked_add(payload_u64))
        .and_then(|bytes| bytes.checked_add(children_u64.checked_mul(32)?))
        .ok_or(IndexError::ResourceLimited)?;
    if wire_bytes > MAX_BATCH_BYTES {
        return Err(IndexError::ResourceLimited);
    }
    let registration_bytes = RegisteredObject::allocation_bytes(0)?
        .checked_mul(count_u64)
        .and_then(|bytes| {
            bytes.checked_add(children_u64.checked_mul(std::mem::size_of::<ObjectRef>() as u64)?)
        })
        .ok_or(IndexError::ResourceLimited)?;
    let path_bytes = BatchShards::allocation_bytes_for_count(objects, count)?;
    let read_path_bytes = read_io::path_memory_bytes(objects)?;
    let max_wire = u64::try_from(MAX_WIRE).map_err(|_error| IndexError::ResourceLimited)?;
    let registration_packet_bytes = RegistrationBatch::allocation_bytes_for_count(count)?;
    let temporary_data_bytes = batch_preparation_memory_bytes(count)?
        .checked_add(wire_bytes)
        .and_then(|bytes| bytes.checked_add(registration_packet_bytes))
        .and_then(|bytes| bytes.checked_add(path_bytes))
        .and_then(|bytes| bytes.checked_add(read_path_bytes))
        .and_then(|bytes| bytes.checked_add(max_wire))
        .ok_or(IndexError::ResourceLimited)?;
    Ok(quanta_index_core::index::MemoryCreditLimits {
        temporary_data_bytes,
        retained_data_bytes: registration_bytes
            .checked_add(quanta_index_core::index::RegisteredObjectBatch::allocation_bytes(count)?)
            .ok_or(IndexError::ResourceLimited)?,
        max_live_temporary_leases: u32::try_from(
            count.checked_add(5).ok_or(IndexError::ResourceLimited)?,
        )
        .map_err(|_error| IndexError::ResourceLimited)?,
        max_live_retained_leases: u32::try_from(
            count.checked_add(1).ok_or(IndexError::ResourceLimited)?,
        )
        .map_err(|_error| IndexError::ResourceLimited)?,
    })
}

impl ImmutableObjectPort for FileObjectStore {
    fn install_batch_memory_bound(
        &self,
        shape: quanta_index_core::index::ports::ObjectInstallMemoryShape,
    ) -> Result<Option<quanta_index_core::index::MemoryCreditLimits>, IndexError> {
        let (Some(authority), Some(read_io), Some(write_io)) =
            (&self.authority, &self.read_io, &self.write_io)
        else {
            return Ok(None);
        };
        let local = local_install_batch_memory_bound(&self.objects, shape)?;
        let Some(catalog) = authority.managed_install_memory_bound(shape)? else {
            return Ok(None);
        };
        let Some(read) = read_io.read_owner_memory_bound()? else {
            return Ok(None);
        };
        let Some(write) = write_io.write_ticket_memory_bound(shape.maximum_inputs)? else {
            return Ok(None);
        };
        // The first matching existing object may pin the macOS barrier FD
        // while a later existing object is read and authenticated.
        Ok(Some(
            local
                .checked_add(catalog)?
                .checked_add(read)?
                .checked_add(read)?
                .checked_add(write)?,
        ))
    }

    fn try_prepare_leaf_install_run(
        &self,
        attempt: AttemptRef,
        maximum_payload: usize,
        packet_bytes: usize,
        budget: &mut WorkBudget,
    ) -> Result<
        Option<Box<dyn quanta_index_core::index::object_install::PreparedObjectInstallRun + '_>>,
        IndexError,
    > {
        prepared_leaf_run::try_prepare(self, attempt, maximum_payload, packet_bytes, budget)
    }

    fn identify(
        &self,
        payload: &[u8],
        children: &[ObjectRef],
        budget: &mut WorkBudget,
    ) -> Result<RegisteredObject, IndexError> {
        write_preparation::identify(payload, children, budget)
    }

    fn put(
        &self,
        payload: &[u8],
        children: &[ObjectRef],
        budget: &mut WorkBudget,
    ) -> Result<RegisteredObject, IndexError> {
        self.put_inner(payload, children, None, budget)
            .map(|(registration, _)| registration)
    }

    fn install(
        &self,
        payload: &[u8],
        children: &[ObjectRef],
        sink: &dyn ObjectWriteSink,
        budget: &mut WorkBudget,
    ) -> Result<RegisteredObject, IndexError> {
        self.put_inner(payload, children, Some(sink), budget)
            .map(|(registration, _)| registration)
    }

    fn install_batch(
        &self,
        inputs: &[ObjectBatchInput],
        sink: &dyn ObjectWriteSink,
        budget: &mut WorkBudget,
    ) -> Result<quanta_index_core::index::RegisteredObjectBatch, IndexError> {
        if inputs.len() > MAX_BATCH_INPUTS {
            return Err(IndexError::ResourceLimited);
        }
        let mut aggregate = 0u64;
        for input in inputs {
            if input.payload.len() > MAX_PAYLOAD || input.children.len() > MAX_CHILDREN {
                return Err(IndexError::ResourceLimited);
            }
            let wire_len = HEADER_LEN
                .checked_add(
                    input
                        .children
                        .len()
                        .checked_mul(32)
                        .ok_or(IndexError::ResourceLimited)?,
                )
                .and_then(|size| size.checked_add(input.payload.len()))
                .ok_or(IndexError::ResourceLimited)?;
            aggregate = aggregate
                .checked_add(
                    u64::try_from(wire_len)
                        .map_err(|_conversion_error| IndexError::ResourceLimited)?,
                )
                .ok_or(IndexError::ResourceLimited)?;
        }
        if aggregate > MAX_BATCH_BYTES {
            return Err(IndexError::ResourceLimited);
        }
        let count =
            u64::try_from(inputs.len()).map_err(|_conversion_error| IndexError::ResourceLimited)?;
        budget.charge(WorkUsage {
            bytes: aggregate
                .checked_add(count.checked_mul(256).ok_or(IndexError::ResourceLimited)?)
                .ok_or(IndexError::ResourceLimited)?,
            ..WorkUsage::default()
        })?;
        let metadata_bytes = batch_preparation_memory_bytes(inputs.len())?;
        let _preparation_memory = if metadata_bytes == 0 {
            None
        } else {
            budget.reserve_memory(
                metadata_bytes,
                quanta_index_core::index::MemoryLifetime::Temporary,
            )?
        };
        let mut ordered =
            quanta_index_core::index::RegisteredObjectBatch::with_capacity(inputs.len(), budget)?;
        let mut unique = Vec::<PreparedObject>::new();
        unique
            .try_reserve_exact(inputs.len())
            .map_err(|_error| IndexError::ResourceLimited)?;
        let mut index_table = BatchIndex::new();
        for input in inputs {
            budget.checkpoint()?;
            let prepared = self.prepare_object(&input.payload, &input.children, budget)?;
            let reference = prepared.registration.reference;
            let index = if let Some(index) = index_table.lookup(reference)? {
                let existing = unique
                    .get(index)
                    .ok_or_else(|| IndexError::BackendViolation("batch index missing".into()))?;
                if existing.wire != prepared.wire {
                    return Err(IndexError::CorruptObject(
                        "same object digest has different batch wire".into(),
                    ));
                }
                index
            } else {
                let index = unique.len();
                index_table.insert(reference, index)?;
                unique.push(prepared);
                index
            };
            let registration = unique
                .get(index)
                .ok_or_else(|| IndexError::BackendViolation("batch object missing".into()))?
                .registration
                .clone();
            ordered.push(registration)?;
        }
        let mut batch = Vec::new();
        batch
            .try_reserve_exact(unique.len())
            .map_err(|_error| IndexError::ResourceLimited)?;
        batch.extend(unique.iter());
        self.install_prepared_batch(&batch, sink, budget)?;
        Ok(ordered)
    }

    fn get(
        &self,
        reference: ObjectRef,
        budget: &mut WorkBudget,
    ) -> Result<StoredObject, IndexError> {
        read_io::read_stored(
            &self.objects,
            self.authority.as_ref().map(|_authority| &self.namespace),
            self.read_io.as_deref(),
            reference,
            budget,
        )
    }

    fn read_retained(
        &self,
        object: &StoredObject,
        budget: &mut WorkBudget,
    ) -> Result<StoredObject, IndexError> {
        // Same-attempt authenticated DATA keeps its original allocation leases.
        // Preserve this adapter's read/wire/decode charge order without IO.
        budget.charge(WorkUsage {
            reads: 1,
            ..WorkUsage::default()
        })?;
        budget.charge(WorkUsage {
            bytes: object.registration.bytes,
            ..WorkUsage::default()
        })?;
        let decoded = object
            .registration
            .children
            .len()
            .checked_mul(std::mem::size_of::<ObjectRef>())
            .and_then(|children| children.checked_add(object.payload.len()))
            .ok_or(IndexError::ResourceLimited)?;
        budget.charge(WorkUsage {
            bytes: u64::try_from(decoded).map_err(|_width| IndexError::ResourceLimited)?,
            ..WorkUsage::default()
        })?;
        Ok(object.clone())
    }
}

pub(crate) fn decode_stored_wire(
    reference: ObjectRef,
    wire: &[u8],
    budget: &mut WorkBudget,
) -> Result<StoredObject, IndexError> {
    authenticate_scratch_wire(reference, wire)?;
    let parts = parse_authenticated_wire(wire)?;
    charge_decoded_wire(parts.payload.len(), parts.children.len(), budget)?;
    let memory = budget.reserve_memory(
        StoredObject::payload_allocation_bytes(parts.payload.len())?,
        quanta_index_core::index::MemoryLifetime::Retained,
    )?;
    let registration = decode_wire_registration(reference, wire.len(), parts.children, budget)?;
    let mut payload = Vec::new();
    payload
        .try_reserve_exact(parts.payload.len())
        .map_err(|_allocation_error| IndexError::ResourceLimited)?;
    payload.extend_from_slice(parts.payload);
    Ok(StoredObject::new(registration, payload, memory))
}

pub(crate) fn decode_staged_registration(
    reference: ObjectRef,
    wire: &[u8],
    budget: &mut WorkBudget,
) -> Result<RegisteredObject, IndexError> {
    authenticate_scratch_wire(reference, wire)?;
    let parts = parse_authenticated_wire(wire)?;
    charge_decoded_wire(parts.payload.len(), parts.children.len(), budget)?;
    decode_wire_registration(reference, wire.len(), parts.children, budget)
}

fn authenticate_scratch_wire(reference: ObjectRef, wire: &[u8]) -> Result<(), IndexError> {
    if !(HEADER_LEN..=MAX_WIRE).contains(&wire.len()) {
        return Err(IndexError::CorruptObject(
            "scratch object length is out of bounds".into(),
        ));
    }
    if digest_wire(wire) != reference {
        return Err(IndexError::CorruptObject(
            "scratch object digest mismatch".into(),
        ));
    }
    Ok(())
}

struct AuthenticatedWireParts<'a> {
    children: &'a [u8],
    payload: &'a [u8],
}

fn parse_authenticated_wire(wire: &[u8]) -> Result<AuthenticatedWireParts<'_>, IndexError> {
    let (_payload_len, child_bytes) = parse_wire_header(wire, wire.len())?;
    let body = wire
        .get(HEADER_LEN..)
        .ok_or_else(|| IndexError::CorruptObject("truncated object header".into()))?;
    let (children, payload) = body
        .split_at_checked(child_bytes)
        .ok_or_else(|| IndexError::CorruptObject("truncated child references".into()))?;
    Ok(AuthenticatedWireParts { children, payload })
}

fn parse_wire_header(header: &[u8], wire_len: usize) -> Result<(usize, usize), IndexError> {
    if header.get(..8) != Some(MAGIC.as_slice()) {
        return Err(IndexError::CorruptObject("object magic mismatch".into()));
    }
    let payload_len = usize::try_from(read_u32(header, 8)?)
        .map_err(|_conversion_error| IndexError::ResourceLimited)?;
    let children_len = usize::try_from(read_u32(header, 12)?)
        .map_err(|_conversion_error| IndexError::ResourceLimited)?;
    if payload_len > MAX_PAYLOAD || children_len > MAX_CHILDREN {
        return Err(IndexError::CorruptObject(
            "encoded lengths exceed physical limits".into(),
        ));
    }
    let child_bytes = children_len
        .checked_mul(32)
        .ok_or(IndexError::ResourceLimited)?;
    if HEADER_LEN
        .checked_add(child_bytes)
        .and_then(|bytes| bytes.checked_add(payload_len))
        != Some(wire_len)
    {
        return Err(IndexError::CorruptObject(
            "encoded lengths do not match object size".into(),
        ));
    }
    Ok((payload_len, child_bytes))
}

fn charge_decoded_wire(
    payload_len: usize,
    child_bytes: usize,
    budget: &mut WorkBudget,
) -> Result<(), IndexError> {
    let decoded_bytes = child_bytes
        .checked_div(32)
        .and_then(|children| children.checked_mul(std::mem::size_of::<ObjectRef>()))
        .and_then(|children_bytes| children_bytes.checked_add(payload_len))
        .ok_or(IndexError::ResourceLimited)?;
    let decoded_bytes =
        u64::try_from(decoded_bytes).map_err(|_conversion_error| IndexError::ResourceLimited)?;
    budget.charge(WorkUsage {
        bytes: decoded_bytes,
        ..WorkUsage::default()
    })?;
    Ok(())
}

fn decode_wire_registration(
    reference: ObjectRef,
    wire_len: usize,
    children: &[u8],
    budget: &WorkBudget,
) -> Result<RegisteredObject, IndexError> {
    RegisteredObject::copy_from_iter(
        reference,
        u64::try_from(wire_len).map_err(|_error| IndexError::ResourceLimited)?,
        children.len() / 32,
        children.chunks_exact(32).map(|bytes| {
            let digest: [u8; 32] = bytes.try_into().map_err(|_conversion_error| {
                IndexError::CorruptObject("invalid child reference".into())
            })?;
            Ok(ObjectRef(Digest(digest)))
        }),
        budget,
    )
}

fn read_u32(wire: &[u8], at: usize) -> Result<u32, IndexError> {
    let end = at.checked_add(4).ok_or_else(|| {
        IndexError::CorruptObject("object header offset overflows address space".into())
    })?;
    let bytes: [u8; 4] = wire
        .get(at..end)
        .ok_or_else(|| IndexError::CorruptObject("truncated object header".into()))?
        .try_into()
        .map_err(|_conversion_error| IndexError::CorruptObject("invalid object header".into()))?;
    Ok(u32::from_be_bytes(bytes))
}

fn digest_wire(wire: &[u8]) -> ObjectRef {
    let mut hasher = wire_hasher();
    hasher.update(wire);
    ObjectRef(Digest(hasher.finalize().into()))
}

fn wire_hasher() -> Sha256 {
    let mut hasher = Sha256::new();
    hasher.update(HASH_DOMAIN);
    hasher
}

pub(crate) fn storage_error(error: &std::io::Error) -> IndexError {
    IndexError::Storage(error.to_string())
}

fn validate_private_directory(path: &Path) -> Result<(), IndexError> {
    let _metadata = private_directory_metadata(path)?;
    Ok(())
}

fn private_directory_metadata(path: &Path) -> Result<fs::Metadata, IndexError> {
    let metadata = fs::symlink_metadata(path).map_err(|error| storage_error(&error))?;
    validate_private_directory_metadata(path, &metadata)?;
    Ok(metadata)
}

fn validate_private_shard(path: &Path, missing: IndexError) -> Result<(), IndexError> {
    let _metadata = private_shard_metadata(path, missing)?;
    Ok(())
}

fn private_shard_metadata(path: &Path, missing: IndexError) -> Result<fs::Metadata, IndexError> {
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            missing
        } else {
            storage_error(&error)
        }
    })?;
    validate_private_directory_metadata(path, &metadata)?;
    Ok(metadata)
}

fn validate_private_directory_metadata(
    path: &Path,
    metadata: &fs::Metadata,
) -> Result<(), IndexError> {
    if !metadata.file_type().is_dir()
        || metadata.uid() != rustix::process::geteuid().as_raw()
        || metadata.mode() & 0o7777 != 0o700
    {
        return Err(IndexError::Storage(format!(
            "storage directory is not owner-private: {}",
            path.display()
        )));
    }
    Ok(())
}

fn ensure_private_directory(path: &Path) -> Result<(), IndexError> {
    let _metadata = ensure_private_directory_metadata(path)?;
    Ok(())
}

fn ensure_private_directory_metadata(path: &Path) -> Result<fs::Metadata, IndexError> {
    // Most installs target an existing shard. Validate its live name without
    // asking the filesystem to create an entry that is already present.
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            validate_private_directory_metadata(path, &metadata)?;
            return Ok(metadata);
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(storage_error(&error)),
    }
    match fs::DirBuilder::new().mode(0o700).create(path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(storage_error(&error)),
    }
    private_directory_metadata(path)
}

fn nofollow_flag() -> Result<i32, IndexError> {
    i32::try_from(rustix::fs::OFlags::NOFOLLOW.bits())
        .map_err(|_conversion_error| IndexError::ResourceLimited)
}

fn validate_private_file(metadata: &fs::Metadata) -> Result<(), IndexError> {
    if !metadata.file_type().is_file()
        || metadata.uid() != rustix::process::geteuid().as_raw()
        || metadata.mode() & 0o7777 != 0o600
        || metadata.nlink() != 1
    {
        return Err(IndexError::CorruptObject(
            "object is not an owner-private single-link regular file".into(),
        ));
    }
    Ok(())
}

fn publish_temp_no_replace(
    temp: &mut TempFile,
    final_path: &Path,
    source: &fs::Metadata,
) -> Result<bool, IndexError> {
    // The temporary file and canonical name are in the same shard. A
    // no-replace rename never exposes the canonical inode with two links.
    match rustix::fs::renameat_with(
        rustix::fs::CWD,
        temp.0.as_path(),
        rustix::fs::CWD,
        final_path,
        rustix::fs::RenameFlags::NOREPLACE,
    ) {
        Ok(()) => {
            // Rename consumed the temporary name. Never unlink the canonical
            // name from TempFile's cleanup path if validation now fails.
            temp.0.clear();
            validate_installed_link(final_path, source)?;
            Ok(true)
        }
        Err(rustix::io::Errno::EXIST) => Ok(false),
        Err(error) => Err(IndexError::Storage(format!(
            "publish immutable object without replacement: {error}"
        ))),
    }
}

fn validate_installed_link(path: &Path, source: &fs::Metadata) -> Result<(), IndexError> {
    let linked = fs::symlink_metadata(path).map_err(|error| storage_error(&error))?;
    if !linked.file_type().is_file()
        || linked.dev() != source.dev()
        || linked.ino() != source.ino()
        || linked.uid() != rustix::process::geteuid().as_raw()
        || linked.mode() & 0o7777 != 0o600
        || linked.nlink() != 1
    {
        return Err(IndexError::CorruptObject(
            "installed link does not match private temporary inode".into(),
        ));
    }
    Ok(())
}

fn sync_directory(path: &Path) -> Result<(), IndexError> {
    sync_directory_witness(path).map(|_| ())
}

fn directory_key(metadata: &fs::Metadata) -> InodeKey {
    InodeKey {
        device: metadata.dev(),
        inode: metadata.ino(),
    }
}

fn object_sync_directories(
    registration: &RegisteredObject,
    objects: InodeKey,
    shards: &[Option<InodeKey>; 256],
) -> Result<ObjectWriteSyncDirectories, IndexError> {
    let prefix = (registration.reference.0)
        .0
        .first()
        .copied()
        .ok_or_else(|| IndexError::BackendViolation("empty object digest".into()))?;
    let shard = shards
        .get(usize::from(prefix))
        .copied()
        .flatten()
        .ok_or_else(|| IndexError::BackendViolation("shard sync FD witness absent".into()))?;
    Ok(ObjectWriteSyncDirectories { objects, shard })
}

fn sync_directory_witness(path: &Path) -> Result<SyncedDirectory, IndexError> {
    let linked = private_directory_metadata(path)?;
    let file = File::from(
        rustix::fs::open(
            path,
            rustix::fs::OFlags::RDONLY
                | rustix::fs::OFlags::DIRECTORY
                | rustix::fs::OFlags::NOFOLLOW
                | rustix::fs::OFlags::CLOEXEC,
            rustix::fs::Mode::empty(),
        )
        .map_err(|error| IndexError::Storage(format!("open storage directory: {error}")))?,
    );
    let metadata = file.metadata().map_err(|error| storage_error(&error))?;
    validate_synced_directory_path(path, &linked, &metadata)?;
    file.sync_all().map_err(|error| storage_error(&error))?;
    validate_synced_directory_path(path, &linked, &metadata)?;
    Ok(SyncedDirectory {
        _file: file,
        identity: directory_key(&metadata),
    })
}

fn validate_synced_directory_path(
    path: &Path,
    linked: &fs::Metadata,
    opened: &fs::Metadata,
) -> Result<(), IndexError> {
    validate_private_directory_metadata(path, opened)?;
    let current = private_directory_metadata(path)?;
    if directory_key(linked) != directory_key(opened)
        || directory_key(opened) != directory_key(&current)
    {
        return Err(IndexError::Storage(
            "storage directory changed during sync".into(),
        ));
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn ensure_same_device(expected: u64, actual: u64) -> Result<(), IndexError> {
    if expected != actual {
        return Err(IndexError::Storage(
            "immutable object batch crosses filesystem devices".into(),
        ));
    }
    Ok(())
}

#[cfg(all(test, target_os = "macos"))]
fn sync_directory_plain(path: &Path, expected_device: u64) -> Result<(), IndexError> {
    sync_directory_plain_witness(path, expected_device).map(|_| ())
}

#[cfg(target_os = "macos")]
fn sync_directory_plain_witness(
    path: &Path,
    expected_device: u64,
) -> Result<SyncedDirectory, IndexError> {
    let linked = private_directory_metadata(path)?;
    let file = File::from(
        rustix::fs::open(
            path,
            rustix::fs::OFlags::RDONLY
                | rustix::fs::OFlags::DIRECTORY
                | rustix::fs::OFlags::NOFOLLOW
                | rustix::fs::OFlags::CLOEXEC,
            rustix::fs::Mode::empty(),
        )
        .map_err(|error| {
            IndexError::Storage(format!("open immutable object directory: {error}"))
        })?,
    );
    let metadata = file.metadata().map_err(|error| storage_error(&error))?;
    validate_synced_directory_path(path, &linked, &metadata)?;
    ensure_same_device(expected_device, metadata.dev())?;
    rustix::fs::fsync(&file).map_err(|error| {
        IndexError::Storage(format!("sync immutable object directory: {error}"))
    })?;
    validate_synced_directory_path(path, &linked, &metadata)?;
    Ok(SyncedDirectory {
        _file: file,
        identity: directory_key(&metadata),
    })
}

fn sync_parent_directory(path: &Path) -> Result<(), IndexError> {
    File::open(path)
        .and_then(|file| file.sync_all())
        .map_err(|error| storage_error(&error))
}

fn hex_digit(nibble: u8) -> u8 {
    if nibble < 10 {
        b'0'.wrapping_add(nibble)
    } else {
        b'a'.wrapping_add(nibble.wrapping_sub(10))
    }
}

struct TempFile(PathBuf);

impl Drop for TempFile {
    fn drop(&mut self) {
        if !self.0.as_os_str().is_empty() {
            drop(fs::remove_file(&self.0));
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        os::unix::fs::PermissionsExt,
        path::PathBuf,
        sync::atomic::{AtomicU64, Ordering},
        time::{Duration, Instant},
    };

    use quanta_index_core::index::WorkLimits;

    use super::*;

    #[test]
    fn install_memory_shape_covers_batch_births_and_existing_read() -> Result<(), IndexError> {
        let objects = Path::new("/private/tmp/qi-test/objects");
        for (count, children, payload) in [(1, 0, 8192), (64, 2, 8192), (256, 0, 8192)] {
            let shape = quanta_index_core::index::ports::ObjectInstallMemoryShape {
                maximum_inputs: count,
                maximum_total_payload_bytes: count * payload,
                maximum_payload_bytes: payload,
                maximum_total_children: count * children,
            };
            let bound = local_install_batch_memory_bound(objects, shape)?;
            let count_u64 = u64::try_from(count).map_err(|_error| IndexError::ResourceLimited)?;
            let each_wire = u64::try_from(HEADER_LEN + payload + children * 32)
                .map_err(|_error| IndexError::ResourceLimited)?;
            let path = u64::try_from(objects.as_os_str().as_encoded_bytes().len() + 3)
                .map_err(|_error| IndexError::ResourceLimited)?;
            let actual_temporary = batch_preparation_memory_bytes(count)?
                + count_u64 * each_wire
                + RegistrationBatch::allocation_bytes_for_count(count)?
                + count_u64 * path
                + u64::try_from(objects.as_os_str().len())
                    .map_err(|_error| IndexError::ResourceLimited)?
                    * 3
                + 512
                + u64::try_from(MAX_WIRE).map_err(|_error| IndexError::ResourceLimited)?;
            let actual_retained = RegisteredObject::allocation_bytes(children)? * count_u64
                + quanta_index_core::index::RegisteredObjectBatch::allocation_bytes(count)?;
            assert!(bound.temporary_data_bytes >= actual_temporary);
            assert!(bound.retained_data_bytes >= actual_retained);
            assert!(
                bound.max_live_temporary_leases
                    >= u32::try_from(count + 5).map_err(|_error| IndexError::ResourceLimited)?
            );
            assert!(
                bound.max_live_retained_leases
                    >= u32::try_from(count + 1).map_err(|_error| IndexError::ResourceLimited)?
            );
        }
        Ok(())
    }

    fn assertion_error<E: From<IndexError>>(detail: String) -> E {
        IndexError::BackendViolation(detail).into()
    }

    // Fallible assertions preserve test predicates in Result-returning fixtures.
    macro_rules! check {
        ($condition:expr $(,)?) => {{
            if !$condition {
                return Err(assertion_error(format!(
                    "assertion failed: {}",
                    stringify!($condition)
                )));
            }
        }};
    }

    macro_rules! check_eq {
        ($left:expr, $right:expr $(,)?) => {{
            let left = &$left;
            let right = &$right;
            if left != right {
                return Err(assertion_error(format!(
                    "assertion failed: left {left:?} != right {right:?}"
                )));
            }
        }};
    }

    static NEXT_TEST_DIR: AtomicU64 = AtomicU64::new(0);

    struct TestDir(PathBuf);

    impl TestDir {
        fn new() -> Self {
            let sequence = NEXT_TEST_DIR.fetch_add(1, Ordering::Relaxed);
            Self(
                std::env::temp_dir()
                    .join(format!("qidx-identify-{}-{sequence}", std::process::id())),
            )
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            drop(fs::remove_dir_all(&self.0));
        }
    }

    struct TestSink;

    #[test]
    fn observed_install_batch_counts_real_wire_syncs_and_deduplicated_replay()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = TestDir::new();
        let observer = Arc::new(ObjectIoObservation::default());
        let store = FileObjectStore::open(&directory.0)?.with_io_observation(Arc::clone(&observer));
        let before = observer.snapshot()?;
        let inputs = [
            ObjectBatchInput {
                payload: b"fixed".to_vec(),
                children: Vec::new(),
            },
            ObjectBatchInput {
                payload: b"fixed".to_vec(),
                children: Vec::new(),
            },
        ];
        let mut work = budget()?;
        let registrations = store.install_batch(&inputs, &TestSink, &mut work)?;
        let [first, second] = registrations.as_slice() else {
            return Err("missing duplicate ordinals".into());
        };
        check!(first.reference == second.reference);
        let wire = fs::read(store.path_for(first.reference))?;
        let gold = b"QIOBJ001\0\0\0\x05\0\0\0\0fixed";
        check!(wire.as_slice() == gold);
        let sample = observer.snapshot()?.checked_since(&before)?;
        check!(sample.write_calls > 0 && sample.written_bytes == 21);
        check!(sample.file_sync_calls == 1 && sample.directory_sync_calls == 2);
        check!(sample.reserve_calls == 1 && sample.confirm_calls == 1);
        check!(sample.reserve_objects == 1 && sample.confirm_objects == 1);
        check!(sample.single_object_reserves == 1 && sample.single_object_confirms == 1);
        check!(sample.failed_reserves == 0 && sample.failed_confirms == 0);
        check!(
            sample.failed_writes == 0
                && sample.failed_file_syncs == 0
                && sample.failed_directory_syncs == 0
                && sample.failed_barriers == 0
        );
        #[cfg(target_os = "macos")]
        check!(sample.barrier_calls == 1);
        #[cfg(not(target_os = "macos"))]
        check!(sample.barrier_calls == 0);
        let replay_before = observer.snapshot()?;
        let replay = store.install_batch(&inputs, &TestSink, &mut work)?;
        check!(replay == registrations);
        let replay = observer.snapshot()?.checked_since(&replay_before)?;
        check!(replay.write_calls == 0 && replay.written_bytes == 0);
        check!(replay.directory_sync_calls == 2);
        check!(replay.reserve_calls == 1 && replay.confirm_calls == 1);
        check!(replay.reserve_objects == 1 && replay.confirm_objects == 1);
        check!(replay.single_object_reserves == 1 && replay.single_object_confirms == 1);
        #[cfg(target_os = "macos")]
        check!(replay.file_sync_calls == 1 && replay.barrier_calls == 1);
        Ok(())
    }

    #[derive(Default)]
    struct RefusingReservation(AtomicU64);

    struct ObservedCustodyRefusal(CustodyKind);

    impl ObjectWriteSink for ObservedCustodyRefusal {
        fn reserve(&self, _object: &RegisteredObject) -> Result<(), IndexError> {
            if matches!(self.0, CustodyKind::Reserve) {
                Err(IndexError::ResourceLimited)
            } else {
                Ok(())
            }
        }

        fn installed(&self, _object: &RegisteredObject) -> Result<(), IndexError> {
            if matches!(self.0, CustodyKind::Confirm) {
                Err(IndexError::FenceLost)
            } else {
                Ok(())
            }
        }

        fn reserve_batch(&self, objects: &[RegisteredObject]) -> Result<(), IndexError> {
            objects.iter().try_for_each(|object| self.reserve(object))
        }

        fn installed_batch(&self, objects: &[RegisteredObject]) -> Result<(), IndexError> {
            objects.iter().try_for_each(|object| self.installed(object))
        }
    }

    #[test]
    fn observed_custody_refusals_preserve_scalar_and_batch_failure_prefixes()
    -> Result<(), Box<dyn std::error::Error>> {
        for batched in [false, true] {
            for stage in [CustodyKind::Reserve, CustodyKind::Confirm] {
                let directory = TestDir::new();
                let observer = Arc::new(ObjectIoObservation::default());
                let store =
                    FileObjectStore::open(&directory.0)?.with_io_observation(Arc::clone(&observer));
                let before = observer.snapshot()?;
                let sink = ObservedCustodyRefusal(stage);
                let mut work = budget()?;
                let result = if batched {
                    store
                        .install_batch(
                            &[ObjectBatchInput {
                                payload: b"fixed".to_vec(),
                                children: Vec::new(),
                            }],
                            &sink,
                            &mut work,
                        )
                        .map(|_installed| ())
                } else {
                    store
                        .install(b"fixed", &[], &sink, &mut work)
                        .map(|_installed| ())
                };
                let sample = observer.snapshot()?.checked_since(&before)?;
                check_eq!(sample.reserve_calls, 1);
                check_eq!(
                    (sample.reserve_objects, sample.single_object_reserves),
                    (1, 1)
                );
                match stage {
                    CustodyKind::Reserve => {
                        check!(matches!(result, Err(IndexError::ResourceLimited)));
                        check_eq!((sample.failed_reserves, sample.confirm_calls), (1, 0));
                        check_eq!(
                            (sample.confirm_objects, sample.single_object_confirms),
                            (0, 0)
                        );
                        check_eq!((sample.write_calls, sample.written_bytes), (0, 0));
                        check_eq!(
                            sample.file_sync_calls
                                + sample.directory_sync_calls
                                + sample.barrier_calls,
                            0
                        );
                        check_eq!(object_file_count(&directory)?, 0);
                    }
                    CustodyKind::Confirm => {
                        check!(matches!(result, Err(IndexError::FenceLost)));
                        check_eq!(
                            (sample.confirm_objects, sample.single_object_confirms),
                            (1, 1)
                        );
                        check_eq!(
                            (
                                sample.failed_reserves,
                                sample.confirm_calls,
                                sample.failed_confirms
                            ),
                            (0, 1, 1)
                        );
                        check_eq!(sample.written_bytes, 21);
                        check_eq!(sample.file_sync_calls, 1);
                        check_eq!(object_file_count(&directory)?, 1);
                    }
                }
                check_eq!(
                    (
                        sample.failed_writes,
                        sample.failed_file_syncs,
                        sample.failed_directory_syncs,
                        sample.failed_barriers
                    ),
                    (0, 0, 0, 0)
                );
            }
        }
        Ok(())
    }

    struct CancellingReservation {
        cancel: quanta_index_core::CancelHandleV1,
        confirmations: AtomicU64,
    }

    impl ObjectWriteSink for CancellingReservation {
        fn reserve(&self, _object: &RegisteredObject) -> Result<(), IndexError> {
            self.cancel.cancel();
            Ok(())
        }

        fn installed(&self, _object: &RegisteredObject) -> Result<(), IndexError> {
            let _prior = self.confirmations.fetch_add(1, Ordering::Relaxed);
            Ok(())
        }

        fn reserve_batch(&self, _objects: &[RegisteredObject]) -> Result<(), IndexError> {
            self.cancel.cancel();
            Ok(())
        }

        fn installed_batch(&self, _objects: &[RegisteredObject]) -> Result<(), IndexError> {
            let _prior = self.confirmations.fetch_add(1, Ordering::Relaxed);
            Ok(())
        }
    }

    #[test]
    fn cancellation_during_reservation_prevents_physical_installation() -> Result<(), IndexError> {
        for batched in [false, true] {
            let root = TestDir::new();
            let observer = Arc::new(ObjectIoObservation::default());
            let store = FileObjectStore::open(&root.0)?.with_io_observation(Arc::clone(&observer));
            let before = observer.snapshot()?;
            let mut work = budget()?;
            let sink = CancellingReservation {
                cancel: work.request().cancel_handle(),
                confirmations: AtomicU64::new(0),
            };
            let result = if batched {
                store
                    .install_batch(
                        &[ObjectBatchInput {
                            payload: b"cancelled parent".to_vec(),
                            children: Vec::new(),
                        }],
                        &sink,
                        &mut work,
                    )
                    .map(|_installed| ())
            } else {
                store
                    .install(b"cancelled parent", &[], &sink, &mut work)
                    .map(|_installed| ())
            };
            check!(matches!(result, Err(IndexError::Cancelled)));
            check_eq!(sink.confirmations.load(Ordering::Relaxed), 0);
            let sample = observer.snapshot()?.checked_since(&before)?;
            check_eq!(
                (
                    sample.reserve_calls,
                    sample.failed_reserves,
                    sample.confirm_calls
                ),
                (1, 0, 0)
            );
            check_eq!((sample.write_calls, sample.written_bytes), (0, 0));
            check_eq!(
                (sample.reserve_objects, sample.single_object_reserves),
                (1, 1)
            );
            check_eq!(
                (sample.confirm_objects, sample.single_object_confirms),
                (0, 0)
            );
            check_eq!(
                fs::read_dir(root.0.join("objects"))
                    .map_err(|error| storage_error(&error))?
                    .count(),
                0
            );
        }
        Ok(())
    }

    impl ObjectWriteSink for RefusingReservation {
        fn reserve(&self, _object: &RegisteredObject) -> Result<(), IndexError> {
            self.reserve_batch(&[])
        }

        fn installed(&self, _object: &RegisteredObject) -> Result<(), IndexError> {
            Err(IndexError::BackendViolation(
                "unexpected install confirmation".into(),
            ))
        }

        fn reserve_batch(&self, _objects: &[RegisteredObject]) -> Result<(), IndexError> {
            let _prior = self.0.fetch_add(1, Ordering::Relaxed);
            Err(IndexError::BackendViolation(
                "reservation refused before child read".into(),
            ))
        }

        fn installed_batch(&self, _objects: &[RegisteredObject]) -> Result<(), IndexError> {
            Err(IndexError::BackendViolation(
                "unexpected install confirmation".into(),
            ))
        }
    }

    #[test]
    fn reservation_precedes_external_child_physical_lookup() -> Result<(), IndexError> {
        let root = TestDir::new();
        let store = FileObjectStore::open(&root.0)?;
        let child = ObjectRef(Digest([7; 32]));
        for batched in [false, true] {
            let sink = RefusingReservation::default();
            let result = if batched {
                store
                    .install_batch(
                        &[ObjectBatchInput {
                            payload: b"parent".to_vec(),
                            children: vec![child],
                        }],
                        &sink,
                        &mut budget()?,
                    )
                    .map(|_installed| ())
            } else {
                store
                    .install(b"parent", &[child], &sink, &mut budget()?)
                    .map(|_installed| ())
            };
            check!(
                matches!(result, Err(IndexError::BackendViolation(ref message))
                if message == "reservation refused before child read")
            );
            check_eq!(sink.0.load(Ordering::Relaxed), 1);
            check_eq!(
                fs::read_dir(root.0.join("objects"))
                    .map_err(|error| storage_error(&error))?
                    .count(),
                0
            );
        }
        Ok(())
    }

    impl ObjectWriteSink for TestSink {
        fn reserve(&self, _object: &RegisteredObject) -> Result<(), IndexError> {
            Ok(())
        }
        fn installed(&self, _object: &RegisteredObject) -> Result<(), IndexError> {
            Ok(())
        }
        fn reserve_batch(&self, _objects: &[RegisteredObject]) -> Result<(), IndexError> {
            Ok(())
        }
        fn installed_batch(&self, _objects: &[RegisteredObject]) -> Result<(), IndexError> {
            Ok(())
        }
    }

    #[derive(Default)]
    struct CountingSink(AtomicU64, AtomicU64);

    impl ObjectWriteSink for CountingSink {
        fn reserve(&self, _object: &RegisteredObject) -> Result<(), IndexError> {
            Err(IndexError::BackendViolation(
                "unexpected scalar reserve".into(),
            ))
        }
        fn installed(&self, _object: &RegisteredObject) -> Result<(), IndexError> {
            Err(IndexError::BackendViolation(
                "unexpected scalar install".into(),
            ))
        }
        fn reserve_batch(&self, objects: &[RegisteredObject]) -> Result<(), IndexError> {
            check_eq!(objects.len(), 2);
            let _prior = self.0.fetch_add(1, Ordering::Relaxed);
            Ok(())
        }
        fn installed_batch(&self, objects: &[RegisteredObject]) -> Result<(), IndexError> {
            check_eq!(objects.len(), 2);
            let _prior = self.1.fetch_add(1, Ordering::Relaxed);
            Ok(())
        }
    }

    fn budget() -> Result<WorkBudget, IndexError> {
        Ok(WorkBudget::new(
            WorkLimits {
                reads: 100,
                writes: 100,
                bytes: 1_000_000,
                mutations: 100,
            },
            Instant::now()
                .checked_add(Duration::from_secs(30))
                .ok_or(IndexError::ResourceLimited)?,
        ))
    }

    fn shard_count(directory: &TestDir) -> Result<usize, std::io::Error> {
        fs::read_dir(directory.0.join("objects")).map(Iterator::count)
    }

    fn object_file_count(directory: &TestDir) -> Result<usize, std::io::Error> {
        let mut count = 0usize;
        for shard in fs::read_dir(directory.0.join("objects"))? {
            count = count
                .checked_add(fs::read_dir(shard?.path())?.count())
                .ok_or_else(|| std::io::Error::other("test object count overflow"))?;
        }
        Ok(count)
    }

    #[test]
    fn object_batch_deduplicates_identical_wire_and_confirms_once()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = TestDir::new();
        let store = FileObjectStore::open(&directory.0)?;
        let mut work = budget()?;
        let child = store.identify(b"same", &[], &mut work)?.reference;
        let sink = CountingSink::default();
        let inputs = [
            ObjectBatchInput {
                payload: b"same".to_vec(),
                children: Vec::new(),
            },
            ObjectBatchInput {
                payload: b"same".to_vec(),
                children: Vec::new(),
            },
            ObjectBatchInput {
                payload: b"parent".to_vec(),
                children: vec![child],
            },
        ];
        let registrations = ImmutableObjectPort::install_batch(&store, &inputs, &sink, &mut work)?;
        check_eq!(registrations.first(), registrations.get(1));
        check_eq!(registrations.len(), 3);
        check_eq!(sink.0.load(Ordering::Relaxed), 1);
        check_eq!(sink.1.load(Ordering::Relaxed), 1);
        let parent = registrations.get(2).ok_or("parent registration missing")?;
        check_eq!(
            store
                .get(parent.reference, &mut work)?
                .registration
                .children,
            vec![child]
        );
        let physical_count = object_file_count(&directory)?;
        let replay = ImmutableObjectPort::install_batch(&store, &inputs, &sink, &mut work)?;
        check_eq!(replay, registrations);
        check_eq!(object_file_count(&directory)?, physical_count);
        check_eq!(sink.0.load(Ordering::Relaxed), 2);
        check_eq!(sink.1.load(Ordering::Relaxed), 2);
        Ok(())
    }

    #[test]
    fn internal_batch_limits_reject_before_reservation_or_physical_install()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = TestDir::new();
        let store = FileObjectStore::open(&directory.0)?;
        let sink = CountingSink::default();
        let mut work = budget()?;
        let tiny = store.prepare_object(b"tiny", &[], &mut work)?;
        let over_count = vec![&tiny; 257];
        check!(matches!(
            store.install_prepared_batch(&over_count, &sink, &mut work),
            Err(IndexError::ResourceLimited)
        ));
        check_eq!(sink.0.load(Ordering::Relaxed), 0);
        check_eq!(sink.1.load(Ordering::Relaxed), 0);
        check_eq!(object_file_count(&directory)?, 0);

        let mut large_work = WorkBudget::new(
            WorkLimits {
                reads: 100,
                writes: 100,
                bytes: 134_217_728,
                mutations: 100,
            },
            Instant::now()
                .checked_add(Duration::from_secs(30))
                .ok_or(IndexError::ResourceLimited)?,
        );
        let large_payload = vec![7u8; MAX_PAYLOAD];
        let large = store.prepare_object(&large_payload, &[], &mut large_work)?;
        let over_bytes = [&large; 4];
        check!(matches!(
            store.install_prepared_batch(&over_bytes, &sink, &mut large_work),
            Err(IndexError::ResourceLimited)
        ));
        check_eq!(sink.0.load(Ordering::Relaxed), 0);
        check_eq!(sink.1.load(Ordering::Relaxed), 0);
        check_eq!(object_file_count(&directory)?, 0);
        Ok(())
    }

    #[test]
    fn protected_snapshot_page_stream_authenticates_exact_wire_at_finish()
    -> Result<(), Box<dyn std::error::Error>> {
        use std::io::Read as _;

        let directory = TestDir::new();
        let store = FileObjectStore::open(&directory.0)?;
        let mut build = budget()?;
        let child = store.put(b"child", &[], &mut build)?.reference;
        let payload = vec![0xa5; 128 * 1024];
        let object = store.put(&payload, &[child], &mut build)?.reference;
        let mut verify = budget()?;
        let mut page = store.open_protected_snapshot_page_v1(object, &mut verify)?;
        check_eq!(page.payload_len(), payload.len());
        check_eq!(page.registration().children.as_slice(), &[child][..]);
        let mut scratch = [0; 4096];
        let mut seen = 0usize;
        loop {
            let count = page.read(&mut scratch)?;
            if count == 0 {
                break;
            }
            check!(scratch[..count].iter().all(|byte| *byte == 0xa5));
            seen += count;
        }
        check_eq!(seen, payload.len());
        page.finish()?;
        check_eq!(verify.usage().reads, 1);
        check_eq!(verify.usage().writes, 0);

        let path = store.path_for(object);
        let original = fs::read(&path)?;
        for offset in [HEADER_LEN, original.len() - 1] {
            let mut wire = original.clone();
            wire[offset] ^= 1;
            fs::write(&path, wire)?;
            let mut tamper_work = budget()?;
            let mut page = store.open_protected_snapshot_page_v1(object, &mut tamper_work)?;
            while page.read(&mut scratch)? != 0 {}
            check!(matches!(
                page.finish(),
                Err(IndexError::CorruptObject(message)) if message == "object digest or size mismatch"
            ));
        }
        fs::write(&path, &original)?;
        let mut truncate_work = budget()?;
        let mut page = store.open_protected_snapshot_page_v1(object, &mut truncate_work)?;
        fs::OpenOptions::new()
            .write(true)
            .open(&path)?
            .set_len(HEADER_LEN as u64)?;
        let error = page
            .read(&mut scratch)
            .expect_err("truncated page was read");
        check!(matches!(
            error
                .get_ref()
                .and_then(|cause| cause.downcast_ref::<IndexError>()),
            Some(IndexError::CorruptObject(message)) if message == "object truncated during read"
        ));
        fs::write(path, original)?;
        Ok(())
    }

    #[test]
    fn small_stored_wire_preserves_edges_and_digest_at_boundary()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = TestDir::new();
        let store = FileObjectStore::open(&directory.0)?;
        let mut work = budget()?;
        let child = store.put(b"child", &[], &mut work)?.reference;
        let small_len = read_io::SMALL_STORED_WIRE_BYTES - HEADER_LEN - 32;
        let small_payload = vec![0xa5; small_len];
        let small = store.put(&small_payload, &[child], &mut work)?.reference;
        let large_payload = vec![0x5a; small_len + 1];
        let large = store.put(&large_payload, &[child], &mut work)?.reference;
        check_eq!(fs::metadata(store.path_for(small))?.len(), 512);
        check_eq!(fs::metadata(store.path_for(large))?.len(), 513);
        for (reference, payload) in [(small, &small_payload), (large, &large_payload)] {
            let stored = store.get(reference, &mut work)?;
            check_eq!(stored.payload.as_slice(), payload.as_slice());
            check_eq!(stored.registration.children.as_slice(), &[child]);
        }
        let path = store.path_for(small);
        let mut damaged = fs::read(&path)?;
        let last = damaged.last_mut().ok_or("missing object payload")?;
        *last ^= 1;
        fs::write(&path, damaged)?;
        check!(matches!(
            store.get(small, &mut work),
            Err(IndexError::CorruptObject(message)) if message == "object digest or size mismatch"
        ));
        Ok(())
    }

    #[test]
    fn identify_matches_durable_install_without_touching_storage()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = TestDir::new();
        let store = FileObjectStore::open(&directory.0)?;
        let mut work = budget()?;
        let child = store.put(b"child", &[], &mut work)?.reference;
        let before = shard_count(&directory)?;
        let identified = store.identify(b"parent", &[child], &mut work)?;
        check_eq!(shard_count(&directory)?, before);
        check!(!store.path_for(identified.reference).exists());
        let installed = store.install(b"parent", &[child], &TestSink, &mut work)?;
        check_eq!(identified, installed);
        check_eq!(
            store.get(identified.reference, &mut work)?.registration,
            identified
        );
        Ok(())
    }

    #[test]
    fn identify_missing_child_is_pure_but_install_refuses_it()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = TestDir::new();
        let store = FileObjectStore::open(&directory.0)?;
        let mut work = budget()?;
        let missing = ObjectRef(Digest([99; 32]));
        let identified = store.identify(b"pending", &[missing], &mut work)?;
        check_eq!(identified.children, vec![missing]);
        check_eq!(shard_count(&directory)?, 0);
        let mut denied = WorkBudget::new(
            WorkLimits {
                reads: 0,
                writes: 0,
                bytes: 0,
                mutations: 0,
            },
            Instant::now()
                .checked_add(Duration::from_secs(30))
                .ok_or(IndexError::ResourceLimited)?,
        );
        check!(matches!(
            store.identify(b"pending", &[missing], &mut denied),
            Err(IndexError::ResourceLimited)
        ));
        check_eq!(shard_count(&directory)?, 0);
        check!(
            store
                .install(b"pending", &[missing], &TestSink, &mut work)
                .is_err()
        );
        check_eq!(shard_count(&directory)?, 0);
        Ok(())
    }

    #[test]
    fn absent_shard_and_empty_shard_have_the_same_object_error_class()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = TestDir::new();
        let store = FileObjectStore::open(&directory.0)?;
        let mut work = budget()?;
        let missing = ObjectRef(Digest([99; 32]));
        let path = store.path_for(missing);
        let shard = path.parent().ok_or("object shard missing")?;
        check!(!shard.exists());
        check!(matches!(
            store.get(missing, &mut work),
            Err(IndexError::CorruptObject(_))
        ));
        check!(matches!(
            store.install(b"parent", &[missing], &TestSink, &mut work),
            Err(IndexError::InvalidInput(_))
        ));

        ensure_private_directory(shard)?;
        check!(!path.exists());
        check!(matches!(
            store.get(missing, &mut work),
            Err(IndexError::CorruptObject(_))
        ));
        check!(matches!(
            store.install(b"parent", &[missing], &TestSink, &mut work),
            Err(IndexError::InvalidInput(_))
        ));

        fs::set_permissions(&directory.0, fs::Permissions::from_mode(0o755))?;
        check!(matches!(
            store.get(missing, &mut work),
            Err(IndexError::Storage(_))
        ));
        fs::set_permissions(&directory.0, fs::Permissions::from_mode(0o700))?;
        Ok(())
    }

    #[test]
    fn no_replace_publication_is_single_link_before_directory_sync()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = TestDir::new();
        let writer = FileObjectStore::open(&directory.0)?;
        let reader = FileObjectStore::open(&directory.0)?;
        let mut work = budget()?;
        let prepared = writer.prepare_object(b"child", &[], &mut work)?;
        let reference = prepared.registration.reference;
        let final_path = writer.path_for(reference);
        let shard = final_path.parent().ok_or("object shard missing")?;
        ensure_private_directory(shard)?;

        let (mut file, temp_path) = writer.create_temp(shard)?;
        file.write_all(&prepared.wire)?;
        file.sync_all()?;
        let source = file.metadata()?;
        validate_private_file(&source)?;
        let mut temp = TempFile(temp_path.clone());
        check!(publish_temp_no_replace(&mut temp, &final_path, &source)?);
        check!(!temp_path.exists());
        check_eq!(fs::symlink_metadata(&final_path)?.nlink(), 1);

        // The other store reads the canonical name while the writer still
        // holds its file descriptor and before the shard directory is synced.
        check_eq!(reader.get(reference, &mut work)?.payload, b"child");
        let parent = reader.put(b"parent", &[reference], &mut work)?;
        check_eq!(
            reader
                .get(parent.reference, &mut work)?
                .registration
                .children,
            vec![reference]
        );

        let winner = fs::symlink_metadata(&final_path)?;
        let (mut loser_file, loser_path) = reader.create_temp(shard)?;
        loser_file.write_all(&prepared.wire)?;
        loser_file.sync_all()?;
        let loser_source = loser_file.metadata()?;
        let mut loser = TempFile(loser_path.clone());
        check!(!publish_temp_no_replace(
            &mut loser,
            &final_path,
            &loser_source,
        )?);
        check!(loser_path.exists());
        let surviving = fs::symlink_metadata(&final_path)?;
        check_eq!(
            (surviving.dev(), surviving.ino(), surviving.nlink()),
            (winner.dev(), winner.ino(), 1)
        );
        check_eq!(reader.get(reference, &mut work)?.payload, b"child");
        drop(loser_file);
        drop(loser);
        check!(!loser_path.exists());
        drop(file);
        Ok(())
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn batch_barrier_failure_leaves_reserved_objects_unconfirmed()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = TestDir::new();
        let observer = Arc::new(ObjectIoObservation::default());
        let store = FileObjectStore::open(&directory.0)?.with_io_observation(Arc::clone(&observer));
        let before = observer.snapshot()?;
        let mut work = budget()?;
        let child = store.prepare_object(b"child", &[], &mut work)?;
        let parent = store.prepare_object(b"parent", &[child.registration.reference], &mut work)?;
        let objects = [&child, &parent];
        let sink = CountingSink::default();
        let barrier_calls = AtomicU64::new(0);
        let result =
            store.install_prepared_batch_with_barrier(&objects, &sink, &mut work, |file| {
                check!(
                    file.metadata()
                        .map_err(|error| storage_error(&error))?
                        .is_file()
                );
                for object in objects {
                    let path = store.path_for(object.registration.reference);
                    check_eq!(
                        fs::read(path).map_err(|error| storage_error(&error))?,
                        object.wire
                    );
                }
                let _prior = barrier_calls.fetch_add(1, Ordering::Relaxed);
                Err(IndexError::Storage("injected batch barrier failure".into()))
            });
        check!(
            matches!(result, Err(IndexError::Storage(message)) if message == "injected batch barrier failure")
        );
        check_eq!(barrier_calls.load(Ordering::Relaxed), 1);
        check_eq!(sink.0.load(Ordering::Relaxed), 1);
        check_eq!(sink.1.load(Ordering::Relaxed), 0);
        check_eq!(object_file_count(&directory)?, 2);
        let sample = observer.snapshot()?.checked_since(&before)?;
        // Fixed child and parent wire lengths: 16+5 and 16+32+6 bytes.
        check_eq!(
            (
                sample.reserve_calls,
                sample.failed_reserves,
                sample.confirm_calls
            ),
            (1, 0, 0)
        );
        check_eq!(sample.written_bytes, 75);
        check_eq!(sample.file_sync_calls, 2);
        check_eq!((sample.barrier_calls, sample.failed_barriers), (1, 1));
        check_eq!(
            (sample.reserve_objects, sample.single_object_reserves),
            (2, 0)
        );
        check_eq!(
            (sample.confirm_objects, sample.single_object_confirms),
            (0, 0)
        );
        check_eq!(
            (
                sample.failed_writes,
                sample.failed_file_syncs,
                sample.failed_directory_syncs
            ),
            (0, 0, 0)
        );
        Ok(())
    }

    #[test]
    fn directory_sync_rejects_replaced_name_after_open() -> Result<(), Box<dyn std::error::Error>> {
        let directory = TestDir::new();
        let store = FileObjectStore::open(&directory.0)?;
        let linked = private_directory_metadata(&store.objects)?;
        let opened = File::open(&store.objects)?;
        let opened_metadata = opened.metadata()?;
        let detached = directory.0.join("detached-objects");
        fs::rename(&store.objects, &detached)?;
        fs::create_dir(&store.objects)?;
        fs::set_permissions(&store.objects, fs::Permissions::from_mode(0o700))?;

        check!(matches!(
            validate_synced_directory_path(&store.objects, &linked, &opened_metadata),
            Err(IndexError::Storage(_))
        ));
        Ok(())
    }

    #[test]
    fn directory_sync_rejects_symlink_name() -> Result<(), Box<dyn std::error::Error>> {
        let directory = TestDir::new();
        let store = FileObjectStore::open(&directory.0)?;
        let link = directory.0.join("objects-link");
        std::os::unix::fs::symlink(&store.objects, &link)?;
        check!(matches!(
            sync_directory_witness(&link),
            Err(IndexError::Storage(_))
        ));
        Ok(())
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn batch_directory_sync_rejects_symlink_before_fsync() -> Result<(), Box<dyn std::error::Error>>
    {
        let directory = TestDir::new();
        let store = FileObjectStore::open(&directory.0)?;
        let link = directory.0.join("objects-link");
        std::os::unix::fs::symlink(&store.objects, &link)?;
        let device = fs::symlink_metadata(&store.objects)?.dev();
        check!(matches!(
            sync_directory_plain(&link, device),
            Err(IndexError::Storage(_))
        ));
        check!(fs::symlink_metadata(&store.objects)?.is_dir());
        Ok(())
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn existing_batch_uses_regular_file_barrier_and_rejects_wrong_device()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = TestDir::new();
        let store = FileObjectStore::open(&directory.0)?;
        let mut work = budget()?;
        let child = store.prepare_object(b"child", &[], &mut work)?;
        let parent = store.prepare_object(b"parent", &[child.registration.reference], &mut work)?;
        let objects = [&child, &parent];
        store.install_prepared_batch(&objects, &CountingSink::default(), &mut work)?;
        let physical_count = object_file_count(&directory)?;

        let device = fs::symlink_metadata(&store.objects)?.dev();
        let other_device = device.checked_add(1).ok_or("test device overflow")?;
        check!(matches!(
            sync_directory_plain(&store.objects, other_device),
            Err(IndexError::Storage(_))
        ));

        let replay_sink = CountingSink::default();
        let barrier_calls = AtomicU64::new(0);
        store.install_prepared_batch_with_barrier(&objects, &replay_sink, &mut work, |file| {
            let metadata = file.metadata().map_err(|error| storage_error(&error))?;
            check!(metadata.is_file());
            check_eq!(metadata.dev(), device);
            rustix::fs::fcntl_fullfsync(file).map_err(|error| {
                IndexError::Storage(format!("full sync existing batch: {error}"))
            })?;
            let _prior = barrier_calls.fetch_add(1, Ordering::Relaxed);
            Ok(())
        })?;
        check_eq!(barrier_calls.load(Ordering::Relaxed), 1);
        check_eq!(replay_sink.0.load(Ordering::Relaxed), 1);
        check_eq!(replay_sink.1.load(Ordering::Relaxed), 1);
        check_eq!(object_file_count(&directory)?, physical_count);
        Ok(())
    }
}
