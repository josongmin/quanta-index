//! Opening the catalog: the connection with its pragmas, then every
//! table's schema, so a catalog is whole from its first open.

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use quanta_index_core::CoreError;

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
        let (connection, path) = open_connection(state_root, busy_timeout)?;
        let legacy_tables: i64 = connection
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
        connection
            .execute_batch(crate::idempotency::SCHEMA)
            .map_err(|error| engine_error("create idempotency schema", &path, &error))?;
        connection
            .execute_batch(crate::sequence::SCHEMA)
            .map_err(|error| engine_error("create sequence schema", &path, &error))?;
        connection
            .execute_batch(crate::auxiliary::SCHEMA)
            .map_err(|error| engine_error("create auxiliary schema", &path, &error))?;
        connection
            .execute_batch(crate::candidate::SCHEMA)
            .map_err(|error| engine_error("create repomap candidate schema", &path, &error))?;
        // Seed the allocator row (self-digested), then reconcile it from
        // the generic ledger and verify the event↔domain pairs
        // (SEP-21-002).
        crate::sequence::seed_allocator(&connection, &path)?;
        let mut connection = connection;
        // Crash recovery (S21-04): the state root admits one writer at a
        // time, so any unfinished journal row found here belongs to a dead
        // process and is aborted before the catalog answers anything.
        let _recovered = crate::idempotency::recover_unfinished_rows(&mut connection, &path)?;
        let _leases = crate::idempotency::release_stale_mutation_leases(&mut connection, &path)?;
        crate::sequence::reconcile(&mut connection, &path)?;
        Ok(Self {
            connection: Mutex::new(connection),
            path,
            clock,
        })
    }
}
