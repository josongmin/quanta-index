//! Scoped native I/O custody around every Catalog connection operation.
//!
//! The composition owner registers the named VFS before construction. Catalog
//! keeps that owner through initialization, each connection lock, and physical
//! connection close. An absent owner is an explicitly unqualified connection.

use std::{
    cell::RefCell,
    ops::{Deref, DerefMut},
    path::Path,
    rc::{Rc, Weak},
    sync::{Arc, Mutex, MutexGuard},
};

use quanta_index_core::index::{IndexError, MemoryLease, ObjectReadFile};
use quanta_index_core::{
    CoreError, DiagnosticContextV1, DiagnosticHealthCodeV1, DiagnosticLaneV1,
    DiagnosticSpanGuardV1, DiagnosticStageV1, DiagnosticTerminalV1, DiagnosticTraceErrorV1,
};
use rusqlite::Connection;

mod close_connection;

pub(crate) fn catalog_lock_span(
    diagnostics: Option<&DiagnosticContextV1>,
    stage: DiagnosticStageV1,
) -> Option<DiagnosticSpanGuardV1> {
    catalog_work_span(diagnostics, stage, DiagnosticLaneV1(0))
}

pub(crate) fn catalog_work_span(
    diagnostics: Option<&DiagnosticContextV1>,
    stage: DiagnosticStageV1,
    lane: DiagnosticLaneV1,
) -> Option<DiagnosticSpanGuardV1> {
    let context = diagnostics?;
    context.enter(stage, lane).map_or_else(
        |error| {
            context.health(match error {
                DiagnosticTraceErrorV1::SpanIdentityExhausted => {
                    DiagnosticHealthCodeV1::CounterOverflow
                }
                DiagnosticTraceErrorV1::ClockRegressed => DiagnosticHealthCodeV1::ClockRegressed,
                DiagnosticTraceErrorV1::ForeignCaptureSpan => {
                    DiagnosticHealthCodeV1::ForeignCaptureSpan
                }
                DiagnosticTraceErrorV1::MissingParentSpan => {
                    DiagnosticHealthCodeV1::MissingParentSpan
                }
            });
            None
        },
        Some,
    )
}

struct CatalogLockHoldTrace(Option<ObservedHold>);

struct ObservedHold {
    span: Option<DiagnosticSpanGuardV1>,
}

impl CatalogLockHoldTrace {
    fn begin(diagnostics: Option<&DiagnosticContextV1>) -> Self {
        let observed = diagnostics.and_then(|context| {
            catalog_lock_span(Some(context), DiagnosticStageV1::CatalogLockHold)
                .map(|span| ObservedHold { span: Some(span) })
        });
        Self(observed)
    }
}

impl Drop for CatalogLockHoldTrace {
    fn drop(&mut self) {
        if let Some(mut observed) = self.0.take()
            && let Some(span) = observed.span.take()
        {
            if std::thread::panicking() {
                // An unfinished span records Aborted after the mutex releases.
                drop(span);
            } else {
                let _observed = span.finish(DiagnosticTerminalV1::Ok);
            }
        }
    }
}

/// A thread-local operation scope supplied by the registered native VFS.
///
/// Implementations distinguish an actual admission refusal from native disk
/// exhaustion using the engine error and their same-operation witness. A
/// string, an old refusal, or an unscoped error cannot authorize classification.
pub trait CatalogIoScope {
    fn is_capacity_refusal(&self, error: &rusqlite::Error) -> bool;
}

