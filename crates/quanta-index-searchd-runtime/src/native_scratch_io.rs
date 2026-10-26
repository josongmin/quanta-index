//! Scratch uses the same file authority and named VFS as the durable Catalog.

use std::{
    ffi::CString,
    fmt,
    fs::{File, OpenOptions, TryLockError},
    os::unix::fs::{MetadataExt as _, OpenOptionsExt as _},
    path::Path,
    sync::Arc,
};

use quanta_index_catalog::{CatalogIoAdmission, CatalogIoScope, CatalogReadOnlyIoAdmission};
use quanta_index_contract::SearchPlaneErrorCodeV2;
use quanta_index_core::publication_admission::CatalogPublicationFundingScope;
use quanta_index_core::publication_capacity::PublicationAllowance;
use quanta_index_core::{
    CoreError,
    index::{
        IndexError, ScratchIoBinding, ScratchIoScope, ScratchNamespaceLease, WorkBudget, WorkUsage,
    },
    publication_capacity::InodeKey,
};
use rusqlite::ffi::quanta_capacity_vfs::{ScopeGuard, close_owned_file};
use rustix::fs::OFlags;

use crate::{
    publication_file_capacity::{InodeObservation, NativeReaderLease, PublicationFileCapacity},
    publication_sqlite_vfs::SqliteCapacityVfs,
};

fn index_error(error: &CoreError) -> IndexError {
    if matches!(
        error,
        CoreError::Typed {
            code: SearchPlaneErrorCodeV2::RequestCancelled,
            ..
        }
    ) {
        IndexError::Cancelled
    } else if matches!(
        error,
        CoreError::Typed {
            code: SearchPlaneErrorCodeV2::IngestResourceBudgetExceeded,
            ..
        }
    ) {
        IndexError::ResourceLimited
    } else {
        IndexError::Storage(format!("native scratch physical I/O: {error}"))
    }
}

fn charge_cleanup(budget: &mut Option<&mut WorkBudget>, usage: WorkUsage) -> Result<(), CoreError> {
    let Some(budget) = budget.as_deref_mut() else {
        return Ok(());
    };
    budget.charge(usage).map_err(|error| {
        let code = match error {
            IndexError::Cancelled => quanta_index_core::REQUEST_CANCELLED_CODE,
            IndexError::ResourceLimited => SearchPlaneErrorCodeV2::IngestResourceBudgetExceeded,
            // Reject a new budget failure rather than relabeling it as a
            // capacity refusal when the WorkBudget contract evolves.
            IndexError::InvalidInput(_)
            | IndexError::Conflict
            | IndexError::DigestConflict
            | IndexError::FenceLost
            | IndexError::WorkFenceLost
            | IndexError::WorkLimitExceeded
            | IndexError::SnapshotUnavailable
            | IndexError::CorruptObject(_)
            | IndexError::BackendViolation(_)
            | IndexError::Storage(_)
            | IndexError::RebuildRequired
            | IndexError::FocusSubjectNotFound
            | IndexError::MaintenanceRequired => {
                return CoreError::InvalidContract(format!(
                    "unexpected scratch work-budget failure: {error}"
                ));
            }
        };
        CoreError::Typed {
            code,
            message: format!("scratch cleanup original work budget: {error}"),
        }
    })
}

struct ScratchScope(ScopeGuard);
impl ScratchIoScope for ScratchScope {
    fn is_capacity_refusal(&self, backend_error_code: i32) -> bool {
        self.0.is_capacity_extended_code(backend_error_code)
    }
}

