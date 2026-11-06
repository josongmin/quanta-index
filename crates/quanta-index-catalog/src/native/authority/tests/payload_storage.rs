use std::error::Error;

use quanta_index_contract::SearchPlaneErrorCodeV2;
use quanta_index_core::{
    CoreError,
    index::{Digest, ObjectRef, RegisteredObject},
};
use rusqlite::{Connection, params};

use super::{TestResult, admit_new, object, open, operation, setup};
use crate::native::authority::{
    digest, encode, object_install, object_key, operation_key, registration_codec,
};

type TableSnapshot = Vec<(String, String, Vec<(Vec<u8>, Vec<u8>, Vec<u8>)>)>;
type IntentRow = (Vec<u8>, i64, Vec<u8>, Vec<u8>, Vec<u8>);

fn tables_and_rows(connection: &Connection) -> Result<TableSnapshot, Box<dyn Error>> {
    let mut snapshot = Vec::new();
    for table in [
        "native_object_v1",
        "native_object_install_v1",
        "native_object_intent_v1",
    ] {
        let ddl: String = connection.query_row(
            "SELECT sql FROM sqlite_master WHERE type='table' AND name=?1",
            params![table],
            |row| row.get(0),
        )?;
        let mut statement = connection.prepare(&format!(
            "SELECT object,payload,row_digest FROM {table} ORDER BY object"
        ))?;
        let rows = statement
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
            .collect::<Result<Vec<_>, _>>()?;
        snapshot.push((table.to_owned(), ddl, rows));
    }
    Ok(snapshot)
}

#[test]
fn old_object_payload_tables_refuse_public_open_without_logical_rewrite() -> TestResult {
    let root = tempfile::tempdir()?;
    let catalog = open(root.path())?;
    let path = catalog.path().to_path_buf();
    drop(catalog);

    // Recast only object and provisional payload layouts. The current intent
    // layout and its named reverse index remain unchanged.
    let connection = Connection::open(&path)?;
    connection.pragma_update(None, "foreign_keys", "OFF")?;
    connection.execute_batch(
        "BEGIN IMMEDIATE;
         DROP TABLE native_object_install_v1;
         DROP TABLE native_object_v1;
         CREATE TABLE native_object_v1 (
             object BLOB PRIMARY KEY NOT NULL,
             payload BLOB NOT NULL,
             row_digest BLOB NOT NULL
         ) WITHOUT ROWID;
         CREATE TABLE native_object_install_v1 (
             object BLOB PRIMARY KEY NOT NULL,
             payload BLOB NOT NULL,
             row_digest BLOB NOT NULL
         ) WITHOUT ROWID;
         COMMIT;",
    )?;
    for (table, tag, domain) in [
        (
            "native_object_v1",
            71_u8,
            b"qidx.native.object.v1".as_slice(),
        ),
        (
            "native_object_install_v1",
            72_u8,
            b"qidx.native.install.v1".as_slice(),
        ),
    ] {
        let canonical = object(tag, Vec::new());
        let payload = encode(&canonical)?;
        let sql = format!("INSERT INTO {table}(object,payload,row_digest) VALUES (?1,?2,?3)");
        check_eq!(
            connection.execute(
                &sql,
                params![
                    object_key(canonical.reference),
                    &payload,
                    digest(domain, &payload).to_vec()
                ]
            )?,
            1
        );
    }
    let before = tables_and_rows(&connection)?;
    check_eq!(before.len(), 3);
    for (table, ddl, _) in &before {
        check_eq!(
            ddl.contains("WITHOUT ROWID"),
            table != "native_object_intent_v1"
        );
    }
    drop(connection);

    match open(root.path()) {
        Err(CoreError::Typed {
            code: SearchPlaneErrorCodeV2::StateRootFormatUnsupported,
            message,
        }) if message.contains("native schema definition differs") => {}
        Err(other) => return Err(format!("unexpected old-layout refusal: {other}").into()),
        Ok(_) => return Err("old physical layout was opened".into()),
    }
    let connection = Connection::open(path)?;
    check_eq!(tables_and_rows(&connection)?, before);
    Ok(())
}

