//! Scratch and Catalog share a publication file authority.

use std::{
    fmt,
    fs::{File, OpenOptions, TryLockError},
    os::unix::fs::{MetadataExt as _, OpenOptionsExt as _},
    path::Path,
    sync::Arc,
};

use crate::owned_fd_close::close_owned_file;
use quanta_index_catalog::{
    CatalogIoAdmission, CatalogIoScope, CatalogReadOnlyIoAdmission, CatalogWriterCachePolicy,
    SqliteCatalog,
};
use quanta_index_contract::SearchPlaneErrorCodeV2;
use quanta_index_core::publication_admission::CatalogPublicationFundingScope;
use quanta_index_core::publication_capacity::PublicationAllowance;
use quanta_index_core::{
    CoreError,
    index::{
        IndexError, MemoryLease, ScratchIoBinding, ScratchIoScope, ScratchNamespaceLease,
        ScratchSqliteVfs, WorkBudget, WorkUsage,
    },
    publication_capacity::InodeKey,
};
use rustix::fs::OFlags;

use crate::{
    catalog_lock_files::{self, CatalogFileFunding},
    publication_file_capacity::{InodeObservation, NativeReaderLease, PublicationFileCapacity},
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
    common: Arc<PublicationFileCapacity>,
    catalog_file_funding: CatalogFileFunding,
    maintenance: Option<MaintenanceFunding>,
    global_sqlite_handles: u32,
}

struct MaintenanceFunding {
    parent: Arc<PublicationAllowance>,
}

struct StockSqliteScope;

impl CatalogIoScope for StockSqliteScope {
    fn is_capacity_refusal(&self, _error: &rusqlite::Error) -> bool {
        false
    }
}

impl ScratchIoScope for StockSqliteScope {
    fn is_capacity_refusal(&self, _backend_error_code: i32) -> bool {
        false
    }
}

impl fmt::Debug for NativeScratchIo {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("NativeScratchIo")
            .field("driver", &ScratchIoBinding::driver_name(self))
            .finish_non_exhaustive()
    }
}

impl NativeScratchIo {
    pub(crate) fn recovery_reader_handle_ceiling_v1(
        &self,
        saved_global_ceiling: u32,
    ) -> Result<usize, IndexError> {
        let actual = self.global_sqlite_handles;
        if actual == 0 || actual > saved_global_ceiling {
            return Err(IndexError::ResourceLimited);
        }
        usize::try_from(actual).map_err(|_| IndexError::ResourceLimited)
    }
    pub(crate) fn register_with_maintenance(
        common: Arc<PublicationFileCapacity>,
        max_open_handles: u32,
    ) -> Result<Arc<Self>, CoreError> {
        let parent = common
            .maintenance_parent()?
            .ok_or_else(|| CoreError::InvalidContract("maintenance reserve is absent".into()))?;
        let max_handles = usize::try_from(max_open_handles).map_err(|_error| {
            CoreError::InvalidContract("SQLite handle ceiling exceeds usize".into())
        })?;
        if max_handles == 0 || max_handles < common.max_open_handles() {
            return Err(CoreError::InvalidContract(
                "global SQLite handle ceiling below root ceiling".into(),
            ));
        }
        let catalog_file_funding = CatalogFileFunding::allocate()?;
        Ok(Arc::new(Self {
            catalog_file_funding,
            common,
            maintenance: Some(MaintenanceFunding { parent }),
            global_sqlite_handles: max_open_handles,
        }))
    }

    pub(crate) fn open_catalog_writer(
        self: &Arc<Self>,
        busy_timeout: std::time::Duration,
        policy: CatalogWriterCachePolicy,
        memory: MemoryLease,
    ) -> Result<SqliteCatalog, CoreError> {
        SqliteCatalog::open_with_stock_vfs_and_io_admission_and_writer_cache(
            self.common.state_root(),
            busy_timeout,
            self.clone(),
            policy,
            memory,
        )
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
        let parent = Arc::clone(&funding.parent);
        self.common
            .validate_held_allowance(&parent)
            .map_err(|error| index_error(&error))?;
        Ok(Box::new(self.catalog_file_funding.bind(parent)))
    }