/// Lifetime owner for Catalog operation scopes and read-session lock files.
/// Existing named-VFS opens use `vfs_name`; explicit stock-VFS opens retain
/// this owner but do not receive its native per-file SQLite callbacks.
pub trait CatalogIoAdmission: Send + Sync {
    fn vfs_name(&self) -> &str;
    /// Entering is infallible after registration. The returned scope is local
    /// to this thread; it must restore any enclosing scope on drop.
    /// Optional request trace metadata; never admission or funding authority.
    /// None must mask any enclosing diagnostic request.
    fn enter_scope(
        &self,
        diagnostics: Option<&quanta_index_core::DiagnosticContextV1>,
    ) -> Box<dyn CatalogIoScope>;
    /// Keep selected close/checkpoint funding through the actual native close.
    fn enter_close_scope(&self) -> Box<dyn CatalogIoScope>;
    /// Admit a regular read-session lock before its exclusive creation/open.
    /// The returned owner must hold the real FD through physical close. A
    /// qualified Catalog never falls back to a raw file on refusal.
    fn open_lock_file(
        &self,
        path: &std::path::Path,
        create_new: bool,
    ) -> Result<Box<dyn ObjectReadFile>, IndexError>;
    /// Delete the linked inode matching the held FD, sync its parent and keep
    /// its charge until every real admitted reader has closed.
    fn unlink_lock_file(
        &self,
        path: &std::path::Path,
        held: &std::fs::File,
    ) -> Result<(), IndexError>;
}

/// Existing-root I/O with native mutation refusal. A selected protected route
/// holds source descriptors and uses either an FD reader or a private offline
/// Main/WAL copy. A named reader uses its admitted VFS scope instead.
/// The root remains held through connection close and the original request is
/// checked during WAL recovery, before SQL VM progress callbacks can run.
pub trait CatalogReadOnlyIoAdmission: Send + Sync {
    /// Legacy named readers enter a VFS scope before opening a connection.
    /// A provider that requires the protected FD route has no named fallback.
    fn enter_named_read_only_scope(
        &self,
        _request: &quanta_index_core::RequestBudgetV1,
        _main_bytes: u64,
        _wal_bytes: u64,
    ) -> Option<(&str, Box<dyn CatalogIoScope>)> {
        None
    }

    /// A provider that selects the FD reader must anchor the directory in its
    /// own root lease and hold the admitted FD envelope through `visit`. It
    /// calls the visitor exactly once and returns true only after checking the
    /// live namespace and retiring the directory. False declines this route
    /// without calling the visitor. Errors refuse the selected route.
    fn with_protected_catalog_directory_v1(
        &self,
        _directory: &std::path::Path,
        _request: &quanta_index_core::RequestBudgetV1,
        _needs_wal: bool,
        _visit: &mut dyn FnMut(&std::fs::File),
    ) -> Result<bool, IndexError> {
        Ok(false)
    }

    /// Retain authority after the FD reader could not witness retirement.
    fn poison_protected_catalog_close_v1(&self) {}
}

thread_local! {
    static CURRENT_SCOPE: RefCell<Option<Weak<dyn CatalogIoScope>>> = RefCell::new(None);
}

pub(crate) struct ScopeLease {
    previous: Option<Weak<dyn CatalogIoScope>>,
    // The backend scope is released after restoring the Catalog scope stack.
    _scope: Option<Rc<dyn CatalogIoScope>>,
}

impl ScopeLease {
    pub(crate) fn enter_read_only(scope: Box<dyn CatalogIoScope>) -> Self {
        let scope: Rc<dyn CatalogIoScope> = Rc::from(scope);
        let previous = CURRENT_SCOPE.with(|slot| slot.replace(Some(Rc::downgrade(&scope))));
        Self {
            previous,
            _scope: Some(scope),
        }
    }

    pub(crate) fn enter(
        owner: Option<&Arc<dyn CatalogIoAdmission>>,
        diagnostics: Option<&quanta_index_core::DiagnosticContextV1>,
    ) -> Self {
        let scope: Option<Rc<dyn CatalogIoScope>> =
            owner.map(|owner| Rc::from(owner.enter_scope(diagnostics)));
        // Explicitly unqualified work masks an enclosing qualified operation.
        let previous = CURRENT_SCOPE.with(|slot| slot.replace(scope.as_ref().map(Rc::downgrade)));
        Self {
            previous,
            _scope: scope,
        }
    }

    fn enter_close(owner: Option<&Arc<dyn CatalogIoAdmission>>) -> Self {
        let scope: Option<Rc<dyn CatalogIoScope>> =
            owner.map(|owner| Rc::from(owner.enter_close_scope()));
        let previous = CURRENT_SCOPE.with(|slot| slot.replace(scope.as_ref().map(Rc::downgrade)));
        Self {
            previous,
            _scope: scope,
        }
    }
}

