use std::{
    collections::BTreeSet,
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
    Digest, IndexError, MAX_RECLAIM_CHILDREN, ObjectBatchInput, ObjectInstallScope, ObjectRef,
    RegisteredObject, ScratchSpacePort, StoredObject, WorkBudget, WorkUsage,
    ports::{ImmutableObjectPort, ManagedObjectAuthority, ObjectWriteSink},
};
use quanta_index_core::{
    DiagnosticCounterV1, DiagnosticHealthCodeV1, DiagnosticLaneV1, DiagnosticSpanGuardV1,
    DiagnosticStageV1, DiagnosticTerminalV1, DiagnosticTraceErrorV1,
};
use sha2::{Digest as _, Sha256};

use crate::scratch::FileScratchSpace;

mod batch_metadata;
mod io_observation;
#[cfg(test)]
mod managed_trace_tests;
mod namespace;
mod read_io;
mod read_only;
mod reclaim;
mod write_io;
mod write_preparation;

use batch_metadata::{BatchIndex, BatchReferences, RegistrationBatch};
use io_observation::{CustodyKind, SyncKind};
pub use io_observation::{ObjectIoObservation, ObjectIoSnapshot};
pub use read_only::FileObjectReader;

use namespace::NamespaceLease;
#[cfg(target_os = "macos")]
use read_io::ObjectIoFile;
use write_io::ObjectWriter;

const MAGIC: &[u8; 8] = b"QIOBJ001";
const HASH_DOMAIN: &[u8] = b"quanta-index-immutable-object-v1\0";
const HEADER_LEN: usize = 16;
const MAX_PAYLOAD: usize = 16 * 1024 * 1024;
const MAX_CHILDREN: usize = MAX_RECLAIM_CHILDREN;
const MAX_WIRE: usize = HEADER_LEN + MAX_CHILDREN * 32 + MAX_PAYLOAD;
const STANDALONE_SCRATCH_CAP: u64 = 16 * 1024 * 1024 * 1024;
// The adapter's caller input limit remains separate from child-first map DAG
// aggregation. Both paths retain the same aggregate wire-byte limit.
pub(crate) const MAX_BATCH_INPUTS: usize = 64;
pub(crate) const MAX_INSTALL_BATCH_INPUTS: usize = 256;
pub(crate) const MAX_BATCH_BYTES: u64 = 64 * 1024 * 1024;

pub(crate) use write_preparation::{AdmittedWire, PreparedObject};