struct NamespaceLease {
    namespace: InodeKey,
    file: Option<File>,
    reader: Option<NativeReaderLease>,
    common: Arc<PublicationFileCapacity>,
}
impl fmt::Debug for NamespaceLease {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("NamespaceLease")
            .field("open", &self.file.is_some())
            .finish_non_exhaustive()
    }
}
impl ScratchNamespaceLease for NamespaceLease {
    fn namespace_identity(&self) -> InodeKey {
        self.namespace
    }
}
impl Drop for NamespaceLease {
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

/// Registration constructs both facets from one authenticated common authority.
pub struct NativeScratchIo {
    vfs: SqliteCapacityVfs,
    common: Arc<PublicationFileCapacity>,
    maintenance: Option<MaintenanceFunding>,
}

struct MaintenanceFunding {
    parent: Arc<PublicationAllowance>,
}

impl fmt::Debug for NativeScratchIo {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("NativeScratchIo")
            .field("driver", &self.vfs.name())
            .finish_non_exhaustive()
    }
}

impl NativeScratchIo {
    pub fn register(
        common: Arc<PublicationFileCapacity>,
        name: CString,
        max_open_handles: u32,
    ) -> Result<Arc<Self>, CoreError> {
        let vfs = SqliteCapacityVfs::register(Arc::clone(&common), name, max_open_handles)?;
        Ok(Arc::new(Self {
            vfs,
            common,
            maintenance: None,
        }))
    }

    pub(crate) fn register_with_maintenance(
        common: Arc<PublicationFileCapacity>,
        name: CString,
        max_open_handles: u32,
    ) -> Result<Arc<Self>, CoreError> {
        let parent = common
            .maintenance_parent()?
            .ok_or_else(|| CoreError::InvalidContract("maintenance reserve is absent".into()))?;
        let vfs = SqliteCapacityVfs::register_with_optional_funding(
            Arc::clone(&common),
            name,
            max_open_handles,
        )?;
        Ok(Arc::new(Self {
            vfs,
            common,
            maintenance: Some(MaintenanceFunding { parent }),
        }))
    }

    pub(crate) fn enter_maintenance_scope(
        &self,
    ) -> Result<Box<dyn CatalogPublicationFundingScope>, IndexError> {
        let funding = self.maintenance.as_ref().ok_or_else(|| {
            IndexError::BackendViolation("native maintenance reserve is absent".into())
        })?;
        // Replenish from actual shared headroom, but preserve a partial window
        // when cleanup itself must free the space needed for a full refill.
        self.common
            .refill_maintenance_reserve()
            .map_err(|error| index_error(&error))?;
        self.vfs
            .enter_funded_scope(Arc::clone(&funding.parent))
            .map_err(|error| index_error(&error))
    }

    pub(crate) fn prepare_lock(&self, path: &Path) -> Result<(), CoreError> {
        self.common.prepare_empty_lock(path, false)
    }

    pub(crate) fn drain_retirements(&self) -> Result<(), CoreError> {
        self.vfs.drain_retirements()
    }

    fn acquire_lock(&self, path: &Path) -> Result<Box<dyn ScratchNamespaceLease>, IndexError> {
        self.prepare_lock(path)
            .map_err(|error| index_error(&error))?;
        let namespace_file = File::open(
            path.parent()
                .ok_or_else(|| IndexError::InvalidInput("scratch lock has no namespace".into()))?,
        )
        .map_err(|error| {
            IndexError::Storage(format!("open scratch namespace identity: {error}"))
        })?;
        let metadata = namespace_file.metadata().map_err(|error| {
            IndexError::Storage(format!("inspect scratch namespace identity: {error}"))
        })?;
        let mut opening = self
            .common
            .begin_native_reader_open(path)
            .map_err(|error| index_error(&error))?;
        let nofollow = i32::try_from(OFlags::NOFOLLOW.bits())
            .map_err(|error| IndexError::Storage(format!("scratch lock nofollow flag: {error}")))?;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(nofollow)
            .open(path)
            .map_err(|error| IndexError::Storage(format!("open admitted scratch lock: {error}")))?;
        let admitted = InodeObservation::from_file(&file)
            .and_then(|observed| opening.opened_witness(observed));
        let reader = match admitted {
            Ok(reader) => reader,
            Err(error) => {
                if close_owned_file(file).is_err() {
                    self.common.mark_close_unwitnessed();
                }
                return Err(index_error(&error));
            }
        };
        let lease = NamespaceLease {
            namespace: InodeKey {
                device: metadata.dev(),
                inode: metadata.ino(),
            },
            file: Some(file),
            reader: Some(reader),
            common: Arc::clone(&self.common),
        };
        let locked = lease
            .file
            .as_ref()
            .ok_or_else(|| {
                IndexError::BackendViolation("admitted scratch lock lost its file".into())
            })?
            .try_lock();
        match locked {
            Ok(()) => Ok(Box::new(lease)),
            Err(TryLockError::WouldBlock) => Err(IndexError::ResourceLimited),
            Err(TryLockError::Error(error)) => Err(IndexError::Storage(format!(
                "lock admitted scratch namespace: {error}"
            ))),
        }
    }
}

