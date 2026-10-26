//! Immutable file outputs reuse the common extent authority and original FD pin.

use std::{
    ffi::OsString,
    fmt,
    fs::File,
    path::{Path, PathBuf},
    sync::Arc,
};

use quanta_index_contract::SearchPlaneErrorCodeV2;
use quanta_index_core::{
    CoreError,
    index::{
        IndexError, MemoryLease, MemoryLifetime, ObjectWriteIoPort, ObjectWriteTicket, WorkBudget,
        WorkUsage,
    },
};

use crate::publication_file_capacity::{FileMutation, PublicationFileCapacity};

#[derive(Clone)]
pub struct NativeObjectWriteIo {
    common: Arc<PublicationFileCapacity>,
}

impl fmt::Debug for NativeObjectWriteIo {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("NativeObjectWriteIo")
            .finish_non_exhaustive()
    }
}

impl NativeObjectWriteIo {
    #[must_use]
    pub fn new(common: Arc<PublicationFileCapacity>) -> Self {
        Self { common }
    }
}

struct WriteTicket {
    mutation: FileMutation,
    temporary: PathBuf,
    _memory: Option<MemoryLease>,
}

fn index_error(error: &CoreError) -> IndexError {
    if matches!(
        error,
        CoreError::Typed {
            code: SearchPlaneErrorCodeV2::IngestResourceBudgetExceeded,
            ..
        }
    ) {
        IndexError::ResourceLimited
    } else {
        IndexError::Storage(format!("native object write I/O: {error}"))
    }
}

fn index_error_at(error: &CoreError, budget: &WorkBudget, boundary: &'static str) -> IndexError {
    if let CoreError::Typed {
        code: SearchPlaneErrorCodeV2::IngestResourceBudgetExceeded,
        message,
    } = error
    {
        budget.record_external_resource_refusal(boundary, message);
    }
    index_error(error)
}

impl ObjectWriteIoPort for NativeObjectWriteIo {
    fn recover_retired_visible_create(
        &self,
        target: &Path,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        budget.checkpoint()?;
        if !self
            .common
            .has_retired_safe_visible_create_pending(target)
            .map_err(|error| index_error_at(&error, budget, "object_write.recovery_pending"))?
        {
            return Ok(());
        }
        // Only this original authority may turn a retired create into a
        // retained output. Admission precedes the FD open and physical sync.
        budget.charge(WorkUsage {
            reads: 1,
            writes: 1,
            mutations: 1,
            ..WorkUsage::default()
        })?;
        let fd = rustix::fs::openat(
            rustix::fs::CWD,
            target,
            rustix::fs::OFlags::RDONLY
                | rustix::fs::OFlags::NOFOLLOW
                | rustix::fs::OFlags::NONBLOCK
                | rustix::fs::OFlags::CLOEXEC,
            rustix::fs::Mode::empty(),
        )
        .map_err(|error| {
            IndexError::Storage(format!("open retired object for recovery: {error}"))
        })?;
        let file = File::from(fd);
        let _retained = self
            .common
            .recover_pending_with_fd(target, target, file)
            .map_err(|error| {
                index_error_at(&error, budget, "object_write.recover_pending_with_fd")
            })?;
        Ok(())
    }

    fn begin(
        &self,
        target: &Path,
        temporary: &Path,
        max_bytes: u64,
        budget: &mut WorkBudget,
    ) -> Result<Box<dyn ObjectWriteTicket>, IndexError> {
        if max_bytes == 0 {
            return Err(IndexError::InvalidInput("empty object write bound".into()));
        }
        let path_bytes = temporary.as_os_str().len();
        if path_bytes > 4096 || target.as_os_str().len() > 4096 {
            return Err(IndexError::ResourceLimited);
        }
        let owner_bytes = std::mem::size_of::<WriteTicket>()
            .checked_add(path_bytes)
            .ok_or(IndexError::ResourceLimited)?;
        let memory = budget.reserve_memory(
            u64::try_from(owner_bytes).map_err(|_error| IndexError::ResourceLimited)?,
            MemoryLifetime::Temporary,
        )?;
        let mut path = OsString::new();
        path.try_reserve_exact(path_bytes)
            .map_err(|_error| IndexError::ResourceLimited)?;
        path.push(temporary);
        // Immutable installation never authorizes replacing an existing inode.
        if self
            .common
            .path_observation(target)
            .map_err(|error| index_error_at(&error, budget, "object_write.path_observation"))?
            .is_some()
        {
            return Err(IndexError::FenceLost);
        }
        let mut mutation = self
            .common
            .begin_create(target)
            .map_err(|error| index_error_at(&error, budget, "object_write.begin_create"))?;
        mutation
            .reserve_end(max_bytes)
            .map_err(|error| index_error_at(&error, budget, "object_write.reserve_end"))?;
        mutation
            .declare_temp_path(temporary)
            .map_err(|error| index_error_at(&error, budget, "object_write.declare_temp_path"))?;
        Ok(Box::new(WriteTicket {
            mutation,
            temporary: PathBuf::from(path),
            _memory: memory,
        }))
    }
}

