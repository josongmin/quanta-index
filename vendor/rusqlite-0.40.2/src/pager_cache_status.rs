//! Non-resetting, connection-local pager cache counters for bundled SQLite.

use crate::error::check;
use crate::{ffi, Connection, Error, Result};

/// Cumulative pager cache events on one SQLite connection.
///
/// The values cover databases attached at the time of the call. They are page
/// counts, not bytes, physical I/O calls, or wall time. Detaching a database
/// or resetting status through the raw handle may decrease a later snapshot.
/// SQLite does not define the subsequent `write` value after a page-write I/O
/// error.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PagerCacheStatus {
    /// Pager cache hits.
    pub hit: u64,
    /// Pager cache misses.
    pub miss: u64,
    /// Dirty pages written to a WAL or database file.
    pub write: u64,
    /// Dirty pages spilled from the cache during a transaction.
    pub spill: u64,
}

impl Connection {
    /// Read cumulative pager cache counters without resetting them.
    ///
    /// This API is available only with bundled, non-SQLCipher SQLite. An
    /// unsupported SQLite status operation or a negative counter is an error;
    /// a returned zero is an observed zero.
    pub fn pager_cache_status(&self) -> Result<PagerCacheStatus> {
        let connection = self.db.borrow();
        let db = connection.db();
        let read = |operation| {
            let mut current = 0_i64;
            let mut highwater = 0_i64;
            // SAFETY: `connection` keeps this handle alive for every call, and
            // SQLite writes only the two provided integers. No reset is made.
            let code =
                unsafe { ffi::sqlite3_db_status64(db, operation, &mut current, &mut highwater, 0) };
            check(code)?;
            u64::try_from(current).map_err(|_| {
                Error::SqliteFailure(
                    ffi::Error::new(ffi::SQLITE_ERROR),
                    Some("negative SQLite pager cache status".into()),
                )
            })
        };
        Ok(PagerCacheStatus {
            hit: read(ffi::SQLITE_DBSTATUS_CACHE_HIT)?,
            miss: read(ffi::SQLITE_DBSTATUS_CACHE_MISS)?,
            write: read(ffi::SQLITE_DBSTATUS_CACHE_WRITE)?,
            spill: read(ffi::SQLITE_DBSTATUS_CACHE_SPILL)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_pager_status_observes_spill_reads_and_nonreset_snapshots(
    ) -> std::result::Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let connection = Connection::open(directory.path().join("pager.db"))?;
        connection.execute_batch(
            "PRAGMA page_size=4096;
             PRAGMA cache_size=10;
             PRAGMA cache_spill=ON;
             CREATE TABLE pager_data (id INTEGER PRIMARY KEY, payload BLOB NOT NULL);",
        )?;
        let before = connection.pager_cache_status()?;
        let payload = vec![0x5a_u8; 3000];
        connection.execute_batch("BEGIN IMMEDIATE")?;
        for id in 0..256_i64 {
            connection.execute(
                "INSERT INTO pager_data (id, payload) VALUES (?1, ?2)",
                (id, payload.as_slice()),
            )?;
        }
        let in_transaction = connection.pager_cache_status()?;
        assert!(in_transaction.spill > before.spill);
        connection.execute_batch("COMMIT; PRAGMA shrink_memory;")?;
        let bytes: i64 =
            connection.query_row("SELECT SUM(length(payload)) FROM pager_data", [], |row| {
                row.get(0)
            })?;
        assert_eq!(bytes, 256 * 3000);
        let after = connection.pager_cache_status()?;
        assert!(after.hit > before.hit);
        assert!(after.miss > before.miss);
        assert!(after.write > before.write);
        assert!(after.spill >= in_transaction.spill);
        assert_eq!(connection.pager_cache_status()?, after);
        Ok(())
    }
}
