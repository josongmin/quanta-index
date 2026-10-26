//! One-use, bounded `SQLite` membership for a complete linked-inode track census.
//!
//! The main database has an explicit page cap. `SQLite` cache, memory journal,
//! filesystem allocation overhead, and other process memory are separate.

use std::path::Path;

use quanta_index_core::domains::reclaim_area::{
    RegularFileInodeV1, visit_regular_inode_tree_in_track,
};
use quanta_index_core::{CoreError, DiagnosticContextV1, RequestBudgetV1};
use rusqlite::{Connection, Transaction, params};
use tempfile::TempDir;

const PAGE_BYTES: u64 = 4096;

fn storage(action: &str, error: impl std::fmt::Display) -> CoreError {
    CoreError::Storage(format!("physical inode census {action}: {error}"))
}

fn sql_error(action: &str, error: &rusqlite::Error) -> CoreError {
    // A main-page cap and native disk exhaustion both report SQLITE_FULL.
    // Without an admission witness the cause remains an engine/storage failure.
    storage(action, error)
}

fn with_cleanup_context(primary: CoreError, cleanup: &CoreError) -> CoreError {
    let context = format!("; temporary directory cleanup failed: {cleanup}");
    match primary {
        CoreError::InvalidContract(message) => {
            CoreError::InvalidContract(format!("{message}{context}"))
        }
        CoreError::Typed { code, message } => CoreError::Typed {
            code,
            message: format!("{message}{context}"),
        },
        CoreError::NotReady(message) => CoreError::NotReady(format!("{message}{context}")),
        CoreError::NotImplemented(message) => {
            CoreError::NotImplemented(format!("{message}{context}"))
        }
        CoreError::NotFound(message) => CoreError::NotFound(format!("{message}{context}")),
        CoreError::Storage(message) => CoreError::Storage(format!("{message}{context}")),
        CoreError::StorageIo {
            kind,
            raw_os_error,
            message,
        } => CoreError::StorageIo {
            kind,
            raw_os_error,
            message: format!("{message}{context}"),
        },
    }
}

fn finish_with_cleanup<T>(
    directory: TempDir,
    measured: Result<T, CoreError>,
) -> Result<T, CoreError> {
    let cleaned = directory
        .close()
        .map_err(|error| storage("close temporary directory", error));
    match (measured, cleaned) {
        (Ok(value), Ok(())) => Ok(value),
        (Ok(_), Err(cleanup)) => Err(cleanup),
        (Err(primary), Ok(())) => Err(primary),
        (Err(primary), Err(cleanup)) => Err(with_cleanup_context(primary, &cleanup)),
    }
}

