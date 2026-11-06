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

use quanta_index_core::index::{IndexError, MemoryLease, ObjectReadFile};
use quanta_index_core::{
    CoreError, DiagnosticContextV1, DiagnosticCounterV1, DiagnosticHealthCodeV1, DiagnosticLaneV1,
    DiagnosticSpanGuardV1, DiagnosticStageV1, DiagnosticTerminalV1, DiagnosticTraceErrorV1,
};
use rusqlite::{Connection, PagerCacheStatus};

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

struct CatalogLockHoldTrace(Option<ObservedHold>);

struct ObservedHold {
    span: Option<DiagnosticSpanGuardV1>,
    context: DiagnosticContextV1,
    before: rusqlite::Result<PagerCacheStatus>,
    after: Option<rusqlite::Result<PagerCacheStatus>>,
}

fn pager_cache_delta(
    before: rusqlite::Result<PagerCacheStatus>,
    after: Option<rusqlite::Result<PagerCacheStatus>>,
) -> Result<PagerCacheStatus, DiagnosticHealthCodeV1> {
    // Preserve raw snapshot errors until this fixed diagnostic classification.
    // No product error is replaced, and unavailable samples emit health only.
    let (Ok(before), Some(Ok(after))) = (before, after) else {
        return Err(DiagnosticHealthCodeV1::CounterUnavailable);
    };
    let (Some(hit), Some(miss), Some(write), Some(spill)) = (
        after.hit.checked_sub(before.hit),
        after.miss.checked_sub(before.miss),
        after.write.checked_sub(before.write),
        after.spill.checked_sub(before.spill),
    ) else {
        return Err(DiagnosticHealthCodeV1::CounterRegressed);
    };
    Ok(PagerCacheStatus {
        hit,
        miss,
        write,
        spill,
    })
}

impl CatalogLockHoldTrace {
    fn begin(diagnostics: Option<&DiagnosticContextV1>, connection: &Connection) -> Self {
        let observed = diagnostics.and_then(|context| {
            catalog_lock_span(Some(context), DiagnosticStageV1::CatalogLockHold).map(|span| {
                ObservedHold {
                    span: Some(span),
                    context: context.clone(),
                    before: connection.pager_cache_status(),
                    after: None,
                }
            })
        });
        Self(observed)
    }

    fn observe_end(&mut self, connection: &Connection) {
        if let Some(observed) = self.0.as_mut() {
            observed.after = Some(connection.pager_cache_status());
        }
    }
}

impl Drop for CatalogLockHoldTrace {
    fn drop(&mut self) {
        if let Some(mut observed) = self.0.take()
            && let Some(span) = observed.span.take()
        {
            match pager_cache_delta(observed.before, observed.after) {
                Ok(delta) => {
                    for (key, value) in [
                        (DiagnosticCounterV1::SqliteCacheHits, delta.hit),
                        (DiagnosticCounterV1::SqliteCacheMisses, delta.miss),
                        (DiagnosticCounterV1::SqliteCacheWrites, delta.write),
                        (DiagnosticCounterV1::SqliteCacheSpills, delta.spill),
                    ] {
                        observed.context.counter(&span, key, value);
                    }
                }
                Err(health) => observed.context.health(health),
            }
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
        // Both samples belong to this exact mutex-held connection. The first
        // precedes native scope replacement; the second is taken in guard
        // destruction before the mutex releases. Emission waits until after.
        let hold = CatalogLockHoldTrace::begin(diagnostics, &guard);
        CatalogConnectionGuard {
            guard,
            hold,
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
        // Keep the native handle and its named VFS/funding authority together.
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
    hold: CatalogLockHoldTrace,
    _scope: ScopeLease,
}

impl Drop for CatalogConnectionGuard<'_> {
    fn drop(&mut self) {
        // Drop runs before fields are released. Only the raw SQLite snapshot
        // happens here; the hold field emits after the guard field unlocks.
        self.hold.observe_end(&self.guard);
    }
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
            Arc, Mutex,
            atomic::{AtomicU64, AtomicUsize, Ordering},
        },
        time::Duration,
    };

    use rusqlite::{Connection, Error, PagerCacheStatus, ffi};