impl ObjectWriteTicket for WriteTicket {
    fn creation_started(&mut self) -> Result<(), IndexError> {
        self.mutation
            .creation_started()
            .map_err(|error| index_error(&error))
    }
    fn created(&mut self, writer: &File) -> Result<(), IndexError> {
        self.mutation
            .created(writer, &self.temporary)
            .map_err(|error| index_error(&error))
    }
    fn uncreated(&mut self) -> Result<(), IndexError> {
        self.mutation
            .confirmed_uncreated_absent_and_synced()
            .map_err(|error| index_error(&error))
    }
    fn retire_writer(&mut self, writer: File) -> Result<(), IndexError> {
        self.mutation
            .confirmed_safe_writer_fd_retired(writer)
            .map_err(|error| index_error(&error))
    }
    fn cancel(&mut self, writer: File) -> Result<(), IndexError> {
        self.mutation
            .cancel_declared_temp(writer)
            .map_err(|error| index_error(&error))
    }
    fn synced(mut self: Box<Self>) -> Result<(), IndexError> {
        let _retained = self
            .mutation
            .settle_after_external_file_and_parent_sync()
            .map_err(|error| index_error(&error))?;
        self.mutation
            .confirmed_safe_pin_retired()
            .map_err(|error| index_error(&error))
    }
}

