//! Explicit, connection-owned `SQLite` writer page-cache policy.

use quanta_index_core::CoreError;
use rusqlite::Connection;

use crate::connection::engine_error;
use std::path::Path;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CatalogWriterCachePolicy {
    pub kib: u32,
}

impl CatalogWriterCachePolicy {
    pub fn validate(self) -> Result<(), CoreError> {
        let _bytes = self.reservation_bytes()?;
        Ok(())
    }

    pub fn reservation_bytes(self) -> Result<u64, CoreError> {
        if self.kib == 0 || self.kib > i32::MAX.cast_unsigned() {
            return Err(CoreError::InvalidContract(
                "catalog writer cache KiB must be positive and fit SQLite's signed pragma".into(),
            ));
        }
        u64::from(self.kib)
            .checked_mul(1024)
            .ok_or_else(|| CoreError::InvalidContract("catalog writer cache bytes overflow".into()))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CatalogWriterSettings {
    pub page_size_bytes: u64,
    pub cache_size: i64,
    pub cache_spill_pages: u64,
    pub wal_autocheckpoint_pages: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CatalogWriterCacheObservation {
    pub before: CatalogWriterSettings,
    pub after: CatalogWriterSettings,
}

fn read_positive(connection: &Connection, path: &Path, name: &str) -> Result<u64, CoreError> {
    let value: i64 = connection
        .pragma_query_value(None, name, |row| row.get(0))
        .map_err(|error| engine_error(&format!("read {name}"), path, &error))?;
    if value <= 0 {
        return Err(CoreError::Storage(format!(
            "catalog: {} has invalid {name}={value}",
            path.display()
        )));
    }
    u64::try_from(value).map_err(|error| {
        CoreError::Storage(format!(
            "catalog: {} cannot represent {name}={value}: {error}",
            path.display()
        ))
    })
}

fn read_nonnegative(connection: &Connection, path: &Path, name: &str) -> Result<u64, CoreError> {
    let value: i64 = connection
        .pragma_query_value(None, name, |row| row.get(0))
        .map_err(|error| engine_error(&format!("read {name}"), path, &error))?;
    u64::try_from(value).map_err(|_error| {
        CoreError::Storage(format!(
            "catalog: {} has invalid {name}={value}",
            path.display()
        ))
    })
}

pub(crate) fn read_settings(
    connection: &Connection,
    path: &Path,
) -> Result<CatalogWriterSettings, CoreError> {
    let cache_size = connection
        .pragma_query_value(None, "cache_size", |row| row.get(0))
        .map_err(|error| engine_error("read cache_size", path, &error))?;
    Ok(CatalogWriterSettings {
        page_size_bytes: read_positive(connection, path, "page_size")?,
        cache_size,
        cache_spill_pages: read_nonnegative(connection, path, "cache_spill")?,
        wal_autocheckpoint_pages: read_nonnegative(connection, path, "wal_autocheckpoint")?,
    })
}

pub(crate) fn apply(
    connection: &Connection,
    path: &Path,
    policy: CatalogWriterCachePolicy,
) -> Result<CatalogWriterCacheObservation, CoreError> {
    let before = read_settings(connection, path)?;
    let requested = i64::from(policy.kib).checked_neg().ok_or_else(|| {
        CoreError::InvalidContract("catalog writer cache signed KiB overflow".into())
    })?;
    connection
        .pragma_update(None, "cache_size", requested)
        .map_err(|error| engine_error("set cache_size", path, &error))?;
    connection
        .pragma_update(None, "cache_spill", "ON")
        .map_err(|error| engine_error("set cache_spill", path, &error))?;
    let after = read_settings(connection, path)?;
    if after.cache_size != requested
        || after.cache_spill_pages == 0
        || after.page_size_bytes != before.page_size_bytes
        || after.wal_autocheckpoint_pages != before.wal_autocheckpoint_pages
    {
        return Err(CoreError::Storage(format!(
            "catalog: {} writer cache settings were not applied exactly: requested cache_size={requested}, before={before:?}, after={after:?}",
            path.display()
        )));
    }
    Ok(CatalogWriterCacheObservation { before, after })
}

#[cfg(test)]
mod tests {
    use std::{
        error::Error,
        fs::File,
        path::Path,
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
        time::Duration,
    };

    use quanta_index_core::{
        DiagnosticContextV1,
        index::{IndexError, MemoryLease, MemoryLeaseOwner, ObjectReadFile},
    };
    use rusqlite::{Connection, params};

    use super::{CatalogWriterCachePolicy, apply};
    use crate::{CatalogIoAdmission, CatalogIoScope, SqliteCatalog};

    fn require(condition: bool, detail: &'static str) -> Result<(), Box<dyn Error>> {
        if condition {
            Ok(())
        } else {
            Err(detail.into())
        }
    }

    struct LeaseCount(AtomicUsize);

    impl MemoryLeaseOwner for LeaseCount {
        fn release(&self, _bytes: u64) {
            let _previous = self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    fn lease(bytes: u64) -> (MemoryLease, Arc<LeaseCount>) {
        let counter = Arc::new(LeaseCount(AtomicUsize::new(0)));
        (MemoryLease::new(bytes, counter.clone()), counter)
    }

    struct NoopScope;

    impl CatalogIoScope for NoopScope {
        fn is_capacity_refusal(&self, _error: &rusqlite::Error) -> bool {
            false
        }
    }

    struct UnixAdmission;

    impl CatalogIoAdmission for UnixAdmission {
        fn vfs_name(&self) -> &'static str {
            "unix"
        }

        fn enter_scope(
            &self,
            _diagnostics: Option<&DiagnosticContextV1>,
        ) -> Box<dyn CatalogIoScope> {
            Box::new(NoopScope)
        }

        fn enter_close_scope(&self) -> Box<dyn CatalogIoScope> {
            Box::new(NoopScope)
        }

        fn open_lock_file(
            &self,
            _path: &Path,
            _create_new: bool,
        ) -> Result<Box<dyn ObjectReadFile>, IndexError> {
            Err(IndexError::BackendViolation("unused test lock file".into()))
        }

        fn unlink_lock_file(&self, _path: &Path, _held: &File) -> Result<(), IndexError> {
            Err(IndexError::BackendViolation("unused test lock file".into()))
        }
    }

    #[test]
    fn invalid_or_unfunded_policy_refuses_before_catalog_file() -> Result<(), Box<dyn Error>> {
        let root = tempfile::tempdir()?;
        let admission: Arc<dyn CatalogIoAdmission> = Arc::new(UnixAdmission);
        for (policy, bytes) in [
            (CatalogWriterCachePolicy { kib: 0 }, 1),
            (
                CatalogWriterCachePolicy {
                    kib: i32::MAX
                        .cast_unsigned()
                        .checked_add(1)
                        .ok_or("invalid policy fixture overflow")?,
                },
                1024,
            ),
            (CatalogWriterCachePolicy { kib: 64 }, 1024),
        ] {
            let (memory, counter) = lease(bytes);
            let result = SqliteCatalog::open_with_io_admission_and_writer_cache(
                root.path(),
                Duration::ZERO,
                admission.clone(),
                policy,
                memory,
            );
            require(result.is_err(), "invalid or unfunded policy was accepted")?;
            require(
                counter.0.load(Ordering::SeqCst) == 1,
                "refused policy did not refund its lease exactly once",
            )?;
            require(
                !root.path().join("catalog").exists(),
                "refused policy created a catalog directory",
            )?;
        }
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn funded_policy_is_read_back_and_lease_survives_until_checked_close()
    -> Result<(), Box<dyn Error>> {
        let root = tempfile::tempdir()?;
        let policy = CatalogWriterCachePolicy { kib: 64 };
        let (memory, counter) = lease(policy.reservation_bytes()?);
        let admission: Arc<dyn CatalogIoAdmission> = Arc::new(UnixAdmission);
        let mut catalog = SqliteCatalog::open_with_io_admission_and_writer_cache(
            root.path(),
            Duration::from_secs(1),
            admission,
            policy,
            memory,
        )?;
        let observed = catalog
            .writer_cache_observation()
            .ok_or("missing cache observation")?;
        require(
            observed.after.cache_size == -64,
            "writer cache size differs",
        )?;
        require(
            observed.after.cache_spill_pages > 0,
            "writer cache disabled spill protection",
        )?;
        require(
            observed.before.page_size_bytes == observed.after.page_size_bytes,
            "writer cache changed page size",
        )?;
        require(
            observed.before.wal_autocheckpoint_pages == observed.after.wal_autocheckpoint_pages,
            "writer cache changed WAL autocheckpoint policy",
        )?;
        require(
            counter.0.load(Ordering::SeqCst) == 0,
            "writer cache lease released before close",
        )?;
        catalog.close_checked()?;
        require(
            counter.0.load(Ordering::SeqCst) == 1,
            "checked close did not release writer cache lease once",
        )?;
        catalog.close_checked()?;
        require(
            counter.0.load(Ordering::SeqCst) == 1,
            "repeat close released writer cache lease twice",
        )?;
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn initialization_refusal_closes_and_refunds_funded_connection() -> Result<(), Box<dyn Error>> {
        let root = tempfile::tempdir()?;
        let directory = root.path().join("catalog");
        std::fs::create_dir_all(&directory)?;
        let path = directory.join(crate::CATALOG_FILE_NAME);
        let prior = Connection::open(&path)?;
        prior.execute_batch("CREATE TABLE idempotency_v1 (id INTEGER)")?;
        drop(prior);
        let policy = CatalogWriterCachePolicy { kib: 64 };
        let (memory, counter) = lease(policy.reservation_bytes()?);
        let admission: Arc<dyn CatalogIoAdmission> = Arc::new(UnixAdmission);
        require(
            SqliteCatalog::open_with_io_admission_and_writer_cache(
                root.path(),
                Duration::from_secs(1),
                admission,
                policy,
                memory,
            )
            .is_err(),
            "invalid catalog initialization was accepted",
        )?;
        require(
            counter.0.load(Ordering::SeqCst) == 1,
            "failed initialization did not refund its lease once",
        )?;
        Ok(())
    }

    #[test]
    fn larger_explicit_cache_reduces_dirty_page_spills_for_identical_rows()
    -> Result<(), Box<dyn Error>> {
        fn run(kib: u32) -> Result<u64, Box<dyn Error>> {
            let root = tempfile::tempdir()?;
            let path = root.path().join("cache.sqlite");
            let mut connection = Connection::open(&path)?;
            connection.pragma_update(None, "journal_mode", "WAL")?;
            let policy = CatalogWriterCachePolicy { kib };
            let observed = apply(&connection, &path, policy)?;
            require(
                observed.after.cache_size == -i64::from(kib),
                "explicit cache size was not applied",
            )?;
            connection.execute_batch(
                "CREATE TABLE rows_v1 (id INTEGER PRIMARY KEY, payload BLOB NOT NULL)",
            )?;
            let before = connection.pager_cache_status()?;
            let transaction = connection.transaction()?;
            let payload = vec![42_u8; 4096];
            for id in 0..512_i64 {
                let inserted = transaction.execute(
                    "INSERT INTO rows_v1 (id, payload) VALUES (?1, ?2)",
                    params![id, payload],
                )?;
                require(inserted == 1, "cache fixture insert count differs")?;
            }
            transaction.commit()?;
            let after = connection.pager_cache_status()?;
            let spills = after
                .spill
                .checked_sub(before.spill)
                .ok_or("pager spill counter regressed")?;
            connection.close().map_err(|(_connection, error)| error)?;
            let reopened = Connection::open(&path)?;
            let (rows, total): (i64, i64) = reopened.query_row(
                "SELECT COUNT(*), SUM(length(payload)) FROM rows_v1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?;
            require(
                (rows, total) == (512, 512 * 4096),
                "cache fixture rows changed after reopen",
            )?;
            Ok(spills)
        }
        let small = run(10)?;
        let large = run(8192)?;
        require(small > 0, "small cache did not exercise pager spilling")?;
        require(
            large < small,
            "explicit larger cache did not reduce pager spilling",
        )?;
        Ok(())
    }
}
