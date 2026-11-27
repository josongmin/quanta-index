//! Semantic version gate for persisted native claims and registration payloads.
//!
//! A schema version alone cannot distinguish the old execution-base hash from
//! the logical capture hash. This row is checked before Catalog DDL or recovery.

use std::{fs::File, io::Read as _, path::Path};

use quanta_index_contract::{
    SearchPlaneErrorCodeV2,
    index::{
        NATIVE_INDEX_STATE_VERSION, NATIVE_SOURCE_CAPTURE_HASH_DOMAIN,
        NATIVE_SOURCE_CAPTURE_VERSION,
    },
};
use quanta_index_core::CoreError;
use rusqlite::{Connection, params, types::ValueRef};

use super::{
    SCHEMA, SCHEMA_OBJECTS, SOURCE_BODY_INCIDENT_OBJECTS, SOURCE_BODY_INCIDENT_SCHEMA,
    VIEW_QUARANTINE_OBJECTS, VIEW_QUARANTINE_SCHEMA, boot_manifest, fields_digest,
    registration_codec,
    source_scrub_progress, source_stage_replay_receipt,
};
use crate::connection::engine_error;

const DIGEST_DOMAIN: &[u8] = b"qidx.native.capture-format.v1\0";
const QUARANTINE_DIGEST_DOMAIN: &[u8] = b"qidx.native.capture-format.view-quarantine.v1\0";
const QUARANTINE_FANOUT_DOMAIN: &[u8] = b"qidx.native.capture-format.view-quarantine.v2\0";
const INCIDENT_PENDING_DOMAIN: &[u8] = b"qidx.native.capture-format.body-incident.pending.v1\0";
const INCIDENT_COMPLETE_DOMAIN: &[u8] = b"qidx.native.capture-format.body-incident.complete.v1\0";
const INCIDENT_IMPORT_DOMAIN: &[u8] = b"qidx.native.capture-format.body-incident.import.v1\0";
pub(super) const QUARANTINE_SCHEMA_ABSENT: u8 = 0;
const QUARANTINE_SCHEMA_LEGACY: u8 = 1;
pub(super) const QUARANTINE_SCHEMA_CURRENT: u8 = 2;

struct CaptureStamp {
    root_nonce: [u8; 32],
    row_digest: [u8; 32],
    quarantine_fanout_complete: bool,
    import_legacy_quarantine: bool,
}

pub(super) struct PendingQuarantine {
    root_nonce: [u8; 32],
    pub(super) import_legacy: bool,
}

fn unsupported(path: &Path, detail: &str) -> CoreError {
    CoreError::Typed {
        code: SearchPlaneErrorCodeV2::StateRootFormatUnsupported,
        message: format!(
            "catalog: {} has unsupported native source-capture format ({detail}); use a new state root and rebuild the index",
            path.display()
        ),
    }
}

fn expected_digest(root_nonce: &[u8; 32], has_quarantine: bool) -> Result<Vec<u8>, CoreError> {
    let domain = if has_quarantine {
        QUARANTINE_DIGEST_DOMAIN
    } else {
        DIGEST_DOMAIN
    };
    stamp_digest(root_nonce, domain)
}

fn stamp_digest(root_nonce: &[u8; 32], domain: &[u8]) -> Result<Vec<u8>, CoreError> {
    fields_digest(
        domain,
        &(
            1_i64,
            i64::from(NATIVE_INDEX_STATE_VERSION),
            i64::from(NATIVE_SOURCE_CAPTURE_VERSION),
            NATIVE_SOURCE_CAPTURE_HASH_DOMAIN,
            registration_codec::VERSION,
            root_nonce,
        ),
    )
    .map_err(|error| CoreError::Storage(format!("encode native capture-format stamp: {error}")))
}