    use super::{
        CatalogConnectionOwner, CatalogIoAdmission, CatalogIoScope, CatalogLockHoldTrace,
        ObservedHold, ScopeLease, is_capacity_refusal, pager_cache_delta,
    };
    use crate::{SqliteCatalog, SystemCatalogClock};
    use quanta_index_core::{
        DiagnosticCaptureIdV1, DiagnosticClockV1, DiagnosticEmitV1, DiagnosticEventKindV1,
        DiagnosticEventSinkV1, DiagnosticHealthCodeV1, DiagnosticLaneV1, DiagnosticRootKindV1,
        DiagnosticStageV1,
        index::{MemoryLease, MemoryLeaseOwner},
    };

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
    fn pager_cache_delta_requires_two_available_monotonic_snapshots() {
        let before = PagerCacheStatus {
            hit: 11,
            miss: 17,
            write: 23,
            spill: 3,
        };
        let after = PagerCacheStatus {
            hit: 18,
            miss: 19,
            write: 29,
            spill: 3,
        };
        assert_eq!(
            pager_cache_delta(Ok(before), Some(Ok(after))),
            Ok(PagerCacheStatus {
                hit: 7,
                miss: 2,
                write: 6,
                spill: 0,
            })
        );
        assert_eq!(
            pager_cache_delta(Err(Error::InvalidQuery), Some(Ok(after))),
            Err(DiagnosticHealthCodeV1::CounterUnavailable)
        );
        assert_eq!(
            pager_cache_delta(Ok(before), None),
            Err(DiagnosticHealthCodeV1::CounterUnavailable)
        );
        assert_eq!(
            pager_cache_delta(Ok(before), Some(Err(Error::InvalidQuery))),
            Err(DiagnosticHealthCodeV1::CounterUnavailable)
        );
        assert_eq!(
            pager_cache_delta(Ok(before), Some(Ok(PagerCacheStatus { spill: 2, ..after }))),
            Err(DiagnosticHealthCodeV1::CounterRegressed)
        );
    }

    #[test]
    fn unavailable_or_regressed_pager_status_emits_health_without_partial_counters()
    -> Result<(), Box<dyn std::error::Error>> {
        #[derive(Debug, Default)]
        struct TickClock(AtomicU64);
        impl DiagnosticClockV1 for TickClock {
            fn now_ns(&self) -> u64 {
                self.0.fetch_add(1, Ordering::Relaxed)
            }
        }
        #[derive(Debug, Default)]
        struct Events(Mutex<Vec<DiagnosticEmitV1>>);
        impl DiagnosticEventSinkV1 for Events {
            fn try_emit(&self, event: DiagnosticEmitV1) -> bool {
                let mut rows = match self.0.lock() {
                    Ok(rows) => rows,
                    Err(poisoned) => poisoned.into_inner(),
                };
                rows.push(event);
                true
            }
        }
        let first = PagerCacheStatus {
            hit: 10,
            miss: 10,
            write: 10,
            spill: 10,
        };
        let lower = PagerCacheStatus { spill: 9, ..first };
        for (ordinal, before, after, expected) in [
            (
                1,
                Err(Error::InvalidQuery),
                Some(Ok(first)),
                DiagnosticHealthCodeV1::CounterUnavailable,
            ),
            (
                2,
                Ok(first),
                Some(Ok(lower)),
                DiagnosticHealthCodeV1::CounterRegressed,
            ),
        ] {
            let events = Arc::new(Events::default());
            let context = quanta_index_core::DiagnosticContextV1::new(
                DiagnosticCaptureIdV1([ordinal; 16]),
                Arc::new(TickClock::default()),
                events.clone(),
            );
            let root = context.enter_root(DiagnosticRootKindV1::Operation, DiagnosticLaneV1(0))?;
            let child = context.child_context(&root);
            let span = child.enter(DiagnosticStageV1::CatalogLockHold, DiagnosticLaneV1(0))?;
            drop(CatalogLockHoldTrace(Some(ObservedHold {
                span: Some(span),
                context: child,
                before,
                after,
            })));
            let rows = match events.0.lock() {
                Ok(rows) => rows,
                Err(poisoned) => poisoned.into_inner(),
            };
            check_eq!(
                rows.iter()
                    .filter(|event| matches!(event.kind, DiagnosticEventKindV1::Counter { .. }))
                    .count(),
                0
            );
            check_eq!(
                rows.iter()
                    .filter(|event| {
                        matches!(event.kind, DiagnosticEventKindV1::Health { code } if code == expected)
                    })
                    .count(),
                1
            );
            drop(rows);
        }
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
