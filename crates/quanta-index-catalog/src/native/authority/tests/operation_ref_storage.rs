use std::error::Error;

use quanta_index_contract::SearchPlaneErrorCodeV2;
use quanta_index_core::CoreError;
use rusqlite::{Connection, params};
use sha2::{Digest as _, Sha256};

use super::*;

type ParentRow = (i64, Vec<u8>, Vec<u8>, Vec<u8>);

fn parent_row(
    connection: &Connection,
    operation: ImmutableOperation,
) -> Result<ParentRow, Box<dyn Error>> {
    let row: ParentRow = connection.query_row(
        "SELECT operation_id,operation,payload,row_digest
         FROM native_operation_v1 WHERE operation=?1",
        params![serde_json::to_vec(&operation.key)?],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
    )?;
    check!(row.0 > 0);
    check_eq!(
        serde_json::from_slice::<OperationKey>(&row.1)?,
        operation.key
    );
    let decoded: OperationRow = serde_json::from_slice(&row.2)?;
    check_eq!(decoded.immutable, operation);
    let mut hasher = Sha256::new();
    hasher.update(b"qidx.native.operation.v1");
    hasher.update(&row.2);
    check_eq!(row.3, hasher.finalize().to_vec());
    Ok(row)
}

fn child_count(
    connection: &Connection,
    table: &'static str,
    operation_id: i64,
    object: ObjectRef,
) -> Result<i64, Box<dyn Error>> {
    let statement = match table {
        "native_operation_object_v1" => {
            "SELECT COUNT(*) FROM native_operation_object_v1 WHERE operation_id=?1 AND object=?2"
        }
        "native_attempt_object_v1" => {
            "SELECT COUNT(*) FROM native_attempt_object_v1 WHERE operation_id=?1 AND object=?2"
        }
        "native_object_intent_v1" => {
            "SELECT COUNT(*) FROM native_object_intent_v1 WHERE operation_id=?1 AND object=?2"
        }
        _ => return Err("unexpected operation child table".into()),
    };
    Ok(connection.query_row(
        statement,
        params![operation_id, object_key(object)],
        |row| row.get(0),
    )?)
}

#[test]
fn operation_child_refs_bind_distinct_canonical_parents_across_reopen() -> TestResult {
    let root = tempfile::tempdir()?;
    let (catalog, _genesis) = setup(root.path())?;
    let first = operation(62_001, 111);
    let second = operation(62_002, 112);
    let first_attempt = admit_new(&catalog, first)?;
    let second_attempt = admit_new(&catalog, second)?;
    let first_extra = object(113, Vec::new());
    let second_extra = object(114, Vec::new());
    catalog.reserve_objects(first_attempt, std::slice::from_ref(&first_extra))?;
    catalog.register_objects(first_attempt, std::slice::from_ref(&first_extra))?;
    catalog.reserve_objects(second_attempt, std::slice::from_ref(&second_extra))?;

    let path = catalog.path().to_path_buf();
    let connection = Connection::open(&path)?;
    let first_row = parent_row(&connection, first)?;
    let second_row = parent_row(&connection, second)?;
    check_ne!(first_row.0, second_row.0);
    check_eq!(
        child_count(
            &connection,
            "native_operation_object_v1",
            first_row.0,
            first.request
        )?,
        1
    );
    check_eq!(
        child_count(
            &connection,
            "native_operation_object_v1",
            second_row.0,
            second.request
        )?,
        1
    );
    check_eq!(
        child_count(
            &connection,
            "native_operation_object_v1",
            first_row.0,
            second.request
        )?,
        0
    );
    check_eq!(
        child_count(
            &connection,
            "native_operation_object_v1",
            second_row.0,
            first.request
        )?,
        0
    );
    let request_key = object_key(first.request);
    let request_custody_digest: Vec<u8> = connection.query_row(
        "SELECT row_digest FROM native_operation_object_v1
         WHERE operation_id=?1 AND object=?2",
        params![first_row.0, &request_key],
        |row| row.get(0),
    )?;
    let mut custody_hasher = Sha256::new();
    custody_hasher.update(b"qidx.native.opobject.v1");
    custody_hasher.update(serde_json::to_vec(&(&first_row.1, &request_key))?);
    check_eq!(request_custody_digest, custody_hasher.finalize().to_vec());
    check_eq!(
        child_count(
            &connection,
            "native_attempt_object_v1",
            first_row.0,
            first_extra.reference
        )?,
        1
    );
    check_eq!(
        child_count(
            &connection,
            "native_attempt_object_v1",
            second_row.0,
            first_extra.reference
        )?,
        0
    );
    check_eq!(
        child_count(
            &connection,
            "native_object_intent_v1",
            first_row.0,
            first_extra.reference
        )?,
        1
    );
    check_eq!(
        child_count(
            &connection,
            "native_object_intent_v1",
            second_row.0,
            second_extra.reference
        )?,
        1
    );
    check_eq!(
        child_count(
            &connection,
            "native_object_intent_v1",
            first_row.0,
            second_extra.reference
        )?,
        0
    );
    let fk_violations: i64 =
        connection.query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |row| {
            row.get(0)
        })?;
    check_eq!(fk_violations, 0);
    drop(connection);
    drop(catalog);

    let reopened = open(root.path())?;
    check_eq!(reopened.operation(first.key)?, first);
    check_eq!(reopened.operation(second.key)?, second);
    let connection = Connection::open(path)?;
    check_eq!(parent_row(&connection, first)?, first_row);
    check_eq!(parent_row(&connection, second)?, second_row);
    check_eq!(
        child_count(
            &connection,
            "native_attempt_object_v1",
            first_row.0,
            first_extra.reference
        )?,
        1
    );
    check_eq!(
        child_count(
            &connection,
            "native_object_intent_v1",
            second_row.0,
            second_extra.reference
        )?,
        1
    );
    Ok(())
}