impl Drop for ScopeLease {
    fn drop(&mut self) {
        CURRENT_SCOPE.with(|slot| {
            let _replaced = slot.replace(self.previous.take());
        });
    }
}

pub(crate) fn is_capacity_refusal(error: &rusqlite::Error) -> bool {
    if !matches!(error, rusqlite::Error::SqliteFailure(failure, _) if failure.code == rusqlite::ErrorCode::DiskFull)
    {
        return false;
    }
    // Drop the RefCell borrow before invoking backend code.
    let scope = CURRENT_SCOPE.with(|slot| slot.borrow().as_ref().and_then(Weak::upgrade));
    scope.is_some_and(|scope| scope.is_capacity_refusal(error))
}

pub(crate) fn storage(action: &str, path: &Path, error: &dyn std::fmt::Display) -> CoreError {
    CoreError::Storage(format!("catalog: {action} {}: {error}", path.display()))
}

/// Map an engine error to the typed codes the ports promise.
pub(crate) fn engine_error(action: &str, path: &Path, error: &rusqlite::Error) -> CoreError {
    if is_capacity_refusal(error) {
        return CoreError::Typed {
            code: quanta_index_contract::SearchPlaneErrorCodeV2::IngestResourceBudgetExceeded,
            message: format!(
                "catalog: {action} {} exceeded admitted file capacity: {error}",
                path.display()
            ),
        };
    }
    if let rusqlite::Error::SqliteFailure(failure, _) = error
        && matches!(
            failure.code,
            rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked
        )
    {
        return CoreError::Typed {
            code: quanta_index_contract::SearchPlaneErrorCodeV2::CatalogBusy,
            message: format!(
                "catalog: {action} {} met a held lock past the busy budget: {error}",
                path.display()
            ),
        };
    }
    storage(action, path, error)
}

#[cfg(test)]
type CloseOnDrop = fn(Connection) -> Result<(), (Connection, rusqlite::Error)>;

pub(crate) struct CatalogConnectionOwner {
    connection: Option<Mutex<Connection>>,
    admission: Option<Arc<dyn CatalogIoAdmission>>,
    cache_memory: Option<MemoryLease>,
    #[cfg(test)]
    close_on_drop: Option<CloseOnDrop>,
}

impl CatalogConnectionOwner {
    pub(crate) fn new(
        connection: Connection,
        admission: Option<Arc<dyn CatalogIoAdmission>>,
    ) -> Self {
        Self {
            connection: Some(Mutex::new(connection)),
            admission,
            cache_memory: None,
            #[cfg(test)]
            close_on_drop: None,
        }
    }

    pub(crate) fn new_with_cache(
        connection: Connection,
        admission: Arc<dyn CatalogIoAdmission>,
        cache_memory: MemoryLease,
    ) -> Self {
        Self {
            connection: Some(Mutex::new(connection)),
            admission: Some(admission),
            cache_memory: Some(cache_memory),
            #[cfg(test)]
            close_on_drop: None,
        }
    }

    pub(crate) fn initialization_connection(&mut self) -> Result<&mut Connection, CoreError> {
        self.connection
            .as_mut()
            .ok_or_else(|| CoreError::InvalidContract("catalog connection already closed".into()))?
            .get_mut()
            .map_err(|error| {
                CoreError::Storage(format!("catalog initialization mutex poisoned: {error}"))
            })
    }

    pub(crate) fn mutex(&self) -> Result<&Mutex<Connection>, CoreError> {
        self.connection
            .as_ref()
            .ok_or_else(|| CoreError::InvalidContract("catalog connection already closed".into()))
    }

    pub(crate) fn admission(&self) -> Option<&Arc<dyn CatalogIoAdmission>> {
        self.admission.as_ref()
    }

