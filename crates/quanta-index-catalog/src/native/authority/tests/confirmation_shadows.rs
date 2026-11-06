use std::error::Error;

use quanta_index_core::index::ports::ManagedObjectInstallPort;
use quanta_index_core::index::{IndexError, ObjectInstallScope, ObjectRef};
use rusqlite::{Connection, params};
use sha2::{Digest as _, Sha256};

use super::{
    TestResult, admission_budget, begin, managed_adoption_fixture, object, object_key, open,
    register_confirmed,
};

type AffectedRows = Vec<(String, Vec<String>)>;

fn affected_rows(
    connection: &Connection,
    first: ObjectRef,
    later: ObjectRef,
) -> Result<AffectedRows, Box<dyn Error>> {
    const TABLES: [(&str, &str, &str); 8] = [
        (
            "native_object_v1",
            "object",
            "hex(object)||':'||hex(payload)||':'||hex(row_digest)",
        ),
        (
            "native_object_edge_v1",
            "parent",
            "hex(parent)||':'||hex(child)||':'||hex(row_digest)",
        ),
        (
            "native_object_install_v1",
            "object",
            "hex(object)||':'||hex(payload)||':'||hex(row_digest)",
        ),
        (
            "native_object_install_edge_v1",
            "parent",
            "hex(parent)||':'||hex(child)||':'||hex(row_digest)",
        ),
        (
            "native_ingress_intent_v1",
            "object",
            "hex(operation)||':'||hex(object)||':'||hex(payload)||':'||hex(row_digest)",
        ),
        (
            "native_ingress_object_v1",
            "object",
            "hex(operation)||':'||hex(object)||':'||hex(row_digest)",
        ),
        (
            "native_gc_object_v1",
            "object",
            "hex(object)||':'||quote(generation)||':'||quote(kind)||':'||quote(phase)||':'||quote(direct_children)||':'||quote(drained_children)||':'||quote(target)||':'||hex(row_digest)",
        ),
        (
            "native_gc_queue_v1",
            "object",
            "hex(object)||':'||hex(row_digest)",
        ),
    ];
    let first_key = object_key(first);
    let later_key = object_key(later);
    let mut result = Vec::with_capacity(TABLES.len());
    for (table, key_column, projection) in TABLES {
        let sql =
            format!("SELECT {projection} FROM {table} WHERE {key_column} IN (?1,?2) ORDER BY 1");
        let mut statement = connection.prepare(&sql)?;
        let rows = statement
            .query_map(params![&first_key, &later_key], |row| {
                row.get::<_, String>(0)
            })?
            .collect::<Result<Vec<_>, _>>()?;
        result.push((table.to_owned(), rows));
    }
    Ok(result)
}

fn object_rows(
    connection: &Connection,
    table: &str,
    reference: ObjectRef,
) -> Result<i64, Box<dyn Error>> {
    let query = format!("SELECT COUNT(*) FROM {table} WHERE object=?1");
    Ok(connection.query_row(&query, params![object_key(reference)], |row| row.get(0))?)
}

fn registered_row_digest(payload: &[u8]) -> Vec<u8> {
    // Independent construction for the SQL-injected permanent row.
    let mut hasher = Sha256::new();
    hasher.update(b"qidx.native.object.v1");
    hasher.update(payload);
    hasher.finalize().to_vec()
}

fn shadow_refused(result: &Result<(), IndexError>) -> bool {
    matches!(
        result,
        Err(IndexError::Storage(_) | IndexError::CorruptObject(_))
    )
}

fn shadow_case(malformed: bool) -> TestResult {
    let (root, catalog, ingress) = managed_adoption_fixture()?;
    let first = object(245, vec![]);
    let later = object(246, vec![]);
    let batch = [first.clone(), later.clone()];
    catalog.reserve_managed(
        ObjectInstallScope::Ingress(ingress),
        &batch,
        &mut admission_budget(),
    )?;
    let before = {
        let guard = catalog.lock()?;
        let provisional: Vec<u8> = guard.query_row(
            "SELECT payload FROM native_object_install_v1 WHERE object=?1",
            params![object_key(later.reference)],
            |row| row.get(0),
        )?;
        let payload = if malformed {
            b"\0".to_vec()
        } else {
            provisional
        };
        let checksum = registered_row_digest(&payload);
        let changed = if malformed {
            guard.execute(
                "INSERT INTO native_object_v1(object,payload,row_digest) VALUES (?1,x'00',?2)",
                params![object_key(later.reference), checksum],
            )?
        } else {
            guard.execute(
                "INSERT INTO native_object_v1(object,payload,row_digest) VALUES (?1,?2,?3)",
                params![object_key(later.reference), payload, checksum],
            )?
        };
        check_eq!(changed, 1);
        check_eq!(object_rows(&guard, "native_object_v1", first.reference)?, 0);
        check_eq!(object_rows(&guard, "native_object_v1", later.reference)?, 1);
        check_eq!(
            object_rows(&guard, "native_object_install_v1", first.reference)?,
            1
        );
        check_eq!(
            object_rows(&guard, "native_object_install_v1", later.reference)?,
            1
        );
        affected_rows(&guard, first.reference, later.reference)?
    };
    {
        let mut guard = catalog.lock()?;
        let tx = begin(&mut guard)?;
        check!(shadow_refused(&register_confirmed(&tx, &batch)));
        // This is before rollback: a late malformed row must not permit
        // an earlier valid registration to be written even transiently.
        check_eq!(object_rows(&tx, "native_object_v1", first.reference)?, 0);
        check_eq!(
            affected_rows(&tx, first.reference, later.reference)?,
            before
        );
        drop(tx);
        drop(guard);
    }
    check!(shadow_refused(&catalog.confirm_managed(
        ObjectInstallScope::Ingress(ingress),
        &batch,
        &mut admission_budget(),
    )));
    let after = {
        let guard = catalog.lock()?;
        affected_rows(&guard, first.reference, later.reference)?
    };
    check_eq!(after, before);
    if !malformed {
        drop(catalog);
        let reopened = open(root.path())?;
        let reopened_before = {
            let guard = reopened.lock()?;
            affected_rows(&guard, first.reference, later.reference)?
        };
        check_eq!(reopened_before, before);
        check!(shadow_refused(&reopened.confirm_managed(
            ObjectInstallScope::Ingress(ingress),
            &batch,
            &mut admission_budget(),
        )));
        let reopened_after = {
            let guard = reopened.lock()?;
            affected_rows(&guard, first.reference, later.reference)?
        };
        check_eq!(reopened_after, before);
    }
    Ok(())
}

#[test]
fn managed_confirmation_rejects_valid_shadow_before_prefix_write() -> TestResult {
    shadow_case(false)
}

#[test]
fn managed_confirmation_rejects_malformed_shadow_before_prefix_write() -> TestResult {
    shadow_case(true)
}
