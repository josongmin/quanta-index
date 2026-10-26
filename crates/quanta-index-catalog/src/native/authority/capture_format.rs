//! Semantic version gate for persisted native source-event claims.
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

use super::{SCHEMA, SCHEMA_OBJECTS, fields_digest};
use crate::connection::engine_error;

const DIGEST_DOMAIN: &[u8] = b"qidx.native.capture-format.v1\0";

fn unsupported(path: &Path, detail: &str) -> CoreError {
    CoreError::Typed {
        code: SearchPlaneErrorCodeV2::StateRootFormatUnsupported,
        message: format!(
            "catalog: {} has unsupported native source-capture format ({detail}); use a new state root and rebuild the index",
            path.display()
        ),
    }
}

fn expected_digest(root_nonce: &[u8; 32]) -> Result<Vec<u8>, CoreError> {
    fields_digest(
        DIGEST_DOMAIN,
        &(
            1_i64,
            i64::from(NATIVE_INDEX_STATE_VERSION),
            i64::from(NATIVE_SOURCE_CAPTURE_VERSION),
            NATIVE_SOURCE_CAPTURE_HASH_DOMAIN,
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
                    CASE WHEN typeof(sql)='text' THEN length(CAST(sql AS BLOB)) END
             FROM sqlite_master
             WHERE (substr(name,1,7)='native_' COLLATE NOCASE
                    OR substr(tbl_name,1,7)='native_' COLLATE NOCASE)
               AND name NOT GLOB 'sqlite_autoindex_*'",
        )
        .map_err(|error| engine_error("inspect native namespace", path, &error))?;
    let mut rows = query
        .query([])
        .map_err(|error| engine_error("inspect native namespace", path, &error))?;
    let mut installed = 0usize;
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
        if !SCHEMA_OBJECTS
            .iter()
            .any(|(expected_kind, expected_name, _)| {
                *expected_kind == kind && *expected_name == name
            })
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
        let max_sql_bytes = i64::try_from(SCHEMA.len()).map_err(|error| {
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
        installed = installed
            .checked_add(1)
            .ok_or_else(|| CoreError::Storage("native schema count overflow".into()))?;
    }
    drop(rows);
    drop(query);
    if installed == 0 {
        return Ok(true);
    }
    if installed != SCHEMA_OBJECTS.len() {
        return Err(unsupported(
            path,
            "partial native namespace or missing format table",
        ));
    }
    // The installed DDL must be checked before even idempotent CREATEs run.
    // Preserve actual SQLite errors; only incompatible DDL is a format refusal.
    verify_native_schema(connection, path)?;
    let _nonce = verify_stamp(connection, path)?;
    let _selected = super::boot_manifest::read_stored(connection)
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
                 (singleton,state_version,capture_version,hash_domain,root_nonce,row_digest)
                 VALUES (1,?1,?2,?3,?4,?5)",
                params![
                    i64::from(NATIVE_INDEX_STATE_VERSION),
                    i64::from(NATIVE_SOURCE_CAPTURE_VERSION),
                    NATIVE_SOURCE_CAPTURE_HASH_DOMAIN,
                    root_nonce.as_slice(),
                    expected_digest(&root_nonce)?
                ],
            )
            .map_err(|error| engine_error("seed native capture format", path, &error))?;
    }
    verify_stamp(connection, path).map(|_nonce| ())
}

pub(super) fn verify_stamp(connection: &Connection, path: &Path) -> Result<[u8; 32], CoreError> {
    let mut query = connection
        .prepare(
            "SELECT
               CASE WHEN typeof(singleton)='integer' AND singleton=1 THEN 1 ELSE 0 END,
               CASE WHEN typeof(state_version)='integer' AND state_version=?1 THEN 1 ELSE 0 END,
               CASE WHEN typeof(capture_version)='integer' AND capture_version=?2 THEN 1 ELSE 0 END,
               CASE WHEN typeof(hash_domain)='blob' THEN length(hash_domain)=?3 ELSE 0 END,
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
            })?
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
        row.get(3)
            .map_err(|error| engine_error("read native capture domain shape", path, &error))?,
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
            "invalid state/capture scalar or domain/digest/root nonce type/length",
        ));
    }
    drop(rows);
    drop(query);
    let contents: Option<([u8; 32], [u8; 32])> = connection
        .query_row(
            "SELECT hash_domain,root_nonce,row_digest FROM native_capture_format_v1 WHERE singleton=1 LIMIT 1",
            [],
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
    if root_nonce == [0; 32] || digest.as_slice() != expected_digest(&root_nonce)?.as_slice() {
        return Err(unsupported(
            path,
            "zero root nonce or capture digest mismatch",
        ));
    }
    Ok(root_nonce)
}