#[test]
fn old_intent_payload_layout_refuses_public_open_without_logical_rewrite() -> TestResult {
    let root = tempfile::tempdir()?;
    let (catalog, _genesis) = setup(root.path())?;
    let attempt = admit_new(&catalog, operation(1, 41))?;
    let path = catalog.path().to_path_buf();
    drop(catalog);

    let connection = Connection::open(&path)?;
    connection.execute_batch(
        "BEGIN IMMEDIATE;
         DROP TABLE native_object_intent_v1;
         CREATE TABLE native_object_intent_v1 (
             operation BLOB NOT NULL,
             fence INTEGER NOT NULL CHECK(fence > 0),
             object BLOB NOT NULL,
             payload BLOB NOT NULL,
             row_digest BLOB NOT NULL,
             PRIMARY KEY(operation, fence, object),
             FOREIGN KEY(operation) REFERENCES native_operation_v1(operation)
         ) WITHOUT ROWID;
         CREATE INDEX native_object_intent_object_idx_v1
             ON native_object_intent_v1(object,operation,fence);
         COMMIT;",
    )?;
    let canonical = object(73, Vec::new());
    let payload = encode(&canonical)?;
    let values = (
        operation_key(attempt.operation)?,
        i64::try_from(attempt.fence)?,
        object_key(canonical.reference),
        payload.clone(),
        digest(b"qidx.native.intent.v1", &payload).to_vec(),
    );
    check_eq!(
        connection.execute(
            "INSERT INTO native_object_intent_v1(operation,fence,object,payload,row_digest)
             VALUES (?1,?2,?3,?4,?5)",
            params![&values.0, values.1, &values.2, &values.3, &values.4],
        )?,
        1
    );
    let before = tables_and_rows(&connection)?;
    drop(connection);

    match open(root.path()) {
        Err(CoreError::Typed {
            code: SearchPlaneErrorCodeV2::StateRootFormatUnsupported,
            message,
        }) if message.contains("native schema definition differs") => {}
        Err(other) => return Err(format!("unexpected old-intent-layout refusal: {other}").into()),
        Ok(_) => return Err("old intent physical layout was opened".into()),
    }
    let connection = Connection::open(path)?;
    check_eq!(tables_and_rows(&connection)?, before);
    let observed: IntentRow = connection.query_row(
        "SELECT operation,fence,object,payload,row_digest FROM native_object_intent_v1",
        [],
        |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
            ))
        },
    )?;
    check_eq!(observed, values);
    Ok(())
}

fn canonical_large_metadata(
    reference: ObjectRef,
    children: &[ObjectRef],
) -> Result<Vec<u8>, Box<dyn Error>> {
    let object = RegisteredObject::unscoped(reference, 4096, children.to_vec());
    let canonical = object_install::checked_object(&object)?;
    let payload = registration_codec::encode(&canonical)?;
    // Independent fixed-layout oracle: no production encoder or decoder is
    // used to construct these bytes, and child duplicates/order are retained.
    let expected_len = children
        .len()
        .checked_mul(32)
        .and_then(|bytes| bytes.checked_add(44))
        .ok_or("fixed payload gold length overflow")?;
    let mut expected = Vec::with_capacity(expected_len);
    expected.extend_from_slice(&reference.0.0);
    expected.extend_from_slice(&4096_u64.to_be_bytes());
    expected.extend_from_slice(&u32::try_from(children.len())?.to_be_bytes());
    for child in children {
        expected.extend_from_slice(&child.0.0);
    }
    check_eq!(payload, expected);
    Ok(payload)
}