    pub(crate) fn prepare_lock(&self, path: &Path) -> Result<(), CoreError> {
        self.common.prepare_empty_lock(path, false)
    }

    pub(crate) fn drain_retirements(&self) -> Result<(), CoreError> {
        self.common.sweep_retired()
    }

    /// Finish the SQLite scope after the connection owner has checked close.
    pub(crate) fn retire_checked(&self) -> Result<(), CoreError> {
        self.common.sweep_retired()
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
        "unix"
    }
    fn enter_scope(
        &self,
        _diagnostics: Option<&quanta_index_core::DiagnosticContextV1>,
    ) -> Box<dyn CatalogIoScope> {
        Box::new(StockSqliteScope)
    }
    fn enter_close_scope(&self) -> Box<dyn CatalogIoScope> {
        Box::new(StockSqliteScope)
    }
    fn open_lock_file(
        &self,
        path: &Path,
        create_new: bool,
    ) -> Result<Box<dyn quanta_index_core::index::ObjectReadFile>, IndexError> {
        self.catalog_file_funding
            .open(Arc::clone(&self.common), path, create_new)
    }
    fn unlink_lock_file(&self, path: &Path, held: &File) -> Result<(), IndexError> {
        catalog_lock_files::unlink(&self.common, path, held)
    }
}

impl CatalogReadOnlyIoAdmission for NativeScratchIo {
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    fn with_protected_catalog_directory_v1(
        &self,
        directory: &Path,
        request: &quanta_index_core::RequestBudgetV1,
        needs_wal: bool,
        visit: &mut dyn FnMut(&File),
    ) -> Result<bool, IndexError> {
        CatalogReadOnlyIoAdmission::with_protected_catalog_directory_v1(
            self.common.as_ref(),
            directory,
            request,
            needs_wal,
            visit,
        )
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    fn poison_protected_catalog_close_v1(&self) {
        self.common.mark_close_unwitnessed();
    }
}

impl ScratchIoBinding for NativeScratchIo {
    fn driver_name(&self) -> &str {
        "unix"
    }

