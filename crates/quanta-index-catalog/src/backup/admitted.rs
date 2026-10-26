//! Caller-owned `SQLite` backup session for a precreated, admitted snapshot file.
//!
//! The caller retains the source lease, output file authority and this session
//! until checked connection retirement. The returned page width is a logical
//! `SQLite` receipt; the caller must compare it with the width observed while
//! syncing the original admitted output FD and its parent before publication.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use quanta_index_contract::SearchPlaneErrorCodeV2;
use quanta_index_core::{CoreError, RequestBudgetV1};
use rusqlite::backup::{Backup, StepResult};
use rusqlite::{Connection, OpenFlags};

use super::{BACKUP_PAGES_PER_STEP, CatalogSnapshotReceiptV1, catalog_content_digest_with_mapper};
use crate::connection::engine_error;
use crate::io_admission::{CatalogConnectionOwner, CatalogIoAdmission, ScopeLease};

/// Owns every `SQLite` connection opened for one admitted catalog snapshot.
///
/// `destination` must already be an empty regular file created exclusively
/// by the caller's physical admission authority. This constructor performs no
/// I/O. After any failed native close, the original connection remains here
/// for `close_checked` retry; the caller must retain this session and its
/// physical source/output owners until retirement succeeds.
pub struct CatalogSnapshotSessionV1 {
    destination: PathBuf,
    io: Arc<dyn CatalogIoAdmission>,
    source_path: Option<PathBuf>,
    source: Option<CatalogConnectionOwner>,
    output: Option<CatalogConnectionOwner>,
    verification: Option<CatalogConnectionOwner>,
    started: bool,
}

impl CatalogSnapshotSessionV1 {
    /// Construct an inert owner for a caller-precreated destination.
    #[must_use]
    pub fn new(destination: PathBuf, io: Arc<dyn CatalogIoAdmission>) -> Self {
        Self {
            destination,
            io,
            source_path: None,
            source: None,
            output: None,
            verification: None,
            started: false,
        }
    }

    /// Copy a read-only source into the admitted destination and verify it.
    ///
    /// This is one-shot: retry after an interrupted copy requires a new
    /// admitted, empty destination. A failed native close can instead be
    /// retried on this same session with `close_checked`.
    pub fn snapshot_from(
        &mut self,
        source: &Path,
        busy_timeout: Duration,
        budget: &RequestBudgetV1,
    ) -> Result<CatalogSnapshotReceiptV1, CoreError> {
        self.snapshot_with_source_receipt(source, busy_timeout, budget)
            .map(|(_source, snapshot)| snapshot)
    }

