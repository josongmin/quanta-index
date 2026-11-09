//! The one `SQLite` connection every catalog table shares.
//!
//! One file under `state_root/catalog/`, opened once per process, with the
//! pragmas the G0-C gate requires read back rather than assumed: `WAL`,
//! `synchronous=FULL`, `fullfsync=ON`. Every table's schema is created
//! when the catalog opens (`open`), so a catalog is whole from its first
//! open. One connection behind a
//! mutex is deliberate: the ingest socket dispatches serially, so a second
//! connection would only add lock contention.

use std::path::{Path, PathBuf};
use std::sync::{Arc, TryLockError};
use std::time::Duration;

use quanta_index_contract::ManifestGeneration;
use quanta_index_core::index::{IndexError, WorkBudget, WorkUsage};
use quanta_index_core::{
    CoreError, DiagnosticHealthCodeV1, DiagnosticLaneV1, DiagnosticStageV1, DiagnosticTerminalV1,
    DiagnosticTraceErrorV1,
};
use rusqlite::{Connection, OpenFlags, params};

use crate::io_admission::{
    CatalogConnectionGuard, CatalogConnectionOwner, catalog_lock_span, is_capacity_refusal,
};
use crate::writer_cache::CatalogWriterCacheObservation;

mod diagnostic;

/// Unix-millisecond clock for catalog lease decisions (TOPT-01 / PO-3).
///
/// Every idempotency transaction samples its `now` once from this port
/// and threads the sample through its claim/recovery/mutation-lease
/// decisions, so tests pin time exactly. Production uses
/// [`SystemCatalogClock`]; tests inject a scripted clock. Durable lease
/// semantics stay Unix milliseconds — never a process-local monotonic
/// clock.
pub trait CatalogClockPort: Send + Sync {
    fn now_unix_ms(&self) -> u64;
}

/// The production catalog clock: the wall clock at the composition edge.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemCatalogClock;

impl CatalogClockPort for SystemCatalogClock {
    fn now_unix_ms(&self) -> u64 {
        quanta_index_core::now_unix_ms()
    }
}

/// The catalog database file, under `state_root/catalog/`.
pub const CATALOG_FILE_NAME: &str = "catalog-v1.sqlite";

/// Result of optional, funded WAL truncation. Native capacity and other
/// storage failures remain errors; only an unfunded attempt or a real `SQLite`
/// reader/writer conflict is deferred.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WalReclaimOutcome {
    BelowThreshold,
    DeferredUnfunded,
    DeferredBusy,
    Completed,
}

/// The catalog directory under a state root.
#[must_use]
pub fn catalog_dir(state_root: &Path) -> PathBuf {
    state_root.join("catalog")
}

/// The search plane's durable catalog over one `SQLite` file: the ingest
/// idempotency records (QI-BB-032) and the auxiliary authority rows
/// (QI-BB-020).
pub struct SqliteCatalog {
    pub(crate) connection: CatalogConnectionOwner,
    pub(crate) path: PathBuf,
    pub(crate) clock: Arc<dyn CatalogClockPort>,
    pub(crate) writer_cache: Option<CatalogWriterCacheObservation>,
}

