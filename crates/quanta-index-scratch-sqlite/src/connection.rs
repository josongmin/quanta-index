//! Scratch I/O custody is retained through actual close, independently of VFS selection.

use std::{
    cell::RefCell,
    ops::Deref,
    path::Path,
    rc::{Rc, Weak},
    sync::Arc,
};

use quanta_index_core::index::{
    IndexError, ScratchAllocation, ScratchIoBinding, ScratchIoScope, ScratchSqliteVfs, WorkBudget,
};
use rusqlite::{Connection, OpenFlags};

thread_local! {
    static CURRENT_SCOPE: RefCell<Option<Weak<dyn ScratchIoScope>>> = RefCell::new(None);
}

struct OperationScope {
    previous: Option<Weak<dyn ScratchIoScope>>,
    _scope: Option<Rc<dyn ScratchIoScope>>,
}

impl OperationScope {
    fn enter(
        binding: Option<&Arc<dyn ScratchIoBinding>>,
        diagnostics: Option<&quanta_index_core::DiagnosticContextV1>,
    ) -> Self {
        let scope: Option<Rc<dyn ScratchIoScope>> =
            binding.map(|binding| Rc::from(binding.enter_scope(diagnostics)));
        let previous = CURRENT_SCOPE.with(|slot| slot.replace(scope.as_ref().map(Rc::downgrade)));
        Self {
            previous,
            _scope: scope,
        }
    }
}

impl Drop for OperationScope {
    fn drop(&mut self) {
        CURRENT_SCOPE.with(|slot| {
            let _replaced = slot.replace(self.previous.take());
        });
    }
}

/// Only the currently active operation may classify a private capacity refusal.
/// Raw FULL/NOMEM/IO and unqualified operations preserve their storage meaning.
#[must_use]
pub fn storage_error(error: &rusqlite::Error, context: &str) -> IndexError {
    if let rusqlite::Error::SqliteFailure(failure, _) = error
        && failure.code == rusqlite::ErrorCode::DiskFull
    {
        let scope = CURRENT_SCOPE.with(|slot| slot.borrow().as_ref().and_then(Weak::upgrade));
        if scope.is_some_and(|scope| scope.is_capacity_refusal(failure.extended_code)) {
            return IndexError::ResourceLimited;
        }
    }
    IndexError::Storage(format!("{context}: {error}"))
}

pub struct ScratchConnection {
    connection: Option<Connection>,
    binding: Option<Arc<dyn ScratchIoBinding>>,
    namespace_retention: Option<Arc<dyn Send + Sync>>,
}

impl ScratchConnection {
    pub fn open(
        path: &Path,
        allocation: &dyn ScratchAllocation,
        budget: &WorkBudget,
    ) -> Result<Self, IndexError> {
        let binding = allocation.io_binding();
        let selection = binding.as_ref().map(|binding| binding.sqlite_vfs());
        let namespace_retention = if matches!(selection, Some(ScratchSqliteVfs::Stock)) {
            if path.parent() != Some(allocation.path())
                || !matches!(
                    path.components().next_back(),
                    Some(std::path::Component::Normal(_))
                )
            {
                return Err(IndexError::InvalidInput(
                    "stock scratch database must be inside its allocation".into(),
                ));
            }
            Some(allocation.retain_namespace().ok_or_else(|| {
                IndexError::InvalidInput("stock scratch VFS requires a namespace lease".into())
            })?)
        } else {
            None
        };
        let _scope = OperationScope::enter(binding.as_ref(), budget.request().diagnostic_context());
        let flags = OpenFlags::default() | OpenFlags::SQLITE_OPEN_NOFOLLOW;
        let connection = match selection {
            Some(ScratchSqliteVfs::Named(name)) => {
                if name.is_empty() {
                    return Err(IndexError::InvalidInput("empty scratch file driver".into()));
                }
                Connection::open_with_flags_and_vfs(path, flags, name)
            }
            Some(ScratchSqliteVfs::Stock) | None => Connection::open_with_flags(path, flags),
        }
        .map_err(|error| storage_error(&error, "open scratch connection"))?;
        Ok(Self {
            connection: Some(connection),
            binding,
            namespace_retention,
        })
    }