    /// Return both receipts from the original read-only source connection
    /// and its admitted backup. The source receipt is read before the first
    /// destination native open; a changed source will fail the caller's
    /// logical-digest equality check against the produced snapshot.
    pub fn snapshot_with_source_receipt(
        &mut self,
        source: &Path,
        busy_timeout: Duration,
        budget: &RequestBudgetV1,
    ) -> Result<(CatalogSnapshotReceiptV1, CatalogSnapshotReceiptV1), CoreError> {
        if self.started {
            return Err(CoreError::InvalidContract(
                "catalog snapshot session already began a copy".into(),
            ));
        }
        budget.checkpoint("catalog snapshot before source open")?;
        self.started = true;
        self.source_path = Some(source.to_path_buf());
        let copied = (|| {
            let connection = Connection::open_with_flags(
                source,
                OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
            )
            .map_err(|error| engine_error("open read-only snapshot source", source, &error))?;
            self.source = Some(CatalogConnectionOwner::new(connection, None));
            let source_owner = self.source.as_ref().ok_or_else(|| {
                CoreError::InvalidContract("catalog snapshot source owner absent".into())
            })?;
            let source_guard = lock_connection(source_owner)?;
            source_guard.busy_timeout(busy_timeout).map_err(|error| {
                engine_error("set snapshot source busy timeout", source, &error)
            })?;
            source_guard
                .execute_batch("BEGIN DEFERRED")
                .map_err(|error| {
                    engine_error(
                        "begin read-only snapshot source transaction",
                        source,
                        &error,
                    )
                })?;
            let source_receipt = read_receipt(&source_guard, source, budget)?;
            drop(source_guard);
            budget.checkpoint("catalog snapshot before destination open")?;
            let _scope = ScopeLease::enter(Some(&self.io), None);
            let connection = Connection::open_with_flags_and_vfs(
                &self.destination,
                OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
                self.io.vfs_name(),
            )
            .map_err(|error| {
                engine_error(
                    "open admitted snapshot destination",
                    &self.destination,
                    &error,
                )
            })?;
            // Install the actual connection before busy-timeout, backup init,
            // page steps or journal normalization can fail.
            self.output = Some(CatalogConnectionOwner::new(
                connection,
                Some(Arc::clone(&self.io)),
            ));
            let copied = self.copy_pages(busy_timeout, budget);
            // The source receipt and backup must observe one SQLite read
            // snapshot. Release that snapshot even when a page step or journal
            // normalization failed, without replacing the original error.
            let source_owner = self.source.as_ref().ok_or_else(|| {
                CoreError::InvalidContract("catalog snapshot source owner absent".into())
            })?;
            let source_guard = lock_connection(source_owner)?;
            let released = source_guard.execute_batch("ROLLBACK").map_err(|error| {
                engine_error("end read-only snapshot source transaction", source, &error)
            });
            drop(source_guard);
            combine(copied, released, "snapshot source transaction release")?;
            Ok(source_receipt)
        })();
        let retired = self.close_checked();
        let source_receipt = combine(copied, retired, "snapshot connection retirement")?;
        let snapshot_receipt = self.verify(budget)?;
        Ok((source_receipt, snapshot_receipt))
    }

    fn copy_pages(
        &self,
        busy_timeout: Duration,
        budget: &RequestBudgetV1,
    ) -> Result<(), CoreError> {
        let source_owner = self.source.as_ref().ok_or_else(|| {
            CoreError::InvalidContract("catalog snapshot source owner absent".into())
        })?;
        let output_owner = self.output.as_ref().ok_or_else(|| {
            CoreError::InvalidContract("catalog snapshot output owner absent".into())
        })?;
        let source_guard = lock_connection(source_owner)?;
        let mut output_guard = lock_connection(output_owner)?;
        output_guard.busy_timeout(busy_timeout).map_err(|error| {
            engine_error(
                "set snapshot output busy timeout",
                &self.destination,
                &error,
            )
        })?;
        let backup = Backup::new(&source_guard, &mut output_guard)
            .map_err(|error| engine_error("begin admitted snapshot", &self.destination, &error))?;
        let mut blocked_since = None;
        loop {
            budget.checkpoint("catalog snapshot backup page batch")?;
            match backup.step(BACKUP_PAGES_PER_STEP).map_err(|error| {
                engine_error("copy admitted snapshot pages", &self.destination, &error)
            })? {
                StepResult::Done => break,
                StepResult::More => blocked_since = None,
                StepResult::Busy | StepResult::Locked => {
                    let since = blocked_since.get_or_insert_with(Instant::now);
                    let remaining = busy_timeout.saturating_sub(since.elapsed());
                    if remaining.is_zero() {
                        return Err(CoreError::Typed {
                            code: SearchPlaneErrorCodeV2::CatalogBusy,
                            message: format!(
                                "catalog: backup into {} met a held lock past the busy budget",
                                self.destination.display()
                            ),
                        });
                    }
                    std::thread::sleep(remaining.min(Duration::from_millis(1)));
                }
                _ => {
                    return Err(CoreError::InvalidContract(
                        "unknown SQLite backup step outcome".into(),
                    ));
                }
            }
        }
        // rusqlite's Backup Drop calls sqlite3_backup_finish but discards its
        // return code. Do not present this as independent finish proof.
        drop(backup);
        drop(source_guard);
        budget.checkpoint("catalog snapshot before journal normalization")?;
        require_delete_journal_mode(&output_guard, &self.destination)?;
        drop(output_guard);
        Ok(())
    }