fn open_db(max_main_file_bytes: u64) -> Result<(TempDir, Connection), CoreError> {
    if !max_main_file_bytes.is_multiple_of(PAGE_BYTES) {
        return Err(CoreError::InvalidContract(
            "physical inode census main-file limit must be page aligned".into(),
        ));
    }
    let max_pages = max_main_file_bytes.checked_div(PAGE_BYTES).ok_or_else(|| {
        CoreError::InvalidContract("physical inode census page size is zero".into())
    })?;
    if max_pages == 0 || max_pages > u64::from(u32::MAX.saturating_sub(1)) {
        return Err(CoreError::InvalidContract(
            "physical inode census main-file limit outside SQLite page range".into(),
        ));
    }
    let directory = tempfile::tempdir().map_err(|error| storage("tempdir", error))?;
    let opened = (|| {
        let db = Connection::open(directory.path().join("physical-inodes.sqlite"))
            .map_err(|error| sql_error("open", &error))?;
        db.execute_batch(
            "PRAGMA page_size=4096;
         PRAGMA journal_mode=MEMORY;
         PRAGMA temp_store=MEMORY;
         PRAGMA synchronous=OFF;
         PRAGMA mmap_size=0;",
        )
        .map_err(|error| sql_error("configure", &error))?;
        let page_size: i64 = db
            .query_row("PRAGMA main.page_size", [], |row| row.get(0))
            .map_err(|error| sql_error("page size", &error))?;
        let journal_mode: String = db
            .query_row("PRAGMA main.journal_mode", [], |row| row.get(0))
            .map_err(|error| sql_error("journal mode", &error))?;
        let temp_store: i64 = db
            .query_row("PRAGMA temp_store", [], |row| row.get(0))
            .map_err(|error| sql_error("temp store", &error))?;
        let page_size = u64::try_from(page_size).map_err(|error| {
            CoreError::Storage(format!(
                "physical inode census SQLite page size invalid: {error}"
            ))
        })?;
        if page_size != PAGE_BYTES || journal_mode != "memory" || temp_store != 2 {
            return Err(CoreError::Storage(
                "physical inode census SQLite bounded configuration refused".into(),
            ));
        }
        let cache_kib = max_main_file_bytes
            .checked_div(1024)
            .ok_or_else(|| {
                CoreError::InvalidContract("physical inode census KiB divisor is zero".into())
            })?
            .clamp(4, 2048);
        db.execute_batch(&format!("PRAGMA cache_size=-{cache_kib};"))
            .map_err(|error| sql_error("cache cap", &error))?;
        let installed: i64 = db
            .query_row(
                &format!("PRAGMA main.max_page_count={max_pages}"),
                [],
                |row| row.get(0),
            )
            .map_err(|error| sql_error("main page cap", &error))?;
        let installed = u64::try_from(installed).map_err(|error| {
            CoreError::Storage(format!(
                "physical inode census SQLite main page cap invalid: {error}"
            ))
        })?;
        if installed != max_pages {
            return Err(CoreError::Storage(
                "physical inode census SQLite main page cap refused".into(),
            ));
        }
        db.execute_batch(
            "CREATE TABLE inode (
           device BLOB NOT NULL CHECK(length(device)=8),
           inode BLOB NOT NULL CHECK(length(inode)=8),
           bytes BLOB NOT NULL CHECK(length(bytes)=8),
           PRIMARY KEY(device,inode)
         ) WITHOUT ROWID;",
        )
        .map_err(|error| sql_error("schema", &error))?;
        Ok(db)
    })();
    match opened {
        Ok(db) => Ok((directory, db)),
        Err(primary) => finish_with_cleanup(directory, Err(primary)),
    }
}

fn record_inode(
    tx: &Transaction<'_>,
    total: &mut u64,
    file: RegularFileInodeV1,
) -> Result<Option<u64>, CoreError> {
    let device = file.device.to_be_bytes();
    let inode = file.inode.to_be_bytes();
    let bytes = file.bytes.to_be_bytes();
    let inserted = tx
        .execute(
            "INSERT INTO inode(device,inode,bytes) VALUES (?1,?2,?3)
             ON CONFLICT(device,inode) DO NOTHING",
            params![&device[..], &inode[..], &bytes[..]],
        )
        .map_err(|error| sql_error("record inode", &error))?;
    if inserted == 0 {
        let prior: Vec<u8> = tx
            .query_row(
                "SELECT bytes FROM inode WHERE device=?1 AND inode=?2",
                params![&device[..], &inode[..]],
                |row| row.get(0),
            )
            .map_err(|error| sql_error("read duplicate inode", &error))?;
        if prior.as_slice() != &bytes[..] {
            return Err(CoreError::Storage(
                "physical inode census conflicting length for one inode".into(),
            ));
        }
        return Ok(None);
    }
    if inserted != 1 {
        return Err(CoreError::Storage(
            "physical inode census inserted unexpected row count".into(),
        ));
    }
    *total = total
        .checked_add(file.bytes)
        .ok_or_else(|| CoreError::Storage("physical inode census byte total overflow".into()))?;
    Ok(Some(file.bytes))
}

/// Count each linked regular-file inode in one track once.
///
/// `max_main_file_bytes` caps this call's disposable `SQLite` main file, not
/// process RSS or full filesystem allocation. The caller owns a stable track
/// snapshot; this helper acquires no writer, pool, or lifecycle lock.
pub fn measure_physical_inode_tree_bytes(
    track_root: &Path,
    skip: &dyn Fn(&str) -> bool,
    max_main_file_bytes: u64,
    budget: &RequestBudgetV1,
    diagnostic: Option<&DiagnosticContextV1>,
) -> Result<u64, CoreError> {
    measure_inode_tree_bytes(track_root, skip, max_main_file_bytes, budget, diagnostic)
}