/// Return true only when this transaction sees no native namespace at all.
///
/// A generic preexisting Catalog is allowed through this gate; its own fence
/// and schema checks remain authoritative later in the same transaction.
pub(crate) fn preflight_capture_format(
    connection: &Connection,
    path: &Path,
) -> Result<bool, CoreError> {
    let mut query = connection
        .prepare(
            "SELECT type,name,typeof(sql),
                    CASE WHEN typeof(sql)='text' THEN octet_length(sql) END
             FROM sqlite_master
             WHERE (substr(name,1,7)='native_' COLLATE NOCASE
                    OR substr(tbl_name,1,7)='native_' COLLATE NOCASE)
               AND name NOT GLOB 'sqlite_autoindex_*'",
        )
        .map_err(|error| engine_error("inspect native namespace", path, &error))?;
    let mut rows = query
        .query([])
        .map_err(|error| engine_error("inspect native namespace", path, &error))?;
    let mut installed_base = 0usize;
    let mut installed_quarantine = false;
    let mut installed_incidents = false;
    let mut installed_source_stage = 0usize;
    let mut installed_source_scrub = 0usize;
    let mut installed_source_views = false;
    while let Some(row) = rows
        .next()
        .map_err(|error| engine_error("inspect native namespace", path, &error))?
    {
        let kind_value = row
            .get_ref(0)
            .map_err(|error| engine_error("read native object kind", path, &error))?;
        let kind = kind_value
            .as_str()
            .map_err(|_error| unsupported(path, "invalid native object kind"))?;
        let name_value = row
            .get_ref(1)
            .map_err(|error| engine_error("read native object name", path, &error))?;
        let name = name_value
            .as_str()
            .map_err(|_error| unsupported(path, "invalid native object name"))?;
        let is_base = SCHEMA_OBJECTS
            .iter()
            .any(|(expected_kind, expected_name, _)| {
                *expected_kind == kind && *expected_name == name
            });
        let is_quarantine =
            VIEW_QUARANTINE_OBJECTS
                .iter()
                .any(|(expected_kind, expected_name, _)| {
                    *expected_kind == kind && *expected_name == name
                });
        let is_incident =
            SOURCE_BODY_INCIDENT_OBJECTS
                .iter()
                .any(|(expected_kind, expected_name, _)| {
                    *expected_kind == kind && *expected_name == name
                });
        let is_source_stage = source_stage_replay_receipt::is_schema_object(kind, name);
        let is_source_scrub = source_scrub_progress::is_schema_object(kind, name);
        let is_source_view = boot_manifest::PROTECTED_SOURCE_VIEW_OBJECTS
            .iter()
            .any(|(expected_kind, expected_name, _)| *expected_kind == kind && *expected_name == name);
        if !is_base
            && !is_quarantine
            && !is_incident
            && !is_source_stage
            && !is_source_scrub
            && !is_source_view
        {
            return Err(unsupported(path, "unknown native namespace object"));
        }
        // The shared canonical-schema verifier reads sqlite_master.sql into
        // a String. Reject recast definitions larger than this build's entire
        // canonical native schema before that read; all canonical definitions
        // are individually smaller than this conservative byte envelope.
        let sql_type_value = row
            .get_ref(2)
            .map_err(|error| engine_error("read native schema SQL type", path, &error))?;
        let sql_type = sql_type_value
            .as_str()
            .map_err(|_error| unsupported(path, "invalid native schema SQL type"))?;
        let sql_bytes: Option<i64> = row
            .get(3)
            .map_err(|error| engine_error("read native schema SQL byte length", path, &error))?;
        let max_sql_bytes = i64::try_from(
            SCHEMA
                .len()
                .max(VIEW_QUARANTINE_SCHEMA.len())
                .max(SOURCE_BODY_INCIDENT_SCHEMA.len())
                .max(source_stage_replay_receipt::SCHEMA.len())
                .max(source_scrub_progress::SCHEMA.len())
                .max(boot_manifest::PROTECTED_SOURCE_VIEW_SCHEMA.len()),
        )
        .map_err(|error| {
            CoreError::Storage(format!("native schema byte envelope overflow: {error}"))
        })?;
        if sql_type != "text"
            || !matches!(sql_bytes, Some(bytes) if bytes >= 0 && bytes <= max_sql_bytes)
        {
            return Err(unsupported(
                path,
                "native schema SQL exceeds canonical byte envelope",
            ));
        }
        if is_base {
            installed_base = installed_base
                .checked_add(1)
                .ok_or_else(|| CoreError::Storage("native schema count overflow".into()))?;
        }
        installed_quarantine |= is_quarantine;
        installed_incidents |= is_incident;
        installed_source_views |= is_source_view;
        if is_source_stage {
            installed_source_stage = installed_source_stage
                .checked_add(1)
                .ok_or_else(|| CoreError::Storage("native schema count overflow".into()))?;
        }
        if is_source_scrub {
            installed_source_scrub = installed_source_scrub
                .checked_add(1)
                .ok_or_else(|| CoreError::Storage("native schema count overflow".into()))?;
        }
    }
    drop(rows);
    drop(query);
    if installed_base == 0
        && !installed_quarantine
        && !installed_incidents
        && installed_source_stage == 0
        && installed_source_scrub == 0
        && !installed_source_views
    {
        return Ok(true);
    }
    if installed_base != SCHEMA_OBJECTS.len() {
        return Err(unsupported(
            path,
            "partial native namespace or missing format table",
        ));
    }
    if installed_incidents && !installed_quarantine {
        return Err(unsupported(
            path,
            "partial native Source body incident schema",
        ));
    }
    if installed_source_stage != 0
        && installed_source_stage != source_stage_replay_receipt::schema_object_count()
    {
        return Err(unsupported(
            path,
            "partial native Source stage replay schema",
        ));
    }
    if installed_source_scrub != 0 && installed_source_scrub != source_scrub_progress::OBJECTS.len()
    {
        return Err(unsupported(
            path,
            "partial native Source scrub progress schema",
        ));
    }
    // The installed DDL must be checked before even idempotent CREATEs run.
    // Preserve actual SQLite errors; only incompatible DDL is a format refusal.
    verify_native_schema(connection, path)?;
    if installed_quarantine {
        crate::connection::verify_installed_schema_objects_with_refusal(
            connection,
            path,
            VIEW_QUARANTINE_SCHEMA,
            VIEW_QUARANTINE_OBJECTS,
            |_label| unsupported(path, "native view quarantine schema definition differs"),
        )?;
    }
    if installed_incidents {
        crate::connection::verify_installed_schema_objects_with_refusal(
            connection,
            path,
            SOURCE_BODY_INCIDENT_SCHEMA,
            SOURCE_BODY_INCIDENT_OBJECTS,
            |_label| {
                unsupported(
                    path,
                    "native Source body incident schema definition differs",
                )
            },
        )?;
    }
    if installed_source_stage != 0 {
        source_stage_replay_receipt::verify_schema_with_refusal(connection, path, |_label| {
            unsupported(path, "native Source stage replay schema definition differs")
        })?;
    }
    if installed_source_scrub != 0 {
        source_scrub_progress::verify_schema_with_refusal(connection, path, |_label| {
            unsupported(
                path,
                "native Source scrub progress schema definition differs",
            )
        })?;
    }
    if installed_source_views {
        crate::connection::verify_installed_schema_objects_with_refusal(
            connection,
            path,
            boot_manifest::PROTECTED_SOURCE_VIEW_SCHEMA,
            boot_manifest::PROTECTED_SOURCE_VIEW_OBJECTS,
            |_| unsupported(path, "native protected Source view schema definition differs"),
        )?;
    }
    let _stamp =
        verify_stamp_for_schema(connection, path, installed_quarantine, installed_incidents)?;
    let _selected = super::boot_record::read_stored(connection)
        .map_err(|error| CoreError::Storage(error.to_string()))?;
    Ok(false)
}

fn verify_native_schema(connection: &Connection, path: &Path) -> Result<(), CoreError> {
    crate::connection::verify_installed_schema_objects_with_refusal(
        connection,
        path,
        SCHEMA,
        SCHEMA_OBJECTS,
        |_label| unsupported(path, "native schema definition differs"),
    )
}

pub(super) fn seed_and_verify(
    connection: &Connection,
    path: &Path,
    native_first_birth: bool,
) -> Result<(), CoreError> {
    if native_first_birth {
        let mut root_nonce = [0; 32];
        File::open("/dev/urandom")
            .and_then(|mut entropy| entropy.read_exact(&mut root_nonce))
            .map_err(|error| CoreError::Storage(format!("issue native root nonce: {error}")))?;
        if root_nonce == [0; 32] {
            return Err(CoreError::Storage(
                "native root entropy returned a zero nonce".into(),
            ));
        }
        let _inserted = connection
            .execute(
                "INSERT INTO native_capture_format_v1
                 (singleton,state_version,capture_version,hash_domain,registration_codec,root_nonce,row_digest)
                 VALUES (1,?1,?2,?3,?4,?5,?6)",
                params![
                    i64::from(NATIVE_INDEX_STATE_VERSION),
                    i64::from(NATIVE_SOURCE_CAPTURE_VERSION),
                    NATIVE_SOURCE_CAPTURE_HASH_DOMAIN,
                    registration_codec::VERSION,
                    root_nonce.as_slice(),
                    expected_digest(&root_nonce, false)?
                ],
            )
            .map_err(|error| engine_error("seed native capture format", path, &error))?;
        return Ok(());
    }
    let has_quarantine = has_quarantine_schema(connection, path)?;
    let has_incidents = has_incident_schema(connection, path)?;
    let _stamp = verify_stamp_for_schema(connection, path, has_quarantine, has_incidents)?;
    if has_quarantine && !has_incidents {
        let _current = ensure_current_quarantine_schema(connection, path)?;
    }
    Ok(())
}

pub(super) fn installed_extensions(
    connection: &Connection,
    path: &Path,
) -> Result<(u8, bool), CoreError> {
    let quarantine = quarantine_schema_state(connection, path)?;
    let source_stage: bool = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",
            params![source_stage_replay_receipt::OBJECTS[0].1],
            |row| row.get(0),
        )
        .map_err(|error| engine_error("inspect native Source stage replay schema", path, &error))?;
    Ok((quarantine, source_stage))
}