    pub(crate) fn scoped<'a>(
        &self,
        guard: MutexGuard<'a, Connection>,
        diagnostics: Option<&quanta_index_core::DiagnosticContextV1>,
    ) -> CatalogConnectionGuard<'a> {
        // Start the lock-hold span before entering the native scope. Field
        // drop order finishes it after the connection mutex is released.
        let hold = CatalogLockHoldTrace::begin(diagnostics);
        CatalogConnectionGuard {
            guard,
            _hold: hold,
            _scope: ScopeLease::enter(self.admission.as_ref(), diagnostics),
        }
    }

    pub(crate) fn close_checked(&mut self) -> Result<(), rusqlite::Error> {
        self.close_checked_with(std::convert::identity)
    }

    pub(crate) fn close_checked_with<E>(
        &mut self,
        map_error: impl FnOnce(rusqlite::Error) -> E,
    ) -> Result<(), E> {
        self.close_checked_with_close(Connection::close, map_error)
    }

    fn close_checked_with_close<E>(
        &mut self,
        close: impl FnOnce(Connection) -> Result<(), (Connection, rusqlite::Error)>,
        map_error: impl FnOnce(rusqlite::Error) -> E,
    ) -> Result<(), E> {
        let result = close_connection::close_connection_with_scope(
            &mut self.connection,
            || ScopeLease::enter_close(self.admission.as_ref()),
            close,
            map_error,
        );
        if result.is_ok() {
            let _released = self.cache_memory.take();
        }
        result
    }

    fn quarantine_failed_cache_close(&mut self) {
        // Keep the native handle and its admission/funding authority together.
        std::mem::forget((
            self.connection.take(),
            self.admission.take(),
            self.cache_memory.take(),
        ));
    }

    #[cfg(test)]
    fn close_for_drop(&mut self) -> Result<(), rusqlite::Error> {
        if let Some(close) = self.close_on_drop.take() {
            self.close_checked_with_close(close, std::convert::identity)
        } else {
            self.close_checked()
        }
    }

    #[cfg(not(test))]
    fn close_for_drop(&mut self) -> Result<(), rusqlite::Error> {
        self.close_checked()
    }
}

impl Drop for CatalogConnectionOwner {
    fn drop(&mut self) {
        // The explicit owner should have handled close failure. Destruction
        // retains the former scoped, best-effort fallback for legacy callers.
        if self.close_for_drop().is_err() {
            if self.cache_memory.is_some() {
                // Native close failed with its original handle restored. A
                // later implicit rusqlite Drop can fail silently, so retain
                // the handle, named VFS authority, and memory charge together.
                self.quarantine_failed_cache_close();
                return;
            }
            let _scope = ScopeLease::enter_close(self.admission.as_ref());
            drop(self.connection.take());
        }
    }
}

pub(crate) struct CatalogConnectionGuard<'a> {
    // Rust drops fields in declaration order: release the mutex, finish the
    // hold span, then restore the VFS scope. The latter is not lock tenure.
    guard: MutexGuard<'a, Connection>,
    _hold: CatalogLockHoldTrace,
    _scope: ScopeLease,
}

impl Deref for CatalogConnectionGuard<'_> {
    type Target = Connection;

    fn deref(&self) -> &Self::Target {
        &self.guard
    }
}

impl DerefMut for CatalogConnectionGuard<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.guard
    }
}

#[cfg(test)]
mod tests {
    use std::{
        path::PathBuf,
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
        time::Duration,
    };

    use rusqlite::{Connection, Error, ffi};

    use super::{
        CatalogConnectionOwner, CatalogIoAdmission, CatalogIoScope, ScopeLease, is_capacity_refusal,
    };
    use crate::{SqliteCatalog, SystemCatalogClock};
    use quanta_index_core::index::{MemoryLease, MemoryLeaseOwner};

    macro_rules! check {
        ($condition:expr $(,)?) => {{
            if !$condition {
                return Err(format!("assertion failed: {}", stringify!($condition)).into());
            }
        }};
    }

    macro_rules! check_eq {
        ($left:expr, $right:expr $(,)?) => {{
            let left = &$left;
            let right = &$right;
            if left != right {
                return Err(format!("assertion failed: {left:?} != {right:?}").into());
            }
        }};
    }

    struct CacheLeaseCount(Arc<AtomicUsize>);