struct ReadWire {
    bytes: Vec<u8>,
    // Drop the bytes before releasing their live allocation custody.
    _memory: Option<quanta_index_core::index::MemoryLease>,
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
            let gate = self.gates.get(index).ok_or_else(|| {
                IndexError::BackendViolation("object gate index out of range".into())
            })?;
            let lock_result = gate.try_lock();
            match lock_result {
                Ok(guard) => return Ok(guard),
                Err(TryLockError::Poisoned(_)) => {
                    return Err(IndexError::BackendViolation(
                        "object mutation gate poisoned".into(),
                    ));
                }
                Err(TryLockError::WouldBlock) => std::thread::sleep(Duration::from_millis(1)),
            }
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
                    |budget| authority.confirm_managed(scope, objects, budget),
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
        #[cfg(target_os = "macos")]
        let barrier = |file: &File| {
            rustix::fs::fcntl_fullfsync(file).map_err(|error| {
                IndexError::Storage(format!("full sync immutable object batch: {error}"))
            })
        };
        #[cfg(not(target_os = "macos"))]
        let barrier = |_: &File| Ok(());
        self.install_prepared_batch_with_barrier(objects, sink, budget, barrier)
    }

    fn install_prepared_batch_with_barrier(
        &self,
        objects: &[&PreparedObject],
        sink: &dyn ObjectWriteSink,
        budget: &mut WorkBudget,
        barrier: impl FnOnce(&File) -> Result<(), IndexError>,
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
        let all_refs = BatchReferences::new(objects, budget)?;
        let metadata = RegistrationBatch::new(objects, budget)?;
        let registrations = metadata.as_slice();
        let mut gate_indexes = [0; MAX_INSTALL_BATCH_INPUTS];
        for (slot, object) in gate_indexes.iter_mut().zip(registrations) {
            *slot = Self::gate_index(object.reference);
        }
        let used_indexes = gate_indexes.get_mut(..objects.len()).ok_or_else(|| {
            IndexError::BackendViolation("batch gate table bounds invalid".into())
        })?;
        used_indexes.sort_unstable();
        let mut gate_guards: [Option<MutexGuard<'_, ()>>; MAX_INSTALL_BATCH_INPUTS] =
            std::array::from_fn(|_| None);
        let mut previous_gate = None;
        for (slot, index) in used_indexes.iter().enumerate() {
            if previous_gate != Some(*index) {
                *gate_guards.get_mut(slot).ok_or_else(|| {
                    IndexError::BackendViolation("batch gate slot bounds invalid".into())
                })? = Some(self.lock_gate(*index, budget)?);
                previous_gate = Some(*index);
            }
        }
        let scope = if self.authority.is_some() {
            Some(sink.managed_scope().ok_or_else(|| {
                IndexError::BackendViolation("managed installation lacks catalog scope".into())
            })?)
        } else {
            None
        };
        self.reserve_install(sink, scope, registrations, budget, true)?;
        budget.checkpoint()?;
        for object in objects {
            for child in &object.registration.children {
                if !all_refs.contains(*child)? {
                    budget.charge(WorkUsage {
                        reads: 1,
                        ..WorkUsage::default()
                    })?;
                    self.validate_child_path(*child)?;
                }
            }
        }

        #[cfg(target_os = "macos")]
        let batch_device = {
            self.validate_store_root()?;
            fs::symlink_metadata(&self.objects)
                .map_err(|error| storage_error(&error))?
                .dev()
        };
        #[cfg(target_os = "macos")]
        let mut barrier_file: Option<ObjectIoFile> = None;
        let mut pending_writes: [Option<Box<dyn quanta_index_core::index::ObjectWriteTicket>>;
            MAX_INSTALL_BATCH_INPUTS] = std::array::from_fn(|_| None);
        let mut referenced_shards = BTreeSet::new();
        for (position, object) in objects.iter().enumerate() {
            budget.checkpoint()?;
            self.validate_store_root()?;
            let final_path = self.path_for(object.registration.reference);
            let shard = final_path
                .parent()
                .ok_or_else(|| IndexError::Storage("object path has no parent".into()))?;
            ensure_private_directory(shard)?;
            #[cfg(target_os = "macos")]
            ensure_same_device(
                batch_device,
                fs::symlink_metadata(shard)
                    .map_err(|error| storage_error(&error))?
                    .dev(),
            )?;
            if final_path.exists() {
                self.verify_existing(
                    object.registration.reference,
                    &final_path,
                    &object.wire,
                    budget,
                )?;
                #[cfg(target_os = "macos")]
                {
                    let existing = self.open_private_regular_file(&final_path, budget)?;
                    ensure_same_device(
                        batch_device,
                        existing
                            .file()?
                            .metadata()
                            .map_err(|error| storage_error(&error))?
                            .dev(),
                    )?;
                    self.sync_observed(SyncKind::File, || {
                        rustix::fs::fsync(existing.file()?).map_err(|error| {
                            IndexError::Storage(format!("sync existing immutable object: {error}"))
                        })
                    })?;
                    if barrier_file.is_none() {
                        barrier_file = Some(existing);
                    }
                }
                let _was_new = referenced_shards.insert(shard.to_path_buf());
                continue;
            }
            let mut writer =
                self.create_writer(shard, &final_path, object.registration.bytes, budget)?;
            #[cfg(target_os = "macos")]
            ensure_same_device(
                batch_device,
                writer
                    .file()?
                    .metadata()
                    .map_err(|error| storage_error(&error))?
                    .dev(),
            )?;
            self.write_observed(writer.file_mut()?, &object.wire)?;
            #[cfg(target_os = "macos")]
            self.sync_observed(SyncKind::File, || {
                rustix::fs::fsync(writer.file()?)
                    .map_err(|error| IndexError::Storage(format!("sync immutable object: {error}")))
            })?;
            #[cfg(not(target_os = "macos"))]
            self.sync_observed(SyncKind::File, || {
                writer
                    .file()?
                    .sync_all()
                    .map_err(|error| storage_error(&error))
            })?;
            let written = writer
                .file()?
                .metadata()
                .map_err(|error| storage_error(&error))?;
            validate_private_file(&written)?;
            match writer.publish(&final_path, &written) {
                Ok(true) => {
                    validate_private_file(
                        &writer
                            .file()?
                            .metadata()
                            .map_err(|error| storage_error(&error))?,
                    )?;
                    #[cfg(target_os = "macos")]
                    {
                        if barrier_file.is_none() {
                            barrier_file = Some(ObjectIoFile::Writer(writer));
                        } else {
                            *pending_writes
                                .get_mut(position)
                                .ok_or(IndexError::ResourceLimited)? = writer.retire()?;
                        }
                    }
                    #[cfg(not(target_os = "macos"))]
                    {
                        *pending_writes
                            .get_mut(position)
                            .ok_or(IndexError::ResourceLimited)? = writer.retire()?;
                    }
                    let _was_new = referenced_shards.insert(shard.to_path_buf());
                }
                Ok(false) => {
                    let _no_output = writer.retire()?;
                    self.verify_existing(
                        object.registration.reference,
                        &final_path,
                        &object.wire,
                        budget,
                    )?;
                    #[cfg(target_os = "macos")]
                    {
                        let existing = self.open_private_regular_file(&final_path, budget)?;
                        ensure_same_device(
                            batch_device,
                            existing
                                .file()?
                                .metadata()
                                .map_err(|error| storage_error(&error))?
                                .dev(),
                        )?;
                        self.sync_observed(SyncKind::File, || {
                            rustix::fs::fsync(existing.file()?).map_err(|error| {
                                IndexError::Storage(format!(
                                    "sync concurrent immutable object: {error}"
                                ))
                            })
                        })?;
                        if barrier_file.is_none() {
                            barrier_file = Some(existing);
                        }
                    }
                    let _was_new = referenced_shards.insert(shard.to_path_buf());
                }
                Err(error) => return Err(error),
            }
        }
        // A concurrently installed matching object may not have had its directory
        // entry synced yet. Every referenced shard must be durable before custody
        // confirmation, including already-existing objects.
        #[cfg(target_os = "macos")]
        {
            self.sync_observed(SyncKind::Directory, || {
                sync_directory_plain(&self.objects, batch_device)
            })?;
            for shard in referenced_shards {
                self.sync_observed(SyncKind::Directory, || {
                    sync_directory_plain(&shard, batch_device)
                })?;
            }
            budget.checkpoint()?;
            let file = barrier_file.ok_or_else(|| {
                IndexError::BackendViolation("nonempty object batch has no barrier file".into())
            })?;
            self.sync_observed(SyncKind::Barrier, || barrier(file.file()?))?;
            if let ObjectIoFile::Writer(writer) = file
                && let Some(ticket) = writer.retire()?
            {
                ticket.synced()?;
            }
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _unused_barrier = barrier;
            self.sync_observed(SyncKind::Directory, || sync_directory(&self.objects))?;
            for shard in referenced_shards {
                self.sync_observed(SyncKind::Directory, || sync_directory(&shard))?;
            }
        }
        for ticket in pending_writes.into_iter().flatten() {
            ticket.synced()?;
        }
        budget.checkpoint()?;
        self.confirm_install(sink, scope, registrations, budget, true)
    }

    pub(crate) fn put_inner(
        &self,
        payload: &[u8],
        children: &[ObjectRef],
        sink: Option<&dyn ObjectWriteSink>,
        budget: &mut WorkBudget,
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
                )?;
            }
            return Ok((prepared.registration, false));
        }
        self.sync_observed(SyncKind::Directory, || sync_directory(&self.objects))?;
        let mut writer = self.create_writer(shard, &final_path, registration.bytes, budget)?;
        self.write_observed(writer.file_mut()?, wire)?;
        self.sync_observed(SyncKind::File, || {
            writer
                .file()?
                .sync_all()
                .map_err(|error| storage_error(&error))
        })?;
        let written = writer
            .file()?
            .metadata()
            .map_err(|error| storage_error(&error))?;
        validate_private_file(&written)?;
        match writer.publish(&final_path, &written) {
            Ok(true) => {
                self.sync_observed(SyncKind::Directory, || sync_directory(shard))?;
                validate_private_file(
                    &writer
                        .file()?
                        .metadata()
                        .map_err(|error| storage_error(&error))?,
                )?;
                if let Some(ticket) = writer.retire()? {
                    ticket.synced()?;
                }
                if let Some(sink) = sink {
                    self.confirm_install(
                        sink,
                        scope,
                        std::slice::from_ref(registration),
                        budget,
                        false,
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
        budget: &mut WorkBudget,
    ) -> Result<ObjectWriter, IndexError> {
        let Some(io) = &self.write_io else {
            let (file, path) = self.create_temp(shard)?;
            return Ok(ObjectWriter::new(file, path, None));
        };
        validate_private_directory(shard)?;
        let path = self.next_temp_path(shard)?;
        let mut ticket = io.begin(target, &path, max_bytes, budget)?;
        ticket.creation_started()?;
        let file = match OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(nofollow_flag()?)
            .open(&path)
        {
            Ok(file) => file,
            Err(error) => {
                ticket.uncreated()?;
                return Err(storage_error(&error));
            }
        };
        let mut writer = ObjectWriter::new(file, path, Some(ticket));
        writer.created()?;
        validate_private_file(
            &writer
                .file()?
                .metadata()
                .map_err(|error| storage_error(&error))?,
        )?;
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
        validate_private_directory(root)?;
        validate_private_directory(&self.objects)?;
        if self.authority.is_some() {
            let _binding = self.namespace.validate_objects(&self.objects)?;
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
        if stored.bytes != expected {
            return Err(IndexError::CorruptObject(
                "content-addressed path has different bytes".into(),
            ));
        }
        Ok(())
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
}

impl ImmutableObjectPort for FileObjectStore {
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
        let metadata_bytes = inputs
            .len()
            .checked_mul(
                std::mem::size_of::<PreparedObject>()
                    .checked_add(std::mem::size_of::<&PreparedObject>())
                    .ok_or(IndexError::ResourceLimited)?,
            )
            .ok_or(IndexError::ResourceLimited)?;
        let metadata_bytes =
            u64::try_from(metadata_bytes).map_err(|_error| IndexError::ResourceLimited)?;
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
        let wire = self.read_wire(reference, budget)?;
        decode_stored_wire(reference, &wire.bytes, budget)
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
    if wire.get(..8) != Some(MAGIC.as_slice()) {
        return Err(IndexError::CorruptObject("object magic mismatch".into()));
    }
    let payload_len = usize::try_from(read_u32(wire, 8)?)
        .map_err(|_conversion_error| IndexError::ResourceLimited)?;
    let children_len = usize::try_from(read_u32(wire, 12)?)
        .map_err(|_conversion_error| IndexError::ResourceLimited)?;
    if payload_len > MAX_PAYLOAD || children_len > MAX_CHILDREN {
        return Err(IndexError::CorruptObject(
            "encoded lengths exceed physical limits".into(),
        ));
    }
    let payload_at = HEADER_LEN
        .checked_add(
            children_len
                .checked_mul(32)
                .ok_or(IndexError::ResourceLimited)?,
        )
        .ok_or(IndexError::ResourceLimited)?;
    if payload_at.checked_add(payload_len) != Some(wire.len()) {
        return Err(IndexError::CorruptObject(
            "encoded lengths do not match object size".into(),
        ));
    }
    let decoded_bytes = children_len
        .checked_mul(std::mem::size_of::<ObjectRef>())
        .and_then(|children_bytes| children_bytes.checked_add(payload_len))
        .ok_or(IndexError::ResourceLimited)?;
    let decoded_bytes =
        u64::try_from(decoded_bytes).map_err(|_conversion_error| IndexError::ResourceLimited)?;
    budget.charge(WorkUsage {
        bytes: decoded_bytes,
        ..WorkUsage::default()
    })?;
    let memory = budget.reserve_memory(
        StoredObject::payload_allocation_bytes(payload_len)?,
        quanta_index_core::index::MemoryLifetime::Retained,
    )?;
    let registration = RegisteredObject::copy_from_iter(
        reference,
        u64::try_from(wire.len()).map_err(|_error| IndexError::ResourceLimited)?,
        children_len,
        (0..children_len).map(|index| {
            let start = index
                .checked_mul(32)
                .and_then(|offset| HEADER_LEN.checked_add(offset))
                .ok_or(IndexError::ResourceLimited)?;
            let end = start.checked_add(32).ok_or(IndexError::ResourceLimited)?;
            let bytes = wire
                .get(start..end)
                .ok_or_else(|| IndexError::CorruptObject("truncated child reference".into()))?;
            let digest: [u8; 32] = bytes.try_into().map_err(|_conversion_error| {
                IndexError::CorruptObject("invalid child reference".into())
            })?;
            Ok(ObjectRef(Digest(digest)))
        }),
        budget,
    )?;
    let payload_bytes = wire
        .get(payload_at..)
        .ok_or_else(|| IndexError::CorruptObject("truncated payload".into()))?;
    let mut payload = Vec::new();
    payload
        .try_reserve_exact(payload_len)
        .map_err(|_allocation_error| IndexError::ResourceLimited)?;
    payload.extend_from_slice(payload_bytes);
    Ok(StoredObject::new(registration, payload, memory))
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
    let mut hasher = Sha256::new();
    hasher.update(HASH_DOMAIN);
    hasher.update(wire);
    ObjectRef(Digest(hasher.finalize().into()))
}

pub(crate) fn storage_error(error: &std::io::Error) -> IndexError {
    IndexError::Storage(error.to_string())
}

fn validate_private_directory(path: &Path) -> Result<(), IndexError> {
    let metadata = fs::symlink_metadata(path).map_err(|error| storage_error(&error))?;
    validate_private_directory_metadata(path, &metadata)
}

fn validate_private_shard(path: &Path, missing: IndexError) -> Result<(), IndexError> {
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            missing
        } else {
            storage_error(&error)
        }
    })?;
    validate_private_directory_metadata(path, &metadata)
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
    match fs::DirBuilder::new().mode(0o700).create(path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(storage_error(&error)),
    }
    validate_private_directory(path)
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
    validate_private_directory(path)?;
    let file = File::open(path).map_err(|error| storage_error(&error))?;
    let metadata = file.metadata().map_err(|error| storage_error(&error))?;
    if !metadata.is_dir()
        || metadata.uid() != rustix::process::geteuid().as_raw()
        || metadata.mode() & 0o7777 != 0o700
    {
        return Err(IndexError::Storage(
            "storage directory changed during sync".into(),
        ));
    }
    file.sync_all().map_err(|error| storage_error(&error))
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

#[cfg(target_os = "macos")]
fn sync_directory_plain(path: &Path, expected_device: u64) -> Result<(), IndexError> {
    validate_private_directory(path)?;
    let file = File::open(path).map_err(|error| storage_error(&error))?;
    let metadata = file.metadata().map_err(|error| storage_error(&error))?;
    if !metadata.is_dir()
        || metadata.uid() != rustix::process::geteuid().as_raw()
        || metadata.mode() & 0o7777 != 0o700
    {
        return Err(IndexError::Storage(
            "storage directory changed during batch sync".into(),
        ));
    }
    ensure_same_device(expected_device, metadata.dev())?;
    rustix::fs::fsync(&file)
        .map_err(|error| IndexError::Storage(format!("sync immutable object directory: {error}")))
}

fn sync_parent_directory(path: &Path) -> Result<(), IndexError> {
    File::open(path)
        .and_then(|file| file.sync_all())
        .map_err(|error| storage_error(&error))
}

fn hex_digit(nibble: u8) -> char {
    if nibble < 10 {
        char::from(b'0'.wrapping_add(nibble))
    } else {
        char::from(b'a'.wrapping_add(nibble.wrapping_sub(10)))
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