    fn open_connection(&self) -> Result<&Connection, IndexError> {
        self.connection
            .as_ref()
            .ok_or_else(|| IndexError::BackendViolation("scratch connection is closed".into()))
    }

    /// Snapshot SQLite's connection-local change counter. This is an in-memory
    /// accessor; database statements must still use `scoped` for I/O custody.
    pub fn change_count(&self) -> Result<u64, IndexError> {
        Ok(self.open_connection()?.total_changes())
    }

    pub fn scoped(&self, budget: &WorkBudget) -> Result<ScratchConnectionScope<'_>, IndexError> {
        Ok(ScratchConnectionScope {
            connection: self.open_connection()?,
            _scope: OperationScope::enter(
                self.binding.as_ref(),
                budget.request().diagnostic_context(),
            ),
        })
    }

    /// Close the SQLite handle under the original binding. On failure, return
    /// the live connection and binding so the owner can retry or quarantine it.
    pub fn close_checked(mut self) -> Result<(), (Self, IndexError)> {
        let Some(connection) = self.connection.take() else {
            return Ok(());
        };
        let result = {
            let _scope = OperationScope::enter(self.binding.as_ref(), None);
            connection.close().map_err(|(connection, error)| {
                (
                    connection,
                    storage_error(&error, "close scratch connection"),
                )
            })
        };
        match result {
            Ok(()) => {
                drop(self.binding.take());
                drop(self.namespace_retention.take());
                Ok(())
            }
            Err((connection, error)) => {
                self.connection = Some(connection);
                Err((self, error))
            }
        }
    }
}

impl Drop for ScratchConnection {
    fn drop(&mut self) {
        // Drop has no live request grant. Keep physical close custody while
        // masking any enclosing observer instead of retaining a stale phase.
        let _scope = OperationScope::enter(self.binding.as_ref(), None);
        if let Some(lease) = self.namespace_retention.take() {
            if let Some(connection) = self.connection.take()
                && let Err((connection, _error)) = connection.close()
            {
                // Close is uncertain. Retain both the SQLite handle and the
                // namespace authority for process lifetime; cleanup must not
                // unlink a namespace that may still have live file handles.
                std::mem::forget(connection);
                std::mem::forget(lease);
                if let Some(binding) = self.binding.take() {
                    std::mem::forget(binding);
                }
            }
        } else {
            drop(self.connection.take());
        }
    }
}

pub struct ScratchConnectionScope<'a> {
    connection: &'a Connection,
    _scope: OperationScope,
}

