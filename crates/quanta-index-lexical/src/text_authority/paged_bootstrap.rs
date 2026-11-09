//! Disk-bounded sort for the five Native maps built from an empty root.
//!
//! This database is private scratch. Only the authenticated object references
//! emitted by `map_bootstrap` become part of a committed generation.

use std::collections::{BTreeMap, btree_map::Entry};
use std::time::Instant;

use quanta_index_core::CoreError;
use rusqlite::{Connection, ErrorCode, params};

use crate::lexical_store::{
    AttemptLease, BlobRef, MapEdit, MapRoot, ObjectKind, ObjectRef, ObjectStore,
};

// Private scratch only: version, kind, physical length/digest, logical length/digest.
// Keeping the value fixed-width avoids per-row CBOR encoding and heap allocation.
const SCRATCH_REF_BYTES: usize = 1 + 1 + 8 + 32 + 8 + 32;
const SCRATCH_REF_VERSION: u8 = 1;

fn encode_scratch_ref(reference: &BlobRef) -> [u8; SCRATCH_REF_BYTES] {
    let mut bytes = [0; SCRATCH_REF_BYTES];
    bytes[0] = SCRATCH_REF_VERSION;
    bytes[1] = reference.root.kind.code();
    bytes[2..10].copy_from_slice(&reference.root.bytes.to_be_bytes());
    bytes[10..42].copy_from_slice(&reference.root.sha256);
    bytes[42..50].copy_from_slice(&reference.bytes.to_be_bytes());
    bytes[50..82].copy_from_slice(&reference.sha256);
    bytes
}

fn decode_scratch_ref(bytes: &[u8]) -> Result<BlobRef, CoreError> {
    let bytes: &[u8; SCRATCH_REF_BYTES] = bytes.try_into().map_err(|_error| {
        CoreError::InvalidContract("lexical: Native bootstrap reference width differs".into())
    })?;
    if bytes[0] != SCRATCH_REF_VERSION {
        return Err(CoreError::InvalidContract(
            "lexical: Native bootstrap reference version differs".into(),
        ));
    }
    let kind = ObjectKind::from_code(bytes[1]).ok_or_else(|| {
        CoreError::InvalidContract("lexical: Native bootstrap reference kind differs".into())
    })?;
    Ok(BlobRef {
        root: ObjectRef {
            kind,
            bytes: u64::from_be_bytes(bytes[2..10].try_into().map_err(|_error| {
                CoreError::InvalidContract("lexical: Native bootstrap root width differs".into())
            })?),
            sha256: bytes[10..42].try_into().map_err(|_error| {
                CoreError::InvalidContract("lexical: Native bootstrap root digest differs".into())
            })?,
        },
        bytes: u64::from_be_bytes(bytes[42..50].try_into().map_err(|_error| {
            CoreError::InvalidContract("lexical: Native bootstrap payload width differs".into())
        })?),
        sha256: bytes[50..82].try_into().map_err(|_error| {
            CoreError::InvalidContract("lexical: Native bootstrap payload digest differs".into())
        })?,
    })
}

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

    const fn diagnostic_label(self) -> &'static str {
        match self {
            Self::Current => "native_bootstrap_current_stage",
            Self::Membership => "native_bootstrap_membership_stage",
            Self::Segments => "native_bootstrap_segments_stage",
            Self::GramRoutes => "native_bootstrap_gram_routes_stage",
            Self::TermRoutes => "native_bootstrap_term_routes_stage",
        }
    }
}

