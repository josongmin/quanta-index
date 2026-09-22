//! Whole-catalog snapshot through the `SQLite` backup API (SEP-21 P10 /
//! S21-11), never a byte copy of the database and its WAL.
//!
//! Copying `catalog-v1.sqlite` and `catalog-v1.sqlite-wal` by hand is not a
//! snapshot: the engine checkpoints on its own schedule, so a hand copy can
//! pair a stale main file with a WAL prefix (or a truncated tail) and open
//! as a database that is missing acknowledged commits. The backup API is
//! the only producer of a consistent copy, and this module adds the two
//! things a receipt needs on top of it:
//!
//! * the copy is re-opened read-only, `PRAGMA integrity_check` must answer
//!   `ok`, and every user table's rows are re-read and digested in a
//!   canonical order (schema first, then per-table rows) so the digest is a
//!   function of the logical content, not of page layout; and
//! * both sides of that digest are returned, so a caller can compare the
//!   snapshot against the live catalog it was taken from and refuse a
//!   snapshot that is not equal to its source.

use std::path::{Path, PathBuf};
use std::time::Duration;

use quanta_index_contract::SearchPlaneErrorCodeV2;
use quanta_index_core::CoreError;
use rusqlite::Connection;
use rusqlite::backup::Backup;
use sha2::{Digest as _, Sha256};

use crate::connection::{CATALOG_FILE_NAME, catalog_dir, engine_error, storage};

/// How many pages the backup step copies per engine call.
const BACKUP_PAGES_PER_STEP: i32 = 256;

/// What a catalog snapshot produced: the destination file, its size, the
/// canonical content digest of the snapshot, and the row count per table.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CatalogSnapshotReceiptV1 {
    pub destination: PathBuf,
    pub byte_size: u64,
    /// Digest over the snapshot's schema plus every user table's rows, in
    /// canonical order (see [`catalog_content_digest`]).
    pub content_digest_hex: String,
    /// `(table name, row count)` in schema order.
    pub table_rows: Vec<(String, u64)>,
}

impl CatalogSnapshotReceiptV1 {
    /// Total rows across every user table.
    #[must_use]
    pub fn total_rows(&self) -> u64 {
        self.table_rows.iter().map(|(_name, rows)| rows).sum()
    }
}

/// The user tables of `connection` in schema order, excluding the engine's
/// own `sqlite_*` bookkeeping.
fn user_tables(connection: &Connection, path: &Path) -> Result<Vec<String>, CoreError> {
    let mut statement = connection
        .prepare(
            "SELECT name FROM sqlite_master \
             WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name",
        )
        .map_err(|error| engine_error("list snapshot tables", path, &error))?;
    let rows = statement
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|error| engine_error("list snapshot tables", path, &error))?;
    let mut tables = Vec::new();
    for row in rows {
        tables.push(row.map_err(|error| engine_error("list snapshot tables", path, &error))?);
    }
    Ok(tables)
}

fn feed_length_prefixed(hasher: &mut Sha256, bytes: &[u8]) {
    // A slice longer than `u64::MAX` cannot exist here; the saturation
    // idiom is the documented shape for a length prefix.
    let length = u64::try_from(bytes.len()).map_or(u64::MAX, |length| length);
    hasher.update(length.to_le_bytes());
    hasher.update(bytes);
}

fn feed_optional(hasher: &mut Sha256, value: Option<&[u8]>) {
    match value {
        Some(bytes) => {
            hasher.update([1_u8]);
            feed_length_prefixed(hasher, bytes);
        }
        None => hasher.update([0_u8]),
    }
}