impl Deref for ScratchConnectionScope<'_> {
    type Target = Connection;

    fn deref(&self) -> &Self::Target {
        self.connection
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    const PRIVATE_FULL: i32 = rusqlite::ffi::SQLITE_FULL | (127 << 8);

    fn test_budget() -> WorkBudget {
        WorkBudget::with_request(
            quanta_index_core::index::WorkLimits {
                reads: 100,
                writes: 100,
                bytes: 1_048_576,
                mutations: 100,
            },
            quanta_index_core::RequestBudgetV1::unbounded(),
        )
    }

    fn real_tempdir() -> std::io::Result<tempfile::TempDir> {
        // macOS /tmp is a symlink to /private/tmp. SQLite NOFOLLOW tests need
        // a real parent so positive opens do not fail before the target check.
        let root = std::fs::canonicalize(std::env::temp_dir())?;
        tempfile::Builder::new()
            .prefix("quanta-scratch-")
            .tempdir_in(root)
    }

    #[derive(Debug)]
    struct Binding {
        name: &'static str,
        stock_vfs: bool,
        live: Arc<AtomicUsize>,
        entries: Option<Arc<AtomicUsize>>,
    }
    struct Scope(Arc<AtomicUsize>);
    #[derive(Debug)]
    struct NamespaceLease;
    impl quanta_index_core::index::ScratchNamespaceLease for NamespaceLease {
        fn namespace_identity(&self) -> quanta_index_core::publication_capacity::InodeKey {
            quanta_index_core::publication_capacity::InodeKey {
                device: 0,
                inode: 0,
            }
        }
    }
    impl ScratchIoScope for Scope {
        fn is_capacity_refusal(&self, code: i32) -> bool {
            code == PRIVATE_FULL
        }
    }
    impl Drop for Scope {
        fn drop(&mut self) {
            let _previous = self.0.fetch_sub(1, Ordering::SeqCst);
        }
    }
    impl ScratchIoBinding for Binding {
        fn driver_name(&self) -> &str {
            self.name
        }
        fn sqlite_vfs(&self) -> ScratchSqliteVfs<'_> {
            if self.stock_vfs {
                ScratchSqliteVfs::Stock
            } else {
                ScratchSqliteVfs::Named(self.driver_name())
            }
        }
        fn enter_scope(
            &self,
            _: Option<&quanta_index_core::DiagnosticContextV1>,
        ) -> Box<dyn ScratchIoScope> {
            let _previous = self.live.fetch_add(1, Ordering::SeqCst);
            if let Some(entries) = self.entries.as_ref() {
                let _previous = entries.fetch_add(1, Ordering::SeqCst);
            }
            Box::new(Scope(Arc::clone(&self.live)))
        }
        fn acquire_namespace(
            &self,
            _: &Path,
        ) -> Result<Box<dyn quanta_index_core::index::ScratchNamespaceLease>, IndexError> {
            Ok(Box::new(NamespaceLease))
        }
        fn retire_namespace(
            &self,
            _: &Path,
            _budget: Option<&mut quanta_index_core::index::WorkBudget>,
        ) -> Result<(), IndexError> {
            Ok(())
        }
    }
    struct Allocation {
        path: std::path::PathBuf,
        binding: Option<Arc<dyn ScratchIoBinding>>,
        retention: Option<Arc<Retention>>,
    }
    struct Retention(Arc<AtomicUsize>);
    impl Drop for Retention {
        fn drop(&mut self) {
            let _prior = self.0.fetch_add(1, Ordering::SeqCst);
        }
    }
    impl ScratchAllocation for Allocation {
        fn path(&self) -> &Path {
            &self.path
        }
        fn max_bytes(&self) -> u64 {
            1_048_576
        }
        fn reserved_bytes(&self) -> Result<u64, IndexError> {
            Ok(1_048_576)
        }
        fn reserve_at_least(&self, _: u64, _: &mut WorkBudget) -> Result<(), IndexError> {
            Ok(())
        }
        fn io_binding(&self) -> Option<Arc<dyn ScratchIoBinding>> {
            self.binding.clone()
        }
        fn retain_namespace(&self) -> Option<Arc<dyn Send + Sync>> {
            self.retention
                .as_ref()
                .map(|retention| Arc::clone(retention) as Arc<dyn Send + Sync>)
        }
    }
    fn require(value: bool) -> Result<(), IndexError> {
        if value {
            Ok(())
        } else {
            Err(IndexError::BackendViolation(
                "scratch connection invariant".into(),
            ))
        }
    }
    fn full(code: i32) -> rusqlite::Error {
        rusqlite::Error::SqliteFailure(rusqlite::ffi::Error::new(code), None)
    }

    #[test]
    fn change_count_observes_sql_changes_without_entering_io_scope()
    -> Result<(), Box<dyn std::error::Error>> {
        let work = test_budget();
        let directory = real_tempdir()?;
        let entries = Arc::new(AtomicUsize::new(0));
        let allocation = Allocation {
            path: directory.path().to_path_buf(),
            retention: None,
            binding: Some(Arc::new(Binding {
                name: "unix",
                stock_vfs: false,
                live: Arc::new(AtomicUsize::new(0)),
                entries: Some(Arc::clone(&entries)),
            })),
        };
        let connection =
            ScratchConnection::open(&allocation.path().join("count.sqlite"), &allocation, &work)?;
        require(connection.change_count()? == 0 && entries.load(Ordering::SeqCst) == 1)?;
        connection.scoped(&work)?.execute_batch(
            "CREATE TABLE observations(v); INSERT INTO observations VALUES(1),(2);",
        )?;
        require(connection.change_count()? == 2 && entries.load(Ordering::SeqCst) == 2)?;
        Ok(())
    }

    #[test]
    fn missing_named_driver_never_falls_back_or_creates_database()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = real_tempdir()?;
        let live = Arc::new(AtomicUsize::new(0));
        let allocation = Allocation {
            path: directory.path().to_path_buf(),
            retention: None,
            binding: Some(Arc::new(Binding {
                name: "quanta-absent-scratch-driver",
                stock_vfs: false,
                live: Arc::clone(&live),
                entries: None,
            })),
        };
        let file = allocation.path().join("denied.sqlite");
        require(matches!(
            ScratchConnection::open(&file, &allocation, &test_budget()),
            Err(IndexError::Storage(_))
        ))?;
        require(!file.exists() && live.load(Ordering::SeqCst) == 0)?;
        Ok(())
    }

    #[test]
    fn default_vfs_keeps_binding_through_attach_and_checked_close()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = real_tempdir()?;
        let live = Arc::new(AtomicUsize::new(0));
        let entries = Arc::new(AtomicUsize::new(0));
        let retired = Arc::new(AtomicUsize::new(0));
        let allocation = Allocation {
            path: directory.path().to_path_buf(),
            retention: Some(Arc::new(Retention(Arc::clone(&retired)))),
            binding: Some(Arc::new(Binding {
                name: "quanta-absent-scratch-driver",
                stock_vfs: true,
                live: Arc::clone(&live),
                entries: Some(Arc::clone(&entries)),
            })),
        };
        let main = allocation.path().join("main.sqlite");
        let attached = allocation.path().join("attached.sqlite");
        let connection = ScratchConnection::open(&main, &allocation, &test_budget())?;
        drop(allocation);
        require(retired.load(Ordering::SeqCst) == 0)?;
        {
            let scope = connection.scoped(&test_budget())?;
            let attached_path = attached
                .to_str()
                .ok_or_else(|| std::io::Error::other("temporary path is not UTF-8"))?;
            let _affected = scope.execute("ATTACH DATABASE ?1 AS aux", [attached_path])?;
            scope.execute_batch(
                "CREATE TABLE aux.rows(value INTEGER); INSERT INTO aux.rows VALUES(7);",
            )?;
            require(
                scope.query_row("SELECT value FROM aux.rows", [], |row| row.get::<_, i64>(0))? == 7,
            )?;
            scope.execute_batch("DETACH DATABASE aux")?;
        }
        require(live.load(Ordering::SeqCst) == 0 && entries.load(Ordering::SeqCst) == 2)?;
        connection
            .close_checked()
            .map_err(|(_connection, error)| error)?;
        require(live.load(Ordering::SeqCst) == 0 && entries.load(Ordering::SeqCst) == 3)?;
        require(retired.load(Ordering::SeqCst) == 1)?;
        require(main.exists() && attached.exists())?;
        Ok(())
    }

    #[test]
    fn unbound_open_retains_legacy_stock_vfs() -> Result<(), Box<dyn std::error::Error>> {
        let directory = real_tempdir()?;
        let allocation = Allocation {
            path: directory.path().to_path_buf(),
            binding: None,
            retention: None,
        };
        let file = directory.path().join("unqualified.sqlite");
        let connection = ScratchConnection::open(&file, &allocation, &test_budget())?;
        connection
            .close_checked()
            .map_err(|(_connection, error)| error)?;
        require(file.exists())?;
        Ok(())
    }

    #[test]
    fn bound_stock_vfs_requires_namespace_retention_before_open()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = real_tempdir()?;
        let live = Arc::new(AtomicUsize::new(0));
        let allocation = Allocation {
            path: directory.path().to_path_buf(),
            binding: Some(Arc::new(Binding {
                name: "quanta-absent-scratch-driver",
                stock_vfs: true,
                live: Arc::clone(&live),
                entries: None,
            })),
            retention: None,
        };
        let file = directory.path().join("denied.sqlite");
        require(matches!(
            ScratchConnection::open(&file, &allocation, &test_budget()),
            Err(IndexError::InvalidInput(_))
        ))?;
        require(!file.exists() && live.load(Ordering::SeqCst) == 0)?;
        Ok(())
    }

    #[test]
    fn stock_busy_close_keeps_namespace_retention() -> Result<(), Box<dyn std::error::Error>> {
        let directory = real_tempdir()?;
        let retired = Arc::new(AtomicUsize::new(0));
        let allocation = Allocation {
            path: directory.path().to_path_buf(),
            binding: Some(Arc::new(Binding {
                name: "quanta-absent-scratch-driver",
                stock_vfs: true,
                live: Arc::new(AtomicUsize::new(0)),
                entries: None,
            })),
            retention: Some(Arc::new(Retention(Arc::clone(&retired)))),
        };
        let connection = ScratchConnection::open(
            &directory.path().join("busy.sqlite"),
            &allocation,
            &test_budget(),
        )?;
        drop(allocation);
        let scope = connection.scoped(&test_budget())?;
        let statement = scope.prepare("SELECT 1")?;
        std::mem::forget(statement); // Simulate an unfinalized SQLite statement.
        drop(scope);
        let Err((connection, error)) = connection.close_checked() else {
            return Err(std::io::Error::other("busy SQLite connection closed").into());
        };
        require(matches!(error, IndexError::Storage(_)))?;
        require(retired.load(Ordering::SeqCst) == 0)?;
        drop(connection); // An uncertain Drop must keep the namespace owner.
        require(retired.load(Ordering::SeqCst) == 0)?;
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn named_stock_driver_refuses_symlinked_scratch_database()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = real_tempdir()?;
        require(directory.path().canonicalize()?.as_path() == directory.path())?;
        let real = directory.path().join("real.sqlite");
        rusqlite::Connection::open(&real)?.execute_batch("CREATE TABLE rows(value INTEGER)")?;
        let link = directory.path().join("linked.sqlite");
        std::os::unix::fs::symlink(&real, &link)?;
        let allocation = Allocation {
            path: directory.path().to_path_buf(),
            retention: None,
            binding: Some(Arc::new(Binding {
                name: "unix",
                stock_vfs: false,
                live: Arc::new(AtomicUsize::new(0)),
                entries: None,
            })),
        };
        require(matches!(
            ScratchConnection::open(&link, &allocation, &test_budget()),
            Err(IndexError::Storage(_))
        ))?;
        require(std::fs::symlink_metadata(&link)?.file_type().is_symlink())?;
        Ok(())
    }

    #[test]
    fn unqualified_nested_operation_masks_and_restores_capacity_witness()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = real_tempdir()?;
        let live = Arc::new(AtomicUsize::new(0));
        let binding: Arc<dyn ScratchIoBinding> = Arc::new(Binding {
            name: "unix",
            stock_vfs: false,
            live: Arc::clone(&live),
            entries: None,
        });
        let outer = OperationScope::enter(Some(&binding), None);
        require(matches!(
            storage_error(&full(PRIVATE_FULL), "fixture"),
            IndexError::ResourceLimited
        ))?;
        let allocation = Allocation {
            path: directory.path().to_path_buf(),
            binding: None,
            retention: None,
        };
        let connection = ScratchConnection::open(
            &allocation.path().join("plain.sqlite"),
            &allocation,
            &test_budget(),
        )?;
        {
            let scope = connection.scoped(&test_budget())?;
            scope.execute_batch("CREATE TABLE t(v);")?;
            require(matches!(
                storage_error(&full(PRIVATE_FULL), "fixture"),
                IndexError::Storage(_)
            ))?;
        }
        require(matches!(
            storage_error(&full(PRIVATE_FULL), "fixture"),
            IndexError::ResourceLimited
        ))?;
        require(matches!(
            storage_error(&full(rusqlite::ffi::SQLITE_FULL), "fixture"),
            IndexError::Storage(_)
        ))?;
        drop(connection);
        drop(outer);
        require(live.load(Ordering::SeqCst) == 0)?;
        Ok(())
    }
}