#[expect(clippy::print_stderr, reason = "Opt-in publication phase diagnosis.")]
fn trace_stage_detail(
    label: &'static str,
    event: &'static str,
    elapsed_from: Option<Instant>,
    rows: usize,
) {
    if let Some(started) = elapsed_from {
        eprintln!(
            "QI_INGEST_PHASE_V1 label={label} event={event} rows={rows} elapsed_ns={}",
            started.elapsed().as_nanos()
        );
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
             CREATE TABLE refs(id INTEGER PRIMARY KEY, value BLOB NOT NULL);
             CREATE TABLE rows(
                 surface INTEGER NOT NULL,
                 key BLOB NOT NULL,
                 ref_id INTEGER NOT NULL,
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
        let label = surface.diagnostic_label();
        let started = crate::causal_profile::begin_phase(label);
        let total_rows = edits.len();
        let progress_stride = total_rows.div_ceil(16).max(256);
        trace_stage_detail(label, "rows_expected", started, total_rows);
        let transaction = self
            .db
            .transaction()
            .map_err(|error| storage("begin scratch cohort", error))?;
        let insert_started = started.map(|_| Instant::now());
        let mut inserted = 0_usize;
        {
            let mut insert_ref = transaction
                .prepare("INSERT INTO refs(value) VALUES (?1)")
                .map_err(|error| storage("prepare scratch references", error))?;
            let mut insert = transaction
                .prepare("INSERT INTO rows(surface,key,ref_id) VALUES (?1,?2,?3)")
                .map_err(|error| storage("prepare scratch cohort", error))?;
            // Routes share their posting-page reference. Keep the wide value
            // out of the sorted key tree, with only one cohort's IDs in memory.
            let mut refs = BTreeMap::new();
            for (key, reference) in edits {
                let reference = reference.ok_or_else(|| {
                    CoreError::InvalidContract(
                        "lexical: Native empty-root bootstrap contains deletion".into(),
                    )
                })?;
                let value = encode_scratch_ref(&reference);
                let ref_id = match refs.entry(value) {
                    Entry::Occupied(entry) => *entry.get(),
                    Entry::Vacant(entry) => {
                        let id = insert_ref
                            .insert(params![entry.key().as_slice()])
                            .map_err(|error| storage("write scratch reference", error))?;
                        *entry.insert(id)
                    }
                };
                match insert.execute(params![surface.code(), key, ref_id]) {
                    Ok(1) => {
                        if started.is_some() {
                            inserted += 1;
                            if inserted < total_rows && inserted.is_multiple_of(progress_stride) {
                                crate::causal_profile::phase_progress(
                                    label, started, inserted, None,
                                );
                            }
                        }
                    }
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
        trace_stage_detail(label, "insert_completed", insert_started, inserted);
        let commit_started = started.map(|_| Instant::now());
        transaction
            .commit()
            .map_err(|error| storage("commit scratch cohort", error))?;
        trace_stage_detail(label, "commit_completed", commit_started, inserted);
        crate::causal_profile::finish_phase(label, started);
        Ok(())
    }

    pub(super) fn finish(
        &self,
        store: &ObjectStore,
        lease: &AttemptLease,
    ) -> Result<BootstrapRoots, CoreError> {
        Ok(BootstrapRoots {
            current: crate::causal_profile::timed_work("native_bootstrap_current_map", || {
                self.bootstrap_map(store, lease, BootstrapMap::Current)
            })?,
            membership: crate::causal_profile::timed_work(
                "native_bootstrap_membership_map",
                || self.bootstrap_map(store, lease, BootstrapMap::Membership),
            )?,
            segments: crate::causal_profile::timed_work("native_bootstrap_segments_map", || {
                self.bootstrap_map(store, lease, BootstrapMap::Segments)
            })?,
            gram_routes: crate::causal_profile::timed_work(
                "native_bootstrap_gram_routes_map",
                || self.bootstrap_map(store, lease, BootstrapMap::GramRoutes),
            )?,
            term_routes: crate::causal_profile::timed_work(
                "native_bootstrap_term_routes_map",
                || self.bootstrap_map(store, lease, BootstrapMap::TermRoutes),
            )?,
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
            .prepare(
                "SELECT rows.key,refs.value FROM rows LEFT JOIN refs ON refs.id=rows.ref_id
                 WHERE rows.surface=?1 ORDER BY rows.key",
            )
            .map_err(|error| storage("prepare scratch scan", error))?;
        let rows = statement
            .query_map(params![surface.code()], |row| {
                let encoded = row.get_ref(1)?.as_blob().map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(
                        1,
                        rusqlite::types::Type::Blob,
                        Box::new(error),
                    )
                })?;
                let mut reference = [0; SCRATCH_REF_BYTES];
                if encoded.len() == SCRATCH_REF_BYTES {
                    reference.copy_from_slice(encoded);
                }
                Ok((row.get::<_, Vec<u8>>(0)?, reference, encoded.len()))
            })
            .map_err(|error| storage("scan scratch map", error))?;
        let edits = rows.map(|row| {
            let (key, encoded, len) = row.map_err(|error| storage("read scratch row", error))?;
            if len != SCRATCH_REF_BYTES {
                return Err(CoreError::InvalidContract(
                    "lexical: Native bootstrap reference width differs".into(),
                ));
            }
            let reference = decode_scratch_ref(&encoded)?;
            Ok(MapEdit {
                key,
                value: Some(reference),
            })
        });
        if matches!(surface, BootstrapMap::GramRoutes) {
            store.map_bootstrap_wide_gram_routes(lease, edits)
        } else {
            store.map_bootstrap(lease, edits)
        }
    }
}

fn storage(context: &str, error: impl std::fmt::Display) -> CoreError {
    CoreError::Storage(format!("lexical: Native bootstrap {context}: {error}"))
}

#[cfg(test)]
mod scratch_ref_tests {
    use super::{decode_scratch_ref, encode_scratch_ref};
    use crate::lexical_store::{BlobRef, ObjectKind, ObjectRef};

    #[test]
    fn private_reference_wire_roundtrips_and_rejects_invalid_shape() {
        let reference = BlobRef {
            root: ObjectRef {
                kind: ObjectKind::NativePositions,
                bytes: 1024,
                sha256: [7; 32],
            },
            bytes: 4096,
            sha256: [9; 32],
        };
        let encoded = encode_scratch_ref(&reference);
        assert_eq!(decode_scratch_ref(&encoded).expect("roundtrip"), reference);
        assert!(decode_scratch_ref(&encoded[..encoded.len() - 1]).is_err());
        let mut wrong_version = encoded;
        wrong_version[0] = 2;
        assert!(decode_scratch_ref(&wrong_version).is_err());
        let mut wrong_kind = encoded;
        wrong_kind[1] = u8::MAX;
        assert!(decode_scratch_ref(&wrong_kind).is_err());
    }
}