/// Canonical content digest of a whole catalog.
///
/// The digest covers the exact schema text of every non-`sqlite_` object and
/// then, per user table in name order, the column names and every row's
/// values in `rowid` order. It is therefore independent of page layout,
/// free pages, WAL checkpoint state and `VACUUM` — two catalogs with equal
/// logical content digest equal, and any added, removed or altered row
/// changes it.
pub fn catalog_content_digest(
    connection: &Connection,
    path: &Path,
) -> Result<(String, Vec<(String, u64)>), CoreError> {
    let mut hasher = Sha256::new();
    hasher.update(b"quanta-index/catalog-content-digest/v1");
    let mut schema = connection
        .prepare(
            "SELECT type, name, tbl_name, COALESCE(sql, '') FROM sqlite_master \
             WHERE name NOT LIKE 'sqlite_%' ORDER BY type, name",
        )
        .map_err(|error| engine_error("read snapshot schema", path, &error))?;
    let schema_rows = schema
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })
        .map_err(|error| engine_error("read snapshot schema", path, &error))?;
    for row in schema_rows {
        let (kind, name, table, sql) =
            row.map_err(|error| engine_error("read snapshot schema", path, &error))?;
        feed_length_prefixed(&mut hasher, kind.as_bytes());
        feed_length_prefixed(&mut hasher, name.as_bytes());
        feed_length_prefixed(&mut hasher, table.as_bytes());
        feed_length_prefixed(&mut hasher, sql.as_bytes());
    }
    drop(schema);

    let mut table_rows = Vec::new();
    for table in user_tables(connection, path)? {
        feed_length_prefixed(&mut hasher, table.as_bytes());
        // The table name is interpolated, never a caller string: it came out
        // of `sqlite_master` and is quoted so a name with a quote cannot
        // change the statement.
        let quoted = table.replace('"', "\"\"");
        // Ordering by every column is the one order a `WITHOUT ROWID` table
        // and a rowid table agree on: a row's content decides its position,
        // and two equal rows are interchangeable. `rowid` is deliberately
        // not used — the catalog's authority tables are `WITHOUT ROWID`.
        let column_names: Vec<String> = {
            let probe = connection
                .prepare(&format!("SELECT * FROM \"{quoted}\" LIMIT 0"))
                .map_err(|error| engine_error("read snapshot table", path, &error))?;
            probe
                .column_names()
                .iter()
                .map(|name| (*name).to_string())
                .collect()
        };
        let order_by = column_names
            .iter()
            .map(|name| format!("\"{}\"", name.replace('"', "\"\"")))
            .collect::<Vec<String>>()
            .join(", ");
        let mut statement = connection
            .prepare(&format!("SELECT * FROM \"{quoted}\" ORDER BY {order_by}"))
            .map_err(|error| engine_error("read snapshot table", path, &error))?;
        let column_count = statement.column_count();
        let mut rows = statement
            .query([])
            .map_err(|error| engine_error("read snapshot table", path, &error))?;
        let mut count: u64 = 0;
        while let Some(row) = rows
            .next()
            .map_err(|error| engine_error("read snapshot table", path, &error))?
        {
            for index in 0..column_count {
                let value = row
                    .get_ref(index)
                    .map_err(|error| engine_error("read snapshot table", path, &error))?;
                match value {
                    rusqlite::types::ValueRef::Null => feed_optional(&mut hasher, None),
                    rusqlite::types::ValueRef::Integer(number) => {
                        hasher.update([2_u8]);
                        hasher.update(number.to_le_bytes());
                    }
                    rusqlite::types::ValueRef::Real(number) => {
                        hasher.update([3_u8]);
                        hasher.update(number.to_bits().to_le_bytes());
                    }
                    rusqlite::types::ValueRef::Text(text) => {
                        hasher.update([4_u8]);
                        feed_length_prefixed(&mut hasher, text);
                    }
                    rusqlite::types::ValueRef::Blob(bytes) => {
                        hasher.update([5_u8]);
                        feed_length_prefixed(&mut hasher, bytes);
                    }
                }
            }
            count = count.saturating_add(1);
        }
        drop(rows);
        table_rows.push((table, count));
    }
    Ok((hex_lower(&hasher.finalize()), table_rows))
}

/// One lowercase hex digit. The nibble is masked, so the value is always
/// `0..=15` and neither branch can overflow.
fn hex_digit(nibble: u8) -> char {
    let nibble = nibble & 0x0f;
    if nibble < 10 {
        char::from(b'0'.wrapping_add(nibble))
    } else {
        char::from(b'a'.wrapping_add(nibble.wrapping_sub(10)))
    }
}