impl std::fmt::Debug for SqliteCatalog {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SqliteCatalog")
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

impl SqliteCatalog {
    /// Reclaim retained WAL extents before they consume cleanup headroom.
    /// The caller supplies the existing maintenance funding and request budget.
    /// A live `SQLite` snapshot may defer truncation; logical Catalog pins do not.
    /// An absent/small WAL or insufficient current allowance never starts
    /// checkpoint IO. This optional space optimization must not prevent the
    /// caller's bounded object reclaim.
    pub fn checkpoint_wal_for_reclaim(
        &self,
        threshold_bytes: u64,
        max_io_bytes: u64,
        budget: &mut WorkBudget,
    ) -> Result<WalReclaimOutcome, IndexError> {
        if threshold_bytes == 0 {
            return Err(IndexError::InvalidInput(
                "zero WAL checkpoint threshold".into(),
            ));
        }
        let guard = self.lock_native_budget(budget)?;
        if let Err(error) = budget.charge(WorkUsage {
            reads: 1,
            ..WorkUsage::default()
        }) {
            if error == IndexError::ResourceLimited {
                budget.checkpoint()?;
                return Ok(WalReclaimOutcome::DeferredUnfunded);
            }
            return Err(error);
        }
        let mut wal = self.path.as_os_str().to_os_string();
        wal.push("-wal");
        let metadata = match std::fs::symlink_metadata(&wal) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(WalReclaimOutcome::BelowThreshold);
            }
            Err(error) => return Err(IndexError::Storage(format!("stat Catalog WAL: {error}"))),
        };
        if !metadata.file_type().is_file() {
            return Err(IndexError::Storage(
                "Catalog WAL is not a regular file".into(),
            ));
        }
        let bytes = metadata.len();
        if bytes < threshold_bytes {
            budget.checkpoint()?;
            return Ok(WalReclaimOutcome::BelowThreshold);
        }
        // Do not wedge every reclaim round on a WAL that outgrew this class.
        // Test before multiplication so even an oversized length is deferred
        // without arithmetic overflow or checkpoint IO.
        let max_wal_bytes = max_io_bytes
            .checked_div(4)
            .ok_or(IndexError::ResourceLimited)?;
        if bytes > max_wal_bytes {
            budget.checkpoint()?;
            return Ok(WalReclaimOutcome::DeferredUnfunded);
        }
        // The connection mutex excludes this Catalog's writers. Charge the
        // observed wire width before checkpoint IO, including repeated frames,
        // page transfer and bounded SQLite page/header overhead.
        let pages = bytes
            .checked_add(511)
            .and_then(|rounded| rounded.checked_div(512))
            .ok_or(IndexError::ResourceLimited)?;
        let io_bytes = bytes.checked_mul(4).ok_or(IndexError::ResourceLimited)?;
        let charge = budget.charge(WorkUsage {
            reads: pages,
            writes: pages,
            bytes: io_bytes,
            mutations: 1,
        });
        if let Err(error) = charge {
            if error == IndexError::ResourceLimited {
                // Composite charge is atomic. Preserve cancellation/deadline
                // failures; a smaller remaining work allowance only skips the
                // optional checkpoint, leaving it for a funded later round.
                budget.checkpoint()?;
                return Ok(WalReclaimOutcome::DeferredUnfunded);
            }
            return Err(error);
        }
        // A live SQLite reader must not consume the maintenance round's busy
        // timeout. The connection mutex protects the exact prior setting and
        // its checked restoration. Classify native VFS errors before restoring
        // the handler so the original failed IO remains the classification input.
        let prior_busy_ms: i64 = guard
            .pragma_query_value(None, "busy_timeout", |row| row.get(0))
            .map_err(|error| IndexError::Storage(format!("read Catalog busy timeout: {error}")))?;
        if prior_busy_ms < 0 {
            return Err(IndexError::Storage(
                "Catalog busy timeout is negative".into(),
            ));
        }
        let attempt = (|| {
            guard
                .pragma_update(None, "busy_timeout", 0_i64)
                .map_err(|error| {
                    IndexError::Storage(format!("set Catalog nonwaiting checkpoint: {error}"))
                })?;
            let nonwaiting: i64 = guard
                .pragma_query_value(None, "busy_timeout", |row| row.get(0))
                .map_err(|error| {
                    IndexError::Storage(format!("read Catalog nonwaiting checkpoint: {error}"))
                })?;
            if nonwaiting != 0 {
                return Err(IndexError::Storage(format!(
                    "Catalog checkpoint busy timeout is {nonwaiting}, expected zero"
                )));
            }
            // Observe only the reached, funded explicit checkpoint. This is
            // not an ambient parent of lower-VFS events or an auto-checkpoint tag.
            let checkpoint = budget.request().diagnostic_context().and_then(|context| {
                context
                    .enter(DiagnosticStageV1::SqliteWork, DiagnosticLaneV1(26))
                    .map_or_else(
                        |error| {
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
                        },
                        Some,
                    )
            });
            let query: rusqlite::Result<(i64, i64, i64)> =
                guard.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| {
                    Ok((row.get(0)?, row.get(1)?, row.get(2)?))
                });
            if let Some(checkpoint) = checkpoint {
                let _finished = checkpoint.finish(DiagnosticTerminalV1::from_result(&query));
            }
            match query {
                Ok((0, _, _)) => Ok(WalReclaimOutcome::Completed),
                Ok((1, _, _)) => Ok(WalReclaimOutcome::DeferredBusy),
                Ok((busy, _, _)) => Err(IndexError::Storage(format!(
                    "checkpoint Catalog WAL returned invalid busy flag {busy}"
                ))),
                Err(error) if is_capacity_refusal(&error) => Err(IndexError::ResourceLimited),
                Err(rusqlite::Error::SqliteFailure(failure, _))
                    if matches!(
                        failure.code,
                        rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked
                    ) =>
                {
                    Ok(WalReclaimOutcome::DeferredBusy)
                }
                Err(error) => Err(IndexError::Storage(format!(
                    "checkpoint Catalog WAL: {error}"
                ))),
            }
        })();
        let restore = guard
            .pragma_update(None, "busy_timeout", prior_busy_ms)
            .and_then(|()| {
                guard.pragma_query_value(None, "busy_timeout", |row| row.get::<_, i64>(0))
            })
            .map_err(|error| IndexError::Storage(format!("restore Catalog busy timeout: {error}")))
            .and_then(|restored| {
                if restored == prior_busy_ms {
                    Ok(())
                } else {
                    Err(IndexError::Storage(format!(
                        "Catalog busy timeout restored to {restored}, expected {prior_busy_ms}"
                    )))
                }
            });
        drop(guard);
        match (attempt, restore) {
            (Err(original), Err(restoration)) => Err(IndexError::Storage(format!(
                "checkpoint Catalog WAL failed: {original}; busy timeout restoration also failed: {restoration}"
            ))),
            (_, Err(restoration)) => Err(restoration),
            (Err(original), Ok(())) => Err(original),
            (Ok(outcome), Ok(())) => {
                budget.checkpoint()?;
                Ok(outcome)
            }
        }
    }

    /// Close the original native connection under its admitted close scope.
    ///
    /// On failure, this catalog retains its connection and I/O admission owner
    /// so the caller can resolve the cause and retry close.
    pub fn close_checked(&mut self) -> Result<(), CoreError> {
        let path = &self.path;
        self.connection
            .close_checked_with(|error| engine_error("close", path, &error))
    }

    #[must_use]
    pub fn writer_cache_observation(&self) -> Option<CatalogWriterCacheObservation> {
        self.writer_cache
    }

    /// Explicit adapter diagnostics for completed `SQLite` statements.
    ///
    /// Replaces this connection's trace callback. Only PROFILE events are
    /// enabled; statements execute with their original admission and durability.
    /// Callbacks must not panic or reenter the catalog. `SQLite` reports elapsed
    /// statement time, not CPU time or a separate filesystem-sync measurement.
    pub fn set_statement_profile(
        &self,
        callback: Option<fn(rusqlite::trace::TraceEvent<'_>)>,
    ) -> Result<(), CoreError> {
        self.lock()?.trace_v2(
            rusqlite::trace::TraceEventCodes::SQLITE_TRACE_PROFILE,
            callback,
        );
        Ok(())
    }
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

/// Require installed `SQLite` objects to match this build's canonical DDL.
///
/// `CREATE IF NOT EXISTS` alone would silently accept an older or weakened
/// table/index definition and change the meaning of catalog writes.
pub(crate) fn verify_installed_schema_objects(
    connection: &Connection,
    path: &Path,
    schema: &str,
    objects: &[(&str, &str, &str)],
) -> Result<(), CoreError> {
    verify_installed_schema_objects_with_refusal(connection, path, schema, objects, |label| {
        CoreError::Storage(format!(
            "catalog: {} has an unsupported {label} schema; this build has no migration reader",
            path.display()
        ))
    })
}

/// Classify only an observed DDL mismatch with the caller's typed refusal.
///
/// SQL read failures retain their original error independently of path text.
pub(crate) fn verify_installed_schema_objects_with_refusal(
    connection: &Connection,
    path: &Path,
    schema: &str,
    objects: &[(&str, &str, &str)],
    refusal: impl Fn(&str) -> CoreError,
) -> Result<(), CoreError> {
    let compact = |sql: &str| sql.split_whitespace().collect::<String>();
    for &(kind, name, label) in objects {
        let keyword = match kind {
            "table" => "TABLE",
            "index" => "INDEX",
            _ => {
                return Err(CoreError::Storage(format!(
                    "catalog: unsupported schema object kind {kind} for {label}"
                )));
            }
        };
        let prefix = format!("CREATE {keyword} IF NOT EXISTS {name}");
        let expected = schema
            .split(';')
            .find(|statement| statement.trim_start().starts_with(&prefix))
            .ok_or_else(|| {
                CoreError::Storage(format!(
                    "catalog: canonical {label} schema has no {kind} definition"
                ))
            })?
            .trim()
            .replacen(
                &format!("CREATE {keyword} IF NOT EXISTS"),
                &format!("CREATE {keyword}"),
                1,
            );
        let installed: String = connection
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type = ?1 AND name = ?2",
                params![kind, name],
                |row| row.get(0),
            )
            .map_err(|error| {
                engine_error(&format!("read installed {label} schema"), path, &error)
            })?;
        if compact(&installed) != compact(&expected) {
            return Err(refusal(label));
        }
    }
    Ok(())
}

