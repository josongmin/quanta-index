//! Opening the catalog: the connection with its pragmas, then every
//! table's schema, so a catalog is whole from its first open.

use std::path::Path;
use std::sync::Mutex;
use std::time::Duration;

use quanta_index_core::CoreError;

use crate::connection::{SqliteCatalog, engine_error, open_connection};

impl SqliteCatalog {
    /// Open (creating if needed) the catalog under `state_root/catalog/`.
    ///
    /// `busy_timeout` is how long a write waits on a held lock before it is
    /// answered typed; the caller maps it from its own deadline.
    pub fn open(state_root: &Path, busy_timeout: Duration) -> Result<Self, CoreError> {
        let (connection, path) = open_connection(state_root, busy_timeout)?;
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
        crate::sequence::reconcile(&mut connection, &path)?;
        Ok(Self {
            connection: Mutex::new(connection),
            path,
        })
    }
}
