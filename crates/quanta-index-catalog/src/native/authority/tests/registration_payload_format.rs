//! Public format gate and actual Catalog custody for the private registration codec.
use std::error::Error;

use quanta_index_contract::SearchPlaneErrorCodeV2;
use quanta_index_core::index::ports::{
    InputStagingPort, ManagedObjectInstallPort, OperationJournalPort,
};
use quanta_index_core::{
    CoreError,
    index::{
        AdmissionOutcome, AttemptRef, ImmutableOperation, ObjectInstallScope, ObjectRef,
        RegisteredObject,
    },
};
use rusqlite::{Connection, params};
use sha2::{Digest as _, Sha256};

use super::{TestResult, admission_budget, managed_adoption_fixture, object, open, operation};
use crate::connection::SqliteCatalog;
use crate::native::authority::{object_key, operation_key, registration_codec};

type Marker = (
    i64,
    i64,
    i64,
    Vec<u8>,
    Vec<u8>,
    Option<Vec<u8>>,
    Option<Vec<u8>>,
    Vec<u8>,
);

fn marker(connection: &Connection) -> Result<Marker, Box<dyn Error>> {
    Ok(connection.query_row(
        "SELECT singleton,state_version,capture_version,hash_domain,root_nonce,
                boot_manifest,boot_manifest_digest,row_digest FROM native_capture_format_v1",
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
    )?)
}

fn expected_payload(value: &RegisteredObject) -> Result<Vec<u8>, Box<dyn Error>> {
    let expected_len = value
        .children
        .len()
        .checked_mul(32)
        .and_then(|bytes| bytes.checked_add(44))
        .ok_or("fixed payload gold length overflow")?;
    let mut bytes = Vec::with_capacity(expected_len);
    bytes.extend_from_slice(&value.reference.0.0);
    bytes.extend_from_slice(&value.bytes.to_be_bytes());
    bytes.extend_from_slice(&u32::try_from(value.children.len())?.to_be_bytes());
    for child in &value.children {
        bytes.extend_from_slice(&child.0.0);
    }
    Ok(bytes)
}

fn row_digest(domain: &[u8], payload: &[u8]) -> Vec<u8> {
    let mut hasher = Sha256::new();
    hasher.update(domain);
    hasher.update(payload);
    hasher.finalize().to_vec()
}

fn admit_managed_attempt(
    catalog: &SqliteCatalog,
    operation: ImmutableOperation,
) -> Result<AttemptRef, Box<dyn Error>> {
    let request = RegisteredObject::unscoped(operation.request, 1, Vec::new());
    let ingress = catalog.open_ingress(operation.key)?;
    catalog.reserve_managed(
        ObjectInstallScope::Ingress(ingress),
        std::slice::from_ref(&request),
        &mut admission_budget(),
    )?;
    catalog.confirm_managed(
        ObjectInstallScope::Ingress(ingress),
        std::slice::from_ref(&request),
        &mut admission_budget(),
    )?;
    catalog.seal_inputs(ingress, operation.request)?;
    match catalog.admit(operation, ingress, &[], &mut admission_budget())? {
        AdmissionOutcome::New(attempt) => Ok(attempt),
        AdmissionOutcome::Existing(status) => {
            Err(format!("expected fresh managed attempt, got {status:?}").into())
        }
    }
}

fn object_payload(
    connection: &Connection,
    table: &str,
    reference: ObjectRef,
) -> Result<(Vec<u8>, Vec<u8>), Box<dyn Error>> {
    let sql = format!("SELECT payload,row_digest FROM {table} WHERE object=?1");
    Ok(
        connection.query_row(&sql, params![object_key(reference)], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })?,
    )
}