#[cfg(test)]
#[path = "native_object_write_io/ann_enospc.rs"]
mod ann_enospc;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::publication_inventory::{PublicationBootInventoryLimits, seed_publication_capacity};
    use crate::{
        native_object_read_io::NativeObjectReadIo, native_object_reclaim_io::NativeObjectReclaimIo,
        native_scratch_io::NativeScratchIo,
    };
    use quanta_index_core::{
        RequestBudgetV1,
        index::{
            IngressRef, ObjectBatchInput, ObjectInstallScope, ObjectReadIoPort, RegisteredObject,
            WorkLimits, WorkspaceKey,
            ports::{CatalogReadPort, ImmutableObjectPort, InputStagingPort, ObjectWriteSink},
        },
        publication_capacity::PublicationCapacity,
    };
    use quanta_index_searchd::app::runtime::StateRootLease;
    use std::{
        fs::{self, OpenOptions},
        io::{Read as _, Write as _},
        os::unix::fs::{MetadataExt as _, OpenOptionsExt as _, PermissionsExt as _},
        sync::atomic::{AtomicBool, Ordering},
        time::{Duration, Instant},
    };

    type TestResult = Result<(), Box<dyn std::error::Error>>;
    type Fixture = (
        tempfile::TempDir,
        PathBuf,
        PublicationCapacity,
        Arc<PublicationFileCapacity>,
    );

    fn require(value: bool, message: &str) -> TestResult {
        if value {
            Ok(())
        } else {
            Err(std::io::Error::other(message).into())
        }
    }
    fn budget() -> Result<WorkBudget, IndexError> {
        Ok(WorkBudget::new(
            WorkLimits {
                reads: 1024,
                writes: 256,
                bytes: 67_108_864,
                mutations: 1024,
            },
            Instant::now()
                .checked_add(Duration::from_secs(60))
                .ok_or(IndexError::ResourceLimited)?,
        ))
    }
    fn fixture(max_bytes: u64) -> Result<Fixture, Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let lease = StateRootLease::acquire(&directory.path().join("state"))?;
        let root = lease.state_root_identity_v1().to_path_buf();
        // A distinct retained winner is available for the no-replace prefix law.
        fs::write(root.join("winner-source"), b"winner")?;
        fs::set_permissions(
            root.join("winner-source"),
            fs::Permissions::from_mode(0o600),
        )?;
        let capacity = seed_publication_capacity(
            &lease,
            &root,
            PublicationBootInventoryLimits {
                max_bytes,
                max_files: 64,
                max_pending: 64,
                max_entries: 256,
                max_depth: 8,
                max_pending_frames: 16,
            },
            &[],
            &RequestBudgetV1::unbounded(),
        )?;
        let common = PublicationFileCapacity::new(lease, capacity.clone(), 16, 256, 256)?;
        Ok((directory, root, capacity, common))
    }
    fn born(
        ticket: &mut dyn ObjectWriteTicket,
        temporary: &Path,
    ) -> Result<File, Box<dyn std::error::Error>> {
        ticket.creation_started()?;
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(temporary)?;
        ticket.created(&file)?;
        Ok(file)
    }

    #[derive(Debug)]
    struct FailOnceAfterRetirement {
        inner: NativeObjectWriteIo,
        armed: Arc<AtomicBool>,
    }

    struct FailOnceTicket {
        inner: Box<dyn ObjectWriteTicket>,
        armed: Arc<AtomicBool>,
    }

    impl ObjectWriteIoPort for FailOnceAfterRetirement {
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
            Ok(Box::new(FailOnceTicket {
                inner: self.inner.begin(target, temporary, max_bytes, budget)?,
                armed: Arc::clone(&self.armed),
            }))
        }
    }

    impl ObjectWriteTicket for FailOnceTicket {
        fn creation_started(&mut self) -> Result<(), IndexError> {
            self.inner.creation_started()
        }

        fn created(&mut self, writer: &File) -> Result<(), IndexError> {
            self.inner.created(writer)
        }

        fn uncreated(&mut self) -> Result<(), IndexError> {
            self.inner.uncreated()
        }

        fn retire_writer(&mut self, writer: File) -> Result<(), IndexError> {
            self.inner.retire_writer(writer)
        }

        fn cancel(&mut self, writer: File) -> Result<(), IndexError> {
            self.inner.cancel(writer)
        }

        fn synced(self: Box<Self>) -> Result<(), IndexError> {
            if self.armed.swap(false, Ordering::SeqCst) {
                return Err(IndexError::Storage(
                    "injected post-retirement failure".into(),
                ));
            }
            self.inner.synced()
        }
    }

    struct IngressScope(IngressRef);
    impl ObjectWriteSink for IngressScope {
        fn managed_scope(&self) -> Option<ObjectInstallScope> {
            Some(ObjectInstallScope::Ingress(self.0))
        }
        fn reserve(&self, _: &RegisteredObject) -> Result<(), IndexError> {
            Err(IndexError::BackendViolation(
                "callback is not Catalog authority".into(),
            ))
        }
        fn installed(&self, _: &RegisteredObject) -> Result<(), IndexError> {
            Err(IndexError::BackendViolation(
                "callback is not Catalog authority".into(),
            ))
        }
        fn reserve_batch(&self, _: &[RegisteredObject]) -> Result<(), IndexError> {
            Err(IndexError::BackendViolation(
                "callback is not Catalog authority".into(),
            ))
        }
        fn installed_batch(&self, _: &[RegisteredObject]) -> Result<(), IndexError> {
            Err(IndexError::BackendViolation(
                "callback is not Catalog authority".into(),
            ))
        }
    }

    fn regular_census(
        root: &Path,
        capacity: &PublicationCapacity,
    ) -> Result<(u64, u64, Vec<PathBuf>), Box<dyn std::error::Error>> {
        let mut directories = vec![root.to_path_buf()];
        let mut inodes = std::collections::BTreeSet::new();
        let mut untracked = Vec::new();
        let mut bytes = 0_u64;
        while let Some(directory) = directories.pop() {
            for entry in fs::read_dir(directory)? {
                let entry = entry?;
                let metadata = fs::symlink_metadata(entry.path())?;
                if metadata.is_dir() {
                    directories.push(entry.path());
                } else if metadata.is_file() && inodes.insert((metadata.dev(), metadata.ino())) {
                    bytes = bytes
                        .checked_add(metadata.len())
                        .ok_or("census byte overflow")?;
                    let inode = quanta_index_core::publication_capacity::InodeKey {
                        device: metadata.dev(),
                        inode: metadata.ino(),
                    };
                    if capacity.retained_inode(inode, metadata.len())?.is_none() {
                        untracked.push(entry.path().strip_prefix(root)?.to_path_buf());
                    }
                } else {
                    return Err("unexpected entry in private fixture".into());
                }
            }
        }
        Ok((bytes, u64::try_from(inodes.len())?, untracked))
    }

    #[test]
    fn admitted_managed_store_installs_scalar_batch_and_replay_with_real_catalog() -> TestResult {
        use quanta_index_storage::{FileObjectStore, FileScratchSpace, ManagedObjectIo};
        const FILE_LIMIT: u64 = 128;
        let directory = tempfile::tempdir()?;
        let lease = StateRootLease::acquire(&directory.path().join("state"))?;
        let root = lease.state_root_identity_v1().to_path_buf();
        let capacity = seed_publication_capacity(
            &lease,
            &root,
            PublicationBootInventoryLimits {
                max_bytes: 8 * 1024 * 1024,
                max_files: FILE_LIMIT,
                max_pending: 128,
                max_entries: 256,
                max_depth: 8,
                max_pending_frames: 16,
            },
            &[],
            &RequestBudgetV1::unbounded(),
        )?;
        let common = PublicationFileCapacity::new(lease, capacity.clone(), 32, 256, 256)?;
        let io = NativeScratchIo::register(
            Arc::clone(&common),
            std::ffi::CString::new("quanta-publication-capacity")?,
            512,
        )?;
        let scratch = Arc::new(FileScratchSpace::open_managed_with_io(
            root.join("scratch"),
            1024 * 1024,
            io.clone(),
        )?);
        let catalog = Arc::new(quanta_index_catalog::SqliteCatalog::open_with_io_admission(
            &root,
            Duration::from_secs(2),
            io.clone(),
        )?);
        let store_root = root.join("object-store");
        let lock_path = store_root.join(".object-namespace.lease");
        io.prepare_lock(&lock_path)?;
        let read = Arc::new(NativeObjectReadIo::new(Arc::clone(&common)));
        let mut work = budget()?;
        let foreign_lock = root.join("foreign-lock");
        io.prepare_lock(&foreign_lock)?;
        let refused = FileObjectStore::open_managed_with_io(
            &store_root,
            scratch.clone(),
            catalog.clone(),
            ManagedObjectIo {
                namespace_lock: read.open(&foreign_lock, &mut work)?,
                read: read.clone(),
                write: Arc::new(NativeObjectWriteIo::new(Arc::clone(&common))),
                reclaim: Arc::new(NativeObjectReclaimIo::new(Arc::clone(&common))),
            },
        );
        require(
            refused.is_err() && !store_root.join("objects").exists(),
            "mismatched namespace lock was adopted or selected raw fallback",
        )?;
        drop(refused);
        let fail_once = Arc::new(AtomicBool::new(false));
        let store = Arc::new(FileObjectStore::open_managed_with_io(
            &store_root,
            scratch.clone(),
            catalog.clone(),
            ManagedObjectIo {
                namespace_lock: read.open(&lock_path, &mut work)?,
                read: read.clone(),
                write: Arc::new(FailOnceAfterRetirement {
                    inner: NativeObjectWriteIo::new(Arc::clone(&common)),
                    armed: Arc::clone(&fail_once),
                }),
                reclaim: Arc::new(NativeObjectReclaimIo::new(Arc::clone(&common))),
            },
        )?);
        let workspace = WorkspaceKey {
            tenant: quanta_index_core::index::TenantId([41; 16]),
            collection: quanta_index_core::index::CollectionId([42; 16]),
            worktree: quanta_index_core::index::WorktreeId([43; 16]),
            slot: quanta_index_core::index::SlotId([44; 16]),
        };
        let kernel = quanta_index_kernel::Kernel::new(
            quanta_index_kernel::KernelServices::new(
                store.clone(),
                store.clone(),
                catalog.clone(),
                scratch.clone(),
                Duration::from_secs(30),
            )?,
            quanta_index_kernel::AdapterRegistry::new(),
        );
        let _genesis = kernel.create_workspace(workspace, 1, &mut work)?;
        let ingress = catalog.open_ingress(quanta_index_core::index::OperationKey {
            workspace,
            epoch: 1,
            producer: quanta_index_core::index::ProducerId([45; 16]),
            stream: quanta_index_core::index::StreamId([46; 16]),
            stream_epoch: 1,
            sequence: 1,
        })?;
        let sink = IngressScope(ingress);
        let expected_scalar = store.identify(b"scalar", &[], &mut work)?;
        let before_fault = capacity.snapshot()?;
        fail_once.store(true, Ordering::SeqCst);
        require(
            matches!(
                store.install(b"scalar", &[], &sink, &mut work),
                Err(IndexError::Storage(_))
            ),
            "injected post-retirement failure did not reach managed store",
        )?;
        let after_fault = capacity.snapshot()?;
        require(
            after_fault.active_tickets
                == before_fault
                    .active_tickets
                    .checked_add(1)
                    .ok_or("ticket overflow")?
                && after_fault.uncertain_pending
                    == before_fault
                        .uncertain_pending
                        .checked_add(1)
                        .ok_or("pending overflow")?,
            "failed managed install refunded retired visible create before replay",
        )?;
        require(
            catalog
                .adopt_registered_inputs(ingress, std::slice::from_ref(&expected_scalar), &mut work)
                .is_err(),
            "post-retirement fault exposed unconfirmed Catalog custody",
        )?;
        let scalar = store.install(b"scalar", &[], &sink, &mut work)?;
        require(
            scalar == expected_scalar,
            "managed replay changed canonical scalar registration",
        )?;
        catalog.adopt_registered_inputs(ingress, std::slice::from_ref(&scalar), &mut work)?;
        require(
            store.get(scalar.reference, &mut work)?.payload.as_slice() == b"scalar",
            "qualified scalar installation changed fixed payload",
        )?;
        let after_replay = capacity.snapshot()?;
        require(
            after_replay.active_tickets == before_fault.active_tickets
                && after_replay.uncertain_pending == before_fault.uncertain_pending,
            "managed replay did not settle exactly the retired visible create",
        )?;
        let child = store.identify(b"leaf", &[], &mut work)?.reference;
        let inputs = [
            ObjectBatchInput {
                payload: b"leaf".to_vec(),
                children: Vec::new(),
            },
            ObjectBatchInput {
                payload: b"leaf".to_vec(),
                children: Vec::new(),
            },
            ObjectBatchInput {
                payload: b"parent".to_vec(),
                children: vec![child],
            },
        ];
        let installed = store.install_batch(&inputs, &sink, &mut work)?;
        require(
            installed.len() == 3 && installed.first() == installed.get(1),
            "qualified batch lost input order or dedup identity",
        )?;
        let parent = installed.get(2).ok_or("parent registration missing")?;
        let loaded = store.get(parent.reference, &mut work)?;
        require(
            loaded.payload.as_slice() == b"parent"
                && loaded.registration.children.as_slice() == [child],
            "qualified batch changed fixed parent/children gold",
        )?;
        let before = regular_census(&root, &capacity)?;
        let replay = store.install_batch(&inputs, &sink, &mut work)?;
        require(
            replay == installed,
            "qualified replay changed registration identity",
        )?;
        let observed = regular_census(&root, &capacity)?;
        let charged = capacity.snapshot()?;
        if charged.used_bytes != observed.0
            || charged.used_files != observed.1
            || charged.active_tickets != 0
            || !observed.2.is_empty()
        {
            return Err(std::io::Error::other(format!(
                "qualified components disagree with regular-file census: charged={charged:?}, observed={observed:?}"
            )).into());
        }
        require(
            before.1 == observed.1,
            "replay added a physical object or temporary",
        )?;
        // Exhaust the same ledger's remaining inode slots. The existing
        // creation lock may be read; a new session file must be refused before
        // birth. Its burned SQL intent remains eligible for exact recovery.
        let all_files = capacity.reserve_publication(
            0,
            FILE_LIMIT
                .checked_sub(charged.used_files)
                .ok_or("file count exceeded configured ceiling")?,
        )?;
        let refused = catalog.open_read_session();
        require(
            matches!(refused, Err(IndexError::ResourceLimited)),
            "session lock creation bypassed the common file ceiling",
        )?;
        require(
            regular_census(&root, &capacity)?.1 == observed.1,
            "refused session creation left an unadmitted file",
        )?;
        drop(all_files);
        let before_recovery = capacity.snapshot()?;
        drop(kernel);
        let recovered = catalog.recover_read_sessions(None, 16, &mut work)?;
        require(
            recovered.recovered == 1 && recovered.open_intents_reaped == 1 && !recovered.has_more,
            "real session recovery did not drain dead and refused creation custody",
        )?;
        let after_recovery = capacity.snapshot()?;
        let recovered_census = regular_census(&root, &capacity)?;
        require(
            after_recovery.used_files.checked_add(1) == Some(before_recovery.used_files)
                && after_recovery.used_bytes == recovered_census.0
                && after_recovery.used_files == recovered_census.1
                && recovered_census.2.is_empty(),
            "session retirement failed exact physical refund or census parity",
        )?;
        let cloned = Arc::clone(&store);
        let before_unlink = capacity.snapshot()?;
        common.begin_delete(&lock_path)?.perform_and_sync()?;
        require(
            capacity.snapshot()? == before_unlink,
            "live namespace FD refunded before close",
        )?;
        drop(store);
        require(
            capacity.snapshot()? == before_unlink,
            "store clone lost namespace FD custody",
        )?;
        drop(cloned);
        let after = capacity.snapshot()?;
        require(
            after.used_files.checked_add(1) == Some(before_unlink.used_files)
                && after.used_bytes == before_unlink.used_bytes,
            "last store owner did not retire exactly its zero-byte namespace inode",
        )?;
        drop(catalog);
        drop(scratch);
        drop(io);
        require(
            common.is_quiescent()?,
            "actual component close retained a handle or mutation",
        )?;
        Ok(())
    }
    #[test]
    fn kernel_short_write_keeps_full_charge_until_cancel_closes_and_syncs() -> TestResult {
        const BOUND: u64 = 8192;
        const CHILD_FLAG: &str = "QUANTA_INDEX_NATIVE_SHORT_WRITE_CHILD";
        const TEST_NAME: &str = "native_object_write_io::tests::kernel_short_write_keeps_full_charge_until_cancel_closes_and_syncs";
        if std::env::var_os(CHILD_FLAG).as_deref() != Some(std::ffi::OsStr::new("1")) {
            // The inherited file-size limit and ignored SIGXFSZ apply only to
            // this child. Passing argv separately avoids shell path quoting.
            let output = std::process::Command::new("/bin/sh")
                .args([
                    "-c",
                    "trap '' XFSZ || exit 96; ulimit -f 1 || exit 97; exec \"$@\"",
                    "native-short-write",
                ])
                .arg(std::env::current_exe()?)
                .args(["--exact", TEST_NAME, "--test-threads=1"])
                .env(CHILD_FLAG, "1")
                .output()?;
            if !output.status.success() {
                return Err(format!(
                    "kernel short-write child failed: {}; stdout={}; stderr={}",
                    output.status,
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr),
                )
                .into());
            }
            let stdout = String::from_utf8_lossy(&output.stdout);
            require(
                stdout.contains("running 1 test")
                    && stdout.contains("test result: ok. 1 passed; 0 failed;"),
                "short-write child exited without executing its exact test",
            )?;
            return Ok(());
        }

        let (_directory, root, capacity, common) = fixture(32 * 1024)?;
        let baseline = capacity.snapshot()?;
        let target = root.join("short-object");
        let temporary = root.join(".tmp-short-object");
        let io = NativeObjectWriteIo::new(common);
        let mut ticket = io.begin(&target, &temporary, BOUND, &mut budget()?)?;
        let mut file = born(ticket.as_mut(), &temporary)?;
        let payload = [0x5a; 8192];
        let written = file.write(&payload)?;
        require(
            written > 0 && written < payload.len(),
            "kernel did not return a nonzero short write",
        )?;
        let error = file
            .write(&[0x5a])
            .err()
            .ok_or("file-size limit did not reject the next write")?;
        require(
            error.raw_os_error() == Some(rustix::io::Errno::FBIG.raw_os_error()),
            "short-write failure was not the kernel file-size limit",
        )?;
        let prefix = payload
            .get(..written)
            .ok_or("kernel short-write count exceeds the input buffer")?;
        require(
            fs::read(&temporary)?.as_slice() == prefix && !target.exists(),
            "partial bytes changed or became a published target",
        )?;
        let pending = capacity.snapshot()?;
        require(
            pending.used_bytes
                == baseline
                    .used_bytes
                    .checked_add(BOUND)
                    .ok_or("byte overflow")?
                && pending.used_files
                    == baseline.used_files.checked_add(1).ok_or("file overflow")?
                && pending.active_tickets
                    == baseline
                        .active_tickets
                        .checked_add(1)
                        .ok_or("ticket overflow")?
                && pending.uncertain_pending
                    == baseline
                        .uncertain_pending
                        .checked_add(1)
                        .ok_or("pending overflow")?,
            "kernel short write refunded the unconfirmed output bound",
        )?;
        // The actual original FD is consumed; unlink, parent sync and final
        // close precede refund through the existing cancellation authority.
        ticket.cancel(file)?;
        drop(ticket);
        require(
            !temporary.exists() && !target.exists() && capacity.snapshot()? == baseline,
            "actual short-write cancellation leaked bytes or refunded early",
        )?;
        Ok(())
    }

    #[test]
    fn actual_write_settles_after_store_sync_and_releases_original_and_pin() -> TestResult {
        let (_directory, root, capacity, common) = fixture(128)?;
        let baseline = capacity.snapshot()?;
        let target = root.join("object");
        let temporary = root.join(".tmp-object");
        let io = NativeObjectWriteIo::new(Arc::clone(&common));
        let mut ticket = io.begin(&target, &temporary, 9, &mut budget()?)?;
        let mut file = born(ticket.as_mut(), &temporary)?;
        file.write_all(b"new-bytes")?;
        file.sync_all()?;
        fs::rename(&temporary, &target)?;
        File::open(&root)?.sync_all()?;
        ticket.retire_writer(file)?;
        ticket.synced()?;
        require(
            fs::read(&target)? == b"new-bytes",
            "settlement changed fixed bytes",
        )?;
        let after = capacity.snapshot()?;
        require(
            after.used_bytes == baseline.used_bytes.checked_add(9).ok_or("byte overflow")?
                && after.used_files == baseline.used_files.checked_add(1).ok_or("file overflow")?
                && after.active_tickets == 0,
            "settlement did not charge exactly one output",
        )?;
        common.begin_delete(&target)?.perform_and_sync()?;
        require(
            capacity.snapshot()?.used_bytes == baseline.used_bytes,
            "original/pin close left an artificial reader",
        )?;
        Ok(())
    }

    #[test]
    fn retired_visible_create_recovery_releases_reader_and_settles_once() -> TestResult {
        let (_directory, root, capacity, common) = fixture(128)?;
        let baseline = capacity.snapshot()?;
        let target = root.join("orphaned-object");
        let temporary = root.join(".tmp-orphaned-object");
        let io = NativeObjectWriteIo::new(Arc::clone(&common));
        let read = NativeObjectReadIo::new(Arc::clone(&common));
        let mut ticket = io.begin(&target, &temporary, 9, &mut budget()?)?;
        let mut file = born(ticket.as_mut(), &temporary)?;
        file.write_all(b"new-bytes")?;
        file.sync_all()?;
        fs::rename(&temporary, &target)?;
        File::open(&root)?.sync_all()?;

        // The original writer remains live; discovery cannot settle it.
        let mut recovery_budget = budget()?;
        let before_recovery_usage = recovery_budget.usage();
        io.recover_retired_visible_create(&target, &mut recovery_budget)?;
        require(
            matches!(
                read.open(&target, &mut budget()?),
                Err(IndexError::ResourceLimited)
            ) && recovery_budget.usage() == before_recovery_usage,
            "live writer granted reader admission",
        )?;
        ticket.retire_writer(file)?;
        drop(ticket);
        let pending = capacity.snapshot()?;
        require(
            pending.active_tickets
                == baseline
                    .active_tickets
                    .checked_add(1)
                    .ok_or("ticket overflow")?
                && pending.uncertain_pending
                    == baseline
                        .uncertain_pending
                        .checked_add(1)
                        .ok_or("pending overflow")?
                && pending.used_bytes
                    == baseline.used_bytes.checked_add(9).ok_or("byte overflow")?
                && pending.used_files
                    == baseline.used_files.checked_add(1).ok_or("file overflow")?,
            "retired visible create lost its pending capacity charge",
        )?;
        require(
            matches!(
                read.open(&target, &mut budget()?),
                Err(IndexError::ResourceLimited)
            ),
            "retired create granted reader admission before reconciliation",
        )?;
        io.recover_retired_visible_create(&target, &mut recovery_budget)?;
        require(
            recovery_budget.usage()
                == WorkUsage {
                    reads: 1,
                    writes: 1,
                    mutations: 1,
                    ..before_recovery_usage
                },
            "retired create recovery did not charge its bounded physical work",
        )?;
        let settled = capacity.snapshot()?;
        require(
            settled.active_tickets == baseline.active_tickets
                && settled.uncertain_pending == baseline.uncertain_pending
                && settled.used_bytes == pending.used_bytes
                && settled.used_files == pending.used_files,
            "recovery failed exact one-time physical settlement",
        )?;
        let opened = read.open(&target, &mut budget()?)?;
        let mut bytes = Vec::new();
        let mut borrowed = opened.file()?;
        let read_bytes = borrowed.read_to_end(&mut bytes)?;
        require(
            read_bytes == 9 && bytes == b"new-bytes",
            "recovered reader changed object bytes",
        )?;
        drop(opened);
        let charged = recovery_budget.usage();
        io.recover_retired_visible_create(&target, &mut recovery_budget)?;
        require(
            capacity.snapshot()? == settled && recovery_budget.usage() == charged,
            "settled replay charged twice",
        )?;
        Ok(())
    }

    #[test]
    fn retired_visible_create_rejects_changed_inode_without_refund() -> TestResult {
        let (_directory, root, capacity, common) = fixture(128)?;
        let target = root.join("orphaned-object");
        let temporary = root.join(".tmp-orphaned-object");
        let saved = root.join("original-inode");
        let io = NativeObjectWriteIo::new(Arc::clone(&common));
        let mut ticket = io.begin(&target, &temporary, 9, &mut budget()?)?;
        let mut file = born(ticket.as_mut(), &temporary)?;
        file.write_all(b"new-bytes")?;
        fs::rename(&temporary, &target)?;
        ticket.retire_writer(file)?;
        drop(ticket);
        let pending = capacity.snapshot()?;
        fs::rename(&target, &saved)?;
        fs::write(&target, b"different")?;
        require(
            io.recover_retired_visible_create(&target, &mut budget()?)
                .is_err()
                && capacity.snapshot()? == pending,
            "replacement inode settled the original pending charge",
        )?;
        let read = NativeObjectReadIo::new(Arc::clone(&common));
        require(
            read.open(&target, &mut budget()?).is_err() && capacity.snapshot()? == pending,
            "quarantined replacement granted reader admission or refunded capacity",
        )?;
        Ok(())
    }

    #[test]
    fn retired_visible_create_rejects_symlink_without_refund() -> TestResult {
        let (_directory, root, capacity, common) = fixture(128)?;
        let target = root.join("orphaned-object");
        let temporary = root.join(".tmp-orphaned-object");
        let saved = root.join("original-inode");
        let io = NativeObjectWriteIo::new(Arc::clone(&common));
        let mut ticket = io.begin(&target, &temporary, 9, &mut budget()?)?;
        let mut file = born(ticket.as_mut(), &temporary)?;
        file.write_all(b"new-bytes")?;
        fs::rename(&temporary, &target)?;
        ticket.retire_writer(file)?;
        drop(ticket);
        let pending = capacity.snapshot()?;
        fs::rename(&target, &saved)?;
        std::os::unix::fs::symlink(&saved, &target)?;
        require(
            io.recover_retired_visible_create(&target, &mut budget()?)
                .is_err()
                && capacity.snapshot()? == pending,
            "symlink settled the original pending charge",
        )?;
        let read = NativeObjectReadIo::new(Arc::clone(&common));
        require(
            read.open(&target, &mut budget()?).is_err() && capacity.snapshot()? == pending,
            "symlink granted reader admission or refunded capacity",
        )?;
        Ok(())
    }
    #[test]
    fn full_bound_refuses_before_create_and_cancel_refunds_after_actual_close() -> TestResult {
        let (_directory, root, capacity, common) = fixture(16)?;
        let baseline = capacity.snapshot()?;
        let target = root.join("object");
        let temporary = root.join(".tmp-object");
        let io = NativeObjectWriteIo::new(common);
        let mut refused_budget = budget()?;
        let before = refused_budget.usage();
        require(
            matches!(
                io.begin(&target, &temporary, 11, &mut refused_budget),
                Err(IndexError::ResourceLimited)
            ),
            "full bound bypassed admission",
        )?;
        let diagnostic = format!("{refused_budget:?}");
        require(
            refused_budget.first_denial().is_none()
                && refused_budget.usage() == before
                && diagnostic.contains("object_write.reserve_end")
                && diagnostic.contains("configured byte or unique-file ceiling exceeded"),
            "writer capacity refusal lost its original external boundary or changed work budget",
        )?;
        require(
            !target.exists() && !temporary.exists() && capacity.snapshot()? == baseline,
            "refused output created bytes or retained reservation",
        )?;
        let mut ticket = io.begin(&target, &temporary, 9, &mut budget()?)?;
        let mut file = born(ticket.as_mut(), &temporary)?;
        file.write_all(b"new-bytes")?;
        ticket.cancel(file)?;
        drop(ticket);
        require(
            !temporary.exists() && capacity.snapshot()? == baseline,
            "cancel refunded without exact close/delete or retained an output",
        )?;
        Ok(())
    }
    #[test]
    fn changed_parent_refuses_cancel_before_unlink_and_keeps_pending_charge() -> TestResult {
        let (_directory, root, capacity, common) = fixture(128)?;
        let baseline = capacity.snapshot()?;
        let parent = root.join("shard");
        fs::create_dir(&parent)?;
        fs::set_permissions(&parent, fs::Permissions::from_mode(0o700))?;
        let target = parent.join("object");
        let temporary = parent.join(".tmp-object");
        let io = NativeObjectWriteIo::new(common);
        let mut ticket = io.begin(&target, &temporary, 9, &mut budget()?)?;
        let mut file = born(ticket.as_mut(), &temporary)?;
        file.write_all(b"new-bytes")?;
        let detached = root.join("old-shard");
        fs::rename(&parent, &detached)?;
        fs::create_dir(&parent)?;
        fs::set_permissions(&parent, fs::Permissions::from_mode(0o700))?;
        require(
            ticket.cancel(file).is_err(),
            "changed parent accepted cancellation",
        )?;
        drop(ticket);
        require(
            fs::read(detached.join(".tmp-object"))? == b"new-bytes"
                && !target.exists()
                && !temporary.exists(),
            "refused cancellation changed either namespace",
        )?;
        let after = capacity.snapshot()?;
        require(
            after.used_bytes == baseline.used_bytes.checked_add(9).ok_or("byte overflow")?
                && after.used_files == baseline.used_files.checked_add(1).ok_or("file overflow")?,
            "unresolved temporary received a false capacity refund",
        )?;
        Ok(())
    }
    #[test]
    fn no_replace_loser_cancels_only_its_temp_and_keeps_charged_winner() -> TestResult {
        let (_directory, root, capacity, common) = fixture(128)?;
        let baseline = capacity.snapshot()?;
        let target = root.join("object");
        let temporary = root.join(".tmp-object");
        let io = NativeObjectWriteIo::new(common);
        let mut ticket = io.begin(&target, &temporary, 9, &mut budget()?)?;
        let mut file = born(ticket.as_mut(), &temporary)?;
        file.write_all(b"new-bytes")?;
        // Actual namespace prefix of an already charged competing winner.
        // This is not a failing rename/fsync syscall injector.
        fs::rename(root.join("winner-source"), &target)?;
        File::open(&root)?.sync_all()?;
        ticket.cancel(file)?;
        drop(ticket);
        require(
            fs::read(&target)? == b"winner"
                && !temporary.exists()
                && capacity.snapshot()? == baseline,
            "loser removed/refunded its winner or leaked its own reservation",
        )?;
        Ok(())
    }
}
