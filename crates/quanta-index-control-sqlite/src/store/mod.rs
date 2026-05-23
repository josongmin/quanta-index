mod bundle_ingest;
mod generation_registry;
mod helpers;
mod schema;

#[cfg(test)]
mod tests;

use std::path::Path;

use quanta_index_core::CoreError;
use rusqlite::Connection;

pub use self::helpers::placeholder_artifact;

pub struct SqliteControlPlane {
    conn: Connection,
}

impl SqliteControlPlane {
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