#[test]
fn missing_codec_stamp_with_old_json_registration_refuses_before_any_rewrite() -> TestResult {
    let root = tempfile::tempdir()?;
    let catalog = open(root.path())?;
    let path = catalog.path().to_path_buf();
    drop(catalog);

    let mut connection = Connection::open(&path)?;
    let old = object(91, Vec::new());
    let old_json = serde_json::to_vec(&old)?;
    check_eq!(old_json.first(), Some(&b'{'));
    let key = object_key(old.reference);
    let checksum = row_digest(b"qidx.native.object.v1", &old_json);
    check_eq!(
        connection.execute(
            "INSERT INTO native_object_v1(object,payload,row_digest) VALUES (?1,?2,?3)",
            params![&key, &old_json, &checksum]
        )?,
        1
    );
    let prior_marker = marker(&connection)?;
    // Recreate the historical marker DDL without registration_codec. Leave the
    // current object/intent table layouts intact to isolate the format gate.
    let transaction = connection.transaction()?;
    transaction.execute_batch(
        "DROP TABLE native_capture_format_v1;
         CREATE TABLE native_capture_format_v1 (
             singleton INTEGER PRIMARY KEY NOT NULL CHECK(singleton = 1),
             state_version INTEGER NOT NULL CHECK(state_version > 0),
             capture_version INTEGER NOT NULL CHECK(capture_version > 0),
             hash_domain BLOB NOT NULL,
             root_nonce BLOB NOT NULL CHECK(typeof(root_nonce) = 'blob' AND length(root_nonce) = 32),
             boot_manifest BLOB,
             boot_manifest_digest BLOB,
             row_digest BLOB NOT NULL,
             CHECK((boot_manifest IS NULL AND boot_manifest_digest IS NULL) OR
               (typeof(boot_manifest) = 'blob' AND length(boot_manifest) BETWEEN 1 AND 4096
                AND typeof(boot_manifest_digest) = 'blob' AND length(boot_manifest_digest) = 32))
         ) WITHOUT ROWID;",
    )?;
    check_eq!(
        transaction.execute(
            "INSERT INTO native_capture_format_v1
             (singleton,state_version,capture_version,hash_domain,root_nonce,
              boot_manifest,boot_manifest_digest,row_digest)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
            params![
                prior_marker.0,
                prior_marker.1,
                prior_marker.2,
                &prior_marker.3,
                &prior_marker.4,
                prior_marker.5.as_deref(),
                prior_marker.6.as_deref(),
                &prior_marker.7
            ]
        )?,
        1
    );
    transaction.commit()?;
    let old_ddl: String = connection.query_row(
        "SELECT sql FROM sqlite_master WHERE type='table' AND name='native_capture_format_v1'",
        [],
        |row| row.get(0),
    )?;
    check!(!old_ddl.contains("registration_codec"));
    let before = object_payload(&connection, "native_object_v1", old.reference)?;
    check_eq!(before, (old_json, checksum));
    drop(connection);

    match open(root.path()) {
        Err(CoreError::Typed {
            code: SearchPlaneErrorCodeV2::StateRootFormatUnsupported,
            ..
        }) => {}
        Err(other) => {
            return Err(format!("unexpected old registration-format refusal: {other}").into());
        }
        Ok(_) => return Err("old registration format was opened".into()),
    }
    let connection = Connection::open(path)?;
    let current_ddl: String = connection.query_row(
        "SELECT sql FROM sqlite_master WHERE type='table' AND name='native_capture_format_v1'",
        [],
        |row| row.get(0),
    )?;
    check_eq!(current_ddl, old_ddl);
    check_eq!(marker(&connection)?, prior_marker);
    check_eq!(
        object_payload(&connection, "native_object_v1", old.reference)?,
        before
    );
    check_eq!(
        connection.query_row("PRAGMA integrity_check", [], |row| row.get::<_, String>(0))?,
        "ok"
    );
    Ok(())
}

#[test]
fn managed_reserve_confirm_and_reopen_keep_private_payloads_and_row_domains() -> TestResult {
    let (root, catalog, _ingress) = managed_adoption_fixture()?;
    let profile = object(30, Vec::new());
    let selected = object(93, vec![profile.reference, profile.reference]);
    let expected = expected_payload(&selected)?;
    check_eq!(expected.len(), 44 + 2 * 32);
    let attempt = admit_managed_attempt(&catalog, operation(12191, 191))?;
    let scope = ObjectInstallScope::Attempt(attempt);
    catalog.reserve_managed(
        scope,
        std::slice::from_ref(&selected),
        &mut admission_budget(),
    )?;
    let operation = operation_key(attempt.operation)?;
    let fence = i64::try_from(attempt.fence)?;
    let key = object_key(selected.reference);
    let connection = Connection::open(catalog.path())?;
    let provisional = object_payload(&connection, "native_object_install_v1", selected.reference)?;
    check_eq!(
        provisional,
        (
            expected.clone(),
            row_digest(b"qidx.native.install.v1", &expected)
        )
    );
    let intent: (Vec<u8>, Vec<u8>) = connection.query_row(
        "SELECT payload,row_digest FROM native_object_intent_v1
         WHERE operation_id=(SELECT operation_id FROM native_operation_v1 WHERE operation=?1) AND fence=?2 AND object=?3",
        params![&operation, fence, &key],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    check_eq!(
        intent,
        (
            expected.clone(),
            row_digest(b"qidx.native.intent.v1", &expected)
        )
    );
    check_eq!(registration_codec::decode(&provisional.0)?, selected);
    drop(connection);

    catalog.confirm_managed(
        scope,
        std::slice::from_ref(&selected),
        &mut admission_budget(),
    )?;
    drop(catalog);
    let reopened = open(root.path())?;
    let connection = Connection::open(reopened.path())?;
    let permanent = object_payload(&connection, "native_object_v1", selected.reference)?;
    check_eq!(
        permanent,
        (
            expected.clone(),
            row_digest(b"qidx.native.object.v1", &expected)
        )
    );
    check_eq!(registration_codec::decode(&permanent.0)?, selected);
    check_eq!(
        connection.query_row(
            "SELECT COUNT(*) FROM native_object_install_v1 WHERE object=?1",
            params![&key],
            |row| row.get::<_, i64>(0)
        )?,
        0
    );
    let retained_intent: (Vec<u8>, Vec<u8>) = connection.query_row(
        "SELECT payload,row_digest FROM native_object_intent_v1
         WHERE operation_id=(SELECT operation_id FROM native_operation_v1 WHERE operation=?1) AND fence=?2 AND object=?3",
        params![&operation, fence, &key],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    check_eq!(retained_intent, intent);
    check_eq!(
        connection.query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |row| {
            row.get::<_, i64>(0)
        })?,
        0
    );
    Ok(())
}
