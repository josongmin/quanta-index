//! Opening the catalog: the connection with its pragmas, then every
//! table's schema, so a catalog is whole from its first open.

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use quanta_index_core::CoreError;
use rusqlite::TransactionBehavior;

use crate::connection::{
    CatalogClockPort, SqliteCatalog, SystemCatalogClock, engine_error, open_connection,
};

impl SqliteCatalog {
    /// Open (creating if needed) the catalog under `state_root/catalog/`.
    ///
    /// `busy_timeout` is how long a write waits on a held lock before it is
    /// answered typed; the caller maps it from its own deadline. Lease
    /// decisions read the wall clock.
    pub fn open(state_root: &Path, busy_timeout: Duration) -> Result<Self, CoreError> {
        Self::open_with_clock(state_root, busy_timeout, Arc::new(SystemCatalogClock))
    }

    /// Open with an explicit lease-decision clock (TOPT-01 / PO-3):
    /// production passes [`SystemCatalogClock`], tests a scripted clock.
    pub fn open_with_clock(
        state_root: &Path,
        busy_timeout: Duration,
        clock: Arc<dyn CatalogClockPort>,
    ) -> Result<Self, CoreError> {
        let (mut connection, path) = open_connection(state_root, busy_timeout)?;
        // Lock before classifying the root. Two concurrent first opens must
        // not both observe an empty catalog and independently decide to seed
        // its first fence row.
        let initialization = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| engine_error("begin catalog initialization", &path, &error))?;
        let legacy_tables: i64 = initialization
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name IN
                 ('idempotency_v1', 'catalog_sequence_v1')",
                [],
                |row| row.get(0),
            )
            .map_err(|error| engine_error("inspect legacy catalog tables", &path, &error))?;
        if legacy_tables != 0 {
            return Err(CoreError::Storage(format!(
                "catalog: {} contains unsupported legacy catalog tables; this build has no migration reader",
                path.display()
            )));
        }
        // Any pre-existing table means this is not a pristine catalog.
        // An allowlist of today's journal tables would misclassify a retained
        // auxiliary-only (or future-domain) root and silently reuse fences.
        let existing_tables: i64 = initialization
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table'",
                [],
                |row| row.get(0),
            )
            .map_err(|error| engine_error("inspect existing catalog tables", &path, &error))?;
        let fresh_root = existing_tables == 0;
        // DDL and allocator seeding are one durable decision. In particular,
        // interruption before the first fence row must not strand a new root
        // with only some current tables, which would look like an older root
        // and (correctly) refuse implicit fence re-seeding on the next open.
        initialization
            .execute_batch(crate::idempotency::SCHEMA)
            .map_err(|error| engine_error("create idempotency schema", &path, &error))?;
        initialization
            .execute_batch(crate::sequence::SCHEMA)
            .map_err(|error| engine_error("create sequence schema", &path, &error))?;
        crate::sequence::verify_installed_schema(&initialization, &path)?;
        initialization
            .execute_batch(crate::auxiliary::SCHEMA)
            .map_err(|error| engine_error("create auxiliary schema", &path, &error))?;
        initialization
            .execute_batch(crate::candidate::SCHEMA)
            .map_err(|error| engine_error("create repomap candidate schema", &path, &error))?;
        crate::candidate::verify_installed_schema(&initialization, &path)?;
        crate::idempotency::seed_fence_allocator(&initialization, &path, fresh_root)?;
        // A missing or recast GC floor must refuse before recovery mutates
        // journal rows or clears old mutation leases.
        crate::sequence::verify_gc_floor_domain_integrity(&initialization, &path)?;
        // Reconcile the sequence allocator before recovery allocates abort
        // events. Recovery, lease cleanup, and the final integrity pass all
        // share this transaction: a failed open must not publish a partial
        // recovery or a partially initialized schema.
        crate::sequence::seed_allocator(&initialization, &path)?;
        crate::sequence::reconcile(&initialization, &path)?;
        // Crash recovery (S21-04): the state root admits one writer at a
        // time, so any unfinished journal row found here belongs to a dead
        // process and is aborted before the catalog answers anything.
        let _recovered = crate::idempotency::recover_unfinished_rows(&initialization, &path)?;
        let _leases = crate::idempotency::release_stale_mutation_leases(&initialization, &path)?;
        crate::sequence::verify_integrity(&initialization, &path)?;
        initialization
            .commit()
            .map_err(|error| engine_error("commit catalog initialization", &path, &error))?;
        Ok(Self {
            connection: Mutex::new(connection),
            path,
            clock,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error;
    use std::time::Duration;

    use quanta_index_core::CoreError;

    use super::SqliteCatalog;
    use crate::connection::{CATALOG_FILE_NAME, catalog_dir};

    #[test]
    fn auxiliary_only_existing_catalog_must_not_seed_a_new_fence() -> Result<(), Box<dyn Error>> {
        let root = tempfile::tempdir()?;
        let directory = catalog_dir(root.path());
        std::fs::create_dir_all(&directory)?;
        let path = directory.join(CATALOG_FILE_NAME);
        let connection = rusqlite::Connection::open(&path)?;
        connection.execute_batch(crate::auxiliary::SCHEMA)?;
        drop(connection);

        if !matches!(
            SqliteCatalog::open(root.path(), Duration::from_millis(100)),
            Err(CoreError::Storage(message)) if message.contains("no durable fence allocator row")
        ) {
            return Err("existing auxiliary-only catalog was classified as fresh".into());
        }
        let connection = rusqlite::Connection::open(path)?;
        let created_journal: i64 = connection.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'idempotency_v2'",
            [],
            |row| row.get(0),
        )?;
        if created_journal != 0 {
            return Err("failed open must roll back current journal creation".into());
        }
        Ok(())
    }
}
