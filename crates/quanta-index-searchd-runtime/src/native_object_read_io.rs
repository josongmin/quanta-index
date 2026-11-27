//! File-store reads retain the actual FD under the common physical authority.

use std::{fmt, fs::File, path::Path, sync::Arc};

use crate::owned_fd_close::close_owned_file;
use crate::publication_file_capacity::{InodeObservation, NativeReaderLease, PublicationFileCapacity};
use quanta_index_contract::SearchPlaneErrorCodeV2;
use quanta_index_core::{
    CoreError,
    index::{
        IndexError, MemoryLease, MemoryLifetime, ObjectReadFile, ObjectReadIoPort, WorkBudget,
    },
};

#[derive(Clone)]
pub struct NativeObjectReadIo {
    common: Arc<PublicationFileCapacity>,
}

impl fmt::Debug for NativeObjectReadIo {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("NativeObjectReadIo")
            .finish_non_exhaustive()
    }
}

impl NativeObjectReadIo {
    #[must_use]
    pub fn new(common: Arc<PublicationFileCapacity>) -> Self {
        Self { common }
    }

    fn open_retained(
        &self,
        path: &Path,
        expected: Option<InodeObservation>,
        budget: &mut WorkBudget,
    ) -> Result<Box<dyn ObjectReadFile>, IndexError> {
        let memory = budget.reserve_memory(read_file_memory_bytes()?, MemoryLifetime::Temporary)?;
        let mut opening = self
            .common
            .begin_native_reader_open_with_expected(path, expected)
            .map_err(|error| {
                index_error_at(&error, budget, "object_read.begin_native_reader_open")
            })?;
        let (file, reader) = opening
            .open_file()
            .map_err(|error| index_error_at(&error, budget, "object_read.opened_witness"))?;
        Ok(Box::new(ReadFile {
            file: Some(file),
            reader: Some(reader),
            common: Arc::clone(&self.common),
            _memory: memory,
        }))
    }
}

struct ReadFile {
    file: Option<File>,
    reader: Option<NativeReaderLease>,
    common: Arc<PublicationFileCapacity>,
    _memory: Option<MemoryLease>,
}

impl ObjectReadFile for ReadFile {
    fn file(&self) -> Result<&File, IndexError> {
        self.file
            .as_ref()
            .ok_or_else(|| IndexError::BackendViolation("object reader FD retired".into()))
    }
}

impl Drop for ReadFile {
    fn drop(&mut self) {
        if let Some(file) = self.file.take() {
            if close_owned_file(file).is_err() {
                self.common.mark_close_unwitnessed();
                return;
            }
            if let Some(reader) = self.reader.take()
                && reader.retire_and_sweep().is_err()
            {
                self.common.mark_close_unwitnessed();
            }
        }
    }
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
    } else if reader_path_corruption(error) {
        IndexError::CorruptObject(format!(
            "object path is not a retained regular file: {error}"
        ))
    } else {
        IndexError::Storage(format!("native object read I/O: {error}"))
    }
}