fn hex_lower(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().saturating_mul(2));
    for byte in bytes {
        out.push(hex_digit(byte >> 4));
        out.push(hex_digit(byte & 0x0f));
    }
    out
}

/// Verify a produced snapshot: open it read-only, prove its integrity, and
/// digest its logical content.
///
/// The snapshot must be dropped before the destination is renamed or
/// removed, so this opens its own read-only handle and closes it before
/// returning.
pub fn verify_snapshot(path: &Path) -> Result<CatalogSnapshotReceiptV1, CoreError> {
    if !path.is_file() {
        return Err(CoreError::Typed {
            code: SearchPlaneErrorCodeV2::NotFound,
            message: format!("catalog snapshot {} is not a file", path.display()),
        });
    }
    let connection = Connection::open_with_flags(
        path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|error| engine_error("open snapshot", path, &error))?;
    let integrity: String = connection
        .query_row("PRAGMA integrity_check", [], |row| row.get(0))
        .map_err(|error| engine_error("integrity_check snapshot", path, &error))?;
    if !integrity.eq_ignore_ascii_case("ok") {
        return Err(CoreError::Typed {
            code: SearchPlaneErrorCodeV2::CatalogRowCorrupt,
            message: format!(
                "catalog: snapshot {} failed integrity_check: {integrity}",
                path.display()
            ),
        });
    }
    let (content_digest_hex, table_rows) = catalog_content_digest(&connection, path)?;
    let byte_size = std::fs::metadata(path)
        .map_err(|error| storage("stat snapshot", path, &error))?
        .len();
    drop(connection);
    Ok(CatalogSnapshotReceiptV1 {
        destination: path.to_path_buf(),
        byte_size,
        content_digest_hex,
        table_rows,
    })
}

/// The canonical content receipt of the catalog that lives under
/// `state_root`.
///
/// The connection is read-write because a WAL database cannot be opened
/// read-only without its shared-memory file; nothing here writes a row, and
/// the receipt is a function of the logical content alone.
pub fn live_catalog_receipt(
    state_root: &Path,
    busy_timeout: Duration,
) -> Result<CatalogSnapshotReceiptV1, CoreError> {
    let path = catalog_dir(state_root).join(CATALOG_FILE_NAME);
    if !path.is_file() {
        return Err(CoreError::Typed {
            code: SearchPlaneErrorCodeV2::NotFound,
            message: format!("catalog {} is not a file", path.display()),
        });
    }
    let connection =
        Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE)
            .map_err(|error| engine_error("open live catalog", &path, &error))?;
    connection
        .busy_timeout(busy_timeout)
        .map_err(|error| engine_error("set live catalog busy_timeout", &path, &error))?;
    let integrity: String = connection
        .query_row("PRAGMA integrity_check", [], |row| row.get(0))
        .map_err(|error| engine_error("integrity_check live catalog", &path, &error))?;
    if !integrity.eq_ignore_ascii_case("ok") {
        return Err(CoreError::Typed {
            code: SearchPlaneErrorCodeV2::CatalogRowCorrupt,
            message: format!(
                "catalog: {} failed integrity_check: {integrity}",
                path.display()
            ),
        });
    }
    let (content_digest_hex, table_rows) = catalog_content_digest(&connection, &path)?;
    let byte_size = std::fs::metadata(&path)
        .map_err(|error| storage("stat live catalog", &path, &error))?
        .len();
    drop(connection);
    Ok(CatalogSnapshotReceiptV1 {
        destination: path,
        byte_size,
        content_digest_hex,
        table_rows,
    })
}