    /// Normalize and verify this admitted destination under its named VFS.
    ///
    /// Verification may follow a deep open that restored WAL mode. It uses a
    /// funded read-write connection for the rollback-journal transition, then
    /// integrity and logical-content checks on that same original connection.
    pub fn verify(
        &mut self,
        budget: &RequestBudgetV1,
    ) -> Result<CatalogSnapshotReceiptV1, CoreError> {
        budget.checkpoint("catalog snapshot before admitted verification")?;
        self.close_checked()?;
        let observed = (|| {
            let _scope = ScopeLease::enter(Some(&self.io), None);
            let connection = Connection::open_with_flags_and_vfs(
                &self.destination,
                OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
                self.io.vfs_name(),
            )
            .map_err(|error| {
                engine_error(
                    "open admitted snapshot verification",
                    &self.destination,
                    &error,
                )
            })?;
            self.verification = Some(CatalogConnectionOwner::new(
                connection,
                Some(Arc::clone(&self.io)),
            ));
            let owner = self.verification.as_ref().ok_or_else(|| {
                CoreError::InvalidContract("catalog snapshot verification owner absent".into())
            })?;
            let guard = lock_connection(owner)?;
            budget.checkpoint("catalog snapshot before journal normalization")?;
            require_delete_journal_mode(&guard, &self.destination)?;
            read_receipt(&guard, &self.destination, budget)
        })();
        let retired = self.close_checked();
        combine(observed, retired, "snapshot verification retirement")
    }

    /// Physically close every session connection, retaining failed owners.
    pub fn close_checked(&mut self) -> Result<(), CoreError> {
        let mut failure = None;
        close_slot(
            &mut self.verification,
            &self.destination,
            "close snapshot verification",
            &mut failure,
        );
        close_slot(
            &mut self.output,
            &self.destination,
            "close snapshot output",
            &mut failure,
        );
        let source = self.source_path.as_deref().unwrap_or(&self.destination);
        close_slot(
            &mut self.source,
            source,
            "close snapshot source",
            &mut failure,
        );
        failure.map_or(Ok(()), Err)
    }
}

impl Drop for CatalogSnapshotSessionV1 {
    fn drop(&mut self) {
        if self.close_checked().is_err() {
            // The original owner still holds the original connection and its
            // admission Arc. Never run CatalogConnectionOwner's best-effort
            // fallback after checked close proved ambiguous. This is a failed
            // process quarantine, not a successful close or refund.
            if let Some(owner) = self.verification.take() {
                std::mem::forget(owner);
            }
            if let Some(owner) = self.output.take() {
                std::mem::forget(owner);
            }
            if let Some(owner) = self.source.take() {
                std::mem::forget(owner);
            }
        }
    }
}

fn lock_connection(
    owner: &CatalogConnectionOwner,
) -> Result<MutexGuard<'_, Connection>, CoreError> {
    owner.mutex()?.lock().map_err(|error| {
        CoreError::Storage(format!(
            "catalog snapshot connection mutex poisoned: {error}"
        ))
    })
}

fn require_delete_journal_mode(connection: &Connection, path: &Path) -> Result<(), CoreError> {
    let mode: String = connection
        .query_row("PRAGMA journal_mode = DELETE", [], |row| row.get(0))
        .map_err(|error| engine_error("normalize admitted snapshot journal", path, &error))?;
    if !mode.eq_ignore_ascii_case("delete") {
        return Err(CoreError::Storage(format!(
            "catalog: snapshot {} remained in journal mode {mode}",
            path.display()
        )));
    }
    Ok(())
}