fn reader_path_corruption(error: &CoreError) -> bool {
    match error {
        CoreError::NotFound(_) => true,
        CoreError::StorageIo {
            raw_os_error: Some(errno),
            ..
        } => [
            rustix::io::Errno::NOENT,
            rustix::io::Errno::NOTDIR,
            rustix::io::Errno::LOOP,
        ]
        .iter()
        .any(|expected| *errno == expected.raw_os_error()),
        CoreError::InvalidContract(message) => [
            "anchored entry is not a regular file",
            "owned FD is not a regular file",
            "native reader pathname absent",
            "native reader inode absent from sealed authority",
            "native pin witness and anchored path differ",
            "native pin inode absent from sealed authority",
            "native open inode/width changed",
        ]
        .iter()
        .any(|reason| message.contains(reason)),
        CoreError::NotReady(_)
        | CoreError::NotImplemented(_)
        | CoreError::Storage(_)
        | CoreError::StorageIo { .. }
        | CoreError::Typed { .. } => false,
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

fn read_file_memory_bytes() -> Result<u64, IndexError> {
    u64::try_from(std::mem::size_of::<ReadFile>()).map_err(|_error| IndexError::ResourceLimited)
}

impl ObjectReadIoPort for NativeObjectReadIo {
    fn read_owner_memory_bound(
        &self,
    ) -> Result<Option<quanta_index_core::index::MemoryCreditLimits>, IndexError> {
        Ok(Some(quanta_index_core::index::MemoryCreditLimits {
            temporary_data_bytes: read_file_memory_bytes()?,
            retained_data_bytes: 0,
            max_live_temporary_leases: 1,
            max_live_retained_leases: 0,
        }))
    }

    fn open(
        &self,
        path: &Path,
        budget: &mut WorkBudget,
    ) -> Result<Box<dyn ObjectReadFile>, IndexError> {
        self.open_retained(path, None, budget)
    }

    fn open_after_path_metadata(
        &self,
        path: &Path,
        expected: &std::fs::Metadata,
        budget: &mut WorkBudget,
    ) -> Result<Box<dyn ObjectReadFile>, IndexError> {
        let expected = InodeObservation::from_metadata(expected).map_err(|error| index_error(&error))?;
        self.open_retained(path, Some(expected), budget)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::publication_inventory::{PublicationBootInventoryLimits, seed_publication_capacity};
    use quanta_index_core::{RequestBudgetV1, publication_capacity::PublicationCapacity};
    use quanta_index_searchd::app::runtime::StateRootLease;
    use std::{
        fs::{self, OpenOptions},
        io::Read as _,
        os::unix::fs::PermissionsExt as _,
        path::PathBuf,
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
        use quanta_index_core::index::WorkLimits;
        use std::time::{Duration, Instant};
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

    fn fixture() -> Result<Fixture, Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let lease = StateRootLease::acquire(&directory.path().join("state"))?;
        let root = lease.state_root_identity_v1().to_path_buf();
        let path = root.join("read-object");
        fs::write(&path, b"physical-bytes")?;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
        let limits = PublicationBootInventoryLimits {
            max_bytes: 1024 * 1024,
            max_files: 32,
            max_pending: 32,
            max_entries: 64,
            max_depth: 4,
            max_pending_frames: 8,
        };
        let capacity =
            seed_publication_capacity(&lease, &root, limits, &[], &RequestBudgetV1::unbounded())?;
        let common = PublicationFileCapacity::new(lease, capacity.clone(), 1, 256, 256)?;
        Ok((directory, path, capacity, common))
    }

    #[test]
    fn managed_store_fixed_get_releases_the_actual_read_owner() -> TestResult {
        use quanta_index_core::index::{WorkBudget, WorkLimits, ports::ImmutableObjectPort as _};
        use quanta_index_storage::{FileObjectStore, FileScratchSpace};
        use std::{
            io::Write as _,
            time::{Duration, Instant},
        };

        let directory = tempfile::tempdir()?;
        let lease = StateRootLease::acquire(&directory.path().join("state"))?;
        let root = lease.state_root_identity_v1().to_path_buf();
        let mut budget = WorkBudget::new(
            WorkLimits {
                reads: 1024,
                writes: 256,
                bytes: 67_108_864,
                mutations: 1024,
            },
            Instant::now()
                .checked_add(Duration::from_secs(60))
                .ok_or(IndexError::ResourceLimited)?,
        );
        let store_root = root.join("objects-store");
        let seed = FileObjectStore::open(&store_root)?;
        let registration = seed.put(b"fixed-reader-gold", &[], &mut budget)?;
        drop(seed);
        let scratch = Arc::new(FileScratchSpace::open_managed(
            root.join("managed-scratch"),
            1024 * 1024,
        )?);
        let catalog = Arc::new(quanta_index_catalog::SqliteCatalog::open(
            &root,
            Duration::from_secs(2),
        )?);
        let store = FileObjectStore::open_managed_with_scratch(&store_root, scratch, catalog)?;
        // Fixture setup uses unqualified writers before inventory. This law
        // qualifies actual managed reads, not the service's aggregate writer cap.
        let capacity = seed_publication_capacity(
            &lease,
            &root,
            PublicationBootInventoryLimits {
                max_bytes: 4 * 1024 * 1024,
                max_files: 64,
                max_pending: 64,
                max_entries: 256,
                max_depth: 8,
                max_pending_frames: 16,
            },
            &[],
            &RequestBudgetV1::unbounded(),
        )?;
        let baseline = capacity.snapshot()?;
        let common = PublicationFileCapacity::new(lease, capacity.clone(), 1, 256, 256)?;
        let store = store.with_read_io(Arc::new(NativeObjectReadIo::new(common)))?;
        let first = store.get(registration.reference, &mut budget)?;
        let second = store.get(registration.reference, &mut budget)?;
        require(
            first.payload.as_slice() == b"fixed-reader-gold"
                && second.payload.as_slice() == b"fixed-reader-gold",
            "managed read or one-handle reopen changed fixed payload",
        )?;
        let shard = fs::read_dir(store_root.join("objects"))?
            .next()
            .transpose()?
            .ok_or("fixture object shard absent")?;
        let object = fs::read_dir(shard.path())?
            .next()
            .transpose()?
            .ok_or("fixture object absent")?;
        let mut corrupt = OpenOptions::new().write(true).open(object.path())?;
        corrupt.write_all(b"BADMAGIC")?;
        corrupt.sync_all()?;
        drop(corrupt);
        for _ in 0..2 {
            require(
                matches!(
                    store.get(registration.reference, &mut budget),
                    Err(IndexError::CorruptObject(_))
                ),
                "corrupt read bypassed integrity or leaked its one admitted handle",
            )?;
        }
        let after = capacity.snapshot()?;
        require(
            after.used_bytes == baseline.used_bytes && after.used_files == baseline.used_files,
            "managed read refunded a linked inode or retained a write charge",
        )?;
        Ok(())
    }

    #[test]
    fn actual_reader_keeps_deleted_extent_until_actual_close() -> TestResult {
        let (_directory, path, capacity, common) = fixture()?;
        let baseline = capacity.snapshot()?;
        let io = NativeObjectReadIo::new(Arc::clone(&common));
        let owner = io.open(&path, &mut budget()?)?;
        common.begin_delete(&path)?.perform_and_sync()?;
        require(!path.exists(), "delete did not unlink the object")?;
        require(
            capacity.snapshot()?.used_bytes == baseline.used_bytes,
            "deleted-but-open reader released its extent early",
        )?;
        let mut bytes = [0; 14];
        owner.file()?.read_exact(&mut bytes)?;
        require(
            bytes == *b"physical-bytes",
            "held FD lost fixed object bytes",
        )?;
        drop(owner);
        let after = capacity.snapshot()?;
        require(
            after.used_bytes.checked_add(14) == Some(baseline.used_bytes)
                && after.used_files.checked_add(1) == Some(baseline.used_files),
            "actual final close did not refund exactly one object",
        )?;
        common.sweep_retired()?;
        require(
            capacity.snapshot()?.used_bytes == after.used_bytes,
            "reader cleanup refunded twice",
        )?;
        Ok(())
    }

    #[test]
    fn native_opening_reservation_transfers_to_one_pin() -> TestResult {
        let (_directory, path, _capacity, common) = fixture()?;
        let mut opening = common.begin_native_reader_open(&path)?;
        let (file, reader) = opening.open_file()?;
        let observed = InodeObservation::from_file(&file)?;
        require(
            matches!(
                opening.opened_witness(observed),
                Err(CoreError::InvalidContract(message))
                    if message == "publication file capacity: native reader opening already finished"
            ),
            "completed opening admitted a second pin or damaged the opening ledger",
        )?;
        let io = NativeObjectReadIo::new(Arc::clone(&common));
        require(
            matches!(
                io.open(&path, &mut budget()?),
                Err(IndexError::ResourceLimited)
            ),
            "opening-to-pin transfer lost the single admitted handle",
        )?;
        close_owned_file(file)?;
        reader.retire_and_sweep()?;
        drop(io.open(&path, &mut budget()?)?);
        Ok(())
    }

    #[test]
    fn native_object_open_rejects_stale_path_metadata_and_releases_the_slot() -> TestResult {
        let (_directory, path, _capacity, common) = fixture()?;
        let io = NativeObjectReadIo::new(common);
        let retained = path.with_extension("retained");
        let replaced = path.with_extension("replaced");
        fs::rename(&path, &retained)?;
        fs::write(&path, b"replaced-bytes")?;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
        let stale = fs::symlink_metadata(&path)?;
        fs::rename(&path, &replaced)?;
        fs::rename(&retained, &path)?;

        require(
            matches!(
                io.open_after_path_metadata(&path, &stale, &mut budget()?),
                Err(IndexError::CorruptObject(_))
            ),
            "stale pathname metadata admitted a different retained inode",
        )?;
        let current = fs::symlink_metadata(&path)?;
        let owner = io.open_after_path_metadata(&path, &current, &mut budget()?)?;
        let mut bytes = [0; 14];
        owner.file()?.read_exact(&mut bytes)?;
        require(
            bytes == *b"physical-bytes",
            "the same reader slot failed to reopen the retained inode",
        )
    }

    #[test]
    fn owned_reader_opening_and_pin_share_the_native_ceiling() -> TestResult {
        let (_directory, path, _capacity, common) = fixture()?;
        let io = NativeObjectReadIo::new(Arc::clone(&common));
        let opening = common.begin_reader_open(&path)?;
        require(
            matches!(
                io.open(&path, &mut budget()?),
                Err(IndexError::ResourceLimited)
            ),
            "owned opening failed to reserve the only reader slot",
        )?;
        let reader = opening.opened_owned(File::open(&path)?)?;
        require(
            matches!(
                io.open(&path, &mut budget()?),
                Err(IndexError::ResourceLimited)
            ),
            "owned opening-to-pin transfer released the occupied slot",
        )?;
        reader.retire()?;
        drop(io.open(&path, &mut budget()?)?);
        Ok(())
    }

    #[test]
    fn native_opening_rejects_leaf_replaced_by_fifo() -> TestResult {
        let (_directory, path, _capacity, common) = fixture()?;
        let io = NativeObjectReadIo::new(Arc::clone(&common));
        let initial = io.open(&path, &mut budget()?)?;
        let flags = rustix::fs::fcntl_getfl(initial.file()?)?;
        drop(initial);
        require(
            flags.contains(rustix::fs::OFlags::NONBLOCK),
            "object reader can block on a substituted FIFO",
        )?;

        let mut opening = common.begin_native_reader_open(&path)?;
        let original = path.with_extension("original");
        fs::rename(&path, &original)?;
        require(
            std::process::Command::new("mkfifo")
                .arg(&path)
                .status()?
                .success(),
            "mkfifo fixture setup failed",
        )?;
        match opening.open_file() {
            Err(error) if matches!(index_error(&error), IndexError::CorruptObject(_)) => {}
            Err(error) => return Err(error.into()),
            Ok((file, reader)) => {
                close_owned_file(file)?;
                reader.retire_and_sweep()?;
                return Err(
                    std::io::Error::other("object reader admitted a substituted FIFO").into(),
                );
            }
        }
        drop(opening);
        fs::remove_file(&path)?;
        fs::rename(original, &path)?;
        drop(io.open(&path, &mut budget()?)?);
        Ok(())
    }

    #[test]
    fn native_opening_rechecks_root_lock_before_pin() -> TestResult {
        use quanta_index_searchd::app::state_format::STATE_ROOT_LEASE_FILE_NAME;

        let (_directory, path, _capacity, common) = fixture()?;
        let root = path
            .parent()
            .ok_or_else(|| std::io::Error::other("fixture requires a state root"))?;
        let lock = root.join(STATE_ROOT_LEASE_FILE_NAME);
        let mut opening = common.begin_native_reader_open(&path)?;
        fs::set_permissions(&lock, fs::Permissions::from_mode(0o640))?;
        let outcome = opening.open_file();
        fs::set_permissions(&lock, fs::Permissions::from_mode(0o600))?;
        match outcome {
            Err(CoreError::InvalidContract(message))
                if message.contains("held lock entry identity or security changed") => {}
            Err(error) => return Err(error.into()),
            Ok((file, reader)) => {
                close_owned_file(file)?;
                reader.retire_and_sweep()?;
                return Err(
                    std::io::Error::other("object reader pinned under an unsafe lock").into(),
                );
            }
        }
        drop(opening);
        let io = NativeObjectReadIo::new(common);
        drop(io.open(&path, &mut budget()?)?);
        Ok(())
    }

    #[test]
    fn reader_ceiling_refuses_then_recovers_after_confirmed_close() -> TestResult {
        let (_directory, path, capacity, common) = fixture()?;
        let baseline = capacity.snapshot()?;
        let io = NativeObjectReadIo::new(Arc::clone(&common));
        let owner = io.open(&path, &mut budget()?)?;
        let mut refused_budget = budget()?;
        let before = refused_budget.usage();
        require(
            matches!(
                io.open(&path, &mut refused_budget),
                Err(IndexError::ResourceLimited)
            ),
            "second reader bypassed pre-open admission",
        )?;
        let diagnostic = format!("{refused_budget:?}");
        require(
            refused_budget.first_denial().is_none()
                && refused_budget.usage() == before
                && diagnostic.contains("object_read.begin_native_reader_open")
                && diagnostic.contains("native reader-open registry busy or full"),
            "reader capacity refusal lost its original external boundary or changed work budget",
        )?;
        require(
            capacity.snapshot()?.used_bytes == baseline.used_bytes,
            "reader refusal changed retained extent",
        )?;
        drop(owner);
        let reopened = io.open(&path, &mut budget()?)?;
        let mut bytes = [0; 14];
        reopened.file()?.read_exact(&mut bytes)?;
        require(
            bytes == *b"physical-bytes",
            "reader reopen changed object bytes",
        )?;
        drop(reopened);
        require(
            capacity.snapshot()?.used_bytes == baseline.used_bytes,
            "reader close released a still-linked object",
        )?;
        Ok(())
    }

    #[test]
    fn missing_and_symlink_object_paths_are_corruption_not_storage() -> TestResult {
        let (directory, path, _capacity, common) = fixture()?;
        let io = NativeObjectReadIo::new(common);
        let outside = directory.path().join("outside-object");
        fs::write(&outside, b"untrusted-target")?;
        fs::remove_file(&path)?;
        std::os::unix::fs::symlink(&outside, &path)?;
        require(
            matches!(
                io.open(&path, &mut budget()?),
                Err(IndexError::CorruptObject(_))
            ),
            "symlink object path escaped the typed corruption boundary",
        )?;
        fs::remove_file(&path)?;
        require(
            matches!(
                io.open(&path, &mut budget()?),
                Err(IndexError::CorruptObject(_))
            ),
            "missing object path escaped the typed corruption boundary",
        )?;
        Ok(())
    }
}