    impl MemoryLeaseOwner for CacheLeaseCount {
        fn release(&self, _bytes: u64) {
            let _previous = self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    fn cache_lease(counter: &Arc<AtomicUsize>) -> MemoryLease {
        MemoryLease::new(1024, Arc::new(CacheLeaseCount(Arc::clone(counter))))
    }

    const TEST_CAPACITY_CODE: i32 = ffi::SQLITE_FULL | (0x51 << 8);

    struct Admission;
    struct Scope;

    impl CatalogIoScope for Scope {
        fn is_capacity_refusal(&self, error: &Error) -> bool {
            matches!(error, Error::SqliteFailure(failure, _) if failure.extended_code == TEST_CAPACITY_CODE)
        }
    }

    impl CatalogIoAdmission for Admission {
        fn vfs_name(&self) -> &'static str {
            "quanta-index-test-missing-vfs"
        }

        fn enter_scope(
            &self,
            _: Option<&quanta_index_core::DiagnosticContextV1>,
        ) -> Box<dyn CatalogIoScope> {
            Box::new(Scope)
        }
        fn enter_close_scope(&self) -> Box<dyn CatalogIoScope> {
            Box::new(Scope)
        }

        fn open_lock_file(
            &self,
            _: &std::path::Path,
            _: bool,
        ) -> Result<
            Box<dyn quanta_index_core::index::ObjectReadFile>,
            quanta_index_core::index::IndexError,
        > {
            Err(quanta_index_core::index::IndexError::BackendViolation(
                "scope-only fixture cannot open a physical lock".into(),
            ))
        }

        fn unlink_lock_file(
            &self,
            _: &std::path::Path,
            _: &std::fs::File,
        ) -> Result<(), quanta_index_core::index::IndexError> {
            Err(quanta_index_core::index::IndexError::BackendViolation(
                "scope-only fixture cannot delete a physical lock".into(),
            ))
        }
    }

    struct CloseTrackingAdmission {
        entries: Arc<AtomicUsize>,
        active: Arc<AtomicUsize>,
    }

    struct CloseTrackingScope(Arc<AtomicUsize>);

    impl Drop for CloseTrackingScope {
        fn drop(&mut self) {
            let _previous = self.0.fetch_sub(1, Ordering::SeqCst);
        }
    }

    impl CatalogIoScope for CloseTrackingScope {
        fn is_capacity_refusal(&self, _: &Error) -> bool {
            false
        }
    }

    impl CatalogIoAdmission for CloseTrackingAdmission {
        fn vfs_name(&self) -> &'static str {
            "close-scope-fixture"
        }

        fn enter_scope(
            &self,
            _: Option<&quanta_index_core::DiagnosticContextV1>,
        ) -> Box<dyn CatalogIoScope> {
            Box::new(Scope)
        }

        fn enter_close_scope(&self) -> Box<dyn CatalogIoScope> {
            let _previous = self.entries.fetch_add(1, Ordering::SeqCst);
            let _previous = self.active.fetch_add(1, Ordering::SeqCst);
            Box::new(CloseTrackingScope(Arc::clone(&self.active)))
        }

        fn open_lock_file(
            &self,
            _: &std::path::Path,
            _: bool,
        ) -> Result<
            Box<dyn quanta_index_core::index::ObjectReadFile>,
            quanta_index_core::index::IndexError,
        > {
            Err(quanta_index_core::index::IndexError::BackendViolation(
                "close-only fixture cannot open a lock".into(),
            ))
        }

        fn unlink_lock_file(
            &self,
            _: &std::path::Path,
            _: &std::fs::File,
        ) -> Result<(), quanta_index_core::index::IndexError> {
            Err(quanta_index_core::index::IndexError::BackendViolation(
                "close-only fixture cannot delete a lock".into(),
            ))
        }
    }

    fn engine_error(code: i32) -> Error {
        Error::SqliteFailure(ffi::Error::new(code), None)
    }