#[test]
fn canonical_medium_payloads_stay_on_table_leaf_pages_after_reopen() -> TestResult {
    let root = tempfile::tempdir()?;
    let (catalog, _genesis) = setup(root.path())?;
    let attempt = admit_new(&catalog, operation(1, 41))?;
    let operation = operation_key(attempt.operation)?;
    let path = catalog.path().to_path_buf();

    let mut children = Vec::new();
    let mut selected = None;
    for tag in 100_u8..=180 {
        children.push(ObjectRef(Digest([tag; 32])));
        let payload = canonical_large_metadata(ObjectRef(Digest([1; 32])), &children)?;
        if (1500..=3000).contains(&payload.len()) {
            selected = Some(children.clone());
            break;
        }
    }
    let children =
        selected.ok_or("canonical metadata did not reach the fixed 1500–3000 byte range")?;
    {
        let mut guard = catalog.lock()?;
        let tx = guard.transaction()?;
        for (registered_tag, provisional_tag) in (1_u8..=24).zip(51_u8..=74) {
            let registered = ObjectRef(Digest([registered_tag; 32]));
            let registered_key = object_key(registered);
            let registered_payload = canonical_large_metadata(registered, &children)?;
            check!((1500..=3000).contains(&registered_payload.len()));
            check_eq!(
                tx.execute(
                    "INSERT INTO native_object_v1(object,payload,row_digest) VALUES (?1,?2,?3)",
                    params![
                        &registered_key,
                        &registered_payload,
                        digest(b"qidx.native.object.v1", &registered_payload).to_vec()
                    ]
                )?,
                1
            );
            let provisional = ObjectRef(Digest([provisional_tag; 32]));
            let provisional_key = object_key(provisional);
            let provisional_payload = canonical_large_metadata(provisional, &children)?;
            check!((1500..=3000).contains(&provisional_payload.len()));
            check_eq!(
                tx.execute(
                    "INSERT INTO native_object_install_v1(object,payload,row_digest) VALUES (?1,?2,?3)",
                    params![
                        &provisional_key,
                        &provisional_payload,
                        digest(b"qidx.native.install.v1", &provisional_payload).to_vec()
                    ]
                )?,
                1
            );
            check_eq!(
                tx.execute(
                    "INSERT INTO native_object_intent_v1(operation_id,fence,object,payload,row_digest)
                     VALUES ((SELECT operation_id FROM native_operation_v1 WHERE operation=?1),?2,?3,?4,?5)",
                    params![
                        &operation,
                        i64::try_from(attempt.fence)?,
                        &provisional_key,
                        &provisional_payload,
                        digest(b"qidx.native.intent.v1", &provisional_payload).to_vec()
                    ]
                )?,
                1
            );
        }
        tx.commit()?;
        drop(guard);
    }
    drop(catalog);

    let connection = Connection::open(path)?;
    let page_size: i64 = connection.pragma_query_value(None, "page_size", |row| row.get(0))?;
    check_eq!(page_size, 4096);
    let fk_violations: i64 =
        connection.query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |row| {
            row.get(0)
        })?;
    check_eq!(fk_violations, 0);
    for (table, domain, tags) in [
        (
            "native_object_v1",
            b"qidx.native.object.v1".as_slice(),
            1_u8..=24,
        ),
        (
            "native_object_install_v1",
            b"qidx.native.install.v1".as_slice(),
            51_u8..=74,
        ),
        (
            "native_object_intent_v1",
            b"qidx.native.intent.v1".as_slice(),
            51_u8..=74,
        ),
    ] {
        let overflow_pages: i64 = connection.query_row(
            "SELECT COUNT(*) FROM dbstat WHERE name=?1 AND pagetype='overflow'",
            params![table],
            |row| row.get(0),
        )?;
        check_eq!(overflow_pages, 0);
        for tag in tags {
            let reference = ObjectRef(Digest([tag; 32]));
            let key = object_key(reference);
            let payload = canonical_large_metadata(reference, &children)?;
            let sql = format!("SELECT payload,row_digest FROM {table} WHERE object=?1");
            let observed: (Vec<u8>, Vec<u8>) =
                connection.query_row(&sql, params![&key], |row| Ok((row.get(0)?, row.get(1)?)))?;
            check_eq!(
                observed,
                (payload.clone(), digest(domain, &payload).to_vec())
            );
        }
    }
    Ok(())
}
