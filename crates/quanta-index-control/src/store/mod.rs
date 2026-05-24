mod bundle_ingest;
mod delta_apply;
mod generation_pin;
mod generation_registry;
mod helpers;
mod metadata;
mod schema;

pub use generation_pin::BoundGenerationPin;
// Re-export crate-internal helper for the `test_support` module in lib.rs.
#[doc(hidden)]
pub use helpers::sqlite_u64_to_i64;

#[cfg(test)]
mod tests;

use std::path::Path;

use quanta_index_core::CoreError;
use rusqlite::Connection;

pub struct ControlPlane {
    conn: Connection,
}

impl ControlPlane {
    /// Direct connection access for the in-crate `test_support` module.
    /// Crate-private; production callers go through port traits.
    pub(crate) fn conn(&self) -> &Connection {
        &self.conn
    }

    pub fn open(path: &Path) -> Result<Self, CoreError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| {
                CoreError::Storage(format!("create parent dir failed: {error}"))
            })?;
        }

        let conn = Connection::open(path)
            .map_err(|error| CoreError::Storage(format!("open sqlite failed: {error}")))?;
        conn.pragma_update(None, "journal_mode", "WAL")
            .map_err(|error| CoreError::Storage(format!("enable WAL failed: {error}")))?;

        let store = Self { conn };
        store.bootstrap_schema()?;
        Ok(store)
    }
}