pub(super) fn ensure_current_quarantine_schema(
    connection: &Connection,
    path: &Path,
) -> Result<u8, CoreError> {
    let state = quarantine_schema_state(connection, path)?;
    if state == QUARANTINE_SCHEMA_CURRENT {
        let _stamp = verify_stamp_for_schema(connection, path, true, true)?;
        return Ok(state);
    }
    let (has_quarantine, domain) = if state == QUARANTINE_SCHEMA_LEGACY {
        (true, INCIDENT_IMPORT_DOMAIN)
    } else {
        (false, INCIDENT_COMPLETE_DOMAIN)
    };
    let stamp = verify_stamp_for_schema(connection, path, has_quarantine, false)?;
    if !has_quarantine {
        connection
            .execute_batch(VIEW_QUARANTINE_SCHEMA)
            .map_err(|error| engine_error("create native view quarantine", path, &error))?;
        crate::connection::verify_installed_schema_objects(
            connection,
            path,
            VIEW_QUARANTINE_SCHEMA,
            VIEW_QUARANTINE_OBJECTS,
        )?;
    }
    connection
        .execute_batch(SOURCE_BODY_INCIDENT_SCHEMA)
        .map_err(|error| engine_error("create native Source body incident schema", path, &error))?;
    crate::connection::verify_installed_schema_objects(
        connection,
        path,
        SOURCE_BODY_INCIDENT_SCHEMA,
        SOURCE_BODY_INCIDENT_OBJECTS,
    )?;
    let changed = connection
        .execute(
            "UPDATE native_capture_format_v1 SET row_digest=?1
             WHERE singleton=1 AND row_digest=?2",
            params![
                stamp_digest(&stamp.root_nonce, domain)?,
                stamp.row_digest.as_slice()
            ],
        )
        .map_err(|error| engine_error("stamp native quarantine extension", path, &error))?;
    if changed != 1 {
        return Err(unsupported(
            path,
            "capture stamp changed while installing quarantine extension",
        ));
    }
    let _stamp = verify_stamp_for_schema(connection, path, true, true)?;
    Ok(QUARANTINE_SCHEMA_CURRENT)
}

pub(super) fn require_current_quarantine_schema(
    connection: &Connection,
    path: &Path,
) -> Result<(), CoreError> {
    if quarantine_schema_state(connection, path)? != QUARANTINE_SCHEMA_CURRENT {
        return Err(unsupported(
            path,
            "installed quarantine extension is incomplete",
        ));
    }
    crate::connection::verify_installed_schema_objects_with_refusal(
        connection,
        path,
        VIEW_QUARANTINE_SCHEMA,
        VIEW_QUARANTINE_OBJECTS,
        |_label| unsupported(path, "native view quarantine schema definition differs"),
    )?;
    crate::connection::verify_installed_schema_objects_with_refusal(
        connection,
        path,
        SOURCE_BODY_INCIDENT_SCHEMA,
        SOURCE_BODY_INCIDENT_OBJECTS,
        |_label| {
            unsupported(
                path,
                "native Source body incident schema definition differs",
            )
        },
    )?;
    let _stamp = verify_stamp_for_schema(connection, path, true, true)?;
    Ok(())
}

pub(super) fn verify_stamp(connection: &Connection, path: &Path) -> Result<[u8; 32], CoreError> {
    Ok(verify_stamp_for_schema(
        connection,
        path,
        has_quarantine_schema(connection, path)?,
        has_incident_schema(connection, path)?,
    )?
    .root_nonce)
}

pub(super) fn pending_quarantine_fanout(
    connection: &Connection,
    path: &Path,
) -> Result<Option<PendingQuarantine>, CoreError> {
    let stamp = verify_stamp_for_schema(connection, path, true, true)?;
    Ok(
        (!stamp.quarantine_fanout_complete).then_some(PendingQuarantine {
            root_nonce: stamp.root_nonce,
            import_legacy: stamp.import_legacy_quarantine,
        }),
    )
}

pub(super) fn mark_quarantine_fanout_pending(
    connection: &Connection,
    path: &Path,
) -> Result<[u8; 32], CoreError> {
    let stamp = verify_stamp_for_schema(connection, path, true, true)?;
    if !stamp.quarantine_fanout_complete {
        return Ok(stamp.root_nonce);
    }
    let changed = connection
        .execute(
            "UPDATE native_capture_format_v1 SET row_digest=?1 WHERE singleton=1 AND row_digest=?2",
            params![
                stamp_digest(&stamp.root_nonce, INCIDENT_PENDING_DOMAIN)?,
                stamp_digest(&stamp.root_nonce, INCIDENT_COMPLETE_DOMAIN)?
            ],
        )
        .map_err(|error| {
            engine_error("mark native view quarantine fan-out pending", path, &error)
        })?;
    if changed != 1 {
        return Err(unsupported(
            path,
            "quarantine fan-out marker changed before pending transition",
        ));
    }
    Ok(stamp.root_nonce)
}

pub(super) fn stamp_quarantine_fanout(
    connection: &Connection,
    path: &Path,
    pending: &PendingQuarantine,
) -> Result<(), CoreError> {
    let domain = if pending.import_legacy {
        INCIDENT_IMPORT_DOMAIN
    } else {
        INCIDENT_PENDING_DOMAIN
    };
    let changed = connection
        .execute(
            "UPDATE native_capture_format_v1 SET row_digest=?1 WHERE singleton=1 AND row_digest=?2",
            params![
                stamp_digest(&pending.root_nonce, INCIDENT_COMPLETE_DOMAIN)?,
                stamp_digest(&pending.root_nonce, domain)?
            ],
        )
        .map_err(|error| {
            engine_error(
                "stamp completed native view quarantine fan-out",
                path,
                &error,
            )
        })?;
    if changed != 1 {
        return Err(unsupported(
            path,
            "quarantine fan-out stamp changed before commit",
        ));
    }
    Ok(())
}

fn has_quarantine_schema(connection: &Connection, path: &Path) -> Result<bool, CoreError> {
    connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",
            params![VIEW_QUARANTINE_OBJECTS[0].1],
            |row| row.get(0),
        )
        .map_err(|error| engine_error("inspect native view quarantine schema", path, &error))
}

fn has_incident_schema(connection: &Connection, path: &Path) -> Result<bool, CoreError> {
    connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",
            params![SOURCE_BODY_INCIDENT_OBJECTS[0].1],
            |row| row.get(0),
        )
        .map_err(|error| engine_error("inspect native Source body incident schema", path, &error))
}

fn quarantine_schema_state(connection: &Connection, path: &Path) -> Result<u8, CoreError> {
    match (
        has_quarantine_schema(connection, path)?,
        has_incident_schema(connection, path)?,
    ) {
        (false, false) => Ok(QUARANTINE_SCHEMA_ABSENT),
        (true, false) => Ok(QUARANTINE_SCHEMA_LEGACY),
        (true, true) => Ok(QUARANTINE_SCHEMA_CURRENT),
        (false, true) => Err(unsupported(
            path,
            "Source body incident schema lacks view quarantine authority",
        )),
    }
}