impl CatalogIoAdmission for NativeScratchIo {
    fn vfs_name(&self) -> &str {
        self.vfs.name()
    }
    fn enter_scope(
        &self,
        diagnostics: Option<&quanta_index_core::DiagnosticContextV1>,
    ) -> Box<dyn CatalogIoScope> {
        self.vfs.enter_scope(diagnostics)
    }
    fn enter_close_scope(&self) -> Box<dyn CatalogIoScope> {
        self.maintenance.as_ref().map_or_else(
            || self.vfs.enter_close_scope(),
            |funding| {
                self.vfs
                    .close_scope_with_parent(Arc::clone(&funding.parent))
            },
        )
    }
    fn open_lock_file(
        &self,
        path: &Path,
        create_new: bool,
    ) -> Result<Box<dyn quanta_index_core::index::ObjectReadFile>, IndexError> {
        self.vfs.open_lock_file(path, create_new)
    }
    fn unlink_lock_file(&self, path: &Path, held: &File) -> Result<(), IndexError> {
        self.vfs.unlink_lock_file(path, held)
    }
}

impl CatalogReadOnlyIoAdmission for NativeScratchIo {
    fn read_only_vfs_name(&self) -> &str {
        self.vfs.read_only_vfs_name()
    }

    fn enter_read_only_scope(
        &self,
        request: &quanta_index_core::RequestBudgetV1,
    ) -> Box<dyn CatalogIoScope> {
        self.vfs.enter_read_only_scope(request)
    }
}