pub(crate) fn generation_i64(generation: ManifestGeneration) -> Result<i64, CoreError> {
    i64::try_from(generation.get()).map_err(|error| {
        CoreError::InvalidContract(format!(
            "catalog: generation {} does not fit the catalog's integer column: {error}",
            generation.get()
        ))
    })
}

pub(crate) fn blob32(label: &str, bytes: &[u8]) -> Result<[u8; 32], CoreError> {
    <[u8; 32]>::try_from(bytes).map_err(|_wrong_length| CoreError::Typed {
        code: quanta_index_contract::SearchPlaneErrorCodeV2::CatalogRowCorrupt,
        message: format!("catalog: {label} is {} bytes, expected 32", bytes.len()),
    })
}

pub(crate) fn count_u64(label: &str, count: usize) -> Result<u64, CoreError> {
    u64::try_from(count)
        .map_err(|error| CoreError::Storage(format!("catalog: {label} count overflow: {error}")))
}

/// Open (creating if needed) the catalog file under `state_root/catalog/`.
/// The caller installs connection custody before any fallible configuration.
pub(crate) fn open_connection(
    state_root: &Path,
    vfs_name: Option<&str>,
) -> Result<(Connection, PathBuf), CoreError> {
    let directory = catalog_dir(state_root);
    std::fs::create_dir_all(&directory)
        .map_err(|error| storage("create directory", &directory, &error))?;
    let path = directory.join(CATALOG_FILE_NAME);
    let flags = OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE;
    let connection = vfs_name
        .map_or_else(
            || Connection::open_with_flags(&path, flags),
            |name| Connection::open_with_flags_and_vfs(&path, flags, name),
        )
        .map_err(|error| engine_error("open", &path, &error))?;
    Ok((connection, path))
}