fn verify_stamp_for_schema(
    connection: &Connection,
    path: &Path,
    has_quarantine: bool,
    has_incidents: bool,
) -> Result<CaptureStamp, CoreError> {
    let mut query = connection
        .prepare(
            "SELECT
               CASE WHEN typeof(singleton)='integer' AND singleton=1 THEN 1 ELSE 0 END,
               CASE WHEN typeof(state_version)='integer' AND state_version=?1 THEN 1 ELSE 0 END,
               CASE WHEN typeof(capture_version)='integer' AND capture_version=?2 THEN 1 ELSE 0 END,
               CASE WHEN typeof(hash_domain)='blob'
                          AND typeof(registration_codec)='integer' AND registration_codec=?4
                    THEN length(hash_domain)=?3 ELSE 0 END,
               CASE WHEN typeof(row_digest)='blob' THEN length(row_digest)=32 ELSE 0 END,
               CASE WHEN typeof(root_nonce)='blob' THEN length(root_nonce)=32 ELSE 0 END
             FROM native_capture_format_v1 LIMIT 2",
        )
        .map_err(|error| engine_error("inspect native capture format", path, &error))?;
    let mut rows = query
        .query(params![
            i64::from(NATIVE_INDEX_STATE_VERSION),
            i64::from(NATIVE_SOURCE_CAPTURE_VERSION),
            i64::try_from(NATIVE_SOURCE_CAPTURE_HASH_DOMAIN.len()).map_err(|error| {
                CoreError::Storage(format!("native capture domain length overflow: {error}"))
            })?,
            registration_codec::VERSION
        ])
        .map_err(|error| engine_error("read native capture format", path, &error))?;
    let Some(row) = rows
        .next()
        .map_err(|error| engine_error("read native capture format", path, &error))?
    else {
        return Err(unsupported(path, "missing capture-format row"));
    };
    let shape: (i64, i64, i64, i64, i64, i64) = (
        row.get(0)
            .map_err(|error| engine_error("read native capture singleton shape", path, &error))?,
        row.get(1)
            .map_err(|error| engine_error("read native state version shape", path, &error))?,
        row.get(2)
            .map_err(|error| engine_error("read native capture version shape", path, &error))?,
        row.get(3).map_err(|error| {
            engine_error("read native capture domain/codec shape", path, &error)
        })?,
        row.get(4)
            .map_err(|error| engine_error("read native capture digest shape", path, &error))?,
        row.get(5)
            .map_err(|error| engine_error("read native root nonce shape", path, &error))?,
    );
    if rows
        .next()
        .map_err(|error| engine_error("read native capture format", path, &error))?
        .is_some()
    {
        return Err(unsupported(path, "capture-format singleton is not unique"));
    }
    if shape != (1, 1, 1, 1, 1, 1) {
        return Err(unsupported(
            path,
            "invalid state/capture/registration scalar or domain/digest/root nonce type/length",
        ));
    }
    drop(rows);
    drop(query);
    let contents: Option<([u8; 32], [u8; 32])> = connection
        .query_row(
            "SELECT CASE WHEN typeof(registration_codec)='integer' AND registration_codec=?1
                         THEN hash_domain END,
                    root_nonce,row_digest
             FROM native_capture_format_v1 WHERE singleton=1 LIMIT 1",
            params![registration_codec::VERSION],
            |row| {
                let domain = row.get_ref(0)?;
                let nonce = row.get_ref(1)?;
                let digest = row.get_ref(2)?;
                Ok(match (domain, nonce, digest) {
                    (ValueRef::Blob(domain), ValueRef::Blob(nonce), ValueRef::Blob(digest))
                        if domain == NATIVE_SOURCE_CAPTURE_HASH_DOMAIN
                            && nonce.len() == 32
                            && digest.len() == 32 =>
                    {
                        let nonce = <[u8; 32]>::try_from(nonce).map_err(|error| {
                            rusqlite::Error::FromSqlConversionFailure(
                                1,
                                rusqlite::types::Type::Blob,
                                Box::new(error),
                            )
                        })?;
                        let digest = <[u8; 32]>::try_from(digest).map_err(|error| {
                            rusqlite::Error::FromSqlConversionFailure(
                                2,
                                rusqlite::types::Type::Blob,
                                Box::new(error),
                            )
                        })?;
                        Some((nonce, digest))
                    }
                    _ => None,
                })
            },
        )
        .map_err(|error| engine_error("verify native capture format", path, &error))?;
    let Some((root_nonce, digest)) = contents else {
        return Err(unsupported(
            path,
            "capture domain, root nonce or digest mismatch",
        ));
    };
    let quarantine_fanout_complete = if has_incidents {
        digest.as_slice() == stamp_digest(&root_nonce, INCIDENT_COMPLETE_DOMAIN)?.as_slice()
    } else {
        has_quarantine
            && digest.as_slice() == stamp_digest(&root_nonce, QUARANTINE_FANOUT_DOMAIN)?.as_slice()
    };
    if root_nonce == [0; 32] {
        return Err(unsupported(
            path,
            "zero root nonce or capture digest mismatch",
        ));
    }
    let mut import_legacy_quarantine = false;
    if !quarantine_fanout_complete {
        let pending_digest = if has_incidents {
            import_legacy_quarantine =
                digest.as_slice() == stamp_digest(&root_nonce, INCIDENT_IMPORT_DOMAIN)?.as_slice();
            stamp_digest(&root_nonce, INCIDENT_PENDING_DOMAIN)?
        } else {
            expected_digest(&root_nonce, has_quarantine)?
        };
        if !import_legacy_quarantine && digest.as_slice() != pending_digest.as_slice() {
            return Err(unsupported(path, "capture digest mismatch"));
        }
    }
    Ok(CaptureStamp {
        root_nonce,
        row_digest: digest,
        quarantine_fanout_complete,
        import_legacy_quarantine,
    })
}

impl crate::SqliteCatalog {
    /// Read the once-issued Catalog incarnation under the existing authority.
    /// This does not authenticate a boot manifest, source cut or physical root.
    pub fn native_root_nonce(
        &self,
        budget: &mut quanta_index_core::index::WorkBudget,
    ) -> Result<[u8; 32], CoreError> {
        use quanta_index_core::index::existing_file::ExistingFileReadFailureV1 as Failure;
        budget.checkpoint().map_err(nonce_budget_error)?;
        let memory = budget
            .reserve_memory(1024, quanta_index_core::index::MemoryLifetime::Temporary)
            .map_err(nonce_budget_error)?;
        self.read_native_root_nonce_with_memory(budget, memory)
            .map_err(|error| match error {
                Failure::AdmissionOrIo(error) => nonce_budget_error(error),
                Failure::Canonical(error) => error,
            })
    }

    /// Observe fixed incarnation DATA through the existing writer connection.
    /// Original memory is mandatory; admission/lock failures keep IndexError.
    /// The caller separately holds and validates its actual StateRootLease.
    /// This does not authenticate a Source, installation, or publication permit.
    pub fn observe_native_root_nonce_v1(
        &self,
        budget: &mut quanta_index_core::index::WorkBudget,
    ) -> Result<[u8; 32], quanta_index_core::index::IndexError> {
        use quanta_index_core::index::{
            IndexError, MemoryLifetime, existing_file::ExistingFileReadFailureV1 as Failure,
        };
        budget.checkpoint()?;
        let memory = budget
            .reserve_memory(1024, MemoryLifetime::Temporary)?
            .ok_or(IndexError::ResourceLimited)?;
        self.read_native_root_nonce_with_memory(budget, Some(memory))
            .map_err(|error| match error {
                Failure::AdmissionOrIo(error) => error,
                Failure::Canonical(CoreError::Typed {
                    code: SearchPlaneErrorCodeV2::StateRootFormatUnsupported,
                    ..
                }) => IndexError::RebuildRequired,
                Failure::Canonical(CoreError::Typed {
                    code: SearchPlaneErrorCodeV2::CatalogBusy,
                    ..
                }) => IndexError::Conflict,
                Failure::Canonical(error) => IndexError::Storage(error.to_string()),
            })
    }

    // ONE canonical admission, connection and stamp parser path. The existing
    // Core-facing API retains its historical error/memory-scope contract; the
    // live Original-facing observer requires an actual grant before any SQL.
    fn read_native_root_nonce_with_memory(
        &self,
        budget: &mut quanta_index_core::index::WorkBudget,
        _memory: Option<quanta_index_core::index::MemoryLease>,
    ) -> Result<[u8; 32], quanta_index_core::index::existing_file::ExistingFileReadFailureV1> {
        use quanta_index_core::index::WorkUsage;
        budget.charge(WorkUsage {
            reads: 2,
            bytes: 160,
            ..WorkUsage::default()
        })?;
        let connection = self.lock_native_budget(budget)?;
        let nonce = verify_stamp(&connection, &self.path)?;
        budget.checkpoint()?;
        drop(connection);
        Ok(nonce)
    }
}