    fn sqlite_vfs(&self) -> ScratchSqliteVfs<'_> {
        ScratchSqliteVfs::Stock
    }
    fn enter_scope(
        &self,
        _diagnostics: Option<&quanta_index_core::DiagnosticContextV1>,
    ) -> Box<dyn ScratchIoScope> {
        Box::new(StockSqliteScope)
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
        self.drain_retirements()
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
mod scratch_test_support {
    use super::{IndexError, WorkBudget};
    use quanta_index_core::index::WorkLimits;
    use std::time::{Duration, Instant};

    pub(super) type TestResult = Result<(), Box<dyn std::error::Error>>;
    pub(super) fn require(value: bool, message: &str) -> TestResult {
        if value {
            Ok(())
        } else {
            Err(std::io::Error::other(message).into())
        }
    }
    pub(super) fn budget() -> Result<WorkBudget, IndexError> {
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
}

#[cfg(test)]
mod stock_profile_tests {
    use super::scratch_test_support::{TestResult, budget, require};
    use super::*;
    use crate::publication_inventory::{PublicationBootInventoryLimits, seed_publication_capacity};
    use quanta_index_core::{
        RequestBudgetV1,
        index::{ScratchRequest, ScratchSpacePort},
        publication_capacity::PublicationCapacity,
    };
    use quanta_index_scratch_sqlite::ScratchConnection;
    use quanta_index_searchd::app::runtime::StateRootLease;
    use quanta_index_storage::FileScratchSpace;
    use std::os::unix::fs::PermissionsExt as _;

    type StockRoot = (
        tempfile::TempDir,
        std::path::PathBuf,
        PublicationCapacity,
        Arc<PublicationFileCapacity>,
    );

    fn stock_root() -> Result<StockRoot, Box<dyn std::error::Error>> {
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
        let common = PublicationFileCapacity::new_with_maintenance(
            lease,
            authority.clone(),
            32,
            256,
            256,
            4,
            2,
        )?;
        Ok((directory, root, authority, common))
    }

    #[test]
    fn stock_sqlite_uses_common_funding() -> TestResult {
        let (_directory, root, _authority, common) = stock_root()?;
        // Stock registration accepts the process-wide handle ceiling.
        let io = NativeScratchIo::register_with_maintenance(Arc::clone(&common), 96)?;
        (io.sqlite_vfs() == ScratchSqliteVfs::Stock)
            .then_some(())
            .ok_or("stock profile selected another scratch driver")?;
        let path = root.join("stock-catalog-retained");
        {
            let _scope = io.enter_maintenance_scope()?;
            let held = io.open_lock_file(&path, true)?;
            (common.maintenance_available()? == (4, 1))
                .then_some(())
                .ok_or("stock Catalog lock did not use held maintenance funding")?;
            io.unlink_lock_file(&path, held.file()?)?;
        }
        io.retire_checked()?;
        common.require_quiescent("stock test")?;
        Ok(())
    }

    #[test]
    fn stock_scratch_releases_namespace_and_reservation_after_sqlite_closes() -> TestResult {
        let (_directory, root, authority, common) = stock_root()?;
        let io = NativeScratchIo::register_with_maintenance(Arc::clone(&common), 96)?;
        let binding: Arc<dyn ScratchIoBinding> = io.clone();
        let space =
            FileScratchSpace::open_managed_with_io(root.join("scratch"), 8 * 1024 * 1024, binding)?
                .into_quota_managed_stock()?;
        let baseline = authority.snapshot()?;
        let request = ScratchRequest {
            initial_bytes: 256 * 1024,
            max_bytes: 256 * 1024,
        };
        let release_probe = ScratchRequest {
            initial_bytes: 8 * 1024 * 1024,
            max_bytes: 8 * 1024 * 1024,
        };
        let mut work = budget()?;
        let allocation = space.allocate(request, &mut work)?;
        let path = allocation.path().to_path_buf();
        let foreign = path.with_file_name("foreign.sqlite");
        require(
            matches!(
                ScratchConnection::open(&foreign, allocation.as_ref(), &work),
                Err(IndexError::InvalidInput(_))
            ) && !foreign.exists(),
            "stock SQLite opened outside its allocation",
        )?;
        let database = path.join("scratch.sqlite");
        let connection = ScratchConnection::open(&database, allocation.as_ref(), &work)?;
        {
            let db = connection.scoped(&work)?;
            quanta_index_scratch_sqlite::configure(&db, allocation.as_ref())?;
            db.execute_batch("CREATE TABLE gold (value INTEGER); INSERT INTO gold VALUES (37)")?;
        }
        let binding = allocation
            .io_binding()
            .ok_or_else(|| std::io::Error::other("stock scratch allocation lost its binding"))?;
        require(
            matches!(
                binding.retire_namespace(&path, None),
                Err(IndexError::InvalidInput(_))
            ),
            "a caller retired stock scratch outside allocation ownership",
        )?;
        drop(allocation);
        require(database.is_file(), "live stock SQLite lost its namespace")?;
        require(
            matches!(
                space.allocate(release_probe, &mut work),
                Err(IndexError::ResourceLimited)
            ),
            "live stock SQLite returned its allocation reservation",
        )?;
        let value: i64 =
            connection
                .scoped(&work)?
                .query_row("SELECT value FROM gold", [], |row| row.get(0))?;
        require(
            value == 37,
            "live stock SQLite could not read admitted data",
        )?;
        connection
            .close_checked()
            .map_err(|(_connection, error)| error)?;
        require(!path.exists(), "stock SQLite allocation was not reclaimed")?;
        drop(space.allocate(release_probe, &mut work)?);
        require(
            authority.snapshot()? == baseline,
            "stock scratch changed durable ledger",
        )?;
        drop(space);
        io.retire_checked()?;
        common.require_quiescent("stock scratch test")?;
        Ok(())
    }

    #[test]
    fn stock_disposal_refuses_live_replaced_hardlinked_and_unsafe_scratch() -> TestResult {
        for mode in 0..4 {
            let (_directory, root, authority, common) = stock_root()?;
            let io = NativeScratchIo::register_with_maintenance(Arc::clone(&common), 96)?;
            let binding: Arc<dyn ScratchIoBinding> = io.clone();
            let space = FileScratchSpace::open_managed_with_io(
                root.join("scratch"),
                8 * 1024 * 1024,
                binding,
            )?
            .into_quota_managed_stock()?;
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
            let connection = ScratchConnection::open(&database, allocation.as_ref(), &budget()?)?;
            {
                let db = connection.scoped(&budget()?)?;
                db.execute_batch("PRAGMA journal_mode=OFF; PRAGMA synchronous=OFF; CREATE TABLE t(v BLOB); INSERT INTO t VALUES(zeroblob(1024));")?;
            }
            if mode == 0 {
                let charged = authority.snapshot()?;
                let original = std::fs::read(&database)?;
                require(
                    io.retire_namespace(&path, None).is_err(),
                    "live stock scratch accepted disposal",
                )?;
                require(
                    std::fs::read(&database)? == original && authority.snapshot()? == charged,
                    "live refusal changed bytes or charge",
                )?;
                connection.close_checked().map_err(|(_, error)| error)?;
                drop(allocation);
                require(
                    !path.exists() && authority.snapshot()? == baseline,
                    "stock close did not enable exact scratch cleanup",
                )?;
                continue;
            }
            connection.close_checked().map_err(|(_, error)| error)?;
            io.drain_retirements()?;
            let original = std::fs::read(&database)?;
            let alternate = if mode == 1 {
                let retired = path.with_file_name("retired-allocation");
                std::fs::rename(&path, &retired)?;
                std::fs::create_dir(&path)?;
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700))?;
                std::fs::write(&database, b"replacement")?;
                std::fs::set_permissions(&database, std::fs::Permissions::from_mode(0o600))?;
                retired.join("rows.sqlite")
            } else if mode == 2 {
                let alternate = path.join("retained.sqlite");
                std::fs::hard_link(&database, &alternate)?;
                alternate
            } else {
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o777))?;
                database.clone()
            };
            let charged = authority.snapshot()?;
            require(
                io.retire_namespace(&path, None).is_err(),
                "changed stock scratch accepted disposal",
            )?;
            require(
                authority.snapshot()? == charged,
                "changed stock scratch refunded custody",
            )?;
            if mode == 3 {
                drop(allocation);
                require(
                    std::fs::read(&database)? == original && authority.snapshot()? == charged,
                    "unsafe stock scratch cleanup changed bytes or charge",
                )?;
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700))?;
            } else {
                require(
                    std::fs::read(&alternate)? == original,
                    "changed stock scratch unlinked original bytes",
                )?;
                drop(allocation);
                let expected = if mode == 1 {
                    b"replacement".as_slice()
                } else {
                    original.as_slice()
                };
                require(
                    path.exists()
                        && std::fs::read(&database)? == expected
                        && authority.snapshot()? == charged,
                    "stock allocation Drop bypassed refused disposal",
                )?;
            }
            require(
                matches!(
                    space.allocate(
                        ScratchRequest {
                            initial_bytes: 8 * 1024 * 1024,
                            max_bytes: 8 * 1024 * 1024,
                        },
                        &mut budget()?,
                    ),
                    Err(IndexError::ResourceLimited)
                ),
                "refused stock retirement refunded the logical reservation",
            )?;
        }
        Ok(())
    }

    #[test]
    fn stock_namespace_lock_charges_handle_until_last_owner() -> TestResult {
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
        let common = PublicationFileCapacity::new_with_maintenance(
            lease,
            authority.clone(),
            1,
            256,
            256,
            4,
            2,
        )?;
        let io = NativeScratchIo::register_with_maintenance(Arc::clone(&common), 96)?;
        let binding: Arc<dyn ScratchIoBinding> = io.clone();
        let space =
            FileScratchSpace::open_managed_with_io(root.join("scratch"), 1024 * 1024, binding)?
                .into_quota_managed_stock()?;
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
            "live stock namespace lock did not refuse reader paths",
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
            "last owner did not retire stock lock FD",
        )?;
        let opening = common.begin_native_reader_open(&lock_path)?;
        drop(opening);
        require(
            common.is_quiescent()?,
            "reader refusal leaked open reservation",
        )?;
        drop(io);
        drop(common);
        drop(authority);
        let reacquired = StateRootLease::acquire(&root)?;
        drop(reacquired);
        Ok(())
    }
}