/// Apply and verify the original durability settings on the same connection.
pub(crate) fn configure_connection(
    connection: &Connection,
    path: &Path,
    busy_timeout: Duration,
) -> Result<(), CoreError> {
    // Each pragma is read back rather than assumed: `journal_mode` is a
    // request the engine may decline, and a catalog that silently ran
    // under `NORMAL` would ack work the engine can forget (G0-C).
    connection
        .pragma_update(None, "journal_mode", "WAL")
        .map_err(|error| engine_error("set journal_mode", path, &error))?;
    connection
        .pragma_update(None, "synchronous", "FULL")
        .map_err(|error| engine_error("set synchronous", path, &error))?;
    connection
        .pragma_update(None, "fullfsync", "ON")
        .map_err(|error| engine_error("set fullfsync", path, &error))?;
    connection
        .pragma_update(None, "foreign_keys", "ON")
        .map_err(|error| engine_error("set foreign_keys", path, &error))?;
    connection
        .busy_timeout(busy_timeout)
        .map_err(|error| engine_error("set busy_timeout", path, &error))?;
    let journal_mode: String = connection
        .pragma_query_value(None, "journal_mode", |row| row.get(0))
        .map_err(|error| engine_error("read journal_mode", path, &error))?;
    let synchronous: i64 = connection
        .pragma_query_value(None, "synchronous", |row| row.get(0))
        .map_err(|error| engine_error("read synchronous", path, &error))?;
    if !journal_mode.eq_ignore_ascii_case("wal") || synchronous != 2 {
        return Err(CoreError::Storage(format!(
            "catalog: {} runs journal_mode={journal_mode} synchronous={synchronous}; the catalog requires wal/FULL",
            path.display()
        )));
    }
    // `fullfsync` is read back too (SEP-21 P02B): on the platforms that
    // honor it, a silent `OFF` would ack directory entries the engine
    // never forced. A platform that reports 0 because it cannot honor
    // the flag is a storage error, not a degraded mode to run in.
    let fullfsync: i64 = connection
        .pragma_query_value(None, "fullfsync", |row| row.get(0))
        .map_err(|error| engine_error("read fullfsync", path, &error))?;
    if fullfsync != 1 {
        return Err(CoreError::Storage(format!(
            "catalog: {} runs fullfsync={fullfsync}; the catalog requires fullfsync=ON where the platform honors it",
            path.display()
        )));
    }
    Ok(())
}

impl SqliteCatalog {
    /// The database file.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) fn observe_owned_renewal_refusal(
        &self,
        observation: quanta_index_core::DiagnosticOwnedRenewalRefusalV1,
    ) {
        if let Some(context) = diagnostic::current(self) {
            context.owned_renewal_refusal(observation);
        }
    }

    pub(crate) fn lock(&self) -> Result<CatalogConnectionGuard<'_>, CoreError> {
        let diagnostics = diagnostic::current(self);
        let wait = catalog_lock_span(diagnostics.as_ref(), DiagnosticStageV1::CatalogLockWait);
        let acquired = (|| {
            self.connection.mutex()?.lock().map_err(|error| {
                CoreError::Storage(format!(
                    "catalog: connection to {} poisoned: {error}",
                    self.path.display()
                ))
            })
        })();
        if let Some(wait) = wait {
            let _observed = wait.finish(DiagnosticTerminalV1::from_result(&acquired));
        }
        let guard = acquired?;
        Ok(self.connection.scoped(guard, diagnostics.as_ref()))
    }

    /// Original-request bounded mutex acquisition for native work. The
    /// legacy catalog API retains its existing blocking lock contract.
    pub(crate) fn lock_native_budget(
        &self,
        budget: &WorkBudget,
    ) -> Result<CatalogConnectionGuard<'_>, IndexError> {
        let diagnostics = budget.request().diagnostic_context();
        let wait = catalog_lock_span(diagnostics, DiagnosticStageV1::CatalogLockWait);
        let acquired = (|| {
            let connection = self
                .connection
                .mutex()
                .map_err(|error| IndexError::Storage(error.to_string()))?;
            loop {
                budget.checkpoint()?;
                match connection.try_lock() {
                    Ok(guard) => {
                        budget.checkpoint()?;
                        return Ok(guard);
                    }
                    Err(TryLockError::Poisoned(_)) => {
                        return Err(IndexError::Storage(format!(
                            "catalog connection to {} poisoned",
                            self.path.display()
                        )));
                    }
                    Err(TryLockError::WouldBlock) => {
                        crate::ticket_diagnostics::retry_sleep(self, Duration::from_millis(1));
                    }
                }
            }
        })();
        if let Some(wait) = wait {
            let outcome = if matches!(&acquired, Err(IndexError::Cancelled)) {
                DiagnosticTerminalV1::Cancelled { code: 1 }
            } else {
                DiagnosticTerminalV1::from_result(&acquired)
            };
            let _observed = wait.finish(outcome);
        }
        let guard = acquired?;
        Ok(self.connection.scoped(guard, diagnostics))
    }
}