fn read_receipt(
    connection: &Connection,
    path: &Path,
    budget: &RequestBudgetV1,
) -> Result<CatalogSnapshotReceiptV1, CoreError> {
    budget.checkpoint("catalog snapshot before integrity check")?;
    with_budgeted_sql(connection, path, budget, |map_error| {
        let integrity: String = connection
            .query_row("PRAGMA integrity_check", [], |row| row.get(0))
            .map_err(|error| map_error("integrity_check snapshot connection", path, &error))?;
        if !integrity.eq_ignore_ascii_case("ok") {
            return Err(CoreError::Typed {
                code: SearchPlaneErrorCodeV2::CatalogRowCorrupt,
                message: format!(
                    "catalog: snapshot connection {} failed integrity_check: {integrity}",
                    path.display()
                ),
            });
        }
        budget.checkpoint("catalog snapshot before content digest")?;
        let (content_digest_hex, table_rows) =
            catalog_content_digest_with_mapper(connection, path, map_error)?;
        budget.checkpoint("catalog snapshot after content digest")?;
        let page_count: i64 = connection
            .query_row("PRAGMA page_count", [], |row| row.get(0))
            .map_err(|error| map_error("read snapshot page count", path, &error))?;
        let page_size: i64 = connection
            .query_row("PRAGMA page_size", [], |row| row.get(0))
            .map_err(|error| map_error("read snapshot page size", path, &error))?;
        let pages = u64::try_from(page_count).map_err(|error| {
            CoreError::Storage(format!("catalog snapshot negative page count: {error}"))
        })?;
        let width = u64::try_from(page_size).map_err(|error| {
            CoreError::Storage(format!(
                "catalog snapshot invalid page size {page_size}: {error}"
            ))
        })?;
        if width == 0 {
            return Err(CoreError::Storage(
                "catalog snapshot page size is zero".into(),
            ));
        }
        let byte_size = pages
            .checked_mul(width)
            .ok_or_else(|| CoreError::Storage("catalog snapshot logical width overflow".into()))?;
        Ok(CatalogSnapshotReceiptV1 {
            destination: path.to_path_buf(),
            byte_size,
            content_digest_hex,
            table_rows,
        })
    })
}

fn with_budgeted_sql<T>(
    connection: &Connection,
    path: &Path,
    budget: &RequestBudgetV1,
    work: impl FnOnce(&dyn Fn(&str, &Path, &rusqlite::Error) -> CoreError) -> Result<T, CoreError>,
) -> Result<T, CoreError> {
    let observed = Arc::new(Mutex::new(None));
    let captured = Arc::clone(&observed);
    let request = budget.clone();
    connection
        .progress_handler(
            1024,
            Some(
                move || match request.checkpoint("catalog snapshot SQLite scan") {
                    Ok(()) => false,
                    Err(error) => {
                        match captured.lock() {
                            Ok(mut observed) => {
                                if observed.is_none() {
                                    *observed = Some(error);
                                }
                            }
                            Err(_) => return true,
                        }
                        true
                    }
                },
            ),
        )
        .map_err(|error| engine_error("install catalog snapshot progress handler", path, &error))?;
    let map_error = |action: &str, path: &Path, error: &rusqlite::Error| {
        if matches!(
            error,
            rusqlite::Error::SqliteFailure(failure, _)
                if failure.code == rusqlite::ErrorCode::OperationInterrupted
        ) {
            let cause = match observed.lock() {
                Ok(captured) => captured.clone(),
                Err(error) => {
                    return CoreError::Storage(format!(
                        "catalog snapshot progress state mutex poisoned: {error}"
                    ));
                }
            };
            if let Some(interruption) = cause {
                return interruption;
            }
        }
        engine_error(action, path, error)
    };
    let result = work(&map_error);
    // A cancelled budget must not interrupt transaction release, checked
    // close, or a later query on the same original connection.
    let cleared = connection
        .progress_handler(0, None::<fn() -> bool>)
        .map_err(|error| engine_error("clear catalog snapshot progress handler", path, &error));
    combine(result, cleared, "snapshot progress handler retirement")
}

