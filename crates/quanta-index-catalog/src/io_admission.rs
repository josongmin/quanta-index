//! Scoped native I/O custody around every Catalog connection operation.
//!
//! The composition owner registers the named VFS before construction. Catalog
//! keeps that owner through initialization, each connection lock, and physical
//! connection close. An absent owner is an explicitly unqualified connection.

use std::{
    cell::RefCell,
    ops::{Deref, DerefMut},
    rc::{Rc, Weak},
    sync::{Arc, Mutex, MutexGuard},
};

use quanta_index_core::index::{IndexError, ObjectReadFile};
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
    let context = diagnostics?;
    context.enter(stage, DiagnosticLaneV1(0)).map_or_else(
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

struct CatalogLockHoldTrace(Option<DiagnosticSpanGuardV1>);

impl Drop for CatalogLockHoldTrace {
    fn drop(&mut self) {
        if let Some(span) = self.0.take() {
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

/// Lifetime owner for one native VFS and its regular read-session lock files.
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

/// Existing-root I/O with native mutation refusal.
///
/// The VFS denies creation, writes, truncation, deletion,
/// mutating file controls and SHM access before the underlying syscall.
/// The scope owns root custody through the connection's physical close.
/// Main-file opens must acquire the actual `SQLite`-compatible exclusive DB lock
/// before the first header read, including a checkpointed main-only URI open.
/// Verification reads must check this original request even during native WAL
/// recovery, which can run before a SQL VM progress callback is invoked.
pub trait CatalogReadOnlyIoAdmission: Send + Sync {
    fn read_only_vfs_name(&self) -> &str;
    fn enter_read_only_scope(
        &self,
        request: &quanta_index_core::RequestBudgetV1,
    ) -> Box<dyn CatalogIoScope>;
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
    pub(crate) fn enter_read_only(
        owner: &dyn CatalogReadOnlyIoAdmission,
        request: &quanta_index_core::RequestBudgetV1,
    ) -> Self {
        let scope: Rc<dyn CatalogIoScope> = Rc::from(owner.enter_read_only_scope(request));
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

pub(crate) struct CatalogConnectionOwner {
    connection: Option<Mutex<Connection>>,
    admission: Option<Arc<dyn CatalogIoAdmission>>,
}

impl CatalogConnectionOwner {
    pub(crate) fn new(
        connection: Connection,
        admission: Option<Arc<dyn CatalogIoAdmission>>,
    ) -> Self {
        Self {
            connection: Some(Mutex::new(connection)),
            admission,
        }
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
        CatalogConnectionGuard {
            guard,
            _hold: CatalogLockHoldTrace(catalog_lock_span(
                diagnostics,
                DiagnosticStageV1::CatalogLockHold,
            )),
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
        close_connection::close_connection_with_scope(
            &mut self.connection,
            || ScopeLease::enter_close(self.admission.as_ref()),
            map_error,
        )
    }
}

impl Drop for CatalogConnectionOwner {
    fn drop(&mut self) {
        // The explicit owner should have handled close failure. Destruction
        // retains the former scoped, best-effort fallback for legacy callers.
        if self.close_checked().is_err() {
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