#[cfg(test)]
mod lock_budget_regressions {
    use std::{
        error::Error,
        io,
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
            mpsc,
        },
        thread,
        time::{Duration, Instant},
    };

    use quanta_index_core::{
        RequestBudgetV1,
        index::{
            CollectionId, IndexError, ObjectBatchInput, ObjectRef, RegisteredObject, SlotId,
            StoredObject, TenantId, WorkBudget, WorkLimits, WorkUsage, WorkspaceKey, WorktreeId,
            ports::{CatalogGenesisPort, ImmutableObjectPort, ObjectWriteSink},
        },
    };

    use super::{SqliteCatalog, WalReclaimOutcome};

    type TestResult = Result<(), Box<dyn Error>>;

    fn require(condition: bool, detail: &'static str) -> TestResult {
        if condition {
            Ok(())
        } else {
            Err(io::Error::other(detail).into())
        }
    }

    fn limits() -> WorkLimits {
        WorkLimits {
            reads: 64,
            writes: 64,
            bytes: 65_536,
            mutations: 64,
        }
    }

    fn workspace() -> WorkspaceKey {
        WorkspaceKey {
            tenant: TenantId([1; 16]),
            collection: CollectionId([2; 16]),
            worktree: WorktreeId([3; 16]),
            slot: SlotId([4; 16]),
        }
    }

    struct IdentitySpy(Arc<AtomicUsize>);

    impl ImmutableObjectPort for IdentitySpy {
        fn identify(
            &self,
            _payload: &[u8],
            _children: &[ObjectRef],
            _budget: &mut WorkBudget,
        ) -> Result<RegisteredObject, IndexError> {
            let _prior = self.0.fetch_add(1, Ordering::SeqCst);
            Err(IndexError::BackendViolation(
                "identity called before budget refusal".into(),
            ))
        }

        fn put(
            &self,
            _payload: &[u8],
            _children: &[ObjectRef],
            _budget: &mut WorkBudget,
        ) -> Result<RegisteredObject, IndexError> {
            Err(IndexError::BackendViolation("identity-only fixture".into()))
        }

        fn install(
            &self,
            _payload: &[u8],
            _children: &[ObjectRef],
            _sink: &dyn ObjectWriteSink,
            _budget: &mut WorkBudget,
        ) -> Result<RegisteredObject, IndexError> {
            Err(IndexError::BackendViolation("identity-only fixture".into()))
        }

        fn install_batch(
            &self,
            _inputs: &[ObjectBatchInput],
            _sink: &dyn ObjectWriteSink,
            _budget: &mut WorkBudget,
        ) -> Result<quanta_index_core::index::RegisteredObjectBatch, IndexError> {
            Err(IndexError::BackendViolation("identity-only fixture".into()))
        }

        fn get(
            &self,
            _reference: ObjectRef,
            _budget: &mut WorkBudget,
        ) -> Result<StoredObject, IndexError> {
            Err(IndexError::BackendViolation("identity-only fixture".into()))
        }
    }

    fn assert_refused_before_guard_release(
        request: RequestBudgetV1,
        expected_cancelled: bool,
    ) -> TestResult {
        let root = tempfile::tempdir()?;
        let catalog = Arc::new(SqliteCatalog::open(root.path(), Duration::from_secs(1))?);
        let guard = catalog.lock()?;
        let before: i64 =
            guard.query_row("SELECT COUNT(*) FROM native_bootstrap_v1", [], |row| {
                row.get(0)
            })?;
        let calls = Arc::new(AtomicUsize::new(0));
        let worker_calls = Arc::clone(&calls);
        let worker_catalog = Arc::clone(&catalog);
        let (done_tx, done_rx) = mpsc::channel();
        let worker = thread::spawn(move || {
            let identity = IdentitySpy(worker_calls);
            let mut budget = WorkBudget::with_request(limits(), request);
            let result = CatalogGenesisPort::begin_bootstrap(
                worker_catalog.as_ref(),
                workspace(),
                1,
                &identity,
                &mut budget,
            );
            let _sent = done_tx.send((result, budget.usage()));
        });
        // The held guard prevents a blocking-lock implementation from returning.
        // Both request states were interrupted before the worker was spawned,
        // so scheduling cannot make an old post-lock checkpoint pass this test.
        let completed_while_held = done_rx.recv_timeout(Duration::from_secs(1));
        let after: i64 =
            guard.query_row("SELECT COUNT(*) FROM native_bootstrap_v1", [], |row| {
                row.get(0)
            })?;
        drop(guard);
        worker
            .join()
            .map_err(|_panic| io::Error::other("bootstrap worker panicked"))?;
        let (result, usage) = completed_while_held.map_err(|_timeout| {
            io::Error::other("bootstrap did not return before catalog guard release")
        })?;
        let typed = if expected_cancelled {
            matches!(result, Err(IndexError::Cancelled))
        } else {
            matches!(result, Err(IndexError::ResourceLimited))
        };
        require(typed, "bootstrap returned the wrong interruption type")?;
        require(
            usage == WorkUsage::default(),
            "bootstrap lock wait charged work",
        )?;
        require(
            after == before,
            "bootstrap changed durable rows while the lock was held",
        )?;
        require(
            calls.load(Ordering::SeqCst) == 0,
            "bootstrap reached identity before budget refusal",
        )?;
        Ok(())
    }

    #[test]
    fn cancelled_genesis_request_refuses_before_catalog_lock_release() -> TestResult {
        let request = RequestBudgetV1::unbounded();
        request.cancel_handle().cancel();
        assert_refused_before_guard_release(request, true)
    }

    #[test]
    fn expired_genesis_request_refuses_before_catalog_lock_release() -> TestResult {
        assert_refused_before_guard_release(RequestBudgetV1::until(Instant::now()), false)
    }

    #[test]
    fn reclaim_checkpoint_preserves_rows_and_refuses_before_unfunded_io() -> TestResult {
        let root = tempfile::tempdir()?;
        let catalog = SqliteCatalog::open(root.path(), Duration::ZERO)?;
        {
            let connection = catalog.lock()?;
            connection.execute_batch(
                "PRAGMA wal_autocheckpoint=0;
                 CREATE TABLE checkpoint_gold(id INTEGER PRIMARY KEY, value BLOB);
                 INSERT INTO checkpoint_gold VALUES(7, zeroblob(32768));",
            )?;
        }
        let wal = catalog.path().with_file_name("catalog-v1.sqlite-wal");
        let before = std::fs::metadata(&wal)?.len();
        require(before > 32_768, "fixture did not create a real WAL")?;
        let mut denied = WorkBudget::with_request(
            WorkLimits {
                reads: 0,
                writes: 0,
                bytes: 0,
                mutations: 0,
            },
            RequestBudgetV1::unbounded(),
        );
        require(
            catalog.checkpoint_wal_for_reclaim(1, 64 * 1024 * 1024, &mut denied)
                == Ok(WalReclaimOutcome::DeferredUnfunded),
            "unfunded checkpoint did not defer",
        )?;
        require(
            std::fs::metadata(&wal)?.len() == before,
            "unfunded checkpoint changed WAL",
        )?;
        require(
            denied.usage() == WorkUsage::default(),
            "unfunded first read changed the original work usage",
        )?;
        let cancelled_request = RequestBudgetV1::unbounded();
        cancelled_request.cancel_handle().cancel();
        let mut cancelled = WorkBudget::with_request(
            WorkLimits {
                reads: 65536,
                writes: 65536,
                bytes: 64 * 1024 * 1024,
                mutations: 8,
            },
            cancelled_request,
        );
        require(
            matches!(
                catalog.checkpoint_wal_for_reclaim(1, 64 * 1024 * 1024, &mut cancelled),
                Err(IndexError::Cancelled)
            ),
            "cancelled checkpoint was deferred",
        )?;
        let mut expired = WorkBudget::with_request(
            WorkLimits {
                reads: 65536,
                writes: 65536,
                bytes: 64 * 1024 * 1024,
                mutations: 8,
            },
            RequestBudgetV1::until(Instant::now()),
        );
        require(
            matches!(
                catalog.checkpoint_wal_for_reclaim(1, 64 * 1024 * 1024, &mut expired),
                Err(IndexError::ResourceLimited)
            ),
            "expired checkpoint was deferred",
        )?;
        require(
            std::fs::metadata(&wal)?.len() == before,
            "cancelled or expired checkpoint changed WAL",
        )?;
        let mut budget = WorkBudget::with_request(
            WorkLimits {
                reads: 65536,
                writes: 65536,
                bytes: 64 * 1024 * 1024,
                mutations: 8,
            },
            RequestBudgetV1::unbounded(),
        );
        let checkpoint_io = before.checked_mul(4).ok_or("checkpoint IO overflow")?;
        for max_io in [0, checkpoint_io.checked_sub(1).ok_or("zero checkpoint IO")?] {
            require(
                catalog.checkpoint_wal_for_reclaim(1, max_io, &mut budget)?
                    == WalReclaimOutcome::DeferredUnfunded,
                "checkpoint exceeded its original IO reservation",
            )?;
        }
        require(
            std::fs::metadata(&wal)?.len() == before,
            "IO-refused checkpoint changed WAL",
        )?;
        let mut small = WorkBudget::with_request(
            WorkLimits {
                reads: 65536,
                writes: 1,
                bytes: 64 * 1024 * 1024,
                mutations: 8,
            },
            RequestBudgetV1::unbounded(),
        );
        require(
            catalog.checkpoint_wal_for_reclaim(1, checkpoint_io, &mut small)?
                == WalReclaimOutcome::DeferredUnfunded,
            "optional checkpoint blocked smaller maintenance work",
        )?;
        require(
            small.usage()
                == WorkUsage {
                    reads: 1,
                    ..WorkUsage::default()
                },
            "deferred checkpoint consumed the original work allowance",
        )?;
        require(
            std::fs::metadata(&wal)?.len() == before,
            "work-deferred checkpoint changed WAL",
        )?;
        small.charge(WorkUsage {
            writes: 1,
            mutations: 1,
            ..WorkUsage::default()
        })?;
        require(
            catalog.checkpoint_wal_for_reclaim(1, checkpoint_io, &mut budget)?
                == WalReclaimOutcome::Completed,
            "funded checkpoint skipped",
        )?;
        require(
            std::fs::metadata(&wal)?.len() == 0,
            "checkpoint retained WAL extents",
        )?;
        require(
            catalog.checkpoint_wal_for_reclaim(1, checkpoint_io, &mut budget)?
                == WalReclaimOutcome::BelowThreshold,
            "empty WAL did not remain below threshold",
        )?;
        let gold: (i64, i64) = catalog.lock()?.query_row(
            "SELECT id, length(value) FROM checkpoint_gold",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        require(gold == (7, 32768), "checkpoint changed fixed SQL gold")
    }

    #[test]
    fn reclaim_checkpoint_defers_real_sqlite_snapshot_without_deleting_it() -> TestResult {
        let root = tempfile::tempdir()?;
        let catalog = SqliteCatalog::open(root.path(), Duration::from_secs(2))?;
        catalog.lock()?.execute_batch(
            "PRAGMA wal_autocheckpoint=0;
             CREATE TABLE checkpoint_gold(id INTEGER PRIMARY KEY);
             INSERT INTO checkpoint_gold VALUES(7);",
        )?;
        let reader = rusqlite::Connection::open(catalog.path())?;
        reader.execute_batch("BEGIN; SELECT * FROM checkpoint_gold;")?;
        let inserted = catalog
            .lock()?
            .execute("INSERT INTO checkpoint_gold VALUES(11)", [])?;
        require(inserted == 1, "fixture did not commit the next row")?;
        let mut budget = WorkBudget::with_request(
            WorkLimits {
                reads: 65536,
                writes: 65536,
                bytes: 64 * 1024 * 1024,
                mutations: 8,
            },
            RequestBudgetV1::unbounded(),
        );
        let prior_busy_ms: i64 =
            catalog
                .lock()?
                .pragma_query_value(None, "busy_timeout", |row| row.get(0))?;
        require(prior_busy_ms == 2000, "fixture busy timeout differs")?;
        let started = Instant::now();
        require(
            catalog.checkpoint_wal_for_reclaim(1, 64 * 1024 * 1024, &mut budget)?
                == WalReclaimOutcome::DeferredBusy,
            "live SQLite snapshot did not defer checkpoint",
        )?;
        require(
            started.elapsed() < Duration::from_millis(1500),
            "optional checkpoint waited through the ordinary busy timeout",
        )?;
        let restored_busy_ms: i64 =
            catalog
                .lock()?
                .pragma_query_value(None, "busy_timeout", |row| row.get(0))?;
        require(
            restored_busy_ms == prior_busy_ms,
            "busy timeout was not restored",
        )?;
        let old: i64 =
            reader.query_row("SELECT sum(id) FROM checkpoint_gold", [], |row| row.get(0))?;
        require(old == 7, "checkpoint invalidated reader snapshot")?;
        reader.execute_batch("ROLLBACK")?;
        drop(reader);
        require(
            catalog.checkpoint_wal_for_reclaim(1, 64 * 1024 * 1024, &mut budget)?
                == WalReclaimOutcome::Completed,
            "checkpoint did not resume after reader release",
        )?;
        let completed_busy_ms: i64 =
            catalog
                .lock()?
                .pragma_query_value(None, "busy_timeout", |row| row.get(0))?;
        require(
            completed_busy_ms == prior_busy_ms,
            "successful checkpoint did not restore the busy timeout",
        )?;
        let current: i64 =
            catalog
                .lock()?
                .query_row("SELECT sum(id) FROM checkpoint_gold", [], |row| row.get(0))?;
        require(current == 18, "checkpoint lost committed rows")
    }
}