    #[test]
    fn failed_checked_close_keeps_funded_connection_then_success_refunds_once()
    -> Result<(), Box<dyn std::error::Error>> {
        let released = Arc::new(AtomicUsize::new(0));
        let entries = Arc::new(AtomicUsize::new(0));
        let active = Arc::new(AtomicUsize::new(0));
        let admission: Arc<dyn CatalogIoAdmission> = Arc::new(CloseTrackingAdmission {
            entries: Arc::clone(&entries),
            active: Arc::clone(&active),
        });
        let mut owner = CatalogConnectionOwner::new_with_cache(
            Connection::open_in_memory()?,
            admission,
            cache_lease(&released),
        );
        let failed = owner.close_checked_with_close(
            |connection| Err((connection, Error::InvalidQuery)),
            std::convert::identity,
        );
        check!(matches!(failed, Err(Error::InvalidQuery)));
        check!(owner.mutex().is_ok());
        check_eq!(released.load(Ordering::SeqCst), 0);
        check_eq!(active.load(Ordering::SeqCst), 0);
        owner.close_checked()?;
        check!(owner.mutex().is_err());
        check_eq!(released.load(Ordering::SeqCst), 1);
        owner.close_checked()?;
        check_eq!(released.load(Ordering::SeqCst), 1);
        check_eq!(entries.load(Ordering::SeqCst), 2);
        Ok(())
    }

    #[test]
    fn failed_close_quarantine_keeps_connection_authority_and_charge()
    -> Result<(), Box<dyn std::error::Error>> {
        let released = Arc::new(AtomicUsize::new(0));
        let entries = Arc::new(AtomicUsize::new(0));
        let active = Arc::new(AtomicUsize::new(0));
        let admission: Arc<dyn CatalogIoAdmission> = Arc::new(CloseTrackingAdmission {
            entries: Arc::clone(&entries),
            active: Arc::clone(&active),
        });
        let weak = Arc::downgrade(&admission);
        let mut owner = CatalogConnectionOwner::new_with_cache(
            Connection::open_in_memory()?,
            admission,
            cache_lease(&released),
        );
        owner.close_on_drop = Some(|connection| Err((connection, Error::InvalidQuery)));
        drop(owner);
        check!(weak.upgrade().is_some());
        check_eq!(released.load(Ordering::SeqCst), 0);
        check_eq!(active.load(Ordering::SeqCst), 0);
        check_eq!(entries.load(Ordering::SeqCst), 1);
        Ok(())
    }

    #[test]
    fn native_disk_full_and_unscoped_private_code_remain_storage_errors() {
        let native_full = engine_error(ffi::SQLITE_FULL);
        let configured_full = engine_error(TEST_CAPACITY_CODE);
        assert!(!is_capacity_refusal(&configured_full));
        let owner: Arc<dyn CatalogIoAdmission> = Arc::new(Admission);
        let _scope = ScopeLease::enter(Some(&owner), None);
        assert!(!is_capacity_refusal(&native_full));
        assert!(is_capacity_refusal(&configured_full));
        assert!(!is_capacity_refusal(&Error::InvalidQuery));
    }

    #[test]
    fn unqualified_nested_catalog_scope_cannot_borrow_outer_admission() {
        let error = engine_error(TEST_CAPACITY_CODE);
        let owner: Arc<dyn CatalogIoAdmission> = Arc::new(Admission);
        let outer = ScopeLease::enter(Some(&owner), None);
        assert!(is_capacity_refusal(&error));
        {
            let _unqualified = ScopeLease::enter(None, None);
            assert!(!is_capacity_refusal(&error));
        }
        assert!(is_capacity_refusal(&error));
        drop(outer);
        assert!(!is_capacity_refusal(&error));
    }