fn close_slot(
    slot: &mut Option<CatalogConnectionOwner>,
    path: &Path,
    action: &str,
    failure: &mut Option<CoreError>,
) {
    let Some(owner) = slot.as_mut() else {
        return;
    };
    match owner.close_checked_with(|error| engine_error(action, path, &error)) {
        Ok(()) => {
            *slot = None;
        }
        Err(error) => match failure {
            Some(primary) => add_additional(primary, action, &error),
            None => *failure = Some(error),
        },
    }
}

fn combine<T>(
    work: Result<T, CoreError>,
    retirement: Result<(), CoreError>,
    context: &str,
) -> Result<T, CoreError> {
    match (work, retirement) {
        (Ok(value), Ok(())) => Ok(value),
        (Err(primary), Ok(())) | (Ok(_), Err(primary)) => Err(primary),
        (Err(mut primary), Err(additional)) => {
            add_additional(&mut primary, context, &additional);
            Err(primary)
        }
    }
}

fn add_additional(primary: &mut CoreError, context: &str, additional: &CoreError) {
    let message = match primary {
        CoreError::InvalidContract(message)
        | CoreError::NotReady(message)
        | CoreError::NotImplemented(message)
        | CoreError::NotFound(message)
        | CoreError::Storage(message)
        | CoreError::StorageIo { message, .. }
        | CoreError::Typed { message, .. } => message,
    };
    message.push_str("; ");
    message.push_str(context);
    message.push_str(" also failed: ");
    message.push_str(&additional.to_string());
}

#[cfg(test)]
mod tests {
    use std::io;
    use std::path::Path;
    use std::sync::mpsc;
    use std::time::Duration;

    use quanta_index_core::{CoreError, REQUEST_CANCELLED_CODE, RequestBudgetV1};
    use rusqlite::Connection;

    use super::with_budgeted_sql;

    #[test]
    fn native_progress_cancellation_retains_typed_cause_and_clears_before_rollback()
    -> Result<(), Box<dyn std::error::Error>> {
        let connection = Connection::open_in_memory()?;
        connection.execute_batch("BEGIN DEFERRED")?;
        let budget = RequestBudgetV1::for_duration(Duration::from_secs(30));
        let cancel = budget.cancel_handle();
        let (entered_tx, entered_rx) = mpsc::channel();
        let canceller = std::thread::spawn(move || -> io::Result<()> {
            entered_rx
                .recv_timeout(Duration::from_secs(5))
                .map_err(io::Error::other)?;
            cancel.cancel();
            Ok(())
        });
        let path = Path::new(":memory:");
        let interrupted = with_budgeted_sql(&connection, path, &budget, |map_error| {
            entered_tx
                .send(())
                .map_err(|error| CoreError::Storage(error.to_string()))?;
            connection
                .query_row(
                    "WITH RECURSIVE seq(x) AS (VALUES(1) UNION ALL \
                     SELECT x + 1 FROM seq WHERE x < 100000000) \
                     SELECT sum(x) FROM seq",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .map_err(|error| map_error("scan cancellation fixture", path, &error))
        });
        canceller.join().map_err(|payload| {
            io::Error::other(format!("canceller thread panicked: {payload:?}"))
        })??;
        match interrupted {
            Err(CoreError::Typed { code, .. }) if code == REQUEST_CANCELLED_CODE => {}
            other => {
                return Err(io::Error::other(format!(
                    "expected original typed request cancellation, got {other:?}"
                ))
                .into());
            }
        }
        let inside: i64 = connection.query_row("SELECT 7", [], |row| row.get(0))?;
        if inside != 7 {
            return Err(io::Error::other("query after cleared handler changed value").into());
        }
        connection.execute_batch("ROLLBACK")?;
        let after: i64 = connection.query_row("SELECT 9", [], |row| row.get(0))?;
        if after != 9 {
            return Err(io::Error::other("query after rollback changed value").into());
        }
        Ok(())
    }
}