#[cfg(test)]
mod checkpoint_diagnostic_regressions {
    use std::{
        error::Error,
        sync::{
            Arc, Mutex,
            atomic::{AtomicU64, Ordering},
        },
        time::Duration,
    };

    use quanta_index_core::{
        DiagnosticCaptureIdV1, DiagnosticClockV1, DiagnosticEmitV1, DiagnosticEventKindV1,
        DiagnosticEventSinkV1, DiagnosticLaneV1, DiagnosticRootKindV1, DiagnosticStageV1,
        DiagnosticTerminalV1, RequestBudgetV1,
        index::ports::CatalogDiagnosticPort,
        index::{WorkBudget, WorkLimits},
    };

    use super::{SqliteCatalog, WalReclaimOutcome};

    #[derive(Debug, Default)]
    struct Clock(AtomicU64);

    impl DiagnosticClockV1 for Clock {
        fn now_ns(&self) -> u64 {
            self.0.fetch_add(1, Ordering::Relaxed)
        }
    }

    #[derive(Debug, Default)]
    struct Events(Mutex<Vec<DiagnosticEmitV1>>);

    impl DiagnosticEventSinkV1 for Events {
        fn try_emit(&self, event: DiagnosticEmitV1) -> bool {
            let Ok(mut rows) = self.0.lock() else {
                return false;
            };
            rows.push(event);
            true
        }
    }