    #[test]
    fn missing_registered_vfs_refuses_without_default_database_creation()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let result = SqliteCatalog::open_with_io_admission(
            root.path(),
            Duration::from_secs(1),
            Arc::new(Admission),
        );
        if result.is_ok()
            || crate::catalog_dir(root.path())
                .join(crate::CATALOG_FILE_NAME)
                .exists()
            || is_capacity_refusal(&engine_error(TEST_CAPACITY_CODE))
        {
            return Err(std::io::Error::other(
                "missing VFS fell back or leaked its operation scope",
            )
            .into());
        }
        Ok(())
    }

    #[test]
    fn explicitly_selected_stock_vfs_keeps_admission_through_checked_close()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let entries = Arc::new(AtomicUsize::new(0));
        let active = Arc::new(AtomicUsize::new(0));
        // The fixture names no registered VFS. Only the explicit stock entry
        // point may create a database while retaining this admission owner.
        let admission: Arc<dyn CatalogIoAdmission> = Arc::new(CloseTrackingAdmission {
            entries: Arc::clone(&entries),
            active: Arc::clone(&active),
        });
        let weak = Arc::downgrade(&admission);
        let mut catalog = SqliteCatalog::open_with_stock_vfs_and_io_admission(
            root.path(),
            Duration::from_secs(1),
            admission,
        )?;
        assert!(
            crate::catalog_dir(root.path())
                .join(crate::CATALOG_FILE_NAME)
                .is_file()
        );
        assert!(weak.upgrade().is_some());
        let external = tempfile::tempdir()?;
        let attachment = external.path().join("attachment.sqlite");
        {
            let connection = catalog.lock()?;
            let attached_limit =
                connection.limit(rusqlite::limits::Limit::SQLITE_LIMIT_ATTACHED)?;
            let temp_store: i64 =
                connection.pragma_query_value(None, "temp_store", |row| row.get(0))?;
            assert_eq!((attached_limit, temp_store), (0, 2));
            assert!(
                connection
                    .execute(
                        "ATTACH DATABASE ?1 AS external",
                        [attachment.to_str().ok_or("attachment path is not UTF-8")?],
                    )
                    .is_err()
            );
            assert!(!attachment.exists());
            connection.execute_batch(
                "CREATE TEMP TABLE work_key(value BLOB PRIMARY KEY) WITHOUT ROWID;
                 INSERT INTO temp.work_key VALUES (x'1234');",
            )?;
            let key: Vec<u8> =
                connection.query_row("SELECT value FROM temp.work_key", [], |row| row.get(0))?;
            assert_eq!(key, [0x12, 0x34]);
        }
        catalog.close_checked()?;
        assert_eq!(entries.load(Ordering::SeqCst), 1);
        assert_eq!(active.load(Ordering::SeqCst), 0);
        Ok(())
    }

    #[test]
    fn checked_close_balances_one_scope_and_rejects_reuse_after_success()
    -> Result<(), Box<dyn std::error::Error>> {
        let connection = Connection::open_in_memory()?;
        let entries = Arc::new(AtomicUsize::new(0));
        let active = Arc::new(AtomicUsize::new(0));
        let admission: Arc<dyn CatalogIoAdmission> = Arc::new(CloseTrackingAdmission {
            entries: Arc::clone(&entries),
            active: Arc::clone(&active),
        });
        let mut catalog = SqliteCatalog {
            connection: CatalogConnectionOwner::new(connection, Some(admission)),
            path: PathBuf::from(":memory:"),
            clock: Arc::new(SystemCatalogClock),
            writer_cache: None,
            pin_data_version: std::sync::atomic::AtomicI64::new(i64::MIN),
            native_quarantine_schema: std::sync::atomic::AtomicU8::new(0),
            native_source_stage_schema: std::sync::atomic::AtomicBool::new(false),
            transient_source_body_quarantines: std::sync::RwLock::new(
                std::collections::BTreeMap::new(),
            ),
            abandoned_read_pins: std::sync::Mutex::new(Vec::new()),
            maintenance_demand: std::sync::Mutex::new(
                crate::native::MaintenanceDemandState::default(),
            ),
        };
        catalog.close_checked()?;
        if entries.load(Ordering::SeqCst) != 1
            || active.load(Ordering::SeqCst) != 0
            || catalog.connection.mutex().is_ok()
        {
            return Err("checked close did not settle the original connection in one scope".into());
        }
        catalog.close_checked()?;
        if entries.load(Ordering::SeqCst) != 1 {
            return Err("already-closed catalog entered another native close scope".into());
        }
        Ok(())
    }
}
