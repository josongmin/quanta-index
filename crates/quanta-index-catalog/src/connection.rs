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
use std::sync::Mutex;
use std::time::Duration;

use quanta_index_contract::ManifestGeneration;
use quanta_index_core::{CATALOG_BUSY_CODE, CATALOG_ROW_CORRUPT_CODE, CoreError};
use rusqlite::{Connection, OpenFlags};

/// The catalog database file, under `state_root/catalog/`.
pub const CATALOG_FILE_NAME: &str = "catalog-v1.sqlite";

/// The catalog directory under a state root.
#[must_use]
pub fn catalog_dir(state_root: &Path) -> PathBuf {
    state_root.join("catalog")
}

/// The search plane's durable catalog over one `SQLite` file: the ingest
/// idempotency records (QI-BB-032) and the auxiliary authority rows
/// (QI-BB-020).
pub struct SqliteCatalog {
    pub(crate) connection: Mutex<Connection>,
    pub(crate) path: PathBuf,
}

impl std::fmt::Debug for SqliteCatalog {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SqliteCatalog")
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

pub(crate) fn storage(action: &str, path: &Path, error: &dyn std::fmt::Display) -> CoreError {
    CoreError::Storage(format!("catalog: {action} {}: {error}", path.display()))
}

/// Map an engine error to the typed codes the ports promise.
pub(crate) fn engine_error(action: &str, path: &Path, error: &rusqlite::Error) -> CoreError {
    if let rusqlite::Error::SqliteFailure(failure, _) = error
        && matches!(
            failure.code,
            rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked
        )
    {
        return CoreError::Typed {
            code: CATALOG_BUSY_CODE.to_string(),
            message: format!(
                "catalog: {action} {} met a held lock past the busy budget: {error}",
                path.display()
            ),
        };
    }
    storage(action, path, error)
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
        code: CATALOG_ROW_CORRUPT_CODE.to_string(),
        message: format!("catalog: {label} is {} bytes, expected 32", bytes.len()),
    })
}

pub(crate) fn count_u64(label: &str, count: usize) -> Result<u64, CoreError> {
    u64::try_from(count)
        .map_err(|error| CoreError::Storage(format!("catalog: {label} count overflow: {error}")))
}

/// Open (creating if needed) the catalog file under `state_root/catalog/`
/// with the pragmas the G0-C gate requires, each read back; returns the
/// connection and the file's path.
pub(crate) fn open_connection(
    state_root: &Path,
    busy_timeout: Duration,
) -> Result<(Connection, PathBuf), CoreError> {
    let directory = catalog_dir(state_root);
    std::fs::create_dir_all(&directory)
        .map_err(|error| storage("create directory", &directory, &error))?;
    let path = directory.join(CATALOG_FILE_NAME);
    let connection = Connection::open_with_flags(
        &path,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE,
    )
    .map_err(|error| engine_error("open", &path, &error))?;
    // Each pragma is read back rather than assumed: `journal_mode` is a
    // request the engine may decline, and a catalog that silently ran
    // under `NORMAL` would ack work the engine can forget (G0-C).
    connection
        .pragma_update(None, "journal_mode", "WAL")
        .map_err(|error| engine_error("set journal_mode", &path, &error))?;
    connection
        .pragma_update(None, "synchronous", "FULL")
        .map_err(|error| engine_error("set synchronous", &path, &error))?;
    connection
        .pragma_update(None, "fullfsync", "ON")
        .map_err(|error| engine_error("set fullfsync", &path, &error))?;
    connection
        .pragma_update(None, "foreign_keys", "ON")
        .map_err(|error| engine_error("set foreign_keys", &path, &error))?;
    connection
        .busy_timeout(busy_timeout)
        .map_err(|error| engine_error("set busy_timeout", &path, &error))?;
    let journal_mode: String = connection
        .pragma_query_value(None, "journal_mode", |row| row.get(0))
        .map_err(|error| engine_error("read journal_mode", &path, &error))?;
    let synchronous: i64 = connection
        .pragma_query_value(None, "synchronous", |row| row.get(0))
        .map_err(|error| engine_error("read synchronous", &path, &error))?;
    if !journal_mode.eq_ignore_ascii_case("wal") || synchronous != 2 {
        return Err(CoreError::Storage(format!(
            "catalog: {} runs journal_mode={journal_mode} synchronous={synchronous}; the catalog requires wal/FULL",
            path.display()
        )));
    }
    Ok((connection, path))
}

impl SqliteCatalog {
    /// The database file.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) fn lock(&self) -> Result<std::sync::MutexGuard<'_, Connection>, CoreError> {
        self.connection.lock().map_err(|error| {
            CoreError::Storage(format!(
                "catalog: connection to {} poisoned: {error}",
                self.path.display()
            ))
        })
    }
}