#[test]
fn valid_foreign_parent_cannot_take_another_operations_request_custody() -> TestResult {
    let root = tempfile::tempdir()?;
    let (catalog, _genesis) = setup(root.path())?;
    let first = operation(62_003, 115);
    let second = operation(62_004, 116);
    let _first_attempt = admit_new(&catalog, first)?;
    let _second_attempt = admit_new(&catalog, second)?;
    let connection = Connection::open(catalog.path())?;
    connection.pragma_update(None, "foreign_keys", "ON")?;
    let first_id = parent_row(&connection, first)?.0;
    let second_id = parent_row(&connection, second)?.0;
    check_ne!(first_id, second_id);
    check_eq!(
        connection.execute(
            "UPDATE native_operation_object_v1 SET operation_id=?1
             WHERE operation_id=?2 AND object=?3",
            params![second_id, first_id, object_key(first.request)],
        )?,
        1
    );
    let fk_violations: i64 =
        connection.query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |row| {
            row.get(0)
        })?;
    check_eq!(fk_violations, 0);
    drop(connection);

    check!(matches!(
        catalog.operation(first.key),
        Err(IndexError::Storage(_))
    ));
    check_eq!(catalog.operation(second.key)?, second);
    Ok(())
}

#[test]
fn old_operation_parent_layout_refuses_public_open_without_logical_rewrite() -> TestResult {
    let root = tempfile::tempdir()?;
    let catalog = open(root.path())?;
    let path = catalog.path().to_path_buf();
    drop(catalog);

    let connection = Connection::open(&path)?;
    connection.pragma_update(None, "foreign_keys", "OFF")?;
    connection.execute_batch(
        "BEGIN IMMEDIATE;
         DROP TABLE native_operation_v1;
         CREATE TABLE native_operation_v1 (
             operation BLOB PRIMARY KEY NOT NULL,
             workspace BLOB NOT NULL,
             epoch INTEGER NOT NULL CHECK(epoch >= 0),
             payload BLOB NOT NULL,
             row_digest BLOB NOT NULL
         ) WITHOUT ROWID;
         CREATE INDEX native_operation_workspace_idx_v1
             ON native_operation_v1(workspace, epoch);
         COMMIT;",
    )?;
    let before: Vec<(String, String)> = {
        let mut statement = connection.prepare(
            "SELECT name,sql FROM sqlite_master
             WHERE type IN ('table','index') AND sql IS NOT NULL ORDER BY type,name",
        )?;
        statement
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<Result<Vec<_>, _>>()?
    };
    drop(connection);

    match open(root.path()) {
        Err(CoreError::Typed {
            code: SearchPlaneErrorCodeV2::StateRootFormatUnsupported,
            message,
        }) if message.contains("native schema definition differs") => {}
        Err(error) => {
            return Err(format!("unexpected old-operation-layout refusal: {error}").into());
        }
        Ok(_) => return Err("old operation parent layout was opened".into()),
    }
    let connection = Connection::open(path)?;
    let after: Vec<(String, String)> = {
        let mut statement = connection.prepare(
            "SELECT name,sql FROM sqlite_master
             WHERE type IN ('table','index') AND sql IS NOT NULL ORDER BY type,name",
        )?;
        statement
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<Result<Vec<_>, _>>()?
    };
    check_eq!(after, before);
    Ok(())
}