/// Best-effort linked-inode census for a live track's diagnostic disk gauge.
///
/// Writers may change the tree during this walk, so the returned total may
/// combine observations from different instants. A conflicting inode length,
/// changed directory, I/O failure, exhausted budget, or `SQLite` failure is
/// still an error. This result must not authorize admission, retention, or an
/// exact physical snapshot. No writer, pool, or lifecycle lock is acquired.
pub fn measure_live_inode_tree_bytes(
    track_root: &Path,
    skip: &dyn Fn(&str) -> bool,
    max_main_file_bytes: u64,
    budget: &RequestBudgetV1,
    diagnostic: Option<&DiagnosticContextV1>,
) -> Result<u64, CoreError> {
    measure_inode_tree_bytes(track_root, skip, max_main_file_bytes, budget, diagnostic)
}

fn measure_inode_tree_bytes(
    track_root: &Path,
    skip: &dyn Fn(&str) -> bool,
    max_main_file_bytes: u64,
    budget: &RequestBudgetV1,
    diagnostic: Option<&DiagnosticContextV1>,
) -> Result<u64, CoreError> {
    budget.checkpoint("physical-inode-census:entry")?;
    let (directory, mut db) = open_db(max_main_file_bytes)?;
    let measured = (|| {
        let tx = db
            .transaction()
            .map_err(|error| sql_error("begin", &error))?;
        let mut total = 0_u64;
        let _present =
            visit_regular_inode_tree_in_track(track_root, skip, budget, diagnostic, &mut |file| {
                record_inode(&tx, &mut total, file)
            })?;
        budget.checkpoint("physical-inode-census:commit")?;
        tx.commit().map_err(|error| sql_error("commit", &error))?;
        Ok(total)
    })();
    drop(db);
    let total = finish_with_cleanup(directory, measured)?;
    budget.checkpoint("physical-inode-census:complete")?;
    Ok(total)
}

#[cfg(test)]
mod tests {
    use std::error::Error;
    use std::fs;
    use std::time::Duration;

    use super::*;

    fn assertion_error(detail: String) -> Box<dyn std::error::Error> {
        std::io::Error::other(detail).into()
    }

    macro_rules! check {
        ($condition:expr $(,)?) => {{
            if !$condition {
                return Err(assertion_error(format!(
                    "assertion failed: {}",
                    stringify!($condition)
                )));
            }
        }};
        ($condition:expr, $($message:tt)+) => {{
            if !$condition {
                return Err(assertion_error(format!($($message)+)));
            }
        }};
    }

    macro_rules! check_eq {
        ($left:expr, $right:expr $(,)?) => {{
            let left = &$left;
            let right = &$right;
            if left != right {
                return Err(assertion_error(format!(
                    "assertion failed: left {left:?} != right {right:?}"
                )));
            }
        }};
        ($left:expr, $right:expr, $($message:tt)+) => {{
            let left = &$left;
            let right = &$right;
            if left != right {
                return Err(assertion_error(format!(
                    "assertion failed: left {left:?} != right {right:?}: {}",
                    format_args!($($message)+)
                )));
            }
        }};
    }

    type TestResult = Result<(), Box<dyn Error>>;

    #[test]
    fn unscoped_full_nomem_and_io_errors_are_not_retention_admission() -> TestResult {
        for code in [
            rusqlite::ffi::SQLITE_FULL,
            rusqlite::ffi::SQLITE_NOMEM,
            rusqlite::ffi::SQLITE_IOERR_WRITE,
            rusqlite::ffi::SQLITE_IOERR_FSYNC,
        ] {
            let error = rusqlite::Error::SqliteFailure(rusqlite::ffi::Error::new(code), None);
            check!(matches!(sql_error("write", &error), CoreError::Storage(_)));
        }
        Ok(())
    }

