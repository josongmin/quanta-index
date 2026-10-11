//! Disk-bounded sort for the five Native maps built from an empty root.
//!
//! This database is private scratch. Only the authenticated object references
//! emitted by `map_bootstrap` become part of a committed generation.

use std::collections::BTreeMap;

use quanta_index_core::CoreError;
use rusqlite::{Connection, ErrorCode, params};

use crate::lexical_store::{AttemptLease, BlobRef, MapEdit, MapRoot, ObjectStore};

const MAX_SCRATCH_REF_BYTES: usize = 1024;

#[derive(Clone, Copy)]
pub(super) enum BootstrapMap {
    Current,
    Membership,
    Segments,
    GramRoutes,
    TermRoutes,
}

impl BootstrapMap {
    const fn code(self) -> i64 {
        match self {
            Self::Current => 1,
            Self::Membership => 2,
            Self::Segments => 3,
            Self::GramRoutes => 4,
            Self::TermRoutes => 5,
        }
    }
}

pub(super) struct BootstrapRoots {
    pub(super) current: MapRoot,
    pub(super) membership: MapRoot,
    pub(super) segments: MapRoot,
    pub(super) gram_routes: MapRoot,
    pub(super) term_routes: MapRoot,
}

pub(super) struct NativeBootstrapMaps {
    // Retain the temporary directory until the connection and all row cursors
    // have been dropped; neither path nor database enters the authority root.
    db: Connection,
    _scratch: tempfile::TempDir,
}

impl NativeBootstrapMaps {
    pub(super) fn new() -> Result<Self, CoreError> {
        let scratch = tempfile::tempdir().map_err(|error| storage("create scratch", error))?;
        let db = Connection::open(scratch.path().join("native-bootstrap.sqlite"))
            .map_err(|error| storage("open scratch", error))?;
        db.execute_batch(
            "PRAGMA journal_mode=DELETE;
             PRAGMA synchronous=OFF;
             PRAGMA temp_store=FILE;
             PRAGMA cache_size=-2048;
             PRAGMA mmap_size=0;
             CREATE TABLE rows(
                 surface INTEGER NOT NULL,
                 key BLOB NOT NULL,
                 value BLOB NOT NULL,
                 PRIMARY KEY(surface,key)
             ) WITHOUT ROWID;",
        )
        .map_err(|error| storage("initialize scratch", error))?;
        Ok(Self {
            db,
            _scratch: scratch,
        })
    }

    #[cfg(test)]
    #[expect(
        clippy::used_underscore_binding,
        reason = "the lifetime oracle observes the directory retained solely for RAII cleanup"
    )]
    pub(super) fn scratch_path(&self) -> &std::path::Path {
        self._scratch.path()
    }

    /// A rejected deletion or duplicate rolls back this map's cohort.
    pub(super) fn stage(
        &mut self,
        surface: BootstrapMap,
        edits: BTreeMap<Vec<u8>, Option<BlobRef>>,
    ) -> Result<(), CoreError> {
        let transaction = self
            .db
            .transaction()
            .map_err(|error| storage("begin scratch cohort", error))?;
        {
            let mut insert = transaction
                .prepare("INSERT INTO rows(surface,key,value) VALUES (?1,?2,?3)")
                .map_err(|error| storage("prepare scratch cohort", error))?;
            for (key, reference) in edits {
                let reference = reference.ok_or_else(|| {
                    CoreError::InvalidContract(
                        "lexical: Native empty-root bootstrap contains deletion".into(),
                    )
                })?;
                let value = crate::channel_payloads::encode_cbor(
                    &reference,
                    "Native bootstrap object reference",
                )?;
                if value.len() > MAX_SCRATCH_REF_BYTES {
                    return Err(CoreError::InvalidContract(
                        "lexical: Native bootstrap reference exceeds scratch bound".into(),
                    ));
                }
                match insert.execute(params![surface.code(), key, value]) {
                    Ok(1) => {}
                    Ok(_) => {
                        return Err(CoreError::InvalidContract(
                            "lexical: Native bootstrap insert count differs".into(),
                        ));
                    }
                    Err(rusqlite::Error::SqliteFailure(failure, _))
                        if failure.code == ErrorCode::ConstraintViolation =>
                    {
                        return Err(CoreError::InvalidContract(
                            "lexical: duplicate Native bootstrap map key".into(),
                        ));
                    }
                    Err(error) => return Err(storage("write scratch cohort", error)),
                }
            }
        }
        transaction
            .commit()
            .map_err(|error| storage("commit scratch cohort", error))
    }

    pub(super) fn finish(
        &self,
        store: &ObjectStore,
        lease: &AttemptLease,
    ) -> Result<BootstrapRoots, CoreError> {
        Ok(BootstrapRoots {
            current: self.bootstrap_map(store, lease, BootstrapMap::Current)?,
            membership: self.bootstrap_map(store, lease, BootstrapMap::Membership)?,
            segments: self.bootstrap_map(store, lease, BootstrapMap::Segments)?,
            gram_routes: self.bootstrap_map(store, lease, BootstrapMap::GramRoutes)?,
            term_routes: self.bootstrap_map(store, lease, BootstrapMap::TermRoutes)?,
        })
    }

    fn bootstrap_map(
        &self,
        store: &ObjectStore,
        lease: &AttemptLease,
        surface: BootstrapMap,
    ) -> Result<MapRoot, CoreError> {
        let mut statement = self
            .db
            .prepare("SELECT key,value FROM rows WHERE surface=?1 ORDER BY key")
            .map_err(|error| storage("prepare scratch scan", error))?;
        let rows = statement
            .query_map(params![surface.code()], |row| {
                Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, Vec<u8>>(1)?))
            })
            .map_err(|error| storage("scan scratch map", error))?;
        store.map_bootstrap(
            lease,
            rows.map(|row| {
                let (key, encoded) = row.map_err(|error| storage("read scratch row", error))?;
                if encoded.len() > MAX_SCRATCH_REF_BYTES {
                    return Err(CoreError::InvalidContract(
                        "lexical: Native bootstrap reference exceeds scratch bound".into(),
                    ));
                }
                let reference: BlobRef = crate::channel_payloads::decode_cbor_exact(&encoded)
                    .map_err(|error| storage("decode scratch reference", error))?;
                Ok(MapEdit {
                    key,
                    value: Some(reference),
                })
            }),
        )
    }
}

fn storage(context: &str, error: impl std::fmt::Display) -> CoreError {
    CoreError::Storage(format!("lexical: Native bootstrap {context}: {error}"))
}