/// Snapshot the catalog file at `source` into a brand-new file at
/// `destination` through the engine's backup API, then verify the copy.
///
/// The copy is left in rollback-journal mode: an offline artifact must be
/// readable without a shared-memory file, and the engine's own open restores
/// the WAL pragmas when the restored catalog becomes live.
///
/// `destination` must not exist: a snapshot that could silently overwrite a
/// previous one is not a freeze boundary. The parent directory is created
/// if needed and fsynced after the copy, and the returned receipt carries
/// the digest of what was actually produced.
pub fn snapshot_catalog_file(
    source: &Path,
    destination: &Path,
    busy_timeout: Duration,
) -> Result<CatalogSnapshotReceiptV1, CoreError> {
    if !source.is_file() {
        return Err(CoreError::Typed {
            code: SearchPlaneErrorCodeV2::NotFound,
            message: format!("catalog {} is not a file", source.display()),
        });
    }
    if destination.exists() {
        return Err(CoreError::Typed {
            code: SearchPlaneErrorCodeV2::InvalidRequest,
            message: format!(
                "catalog snapshot destination {} already exists; a snapshot never overwrites",
                destination.display()
            ),
        });
    }
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| storage("create snapshot directory", parent, &error))?;
    }
    {
        let source_connection =
            Connection::open_with_flags(source, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
                .map_err(|error| engine_error("open snapshot source", source, &error))?;
        source_connection
            .busy_timeout(busy_timeout)
            .map_err(|error| engine_error("set snapshot busy_timeout", source, &error))?;
        let mut destination_connection = Connection::open(destination)
            .map_err(|error| engine_error("create snapshot", destination, &error))?;
        let backup = Backup::new(&source_connection, &mut destination_connection)
            .map_err(|error| engine_error("begin snapshot", destination, &error))?;
        backup
            .run_to_completion(BACKUP_PAGES_PER_STEP, Duration::from_millis(0), None)
            .map_err(|error| engine_error("run snapshot", destination, &error))?;
        drop(backup);
        destination_connection
            .pragma_update(None, "journal_mode", "DELETE")
            .map_err(|error| engine_error("set snapshot journal_mode", destination, &error))?;
    }
    // The snapshot's own bytes must be durable before anything downstream
    // (a manifest, a rename) can advertise them.
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(destination)
        .map_err(|error| storage("reopen snapshot", destination, &error))?;
    file.sync_all()
        .map_err(|error| storage("fsync snapshot", destination, &error))?;
    drop(file);
    let receipt = verify_snapshot(destination)?;
    if let Some(parent) = destination.parent() {
        crate::fsync_directory(parent)?;
    }
    Ok(receipt)
}

/// Normalize a database that this process just produced into the offline
/// snapshot form: checkpoint it out of WAL and prove the result.
///
/// A root an offline operation produced itself (a migrated staging root's
/// freshly created catalog) still carries `WAL` from the engine's open. An
/// offline artifact must be readable without a shared-memory file, so the
/// journal mode is checkpointed to rollback here and the result re-verified
/// through the same read-only path an operator's `verify-state` takes.
pub fn normalize_catalog_journal_mode(path: &Path) -> Result<CatalogSnapshotReceiptV1, CoreError> {
    if !path.is_file() {
        return Err(CoreError::Typed {
            code: SearchPlaneErrorCodeV2::NotFound,
            message: format!("catalog {} is not a file", path.display()),
        });
    }
    {
        let connection =
            Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE)
                .map_err(|error| engine_error("open produced catalog", path, &error))?;
        let mode: String = connection
            .query_row("PRAGMA journal_mode = DELETE", [], |row| row.get(0))
            .map_err(|error| engine_error("checkpoint produced catalog", path, &error))?;
        if !mode.eq_ignore_ascii_case("delete") {
            return Err(CoreError::Storage(format!(
                "catalog: {} would not leave wal for the offline snapshot form (engine answered journal_mode={mode})",
                path.display()
            )));
        }
    }
    fsync_file(path)?;
    verify_snapshot(path)
}

fn fsync_file(path: &Path) -> Result<(), CoreError> {
    let handle = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .map_err(|error| storage("open produced catalog to fsync", path, &error))?;
    handle
        .sync_all()
        .map_err(|error| storage("fsync produced catalog", path, &error))
}

/// fsync a directory's own entries (`rename`, `create`, `unlink`).
pub fn fsync_directory(path: &Path) -> Result<(), CoreError> {
    let handle = std::fs::File::open(path)
        .map_err(|error| storage("open directory to fsync", path, &error))?;
    handle
        .sync_all()
        .map_err(|error| storage("fsync directory", path, &error))
}