    #[test]
    fn whole_track_counts_fixed_hardlink_oracle_and_absence() -> TestResult {
        let root = tempfile::tempdir()?;
        let track = root.path().join("track");
        fs::create_dir_all(track.join("family/g1"))?;
        fs::create_dir(track.join(".reclaim"))?;
        fs::write(track.join("family/g1/one"), b"12345")?;
        fs::hard_link(track.join("family/g1/one"), track.join(".reclaim/shared"))?;
        fs::write(track.join("family/g1/two"), b"1234567")?;
        fs::write(track.join("family/g1/skip"), b"999")?;
        let budget = RequestBudgetV1::for_duration(Duration::from_secs(5));
        check_eq!(
            measure_physical_inode_tree_bytes(
                &track,
                &|name| name == "skip",
                128 * 1024,
                &budget,
                None
            )?,
            12,
        );
        check_eq!(
            measure_physical_inode_tree_bytes(
                &root.path().join("absent"),
                &|_| false,
                128 * 1024,
                &budget,
                None
            )?,
            0,
        );
        Ok(())
    }

    #[test]
    fn live_track_counts_each_linked_inode_once_and_keeps_validation() -> TestResult {
        let root = tempfile::tempdir()?;
        let track = root.path().join("live");
        fs::create_dir_all(track.join("generation"))?;
        fs::create_dir(track.join("reclaim"))?;
        fs::write(track.join("generation/first"), b"12345")?;
        fs::hard_link(
            track.join("generation/first"),
            track.join("reclaim/same-inode"),
        )?;
        fs::write(track.join("generation/second"), b"1234567")?;
        fs::write(track.join("generation/ignored"), b"999")?;
        let budget = RequestBudgetV1::for_duration(Duration::from_secs(5));
        check_eq!(
            measure_live_inode_tree_bytes(
                &track,
                &|name| name == "ignored",
                128 * 1024,
                &budget,
                None,
            )?,
            12,
        );
        check!(matches!(
            measure_live_inode_tree_bytes(&track, &|_| false, 7, &budget, None),
            Err(CoreError::InvalidContract(_))
        ));
        Ok(())
    }

    #[test]
    fn cap_and_overflow_refuse_without_publishing_partial_total() -> TestResult {
        let (_directory, mut db) = open_db(128 * 1024)?;
        let tx = db.transaction()?;
        let mut total = 0_u64;
        check_eq!(
            record_inode(
                &tx,
                &mut total,
                RegularFileInodeV1 {
                    device: 1,
                    inode: 1,
                    bytes: u64::MAX
                },
            )?,
            Some(u64::MAX),
        );
        check!(matches!(
            record_inode(
                &tx,
                &mut total,
                RegularFileInodeV1 {
                    device: 1,
                    inode: 2,
                    bytes: 1
                },
            ),
            Err(CoreError::Storage(_))
        ));
        check_eq!(total, u64::MAX);
        check!(matches!(
            record_inode(
                &tx,
                &mut total,
                RegularFileInodeV1 {
                    device: 1,
                    inode: 1,
                    bytes: 5
                },
            ),
            Err(CoreError::Storage(_))
        ));
        drop(tx);

        let tx = db.transaction()?;
        let mut charged = 0_u64;
        let mut refused = false;
        for inode in 0..20_000_u64 {
            match record_inode(
                &tx,
                &mut charged,
                RegularFileInodeV1 {
                    device: 2,
                    inode,
                    bytes: 1,
                },
            ) {
                Ok(_) => {}
                Err(CoreError::Storage(_) | CoreError::StorageIo { .. }) => {
                    refused = true;
                    break;
                }
                Err(error) => return Err(Box::new(error)),
            }
        }
        check!(
            refused,
            "fixed 128 KiB main-file cap must refuse 20,000 unique rows"
        );
        drop(tx);
        let root = tempfile::tempdir()?;
        fs::write(root.path().join("small"), b"ok")?;
        check_eq!(
            measure_physical_inode_tree_bytes(
                root.path(),
                &|_| false,
                128 * 1024,
                &RequestBudgetV1::unbounded(),
                None,
            )?,
            2,
        );
        Ok(())
    }

    #[test]
    fn cancelled_budget_refuses_before_scratch_mutation() -> TestResult {
        let root = tempfile::tempdir()?;
        fs::write(root.path().join("file"), b"payload")?;
        let budget = RequestBudgetV1::for_duration(Duration::from_secs(1));
        budget.cancel_handle().cancel();
        check!(matches!(
            measure_physical_inode_tree_bytes(root.path(), &|_| false, 128 * 1024, &budget, None),
            Err(CoreError::Typed {
                code: quanta_index_core::REQUEST_CANCELLED_CODE,
                ..
            })
        ));
        Ok(())
    }
}