fn nonce_budget_error(error: quanta_index_core::index::IndexError) -> CoreError {
    use quanta_index_core::index::IndexError;
    match error {
        IndexError::ResourceLimited => CoreError::Typed {
            code: SearchPlaneErrorCodeV2::IngestResourceBudgetExceeded,
            message: error.to_string(),
        },
        IndexError::Cancelled => CoreError::Typed {
            code: SearchPlaneErrorCodeV2::RequestCancelled,
            message: error.to_string(),
        },
        error @ (IndexError::InvalidInput(_)
        | IndexError::Conflict
        | IndexError::DigestConflict
        | IndexError::FenceLost
        | IndexError::WorkFenceLost
        | IndexError::WorkLimitExceeded
        | IndexError::SnapshotUnavailable
        | IndexError::CorruptObject(_)
        | IndexError::BackendViolation(_)
        | IndexError::Storage(_)
        | IndexError::RebuildRequired
        | IndexError::FocusSubjectNotFound
        | IndexError::MaintenanceRequired) => CoreError::Storage(error.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::NATIVE_INDEX_STATE_VERSION;
    use std::{error::Error, path::Path, time::Duration};

    use quanta_index_contract::{
        SearchPlaneErrorCodeV2,
        index::{
            CollectionId, Digest, OperationKey, ProducerId, SlotId, SourceEvent, StreamId,
            TenantId, WorkspaceKey, WorktreeId,
        },
    };
    use quanta_index_core::CoreError;
    use rusqlite::{Connection, OptionalExtension as _, params, types::Value};

    use super::{
        super::{EventClaim, EventClaimState, operation_key, read_event_claim, write_event_claim},
        preflight_capture_format, verify_native_schema,
    };
    use crate::{
        SqliteCatalog,
        connection::{CATALOG_FILE_NAME, catalog_dir},
    };

    type StampRow = (Value, Value, Value, Value, Value, Value, Value);
    type StampShape = (
        String,
        String,
        String,
        String,
        i64,
        String,
        i64,
        String,
        i64,
    );

    macro_rules! check {
        ($condition:expr $(,)?) => {
            if !$condition {
                return Err(format!("check failed: {}", stringify!($condition)).into());
            }
        };
    }

    macro_rules! check_eq {
        ($left:expr, $right:expr $(,)?) => {{
            let left = &$left;
            let right = &$right;
            if left != right {
                return Err(format!("check failed: {left:?} != {right:?}").into());
            }
        }};
    }

    fn open(root: &std::path::Path) -> Result<SqliteCatalog, CoreError> {
        SqliteCatalog::open(root, Duration::from_millis(100))
    }

    fn path(root: &std::path::Path) -> std::path::PathBuf {
        catalog_dir(root).join(CATALOG_FILE_NAME)
    }

    fn require_format_refusal(root: &std::path::Path) -> Result<(), Box<dyn Error>> {
        check!(matches!(
            open(root),
            Err(CoreError::Typed {
                code: SearchPlaneErrorCodeV2::StateRootFormatUnsupported,
                ..
            })
        ));
        Ok(())
    }

    #[test]
    fn protected_source_view_schema_upgrades_and_refuses_changed_ddl() -> Result<(), Box<dyn Error>> {
        let root = tempfile::tempdir()?;
        drop(open(root.path())?);
        let file = path(root.path());
        let db = Connection::open(&file)?;
        db.execute_batch("DROP TABLE native_protected_source_view_v1")?;
        drop(db);

        drop(open(root.path())?);
        let db = Connection::open(&file)?;
        let ddl: String = db.query_row(
            "SELECT sql FROM sqlite_master WHERE name='native_protected_source_view_v1'",
            [],
            |row| row.get(0),
        )?;
        check!(ddl.contains("ON DELETE CASCADE"));
        db.execute_batch(
            "DROP TABLE native_protected_source_view_v1;
             CREATE TABLE native_protected_source_view_v1 (row_digest BLOB NOT NULL)",
        )?;
        drop(db);
        require_format_refusal(root.path())
    }

    fn claim(
        sequence: u64,
        predecessor: u64,
        payload: Digest,
        state: EventClaimState,
    ) -> EventClaim {
        let key = OperationKey {
            workspace: WorkspaceKey {
                tenant: TenantId([1; 16]),
                collection: CollectionId([2; 16]),
                worktree: WorktreeId([3; 16]),
                slot: SlotId([4; 16]),
            },
            epoch: 7,
            producer: ProducerId([8; 16]),
            stream: StreamId([9; 16]),
            stream_epoch: 12,
            sequence,
        };
        EventClaim {
            operation: key,
            event: SourceEvent {
                producer: key.producer,
                stream: key.stream,
                stream_epoch: key.stream_epoch,
                predecessor,
                sequence,
                payload,
            },
            state,
        }
    }

    #[test]
    fn sql_read_failure_is_not_reclassified_by_path_text() -> Result<(), Box<dyn Error>> {
        let connection = Connection::open_in_memory()?;
        // SQLite's missing-row error must remain Storage even when the path
        // contains the old DDL-refusal wording. No format mismatch was observed.
        check!(matches!(
            verify_native_schema(&connection, Path::new("has an unsupported/catalog.sqlite")),
            Err(CoreError::Storage(_))
        ));
        Ok(())
    }

    #[test]
    fn fixed_nonce_stamp_has_independent_golden() -> Result<(), Box<dyn Error>> {
        // Independently computed SHA-256 of the stamp domain and canonical
        // JSON [1,20,2,byte-array hash domain,1,[7;32]] with compact separators.
        const GOLDEN: [u8; 32] = [
            0xc9, 0xfb, 0x42, 0x23, 0x7d, 0x0c, 0x58, 0x89, 0x3c, 0x7a, 0x0c, 0x22, 0xbf, 0x04,
            0x99, 0xd4, 0x9e, 0x18, 0x2a, 0xee, 0xda, 0x0d, 0xac, 0x77, 0x7f, 0x29, 0xdc, 0x42,
            0x77, 0x66, 0x2c, 0x1c,
        ];
        let actual = super::expected_digest(&[7; 32], false)?;
        check_eq!(actual.as_slice(), GOLDEN.as_slice());
        Ok(())
    }

    #[test]
    fn ordinary_v20_catalog_defers_optional_extensions_and_reopens() -> Result<(), Box<dyn Error>> {
        let root = tempfile::tempdir()?;
        drop(open(root.path())?);
        let file = path(root.path());
        let db = Connection::open(&file)?;
        let nonce = super::verify_stamp(&db, &file)?;
        let optional: i64 = db.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE name IN (
               'native_view_source_body_quarantine_v1',
               'native_source_body_incident_v1',
               'native_source_stage_replay_v1',
               'native_source_stage_replay_rotate_v1',
               'native_source_stage_replay_delete_v1')",
            [],
            |row| row.get(0),
        )?;
        check_eq!(optional, 0);
        let digest: Vec<u8> = db.query_row(
            "SELECT row_digest FROM native_capture_format_v1 WHERE singleton=1",
            [],
            |row| row.get(0),
        )?;
        check_eq!(digest, super::expected_digest(&nonce, false)?);
        drop(db);

        drop(open(root.path())?);
        let db = Connection::open(&file)?;
        check_eq!(super::verify_stamp(&db, &file)?, nonce);
        check!(!super::has_quarantine_schema(&db, &file)?);
        check!(!super::has_incident_schema(&db, &file)?);
        Ok(())
    }

    #[test]
    fn previous_view_quarantine_catalog_upgrades_once_and_refuses_missing_incident_table()
    -> Result<(), Box<dyn Error>> {
        let root = tempfile::tempdir()?;
        drop(open(root.path())?);
        let file = path(root.path());
        let db = Connection::open(&file)?;
        let nonce = super::verify_stamp(&db, &file)?;
        db.execute_batch(super::VIEW_QUARANTINE_SCHEMA)?;
        let changed = db.execute(
            "UPDATE native_capture_format_v1 SET row_digest=?1 WHERE singleton=1",
            params![super::expected_digest(&nonce, true)?],
        )?;
        check_eq!(changed, 1);
        drop(db);

        drop(open(root.path())?);
        let db = Connection::open(&file)?;
        check_eq!(super::verify_stamp(&db, &file)?, nonce);
        check!(super::has_quarantine_schema(&db, &file)?);
        check!(super::has_incident_schema(&db, &file)?);
        db.execute_batch("DROP TABLE native_source_body_incident_v1")?;
        drop(db);
        require_format_refusal(root.path())?;
        Ok(())
    }

    #[test]
    fn first_birth_stamps_current_semantics_and_reopen_verifies_it() -> Result<(), Box<dyn Error>> {
        let root = tempfile::tempdir()?;
        drop(open(root.path())?);
        let db = Connection::open(path(root.path()))?;
        let (state, capture, domain, codec, nonce, digest): (i64, i64, Vec<u8>, i64, Vec<u8>, Vec<u8>) = db.query_row(
            "SELECT state_version,capture_version,hash_domain,registration_codec,root_nonce,row_digest FROM native_capture_format_v1 WHERE singleton=1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?)),
        )?;
        check_eq!(state, i64::from(NATIVE_INDEX_STATE_VERSION));
        check_eq!(capture, 2);
        check_eq!(codec, super::registration_codec::VERSION);
        check_eq!(
            domain.as_slice(),
            b"quanta-index-native-source-capture-v2\0".as_slice()
        );
        let nonce: [u8; 32] = nonce.try_into().map_err(|_nonce| "nonce width differs")?;
        check!(nonce != [0; 32]);
        check_eq!(digest, super::expected_digest(&nonce, false)?);
        drop(db);
        let catalog = open(root.path())?;
        let mut budget = quanta_index_core::index::WorkBudget::with_request(
            quanta_index_core::index::WorkLimits {
                reads: 2,
                writes: 0,
                bytes: 160,
                mutations: 0,
            },
            quanta_index_core::RequestBudgetV1::for_duration(Duration::from_secs(2)),
        );
        check_eq!(catalog.native_root_nonce(&mut budget)?, nonce);
        let usage = budget.usage();
        check_eq!(usage.reads, 2);
        check_eq!(usage.writes, 0);
        check_eq!(usage.bytes, 160);
        drop(catalog);
        drop(open(root.path())?);
        Ok(())
    }

    #[test]
    fn source_stage_replay_schema_reopens_and_refuses_trigger_drift() -> Result<(), Box<dyn Error>>
    {
        let healthy = tempfile::tempdir()?;
        drop(open(healthy.path())?);
        let db = Connection::open(path(healthy.path()))?;
        db.execute_batch(super::source_stage_replay_receipt::SCHEMA)?;
        drop(db);
        drop(open(healthy.path())?);
        let db = Connection::open(path(healthy.path()))?;
        let installed: i64 = db.query_row(
            "SELECT COUNT(*) FROM sqlite_master
             WHERE (type='table' AND name='native_source_stage_replay_v1')
                OR (type='trigger' AND name IN (
                    'native_source_stage_replay_rotate_v1',
                    'native_source_stage_replay_delete_v1'
                ))",
            [],
            |row| row.get(0),
        )?;
        check_eq!(installed, 3);
        drop(db);

        for mutation in [
            "DROP TRIGGER native_source_stage_replay_rotate_v1",
            "DROP TRIGGER native_source_stage_replay_rotate_v1;
             CREATE TRIGGER native_source_stage_replay_rotate_v1
             AFTER UPDATE OF fence ON native_ingress_v1
             BEGIN SELECT 1; END",
        ] {
            let root = tempfile::tempdir()?;
            drop(open(root.path())?);
            let file = path(root.path());
            let db = Connection::open(&file)?;
            db.execute_batch(super::source_stage_replay_receipt::SCHEMA)?;
            db.execute_batch(mutation)?;
            let before: Option<String> = db
                .query_row(
                    "SELECT sql FROM sqlite_master
                     WHERE type='trigger' AND name='native_source_stage_replay_rotate_v1'",
                    [],
                    |row| row.get(0),
                )
                .optional()?;
            drop(db);

            require_format_refusal(root.path())?;
            let db = Connection::open(&file)?;
            let after: Option<String> = db
                .query_row(
                    "SELECT sql FROM sqlite_master
                     WHERE type='trigger' AND name='native_source_stage_replay_rotate_v1'",
                    [],
                    |row| row.get(0),
                )
                .optional()?;
            check_eq!(after, before);
        }
        Ok(())
    }

    #[test]
    fn nonce_getter_preserves_budget_and_state_refusal_classes() -> Result<(), Box<dyn Error>> {
        use quanta_index_core::{
            RequestBudgetV1,
            index::{WorkBudget, WorkLimits},
        };
        let root = tempfile::tempdir()?;
        let catalog = open(root.path())?;
        let limited = WorkLimits {
            reads: 1,
            writes: 0,
            bytes: 160,
            mutations: 0,
        };
        let mut budget = WorkBudget::with_request(limited, RequestBudgetV1::unbounded());
        check!(matches!(
            catalog.native_root_nonce(&mut budget),
            Err(CoreError::Typed {
                code: SearchPlaneErrorCodeV2::IngestResourceBudgetExceeded,
                ..
            })
        ));
        check_eq!(budget.usage().reads, 0);
        let request = RequestBudgetV1::unbounded();
        request.cancel_handle().cancel();
        let mut budget = WorkBudget::with_request(limited, request);
        check!(matches!(
            catalog.native_root_nonce(&mut budget),
            Err(CoreError::Typed {
                code: SearchPlaneErrorCodeV2::RequestCancelled,
                ..
            })
        ));
        check_eq!(budget.usage().reads, 0);
        let db = Connection::open(path(root.path()))?;
        db.execute_batch("UPDATE native_capture_format_v1 SET root_nonce=zeroblob(32)")?;
        let mut budget = WorkBudget::with_request(
            WorkLimits {
                reads: 2,
                ..limited
            },
            RequestBudgetV1::unbounded(),
        );
        check!(matches!(
            catalog.native_root_nonce(&mut budget),
            Err(CoreError::Typed {
                code: SearchPlaneErrorCodeV2::StateRootFormatUnsupported,
                ..
            })
        ));
        Ok(())
    }

    #[test]
    fn non_native_tables_do_not_claim_native_format() -> Result<(), Box<dyn Error>> {
        let db = Connection::open_in_memory()?;
        db.execute_batch("CREATE TABLE generic_catalog_table(id INTEGER PRIMARY KEY)")?;
        check!(preflight_capture_format(&db, Path::new(":memory:"))?);
        Ok(())
    }

    #[test]
    fn old_native_namespace_refuses_without_rewriting_pending_or_refused_claims()
    -> Result<(), Box<dyn Error>> {
        let root = tempfile::tempdir()?;
        drop(open(root.path())?);
        let db = Connection::open(path(root.path()))?;
        let claims = [
            claim(1, 0, Digest([1; 32]), EventClaimState::Pending),
            claim(2, 1, Digest([2; 32]), EventClaimState::Refused),
        ];
        for claim in claims {
            let operation = operation_key(claim.operation)?;
            let _inserted = db.execute(
                "INSERT INTO native_operation_v1(operation,workspace,epoch,payload,row_digest)
                 VALUES (?1,x'01',0,?2,x'02')",
                params![operation, b"retained-operation".as_slice()],
            )?;
            write_event_claim(&db, claim, true)?;
            check_eq!(
                read_event_claim(&db, claim.operation, claim.event)?,
                Some(claim)
            );
        }
        db.execute_batch("DROP TABLE native_capture_format_v1")?;
        drop(db);
        require_format_refusal(root.path())?;
        let db = Connection::open(path(root.path()))?;
        for claim in claims {
            check_eq!(
                read_event_claim(&db, claim.operation, claim.event)?,
                Some(claim)
            );
            let operation_payload: Vec<u8> = db.query_row(
                "SELECT payload FROM native_operation_v1 WHERE operation=?1",
                params![operation_key(claim.operation)?],
                |row| row.get(0),
            )?;
            check_eq!(
                operation_payload.as_slice(),
                b"retained-operation".as_slice()
            );
        }
        Ok(())
    }

    #[test]
    fn corrupt_or_missing_capture_stamp_refuses_before_recovery() -> Result<(), Box<dyn Error>> {
        for mutation in [
            "DELETE FROM native_capture_format_v1",
            // The fully authenticated predecessor must refuse before DDL or
            // recovery. A v18 root is rebuilt in a separate v19 incarnation.
            "UPDATE native_capture_format_v1 SET state_version=18,
             root_nonce=x'0707070707070707070707070707070707070707070707070707070707070707',
             row_digest=x'd240ff8800f7b6af1148741ddeceb2e10f3c887f151519508895b92e2da6d1da'",
            // Authenticated previous17 must refuse before recovery, too.
            "UPDATE native_capture_format_v1 SET state_version=17,
             root_nonce=x'0707070707070707070707070707070707070707070707070707070707070707',
             row_digest=x'ecf3934e4b7e22db7024c8cccb0dc425f48dd0f1e52f19a39a1f28802048ab14'",
            // This checksum is the independent, formerly current v15 oracle.
            "UPDATE native_capture_format_v1 SET state_version=16,
             root_nonce=x'0707070707070707070707070707070707070707070707070707070707070707',
             row_digest=x'782cdffb296fab62665f9d1ae082a76b6aeecc69a592ecd1731c97de25d28bc5'",
            "UPDATE native_capture_format_v1 SET state_version=15,
             root_nonce=x'0707070707070707070707070707070707070707070707070707070707070707',
             row_digest=x'fe6f32b78b792b0a375ca033b6e105abfe29098c11e106d10a40a877562334aa'",
            "UPDATE native_capture_format_v1 SET state_version=14",
            // A fully authenticated v14 stamp still requires an explicit
            // rebuild: rejection must not depend on a stale checksum.
            "UPDATE native_capture_format_v1 SET state_version=14,
             root_nonce=x'0707070707070707070707070707070707070707070707070707070707070707',
             row_digest=x'55c2f71343d0ff2b4da237fa5c7bf968dafe9164717d8bd29bca7b76ea57efe2'",
            "UPDATE native_capture_format_v1 SET state_version=12",
            "UPDATE native_capture_format_v1 SET state_version=10",
            "UPDATE native_capture_format_v1 SET capture_version=1",
            "PRAGMA ignore_check_constraints=ON;
             UPDATE native_capture_format_v1 SET registration_codec=2",
            "PRAGMA ignore_check_constraints=ON;
             UPDATE native_capture_format_v1 SET registration_codec=x'01'",
            "UPDATE native_capture_format_v1 SET hash_domain=x'01'",
            "UPDATE native_capture_format_v1 SET row_digest=x'01'",
            "UPDATE native_capture_format_v1 SET root_nonce=zeroblob(32)",
            // Independent SHA-256 for zero nonce with otherwise valid v19
            // stamp: nonzero identity must refuse even when checksum matches.
            "UPDATE native_capture_format_v1 SET root_nonce=zeroblob(32),
             row_digest=x'e5b493a2896ae3234e0078b22a628f0cd2dc2fb06a8585bf76c6c2c57a7c004d'",
            "UPDATE native_capture_format_v1 SET root_nonce=randomblob(32)",
            "PRAGMA ignore_check_constraints=ON;
             UPDATE native_capture_format_v1 SET root_nonce=x'01'",
            "PRAGMA ignore_check_constraints=ON;
             UPDATE native_capture_format_v1 SET singleton=2",
            "PRAGMA ignore_check_constraints=ON;
             INSERT INTO native_capture_format_v1
             (singleton,state_version,capture_version,hash_domain,registration_codec,root_nonce,row_digest)
             SELECT 2,state_version,capture_version,hash_domain,registration_codec,root_nonce,row_digest
             FROM native_capture_format_v1 WHERE singleton=1",
        ] {
            let root = tempfile::tempdir()?;
            drop(open(root.path())?);
            let db = Connection::open(path(root.path()))?;
            db.execute_batch(mutation)?;
            let before: Option<StampRow> = db
                .query_row(
                    "SELECT singleton,state_version,capture_version,hash_domain,registration_codec,row_digest,root_nonce
                     FROM native_capture_format_v1 ORDER BY singleton LIMIT 1",
                    [],
                    |row| {
                        Ok((
                            row.get(0)?,
                            row.get(1)?,
                            row.get(2)?,
                            row.get(3)?,
                            row.get(4)?,
                            row.get(5)?,
                            row.get(6)?,
                        ))
                    },
                )
                .optional()?;
            let before_count: i64 =
                db.query_row("SELECT COUNT(*) FROM native_capture_format_v1", [], |row| {
                    row.get(0)
                })?;
            drop(db);
            require_format_refusal(root.path())?;
            let db = Connection::open(path(root.path()))?;
            let after: Option<StampRow> = db
                .query_row(
                    "SELECT singleton,state_version,capture_version,hash_domain,registration_codec,row_digest,root_nonce
                     FROM native_capture_format_v1 ORDER BY singleton LIMIT 1",
                    [],
                    |row| {
                        Ok((
                            row.get(0)?,
                            row.get(1)?,
                            row.get(2)?,
                            row.get(3)?,
                            row.get(4)?,
                            row.get(5)?,
                            row.get(6)?,
                        ))
                    },
                )
                .optional()?;
            let after_count: i64 =
                db.query_row("SELECT COUNT(*) FROM native_capture_format_v1", [], |row| {
                    row.get(0)
                })?;
            check_eq!(before, after);
            check_eq!(before_count, after_count);
        }
        Ok(())
    }

    #[test]
    fn v12_stamp_without_nonce_refuses_without_backfill() -> Result<(), Box<dyn Error>> {
        let root = tempfile::tempdir()?;
        drop(open(root.path())?);
        let db = Connection::open(path(root.path()))?;
        db.execute_batch(
            "DROP TABLE native_capture_format_v1;
             CREATE TABLE native_capture_format_v1 (
                singleton INTEGER PRIMARY KEY NOT NULL CHECK(singleton=1),
                state_version INTEGER NOT NULL CHECK(state_version>0),
                capture_version INTEGER NOT NULL CHECK(capture_version>0),
                hash_domain BLOB NOT NULL, row_digest BLOB NOT NULL
             ) WITHOUT ROWID;
             INSERT INTO native_capture_format_v1 VALUES (1,12,2,x'01',x'02');",
        )?;
        let before: String = db.query_row(
            "SELECT sql FROM sqlite_master WHERE name='native_capture_format_v1'",
            [],
            |row| row.get(0),
        )?;
        drop(db);
        require_format_refusal(root.path())?;
        let db = Connection::open(path(root.path()))?;
        let after: String = db.query_row(
            "SELECT sql FROM sqlite_master WHERE name='native_capture_format_v1'",
            [],
            |row| row.get(0),
        )?;
        check_eq!(before, after);
        let row: (i64, i64, Vec<u8>, Vec<u8>) = db.query_row(
            "SELECT state_version,capture_version,hash_domain,row_digest FROM native_capture_format_v1",
            [], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )?;
        check_eq!(row, (12, 2, vec![1], vec![2]));
        Ok(())
    }

    #[test]
    fn oversized_or_wrong_typed_stamp_refuses_without_materializing_payloads()
    -> Result<(), Box<dyn Error>> {
        for mutation in [
            "UPDATE native_capture_format_v1 SET hash_domain=zeroblob(1048576)",
            "UPDATE native_capture_format_v1 SET row_digest=zeroblob(1048576)",
            "UPDATE native_capture_format_v1 SET state_version=x'01'",
            "UPDATE native_capture_format_v1 SET capture_version='bad'",
            "UPDATE native_capture_format_v1 SET registration_codec='bad'",
            "UPDATE native_capture_format_v1 SET hash_domain='text-domain'",
            "UPDATE native_capture_format_v1 SET row_digest='text-digest'",
            "UPDATE native_capture_format_v1 SET root_nonce=zeroblob(1048576)",
            "UPDATE native_capture_format_v1 SET root_nonce='text-nonce'",
        ] {
            let root = tempfile::tempdir()?;
            drop(open(root.path())?);
            let db = Connection::open(path(root.path()))?;
            db.execute_batch("PRAGMA ignore_check_constraints=ON")?;
            db.execute_batch(mutation)?;
            let before: StampShape = db.query_row(
                "SELECT typeof(state_version),typeof(capture_version),typeof(registration_codec),
                        typeof(hash_domain),length(hash_domain),
                        typeof(row_digest),length(row_digest),typeof(root_nonce),length(root_nonce)
                 FROM native_capture_format_v1",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                        row.get(6)?,
                        row.get(7)?,
                        row.get(8)?,
                    ))
                },
            )?;
            drop(db);
            require_format_refusal(root.path())?;
            let db = Connection::open(path(root.path()))?;
            let after: StampShape = db.query_row(
                "SELECT typeof(state_version),typeof(capture_version),typeof(registration_codec),
                        typeof(hash_domain),length(hash_domain),
                        typeof(row_digest),length(row_digest),typeof(root_nonce),length(root_nonce)
                 FROM native_capture_format_v1",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                        row.get(6)?,
                        row.get(7)?,
                        row.get(8)?,
                    ))
                },
            )?;
            check_eq!(before, after);
        }
        Ok(())
    }

    #[test]
    fn unknown_or_partial_native_namespace_refuses_before_catalog_ddl() -> Result<(), Box<dyn Error>>
    {
        for fixture in [
            "CREATE TABLE native_future_v1(id INTEGER PRIMARY KEY)",
            "CREATE TABLE native_workspace_v1(id INTEGER PRIMARY KEY)",
            "CREATE TABLE NATIVE_workspace_v1(id INTEGER PRIMARY KEY)",
        ] {
            let root = tempfile::tempdir()?;
            let directory = catalog_dir(root.path());
            std::fs::create_dir_all(&directory)?;
            let db = Connection::open(path(root.path()))?;
            db.execute_batch(fixture)?;
            drop(db);
            require_format_refusal(root.path())?;
            let db = Connection::open(path(root.path()))?;
            let journal_exists: i64 = db.query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE name='idempotency_v2'",
                [],
                |row| row.get(0),
            )?;
            check_eq!(journal_exists, 0);
        }
        Ok(())
    }

    #[test]
    fn recast_native_schema_refuses_before_any_catalog_repair() -> Result<(), Box<dyn Error>> {
        let root = tempfile::tempdir()?;
        drop(open(root.path())?);
        let db = Connection::open(path(root.path()))?;
        db.execute_batch(
            "DROP INDEX native_operation_workspace_idx_v1;
             CREATE INDEX native_operation_workspace_idx_v1 ON native_operation_v1(epoch)",
        )?;
        let before: String = db.query_row(
            "SELECT sql FROM sqlite_master WHERE name='native_operation_workspace_idx_v1'",
            [],
            |row| row.get(0),
        )?;
        drop(db);
        require_format_refusal(root.path())?;
        let db = Connection::open(path(root.path()))?;
        let after: String = db.query_row(
            "SELECT sql FROM sqlite_master WHERE name='native_operation_workspace_idx_v1'",
            [],
            |row| row.get(0),
        )?;
        check_eq!(before, after);
        Ok(())
    }

    #[test]
    fn oversized_valid_native_ddl_refuses_before_shared_schema_reader() -> Result<(), Box<dyn Error>>
    {
        let root = tempfile::tempdir()?;
        drop(open(root.path())?);
        let db = Connection::open(path(root.path()))?;
        let comment = "x".repeat(super::super::SCHEMA.len());
        db.execute_batch(&format!(
            "DROP INDEX native_operation_workspace_idx_v1;
             CREATE INDEX native_operation_workspace_idx_v1
             ON native_operation_v1(workspace, /* {comment} */ epoch)"
        ))?;
        let before: String = db.query_row(
            "SELECT sql FROM sqlite_master WHERE name='native_operation_workspace_idx_v1'",
            [],
            |row| row.get(0),
        )?;
        check!(before.len() > super::super::SCHEMA.len());
        drop(db);
        require_format_refusal(root.path())?;
        let db = Connection::open(path(root.path()))?;
        let after: String = db.query_row(
            "SELECT sql FROM sqlite_master WHERE name='native_operation_workspace_idx_v1'",
            [],
            |row| row.get(0),
        )?;
        check_eq!(before, after);
        Ok(())
    }
    #[test]
    fn live_nonce_observer_refuses_unfunded_and_cancelled_before_sql() -> Result<(), Box<dyn Error>>
    {
        use quanta_index_core::{
            RequestBudgetV1,
            index::{IndexError, WorkBudget, WorkLimits},
        };
        let root = tempfile::tempdir()?;
        let catalog = open(root.path())?;
        let limits = WorkLimits {
            reads: 2,
            writes: 0,
            bytes: 160,
            mutations: 0,
        };
        let mut work = WorkBudget::with_request(limits, RequestBudgetV1::unbounded());
        check!(matches!(
            catalog.observe_native_root_nonce_v1(&mut work),
            Err(IndexError::ResourceLimited)
        ));
        check_eq!(work.usage().reads, 0);
        let request = RequestBudgetV1::unbounded();
        request.cancel_handle().cancel();
        let mut work = WorkBudget::with_request(limits, request);
        check!(matches!(
            catalog.observe_native_root_nonce_v1(&mut work),
            Err(IndexError::Cancelled)
        ));
        check_eq!(work.usage().reads, 0);
        Ok(())
    }
}