impl ScratchIoBinding for NativeScratchIo {
    fn driver_name(&self) -> &str {
        self.vfs.name()
    }
    fn enter_scope(&self) -> Box<dyn ScratchIoScope> {
        Box::new(ScratchScope(self.vfs.scope()))
    }
    fn acquire_namespace(&self, path: &Path) -> Result<Box<dyn ScratchNamespaceLease>, IndexError> {
        self.acquire_lock(&path.join(".lease"))
    }
    fn retire_namespace(
        &self,
        path: &Path,
        mut budget: Option<&mut WorkBudget>,
    ) -> Result<(), IndexError> {
        if let Some(budget) = budget.as_deref_mut() {
            budget.checkpoint()?;
        }
        self.vfs
            .drain_retirements()
            .map_err(|error| index_error(&error))?;
        self.common
            .discard_retired_native_pending_children(path, &mut || {
                charge_cleanup(
                    &mut budget,
                    WorkUsage {
                        reads: 1,
                        writes: 1,
                        mutations: 1,
                        ..WorkUsage::default()
                    },
                )
            })
            .map_err(|error| index_error(&error))?;
        let mut ticket = self
            .common
            .begin_directory_delete(path, &mut || {
                charge_cleanup(
                    &mut budget,
                    WorkUsage {
                        reads: 1,
                        ..WorkUsage::default()
                    },
                )
            })
            .map_err(|error| index_error(&error))?;
        // Admit the complete bounded directory transaction before its first
        // unlink; cancellation cannot abandon a partially settled directory.
        ticket
            .perform_and_sync(&mut || {
                charge_cleanup(
                    &mut budget,
                    WorkUsage {
                        reads: 1,
                        writes: 1,
                        mutations: 1,
                        ..WorkUsage::default()
                    },
                )
            })
            .map_err(|error| index_error(&error))?;
        if let Some(budget) = budget {
            budget.checkpoint()?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::publication_inventory::{PublicationBootInventoryLimits, seed_publication_capacity};
    use quanta_index_core::{
        RequestBudgetV1,
        index::{ScratchRequest, ScratchSpacePort, WorkBudget, WorkLimits},
    };
    use quanta_index_scratch_sqlite::ScratchConnection;
    use quanta_index_searchd::app::runtime::StateRootLease;
    use quanta_index_storage::FileScratchSpace;
    use std::{
        os::unix::fs::PermissionsExt as _,
        time::{Duration, Instant},
    };

    type TestResult = Result<(), Box<dyn std::error::Error>>;
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
                reads: 1000,
                writes: 1000,
                bytes: 16 * 1024 * 1024,
                mutations: 1000,
            },
            Instant::now()
                .checked_add(Duration::from_secs(30))
                .ok_or(IndexError::ResourceLimited)?,
        ))
    }

    type Fixture = (
        tempfile::TempDir,
        std::path::PathBuf,
        quanta_index_core::publication_capacity::PublicationCapacity,
        Arc<PublicationFileCapacity>,
        Arc<NativeScratchIo>,
        FileScratchSpace,
    );
    fn disposal_fixture() -> Result<Fixture, Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let lease = StateRootLease::acquire(&directory.path().join("state"))?;
        let root = lease.state_root_identity_v1().to_path_buf();
        let authority = seed_publication_capacity(
            &lease,
            &root,
            PublicationBootInventoryLimits {
                max_bytes: 8 * 1024 * 1024,
                max_files: 64,
                max_pending: 128,
                max_entries: 256,
                max_depth: 8,
                max_pending_frames: 16,
            },
            &[],
            &RequestBudgetV1::unbounded(),
        )?;
        let common = PublicationFileCapacity::new(lease, authority.clone(), 32, 256, 256)?;
        let io = NativeScratchIo::register(
            Arc::clone(&common),
            CString::new("quanta-publication-capacity")?,
            512,
        )?;
        let binding: Arc<dyn ScratchIoBinding> = io.clone();
        let space =
            FileScratchSpace::open_managed_with_io(root.join("scratch"), 8 * 1024 * 1024, binding)?;
        Ok((directory, root, authority, common, io, space))
    }

    #[test]
    fn budgeted_disposal_preserves_remaining_custody_for_allocation_drop() -> TestResult {
        let (_directory, _root, authority, _common, io, space) = disposal_fixture()?;
        let baseline = authority.snapshot()?;
        let allocation = space.allocate(
            ScratchRequest {
                initial_bytes: 256 * 1024,
                max_bytes: 2 * 1024 * 1024,
            },
            &mut budget()?,
        )?;
        let path = allocation.path().to_path_buf();
        for name in ["a.sqlite", "b.sqlite"] {
            let connection = ScratchConnection::open(&path.join(name), allocation.as_ref())?;
            {
                let db = connection.scoped()?;
                db.execute_batch("PRAGMA journal_mode=OFF; PRAGMA synchronous=OFF; CREATE TABLE t(v); INSERT INTO t VALUES(42);")?;
            }
            drop(connection);
        }
        io.vfs.drain_retirements()?;
        let charged = authority.snapshot()?;
        let original = std::fs::read(path.join("b.sqlite"))?;
        let request = RequestBudgetV1::for_duration(Duration::from_secs(30));
        let mut cancelled = WorkBudget::with_request(
            WorkLimits {
                reads: 100,
                writes: 100,
                bytes: 0,
                mutations: 100,
            },
            request.clone(),
        );
        request.cancel_handle().cancel();
        require(
            io.retire_namespace(&path, Some(&mut cancelled)) == Err(IndexError::Cancelled),
            "cancelled request mutated scratch",
        )?;
        require(
            authority.snapshot()? == charged && std::fs::read(path.join("b.sqlite"))? == original,
            "pre-cancel changed bytes or charges",
        )?;
        require(
            cancelled.usage() == WorkUsage::default(),
            "pre-cancel spent work",
        )?;

        let mut prefix = WorkBudget::with_request(
            WorkLimits {
                reads: 100,
                writes: 1,
                bytes: 0,
                mutations: 1,
            },
            RequestBudgetV1::for_duration(Duration::from_secs(30)),
        );
        require(
            io.retire_namespace(&path, Some(&mut prefix)) == Err(IndexError::ResourceLimited),
            "one-child budget did not stop disposal",
        )?;
        require(
            !path.join("a.sqlite").exists() && path.join("b.sqlite").exists(),
            "bounded prefix did not preserve remaining file",
        )?;
        require(
            std::fs::read(path.join("b.sqlite"))? == original,
            "budget refusal changed remaining bytes",
        )?;
        require(
            prefix.usage().writes == 1 && prefix.usage().mutations == 1,
            "physical prefix escaped original work ledger",
        )?;
        require(
            authority.snapshot()?.uncertain_pending == 1,
            "remaining pending charge was lost",
        )?;

        // Lifecycle cleanup owns the remaining allocation. Do not erase its
        // directory behind the allocation and then mistake a failed Drop for
        // a successful release of the separate logical scratch reservation.
        drop(allocation);
        require(
            !path.exists() && authority.snapshot()? == baseline,
            "allocation Drop did not settle exact remaining custody",
        )?;
        let full = space.allocate(
            ScratchRequest {
                initial_bytes: 8 * 1024 * 1024,
                max_bytes: 8 * 1024 * 1024,
            },
            &mut budget()?,
        )?;
        drop(full);
        require(
            authority.snapshot()? == baseline,
            "logical reservation release changed physical capacity",
        )?;
        Ok(())
    }

    #[test]
    fn disposal_refuses_live_replaced_hardlinked_and_unsafe_scratch_before_unlink() -> TestResult {
        for mode in 0..4 {
            let (_directory, _root, authority, _common, io, space) = disposal_fixture()?;
            let baseline = authority.snapshot()?;
            let allocation = space.allocate(
                ScratchRequest {
                    initial_bytes: 256 * 1024,
                    max_bytes: 2 * 1024 * 1024,
                },
                &mut budget()?,
            )?;
            let path = allocation.path().to_path_buf();
            let database = path.join("rows.sqlite");
            let connection = ScratchConnection::open(&database, allocation.as_ref())?;
            {
                let db = connection.scoped()?;
                db.execute_batch("PRAGMA journal_mode=OFF; PRAGMA synchronous=OFF; CREATE TABLE t(v BLOB); INSERT INTO t VALUES(zeroblob(1024));")?;
            }
            if mode == 0 {
                let charged = authority.snapshot()?;
                let original = std::fs::read(&database)?;
                require(
                    io.retire_namespace(&path, None).is_err(),
                    "live scratch accepted disposal",
                )?;
                require(
                    std::fs::read(&database)? == original && authority.snapshot()? == charged,
                    "live refusal changed bytes or charge",
                )?;
                drop(connection);
                // No explicit fixture retirement drain: production cleanup owns it.
                drop(allocation);
                require(
                    !path.exists() && authority.snapshot()? == baseline,
                    "actual close did not enable exact scratch cleanup",
                )?;
            } else if mode == 3 {
                drop(connection);
                io.vfs.drain_retirements()?;
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o777))?;
                let original = std::fs::read(&database)?;
                let charged = authority.snapshot()?;
                require(
                    io.retire_namespace(&path, None).is_err(),
                    "unsafe namespace accepted disposal",
                )?;
                drop(allocation);
                require(
                    std::fs::read(&database)? == original && authority.snapshot()? == charged,
                    "unsafe namespace cleanup changed bytes or charge",
                )?;
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700))?;
            } else {
                drop(connection);
                io.vfs.drain_retirements()?;
                let original = std::fs::read(&database)?;
                let alternate = path.join("retained.sqlite");
                if mode == 1 {
                    std::fs::rename(&database, &alternate)?;
                    std::fs::write(&database, b"replacement")?;
                    std::fs::set_permissions(&database, std::fs::Permissions::from_mode(0o600))?;
                } else {
                    std::fs::hard_link(&database, &alternate)?;
                }
                let charged = authority.snapshot()?;
                require(
                    io.retire_namespace(&path, None).is_err(),
                    "changed scratch accepted disposal",
                )?;
                require(
                    std::fs::read(&alternate)? == original && authority.snapshot()? == charged,
                    "changed-file refusal unlinked bytes or refunded custody",
                )?;
                let current = std::fs::read(&database)?;
                let expected = if mode == 1 {
                    b"replacement".as_slice()
                } else {
                    original.as_slice()
                };
                require(
                    current.as_slice() == expected,
                    "changed-file refusal altered current name",
                )?;
                drop(allocation);
                require(
                    path.exists() && authority.snapshot()? == charged,
                    "allocation Drop bypassed refused disposal",
                )?;
            }
        }
        Ok(())
    }

    #[test]
    fn synced_then_unsynced_resize_refunds_only_after_actual_retirement_and_unlink() -> TestResult {
        let (_directory, _root, authority, common, _io, space) = disposal_fixture()?;
        let baseline = authority.snapshot()?;
        let allocation = space.allocate(
            ScratchRequest {
                initial_bytes: 256 * 1024,
                max_bytes: 2 * 1024 * 1024,
            },
            &mut budget()?,
        )?;
        let path = allocation.path().to_path_buf();
        let connection = ScratchConnection::open(&path.join("rows.sqlite"), allocation.as_ref())?;
        {
            let db = connection.scoped()?;
            db.execute_batch(
                "PRAGMA journal_mode=OFF; PRAGMA synchronous=FULL; CREATE TABLE t(v BLOB);",
            )?;
            let synced = authority.snapshot()?;
            require(
                synced.used_bytes > baseline.used_bytes && synced.active_tickets == 0,
                "first sync did not settle the native file",
            )?;
            db.execute_batch("PRAGMA synchronous=OFF; INSERT INTO t VALUES(zeroblob(32768));")?;
            let length: i64 = db.query_row("SELECT length(v) FROM t", [], |row| row.get(0))?;
            require(length == 32768, "unsynced resize changed fixed query gold")?;
        }
        drop(connection);
        drop(allocation);
        require(
            !path.exists() && authority.snapshot()? == baseline,
            "retired resize did not refund exactly its base and growth",
        )?;
        drop(space);
        require(
            common.is_quiescent()?,
            "retired resize retained a pin or mutation",
        )?;
        Ok(())
    }

    #[test]
    fn actual_scratch_driver_accounts_growth_and_refunds_only_after_close_and_delete() -> TestResult
    {
        let directory = tempfile::tempdir()?;
        let lease = StateRootLease::acquire(&directory.path().join("state"))?;
        let root = lease.state_root_identity_v1().to_path_buf();
        let authority = seed_publication_capacity(
            &lease,
            &root,
            PublicationBootInventoryLimits {
                max_bytes: 8 * 1024 * 1024,
                max_files: 64,
                max_pending: 128,
                max_entries: 256,
                max_depth: 8,
                max_pending_frames: 16,
            },
            &[],
            &RequestBudgetV1::unbounded(),
        )?;
        let common = PublicationFileCapacity::new(lease, authority.clone(), 32, 256, 256)?;
        let io = NativeScratchIo::register(
            Arc::clone(&common),
            CString::new("quanta-publication-capacity")?,
            512,
        )?;
        let binding: Arc<dyn ScratchIoBinding> = io.clone();
        let space =
            FileScratchSpace::open_managed_with_io(root.join("scratch"), 8 * 1024 * 1024, binding)?;
        let baseline = authority.snapshot()?;
        let allocation = space.allocate(
            ScratchRequest {
                initial_bytes: 256 * 1024,
                max_bytes: 2 * 1024 * 1024,
            },
            &mut budget()?,
        )?;
        let path = allocation.path().to_path_buf();
        let connection = ScratchConnection::open(&path.join("rows.sqlite"), allocation.as_ref())?;
        {
            let db = connection.scoped()?;
            db.execute_batch("PRAGMA journal_mode=OFF; PRAGMA synchronous=OFF; PRAGMA temp_store=FILE; CREATE TABLE t(v BLOB); INSERT INTO t VALUES(zeroblob(32768));")?;
            let length: i64 = db.query_row("SELECT length(v) FROM t", [], |row| row.get(0))?;
            require(length == 32768, "scratch data golden differs")?;
        }
        require(
            authority.snapshot()?.used_bytes > baseline.used_bytes,
            "scratch growth was uncharged",
        )?;
        drop(connection);
        io.vfs.drain_retirements()?;
        require(
            !common.is_quiescent()?,
            "namespace lock retired before its space and allocations",
        )?;
        drop(allocation);
        let after = authority.snapshot()?;
        require(
            !path.exists(),
            "scratch namespace survived confirmed deletion",
        )?;
        require(
            after.used_bytes == baseline.used_bytes
                && after.used_files == baseline.used_files
                && after.active_tickets == 0
                && after.active_allowances == 0,
            "scratch cleanup did not refund exact baseline",
        )?;
        drop(space);
        require(
            common.is_quiescent()?,
            "last scratch owner retained a live FD",
        )?;
        drop(io);
        drop(common);
        drop(authority);
        let reacquired = StateRootLease::acquire(&root)?;
        drop(reacquired);
        Ok(())
    }

    #[test]
    fn actual_scratch_capacity_refusal_is_distinct_from_raw_full_and_cleanup_refunds() -> TestResult
    {
        let directory = tempfile::tempdir()?;
        let lease = StateRootLease::acquire(&directory.path().join("state"))?;
        let root = lease.state_root_identity_v1().to_path_buf();
        let maximum = 128 * 1024;
        let authority = seed_publication_capacity(
            &lease,
            &root,
            PublicationBootInventoryLimits {
                max_bytes: maximum,
                max_files: 64,
                max_pending: 128,
                max_entries: 256,
                max_depth: 8,
                max_pending_frames: 16,
            },
            &[],
            &RequestBudgetV1::unbounded(),
        )?;
        let common = PublicationFileCapacity::new(lease, authority.clone(), 32, 256, 256)?;
        let io = NativeScratchIo::register(
            Arc::clone(&common),
            CString::new("quanta-publication-capacity")?,
            512,
        )?;
        let binding: Arc<dyn ScratchIoBinding> = io.clone();
        let space =
            FileScratchSpace::open_managed_with_io(root.join("scratch"), 8 * 1024 * 1024, binding)?;
        let baseline = authority.snapshot()?;
        let allocation = space.allocate(
            ScratchRequest {
                initial_bytes: 2 * 1024 * 1024,
                max_bytes: 2 * 1024 * 1024,
            },
            &mut budget()?,
        )?;
        let path = allocation.path().to_path_buf();
        let connection = ScratchConnection::open(&path.join("rows.sqlite"), allocation.as_ref())?;
        {
            let db = connection.scoped()?;
            db.execute_batch(
                "PRAGMA journal_mode=OFF; PRAGMA synchronous=OFF; CREATE TABLE t(v BLOB);",
            )?;
            quanta_index_scratch_sqlite::configure(&db, allocation.as_ref())?;
            let error = db
                .execute("INSERT INTO t VALUES(zeroblob(1048576))", [])
                .err()
                .ok_or("aggregate capacity did not refuse scratch growth")?;
            require(
                matches!(
                    quanta_index_scratch_sqlite::storage_error(&error, "actual scratch"),
                    IndexError::ResourceLimited
                ),
                "actual scoped capacity witness was lost",
            )?;
            let raw = rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_FULL),
                None,
            );
            require(
                matches!(
                    quanta_index_scratch_sqlite::storage_error(&raw, "raw engine full"),
                    IndexError::Storage(_)
                ),
                "raw FULL inherited a preceding capacity witness",
            )?;
            require(
                authority.snapshot()?.used_bytes <= maximum,
                "aggregate file lengths exceeded cap",
            )?;
        }
        drop(connection);
        io.vfs.drain_retirements()?;
        drop(allocation);
        let after = authority.snapshot()?;
        require(
            !path.exists()
                && after.used_bytes == baseline.used_bytes
                && after.used_files == baseline.used_files
                && after.active_tickets == 0
                && after.active_allowances == 0,
            "failed scratch growth leaked its confirmed cleanup charge",
        )?;
        Ok(())
    }

    #[test]
    fn namespace_lock_charges_handle_until_last_space_or_allocation_owner() -> TestResult {
        let directory = tempfile::tempdir()?;
        let lease = StateRootLease::acquire(&directory.path().join("state"))?;
        let root = lease.state_root_identity_v1().to_path_buf();
        let authority = seed_publication_capacity(
            &lease,
            &root,
            PublicationBootInventoryLimits {
                max_bytes: 1024 * 1024,
                max_files: 64,
                max_pending: 128,
                max_entries: 256,
                max_depth: 8,
                max_pending_frames: 16,
            },
            &[],
            &RequestBudgetV1::unbounded(),
        )?;
        let common = PublicationFileCapacity::new(lease, authority.clone(), 1, 256, 256)?;
        let io = NativeScratchIo::register(
            Arc::clone(&common),
            CString::new("quanta-publication-capacity")?,
            512,
        )?;
        let binding: Arc<dyn ScratchIoBinding> = io.clone();
        let space =
            FileScratchSpace::open_managed_with_io(root.join("scratch"), 1024 * 1024, binding)?;
        let lock_path = root.join("scratch/.lease");
        require(
            matches!(
                common.begin_native_reader_open(&lock_path),
                Err(CoreError::Typed {
                    code: SearchPlaneErrorCodeV2::IngestResourceBudgetExceeded,
                    ..
                })
            ) && matches!(
                common.begin_reader_open(&lock_path),
                Err(CoreError::Typed {
                    code: SearchPlaneErrorCodeV2::IngestResourceBudgetExceeded,
                    ..
                })
            ),
            "live namespace lock did not refuse both reader paths before open",
        )?;
        let cloned_space = space.clone();
        let allocation = space.allocate(
            ScratchRequest {
                initial_bytes: 40,
                max_bytes: 80,
            },
            &mut budget()?,
        )?;
        drop(space);
        require(
            !common.is_quiescent()?,
            "space clone lost namespace custody",
        )?;
        drop(cloned_space);
        require(!common.is_quiescent()?, "allocation lost namespace custody")?;
        drop(allocation);
        require(
            common.is_quiescent()?,
            "last owner did not retire actual lock FD",
        )?;
        let opening = common.begin_native_reader_open(&lock_path)?;
        drop(opening);
        require(
            common.is_quiescent()?,
            "reader refusal leaked an open reservation",
        )?;
        drop(io);
        drop(common);
        drop(authority);
        let reacquired = StateRootLease::acquire(&root)?;
        drop(reacquired);
        Ok(())
    }
}