    fn budget(request: RequestBudgetV1) -> WorkBudget {
        WorkBudget::with_request(
            WorkLimits {
                reads: 65_536,
                writes: 65_536,
                bytes: 64 * 1024 * 1024,
                mutations: 8,
            },
            request,
        )
    }

    #[test]
    fn funded_explicit_checkpoint_has_one_typed_span_and_off_keeps_result()
    -> Result<(), Box<dyn Error>> {
        let root = tempfile::tempdir()?;
        let catalog = SqliteCatalog::open(root.path(), Duration::ZERO)?;
        catalog.lock()?.execute_batch(
            "PRAGMA wal_autocheckpoint=0;
             CREATE TABLE checkpoint_trace_gold(id INTEGER PRIMARY KEY);
             INSERT INTO checkpoint_trace_gold VALUES(7);",
        )?;

        let clock = Arc::new(Clock::default());
        let events = Arc::new(Events::default());
        let context = quanta_index_core::DiagnosticContextV1::new(
            DiagnosticCaptureIdV1([26; 16]),
            clock.clone(),
            events.clone(),
        );
        let root_span = context.enter_root(DiagnosticRootKindV1::Operation, DiagnosticLaneV1(0))?;
        let child = context.child_context(&root_span);
        let mut record =
            budget(RequestBudgetV1::unbounded().with_diagnostic_context(child.clone()));
        if catalog.checkpoint_wal_for_reclaim(1, 64 * 1024 * 1024, &mut record)?
            != WalReclaimOutcome::Completed
        {
            return Err("funded recorded checkpoint skipped".into());
        }

        let rows = events.0.lock().map_err(|error| {
            std::io::Error::other(format!("checkpoint trace sink lock poisoned: {error}"))
        })?;
        let matching: Vec<_> = rows
            .iter()
            .filter_map(|event| {
                let DiagnosticEventKindV1::SpanBegin {
                    span_id,
                    parent_span_id,
                    stage: DiagnosticStageV1::SqliteWork,
                    lane: DiagnosticLaneV1(26),
                    ..
                } = event.kind
                else {
                    return None;
                };
                Some((span_id, parent_span_id))
            })
            .collect();
        let [(span_id, parent)] = matching.as_slice() else {
            return Err("expected exactly one checkpoint SqliteWork span".into());
        };
        if *parent != Some(root_span.span_id()) {
            return Err("checkpoint SqliteWork parent differs".into());
        }
        if rows
            .iter()
            .filter(|event| {
                matches!(
                    event.kind,
                    DiagnosticEventKindV1::SpanEnd {
                        span_id: ended,
                        outcome: DiagnosticTerminalV1::Ok,
                    } if ended == *span_id
                )
            })
            .count()
            != 1
        {
            return Err("checkpoint SqliteWork terminal differs".into());
        }
        let prior_events = rows.len();
        drop(rows);

        let inserted = catalog
            .lock()?
            .execute("INSERT INTO checkpoint_trace_gold VALUES(11)", [])?;
        if inserted != 1 {
            return Err("checkpoint fixture insert count differs".into());
        }
        let outer = catalog.enter_diagnostic_scope(Some(&child));
        let prior_clock = clock.0.load(Ordering::Relaxed);
        let mut off = budget(RequestBudgetV1::unbounded());
        if catalog.checkpoint_wal_for_reclaim(1, 64 * 1024 * 1024, &mut off)?
            != WalReclaimOutcome::Completed
        {
            return Err("funded Off checkpoint skipped".into());
        }
        if clock.0.load(Ordering::Relaxed) != prior_clock {
            return Err("Off checkpoint read the diagnostic clock".into());
        }
        if events
            .0
            .lock()
            .map_err(|error| {
                std::io::Error::other(format!("checkpoint trace sink lock poisoned: {error}"))
            })?
            .len()
            != prior_events
        {
            return Err("Off checkpoint emitted diagnostic events".into());
        }
        drop(outer);

        let sum: i64 =
            catalog
                .lock()?
                .query_row("SELECT sum(id) FROM checkpoint_trace_gold", [], |row| {
                    row.get(0)
                })?;
        if sum != 18 {
            return Err("checkpoint changed fixed SQL gold".into());
        }
        let _finished = root_span.finish(DiagnosticTerminalV1::Ok);
        Ok(())
    }
}