impl crate::SqliteCatalog {
    /// Read the once-issued Catalog incarnation under the existing authority.
    /// This does not authenticate a boot manifest, source cut or physical root.
    pub fn native_root_nonce(
        &self,
        budget: &mut quanta_index_core::index::WorkBudget,
    ) -> Result<[u8; 32], CoreError> {
        use quanta_index_core::index::WorkUsage;
        budget
            .charge(WorkUsage {
                reads: 2,
                bytes: 160,
                ..WorkUsage::default()
            })
            .map_err(nonce_budget_error)?;
        // Both result rows are fixed-width: six scalar flags, the fixed
        // capture domain, nonce and digest. Fund their bounded hash scratch.
        let _memory = budget
            .reserve_memory(1024, quanta_index_core::index::MemoryLifetime::Temporary)
            .map_err(nonce_budget_error)?;
        let connection = self
            .lock_native_budget(budget)
            .map_err(nonce_budget_error)?;
        let nonce = verify_stamp(&connection, &self.path)?;
        budget.checkpoint().map_err(nonce_budget_error)?;
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
        // JSON [1,18,2,byte-array hash domain,[7;32]] with compact separators.
        const GOLDEN: [u8; 32] = [
            0x37, 0xed, 0xc7, 0x9b, 0x60, 0x72, 0x27, 0x64, 0xec, 0x2f, 0x00, 0xee, 0x70, 0x53,
            0x1b, 0x76, 0x11, 0x6f, 0xfc, 0x74, 0x05, 0xd4, 0x3f, 0x28, 0x22, 0x66, 0xd0, 0x4a,
            0x7f, 0x39, 0x93, 0xaf,
        ];
        let actual = super::expected_digest(&[7; 32])?;
        check_eq!(actual.as_slice(), GOLDEN.as_slice());
        Ok(())
    }

    #[test]
    fn first_birth_stamps_current_semantics_and_reopen_verifies_it() -> Result<(), Box<dyn Error>> {
        let root = tempfile::tempdir()?;
        drop(open(root.path())?);
        let db = Connection::open(path(root.path()))?;
        let (state, capture, domain, nonce, digest): (i64, i64, Vec<u8>, Vec<u8>, Vec<u8>) = db.query_row(
            "SELECT state_version,capture_version,hash_domain,root_nonce,row_digest FROM native_capture_format_v1 WHERE singleton=1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
        )?;
        check_eq!(state, i64::from(NATIVE_INDEX_STATE_VERSION));
        check_eq!(capture, 2);
        check_eq!(
            domain.as_slice(),
            b"quanta-index-native-source-capture-v2\0".as_slice()
        );
        let nonce: [u8; 32] = nonce.try_into().map_err(|_nonce| "nonce width differs")?;
        check!(nonce != [0; 32]);
        check_eq!(digest.len(), 32);
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
            "UPDATE native_capture_format_v1 SET hash_domain=x'01'",
            "UPDATE native_capture_format_v1 SET row_digest=x'01'",
            "UPDATE native_capture_format_v1 SET root_nonce=zeroblob(32)",
            // Independent SHA-256 for zero nonce with otherwise valid v18
            // stamp: nonzero identity must refuse even when checksum matches.
            "UPDATE native_capture_format_v1 SET root_nonce=zeroblob(32),
             row_digest=x'291662fe726c4b25d2457d176b9e3cc31a1cd8feb7d77edfcba9bb465865ef12'",
            "UPDATE native_capture_format_v1 SET root_nonce=randomblob(32)",
            "PRAGMA ignore_check_constraints=ON;
             UPDATE native_capture_format_v1 SET root_nonce=x'01'",
            "PRAGMA ignore_check_constraints=ON;
             UPDATE native_capture_format_v1 SET singleton=2",
            "PRAGMA ignore_check_constraints=ON;
             INSERT INTO native_capture_format_v1
             (singleton,state_version,capture_version,hash_domain,root_nonce,row_digest)
             SELECT 2,state_version,capture_version,hash_domain,root_nonce,row_digest
             FROM native_capture_format_v1 WHERE singleton=1",
        ] {
            let root = tempfile::tempdir()?;
            drop(open(root.path())?);
            let db = Connection::open(path(root.path()))?;
            db.execute_batch(mutation)?;
            let before: Option<(Value, Value, Value, Value, Value, Value)> = db
                .query_row(
                    "SELECT singleton,state_version,capture_version,hash_domain,row_digest,root_nonce
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
            let after: Option<(Value, Value, Value, Value, Value, Value)> = db
                .query_row(
                    "SELECT singleton,state_version,capture_version,hash_domain,row_digest,root_nonce
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
            let before: (String, String, String, i64, String, i64, String, i64) = db.query_row(
                "SELECT typeof(state_version),typeof(capture_version),
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
                    ))
                },
            )?;
            drop(db);
            require_format_refusal(root.path())?;
            let db = Connection::open(path(root.path()))?;
            let after: (String, String, String, i64, String, i64, String, i64) = db.query_row(
                "SELECT typeof(state_version),typeof(capture_version),
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
}
