//! File-store reads retain the actual FD under the common physical authority.

use std::{
    fmt,
    fs::{File, OpenOptions},
    os::unix::fs::OpenOptionsExt as _,
    path::Path,
    sync::Arc,
};

use quanta_index_contract::SearchPlaneErrorCodeV2;
use quanta_index_core::{
    CoreError,
    index::{
        IndexError, MemoryLease, MemoryLifetime, ObjectReadFile, ObjectReadIoPort, WorkBudget,
    },
};
use rusqlite::ffi::quanta_capacity_vfs::close_owned_file;
use rustix::fs::OFlags;

use crate::publication_file_capacity::{
    InodeObservation, NativeReaderLease, PublicationFileCapacity,
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
                && reader
                    .retire()
                    .and_then(|()| self.common.sweep_retired())
                    .is_err()
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
    } else {
        IndexError::Storage(format!("native object read I/O: {error}"))
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

impl ObjectReadIoPort for NativeObjectReadIo {
    fn open(
        &self,
        path: &Path,
        budget: &mut WorkBudget,
    ) -> Result<Box<dyn ObjectReadFile>, IndexError> {
        let memory = budget.reserve_memory(
            u64::try_from(std::mem::size_of::<ReadFile>())
                .map_err(|_error| IndexError::ResourceLimited)?,
            MemoryLifetime::Temporary,
        )?;
        let mut opening = self
            .common
            .begin_native_reader_open(path)
            .map_err(|error| {
                index_error_at(&error, budget, "object_read.begin_native_reader_open")
            })?;
        let nofollow = i32::try_from(OFlags::NOFOLLOW.bits())
            .map_err(|error| IndexError::Storage(format!("object reader nofollow: {error}")))?;
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(nofollow)
            .open(path)
            .map_err(|error| {
                if error.kind() == std::io::ErrorKind::NotFound {
                    IndexError::CorruptObject("referenced object is absent".into())
                } else {
                    IndexError::Storage(format!("open admitted object reader: {error}"))
                }
            })?;
        let admitted = InodeObservation::from_file(&file)
            .and_then(|observed| opening.opened_witness(observed));
        let reader = match admitted {
            Ok(reader) => reader,
            Err(error) => {
                if close_owned_file(file).is_err() {
                    self.common.mark_close_unwitnessed();
                }
                return Err(index_error_at(&error, budget, "object_read.opened_witness"));
            }
        };
        Ok(Box::new(ReadFile {
            file: Some(file),
            reader: Some(reader),
            common: Arc::clone(&self.common),
            _memory: memory,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::publication_inventory::{PublicationBootInventoryLimits, seed_publication_capacity};
    use quanta_index_core::{RequestBudgetV1, publication_capacity::PublicationCapacity};
    use quanta_index_searchd::app::runtime::StateRootLease;
    use std::{fs, io::Read as _, os::unix::fs::PermissionsExt as _, path::PathBuf};

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
}
