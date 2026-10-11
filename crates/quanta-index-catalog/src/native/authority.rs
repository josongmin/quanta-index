//! Native incremental catalog authority. These tables share the existing
//! catalog connection, durability settings and initialization transaction.
//!
//! The operation key and the source frontier occupy separate namespaces. A
//! committed view, its work, custody transfer, and original operation receipt
//! are one `SQLite` transaction. This module never scans a corpus on admission
//! or publication.

use std::{
    path::Path,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use quanta_index_core::CoreError;
use quanta_index_core::index::ports::{
    CatalogPublicationPort, CatalogReadPort, InputStagingPort, ObjectCustodyPort,
    OperationJournalPort, WorkActionCursor, WorkClaimPort,
};
use quanta_index_core::index::{
    AdmissionOutcome, AttemptId, AttemptRef, CatalogView, ImmutableOperation, IndexError,
    IngressRef, ObjectRef, OperationKey, OperationStatus, OwnerMemberBinding, PreparedPublication,
    Refusal, RegisteredObject, SourceEvent, TicketId, TicketRecord, ViewRef, ViewSelection,
    WorkAction, WorkActionDigest, WorkActionSet, WorkBudget, WorkClaim, WorkIntent, WorkScope,
    WorkStatus, WorkUsage, WorkspaceKey,
};
use rusqlite::{Connection, OptionalExtension as _, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use crate::connection::{SqliteCatalog, engine_error};

/// Only native tables are added here; no competing `SQLite` file or head.
const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS native_workspace_v1 (
    workspace BLOB PRIMARY KEY NOT NULL,
    epoch INTEGER NOT NULL CHECK(epoch >= 0),
    sequence INTEGER NOT NULL CHECK(sequence >= 0),
    payload BLOB NOT NULL,
    row_digest BLOB NOT NULL
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS native_view_v1 (
    workspace BLOB NOT NULL,
    epoch INTEGER NOT NULL CHECK(epoch >= 0),
    sequence INTEGER NOT NULL CHECK(sequence >= 0),
    identity_digest BLOB NOT NULL,
    payload BLOB NOT NULL,
    row_digest BLOB NOT NULL,
    PRIMARY KEY(workspace, epoch, sequence)
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS native_operation_v1 (
    operation BLOB PRIMARY KEY NOT NULL,
    workspace BLOB NOT NULL,
    epoch INTEGER NOT NULL CHECK(epoch >= 0),
    payload BLOB NOT NULL,
    row_digest BLOB NOT NULL
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS native_operation_object_v1 (
    operation BLOB NOT NULL,
    object BLOB NOT NULL,
    row_digest BLOB NOT NULL,
    PRIMARY KEY(operation, object),
    FOREIGN KEY(operation) REFERENCES native_operation_v1(operation),
    FOREIGN KEY(object) REFERENCES native_object_v1(object)
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS native_ingress_v1 (
    operation BLOB PRIMARY KEY NOT NULL,
    workspace BLOB NOT NULL,
    epoch INTEGER NOT NULL CHECK(epoch >= 0),
    fence INTEGER NOT NULL CHECK(fence > 0),
    payload BLOB NOT NULL,
    row_digest BLOB NOT NULL
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS native_ingress_intent_v1 (
    operation BLOB NOT NULL,
    object BLOB NOT NULL,
    payload BLOB NOT NULL,
    row_digest BLOB NOT NULL,
    PRIMARY KEY(operation, object),
    FOREIGN KEY(operation) REFERENCES native_ingress_v1(operation)
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS native_ingress_object_v1 (
    operation BLOB NOT NULL,
    object BLOB NOT NULL,
    row_digest BLOB NOT NULL,
    PRIMARY KEY(operation, object),
    FOREIGN KEY(operation) REFERENCES native_ingress_v1(operation),
    FOREIGN KEY(object) REFERENCES native_object_v1(object)
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS native_event_claim_v1 (
    identity BLOB PRIMARY KEY NOT NULL,
    operation BLOB NOT NULL,
    payload BLOB NOT NULL,
    row_digest BLOB NOT NULL,
    FOREIGN KEY(operation) REFERENCES native_operation_v1(operation)
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS native_counter_v1 (
    name TEXT PRIMARY KEY NOT NULL,
    next INTEGER NOT NULL CHECK(next > 0),
    row_digest BLOB NOT NULL
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS native_object_v1 (
    object BLOB PRIMARY KEY NOT NULL,
    payload BLOB NOT NULL,
    row_digest BLOB NOT NULL
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS native_object_edge_v1 (
    parent BLOB NOT NULL,
    child BLOB NOT NULL,
    row_digest BLOB NOT NULL,
    PRIMARY KEY(parent, child),
    FOREIGN KEY(parent) REFERENCES native_object_v1(object),
    FOREIGN KEY(child) REFERENCES native_object_v1(object)
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS native_attempt_object_v1 (
    operation BLOB NOT NULL,
    fence INTEGER NOT NULL CHECK(fence > 0),
    object BLOB NOT NULL,
    row_digest BLOB NOT NULL,
    PRIMARY KEY(operation, fence, object),
    FOREIGN KEY(operation) REFERENCES native_operation_v1(operation),
    FOREIGN KEY(object) REFERENCES native_object_v1(object)
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS native_object_intent_v1 (
    operation BLOB NOT NULL,
    fence INTEGER NOT NULL CHECK(fence > 0),
    object BLOB NOT NULL,
    payload BLOB NOT NULL,
    row_digest BLOB NOT NULL,
    PRIMARY KEY(operation, fence, object),
    FOREIGN KEY(operation) REFERENCES native_operation_v1(operation)
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS native_view_root_v1 (
    workspace BLOB NOT NULL,
    epoch INTEGER NOT NULL,
    sequence INTEGER NOT NULL,
    object BLOB NOT NULL,
    row_digest BLOB NOT NULL,
    PRIMARY KEY(workspace, epoch, sequence, object),
    FOREIGN KEY(workspace, epoch, sequence) REFERENCES native_view_v1(workspace, epoch, sequence),
    FOREIGN KEY(object) REFERENCES native_object_v1(object)
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS native_pin_v1 (
    ticket INTEGER PRIMARY KEY AUTOINCREMENT,
    workspace BLOB NOT NULL,
    epoch INTEGER NOT NULL,
    sequence INTEGER NOT NULL,
    row_digest BLOB,
    FOREIGN KEY(workspace, epoch, sequence) REFERENCES native_view_v1(workspace, epoch, sequence)
);
CREATE TABLE IF NOT EXISTS native_work_v1 (
    work BLOB PRIMARY KEY NOT NULL,
    operation BLOB NOT NULL,
    workspace BLOB NOT NULL,
    epoch INTEGER NOT NULL,
    sequence INTEGER NOT NULL,
    status INTEGER NOT NULL CHECK(status IN (0,1,2,3)),
    ready_since_ms INTEGER NOT NULL CHECK(ready_since_ms >= 0),
    lease_until_ms INTEGER NOT NULL CHECK(lease_until_ms >= 0),
    payload BLOB NOT NULL,
    row_digest BLOB NOT NULL,
    FOREIGN KEY(operation) REFERENCES native_operation_v1(operation),
    FOREIGN KEY(workspace, epoch, sequence) REFERENCES native_view_v1(workspace, epoch, sequence)
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS native_work_object_v1 (
    work BLOB NOT NULL,
    object BLOB NOT NULL,
    row_digest BLOB NOT NULL,
    PRIMARY KEY(work, object),
    FOREIGN KEY(work) REFERENCES native_work_v1(work),
    FOREIGN KEY(object) REFERENCES native_object_v1(object)
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS native_work_clock_v1 (
    singleton INTEGER PRIMARY KEY CHECK(singleton = 1),
    last_ms INTEGER NOT NULL CHECK(last_ms >= 0),
    row_digest BLOB NOT NULL
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS native_frontier_v1 (
    workspace BLOB NOT NULL,
    producer BLOB NOT NULL,
    stream BLOB NOT NULL,
    stream_epoch INTEGER NOT NULL CHECK(stream_epoch >= 0),
    sequence INTEGER NOT NULL CHECK(sequence >= 0),
    row_digest BLOB NOT NULL,
    PRIMARY KEY(workspace, producer, stream)
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS native_operation_workspace_idx_v1
    ON native_operation_v1(workspace, epoch);
CREATE INDEX IF NOT EXISTS native_event_claim_operation_idx_v1
    ON native_event_claim_v1(operation);
CREATE INDEX IF NOT EXISTS native_pin_view_idx_v1
    ON native_pin_v1(workspace, epoch, sequence);
CREATE INDEX IF NOT EXISTS native_work_view_idx_v1
    ON native_work_v1(workspace, epoch, status, lease_until_ms, work);
CREATE INDEX IF NOT EXISTS native_work_pending_idx_v1
    ON native_work_v1(workspace, epoch, status, ready_since_ms, work);
";

const READY_PENDING_QUERY: &str = "SELECT work,length(payload),ready_since_ms FROM native_work_v1
    WHERE workspace=?1 AND epoch=?2 AND status=0 ORDER BY ready_since_ms,work LIMIT ?3";
const READY_EXPIRED_QUERY: &str = "SELECT work,length(payload),lease_until_ms FROM native_work_v1
    WHERE workspace=?1 AND epoch=?2 AND status=1 AND lease_until_ms<=?3
    ORDER BY lease_until_ms,work LIMIT ?4";

const SCHEMA_OBJECTS: &[(&str, &str, &str)] = &[
    ("table", "native_workspace_v1", "native workspace"),
    ("table", "native_view_v1", "native view"),
    ("table", "native_operation_v1", "native operation"),
    (
        "table",
        "native_operation_object_v1",
        "native operation object",
    ),
    ("table", "native_ingress_v1", "native ingress"),
    ("table", "native_ingress_intent_v1", "native ingress intent"),
    ("table", "native_ingress_object_v1", "native ingress object"),
    ("table", "native_event_claim_v1", "native event claim"),
    ("table", "native_counter_v1", "native counter"),
    ("table", "native_object_v1", "native object"),
    ("table", "native_object_edge_v1", "native object edge"),
    ("table", "native_attempt_object_v1", "native attempt object"),
    ("table", "native_object_intent_v1", "native object intent"),
    ("table", "native_view_root_v1", "native view root"),
    ("table", "native_pin_v1", "native pin"),
    ("table", "native_work_v1", "native work"),
    ("table", "native_work_object_v1", "native work object"),
    ("table", "native_work_clock_v1", "native work clock"),
    ("table", "native_frontier_v1", "native frontier"),
    (
        "index",
        "native_operation_workspace_idx_v1",
        "native operation workspace index",
    ),
    (
        "index",
        "native_event_claim_operation_idx_v1",
        "native event claim operation index",
    ),
    ("index", "native_pin_view_idx_v1", "native pin view index"),
    ("index", "native_work_view_idx_v1", "native work view index"),
    (
        "index",
        "native_work_pending_idx_v1",
        "native work pending index",
    ),
];

pub(crate) fn initialize(connection: &Connection, path: &Path) -> Result<(), CoreError> {
    let mut installed = 0usize;
    for (kind, name, _) in SCHEMA_OBJECTS {
        let exists: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type=?1 AND name=?2",
                params![kind, name],
                |row| row.get(0),
            )
            .map_err(|error| engine_error("inspect native catalog schema", path, &error))?;
        if exists == 1 {
            installed = installed
                .checked_add(1)
                .ok_or_else(|| CoreError::Storage("native schema count overflow".into()))?;
        }
    }
    if installed != 0 && installed != SCHEMA_OBJECTS.len() {
        return Err(CoreError::Storage(format!(
            "catalog: {} has a partial native catalog schema; no migration reader is available",
            path.display()
        )));
    }
    connection
        .execute_batch(SCHEMA)
        .map_err(|error| engine_error("create native catalog schema", path, &error))?;
    crate::connection::verify_installed_schema_objects(connection, path, SCHEMA, SCHEMA_OBJECTS)?;
    let name = "attempt_fence";
    let inserted = connection
        .execute(
            "INSERT OR IGNORE INTO native_counter_v1(name,next,row_digest) VALUES (?1,1,?2)",
            params![
                name,
                fields_digest(b"qidx.native.counter.v1", &(name, 1_i64))
                    .map_err(|error| CoreError::Storage(error.to_string()))?
            ],
        )
        .map_err(|error| engine_error("seed native counter", path, &error))?;
    if inserted > 1 {
        return Err(CoreError::Storage(
            "native counter seed affected multiple rows".into(),
        ));
    }
    let (next, row_digest): (i64, Vec<u8>) = connection
        .query_row(
            "SELECT next,row_digest FROM native_counter_v1 WHERE name=?1",
            params![name],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(|error| engine_error("read native counter", path, &error))?;
    if row_digest
        != fields_digest(b"qidx.native.counter.v1", &(name, next))
            .map_err(|error| CoreError::Storage(error.to_string()))?
    {
        return Err(CoreError::Storage(
            "native catalog counter digest mismatch".into(),
        ));
    }
    let clock_seed = connection
        .execute(
            "INSERT OR IGNORE INTO native_work_clock_v1(singleton,last_ms,row_digest) VALUES (1,0,?1)",
            params![fields_digest(b"qidx.native.workclock.v1", &(1_i64, 0_i64))
                .map_err(|error| CoreError::Storage(error.to_string()))?],
        )
        .map_err(|error| engine_error("seed native work clock", path, &error))?;
    if clock_seed > 1 {
        return Err(CoreError::Storage(
            "native work clock seed affected multiple rows".into(),
        ));
    }
    let (last_ms, clock_digest): (i64, Vec<u8>) = connection
        .query_row(
            "SELECT last_ms,row_digest FROM native_work_clock_v1 WHERE singleton=1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(|error| engine_error("read native work clock", path, &error))?;
    if clock_digest
        != fields_digest(b"qidx.native.workclock.v1", &(1_i64, last_ms))
            .map_err(|error| CoreError::Storage(error.to_string()))?
    {
        return Err(CoreError::Storage(
            "native work clock digest mismatch".into(),
        ));
    }
    Ok(())
}

fn sql(error: &rusqlite::Error) -> IndexError {
    IndexError::Storage(error.to_string())
}
fn exactly_one(changed: usize, action: &'static str) -> Result<(), IndexError> {
    if changed == 1 {
        Ok(())
    } else {
        Err(IndexError::Storage(format!(
            "native catalog {action} affected {changed} rows"
        )))
    }
}
fn at_most_one(changed: usize, action: &'static str) -> Result<(), IndexError> {
    if changed <= 1 {
        Ok(())
    } else {
        Err(IndexError::Storage(format!(
            "native catalog {action} affected {changed} rows"
        )))
    }
}
fn encode<T: Serialize>(value: &T) -> Result<Vec<u8>, IndexError> {
    serde_json::to_vec(value)
        .map_err(|error| IndexError::Storage(format!("encode native catalog row: {error}")))
}
fn decode<T: for<'a> Deserialize<'a>>(payload: &[u8]) -> Result<T, IndexError> {
    serde_json::from_slice(payload)
        .map_err(|error| IndexError::Storage(format!("decode native catalog row: {error}")))
}
fn digest(domain: &[u8], payload: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(domain);
    hasher.update(payload);
    hasher.finalize().into()
}
fn fields_digest<T: Serialize>(domain: &[u8], fields: &T) -> Result<Vec<u8>, IndexError> {
    Ok(digest(domain, &encode(fields)?).to_vec())
}
fn check_digest(domain: &[u8], payload: &[u8], expected: &[u8]) -> Result<(), IndexError> {
    if expected == &digest(domain, payload)[..] {
        Ok(())
    } else {
        Err(IndexError::Storage(
            "native catalog row digest mismatch".into(),
        ))
    }
}
fn int(value: u64) -> Result<i64, IndexError> {
    i64::try_from(value).map_err(|_conversion_error| {
        IndexError::InvalidInput("native counter exceeds SQLite integer domain".into())
    })
}
fn uint(value: i64) -> Result<u64, IndexError> {
    u64::try_from(value)
        .map_err(|_conversion_error| IndexError::Storage("negative native counter".into()))
}
fn workspace_key(key: WorkspaceKey) -> Result<Vec<u8>, IndexError> {
    encode(&key)
}
fn operation_key(key: OperationKey) -> Result<Vec<u8>, IndexError> {
    encode(&key)
}
fn object_key(reference: ObjectRef) -> Vec<u8> {
    reference.0.0.to_vec()
}

fn roots(view: &CatalogView) -> Vec<ObjectRef> {
    let mut roots = vec![view.profile];
    for root in [
        view.source.owners,
        view.source.paths,
        view.source.frontiers,
        view.projections,
        view.members,
        view.coverage,
    ] {
        if let Some(reference) = root.0 {
            roots.push(reference);
        }
    }
    roots.sort_unstable_by_key(|reference| reference.0.0);
    roots.dedup();
    roots
}

fn distinct_roots(view: &CatalogView) -> Vec<ObjectRef> {
    let mut roots = roots(view);
    roots.sort_unstable();
    roots.dedup();
    roots
}

fn validate_view_identity(view: &CatalogView) -> Result<(), IndexError> {
    let expected = view
        .identity_digest()
        .map_err(|error| IndexError::InvalidInput(format!("encode view identity: {error}")))?;
    if view.identity.digest != expected {
        return Err(IndexError::InvalidInput(
            "view identity digest does not bind its contents".into(),
        ));
    }
    Ok(())
}

fn require_object(connection: &Connection, reference: ObjectRef) -> Result<(), IndexError> {
    let key = object_key(reference);
    let row: Option<(Vec<u8>, Vec<u8>)> = connection
        .query_row(
            "SELECT payload, row_digest FROM native_object_v1 WHERE object = ?1",
            params![key],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(|error| sql(&error))?;
    let Some((payload, row_digest)) = row else {
        return Err(IndexError::CorruptObject(
            "referenced root or child is not registered".into(),
        ));
    };
    check_digest(b"qidx.native.object.v1", &payload, &row_digest)?;
    let registration: RegisteredObject = decode(&payload)?;
    if registration.reference != reference {
        return Err(IndexError::CorruptObject(
            "object key does not match registration".into(),
        ));
    }
    let parent = object_key(reference);
    let mut edges = connection
        .prepare(
            "SELECT child,row_digest FROM native_object_edge_v1 WHERE parent=?1 ORDER BY child",
        )
        .map_err(|error| sql(&error))?;
    let rows = edges
        .query_map(params![&parent], |row| {
            Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, Vec<u8>>(1)?))
        })
        .map_err(|error| sql(&error))?;
    let rows = rows
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| sql(&error))?;
    let mut actual = Vec::with_capacity(rows.len());
    for (child, row_digest) in rows {
        if row_digest != fields_digest(b"qidx.native.edge.v1", &(&parent, &child))? {
            return Err(IndexError::CorruptObject(
                "object edge digest mismatch".into(),
            ));
        }
        actual.push(child);
    }
    let mut expected = registration
        .children
        .iter()
        .map(|child| object_key(*child))
        .collect::<Vec<_>>();
    expected.sort_unstable();
    expected.dedup();
    if actual != expected {
        return Err(IndexError::CorruptObject(
            "object edge ledger differs from registration".into(),
        ));
    }
    Ok(())
}

fn canonical_object(object: &RegisteredObject) -> Result<RegisteredObject, IndexError> {
    if object.children.contains(&object.reference) {
        return Err(IndexError::InvalidInput("self object edge".into()));
    }
    Ok(object.clone())
}

fn register(connection: &Connection, objects: &[RegisteredObject]) -> Result<(), IndexError> {
    for object in objects {
        let canonical = canonical_object(object)?;
        for child in &canonical.children {
            require_object(connection, *child)?;
        }
        let payload = encode(&canonical)?;
        let key = object_key(object.reference);
        let prior: Option<(Vec<u8>, Vec<u8>)> = connection
            .query_row(
                "SELECT payload, row_digest FROM native_object_v1 WHERE object = ?1",
                params![&key],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(|error| sql(&error))?;
        if let Some((prior_payload, prior_digest)) = prior {
            check_digest(b"qidx.native.object.v1", &prior_payload, &prior_digest)?;
            if prior_payload != payload {
                return Err(IndexError::CorruptObject(
                    "object registration changed for one digest".into(),
                ));
            }
        } else {
            let inserted = connection
                .execute(
                    "INSERT INTO native_object_v1(object,payload,row_digest) VALUES (?1,?2,?3)",
                    params![
                        key,
                        payload,
                        digest(b"qidx.native.object.v1", &payload).to_vec()
                    ],
                )
                .map_err(|error| sql(&error))?;
            exactly_one(inserted, "register object")?;
            let mut physical_children = canonical.children.clone();
            physical_children.sort_unstable_by_key(|reference| reference.0.0);
            physical_children.dedup();
            for child in &physical_children {
                let inserted = connection
                    .execute(
                        "INSERT INTO native_object_edge_v1(parent,child,row_digest) VALUES (?1,?2,?3)",
                        params![object_key(object.reference), object_key(*child),
                            fields_digest(b"qidx.native.edge.v1", &(object_key(object.reference), object_key(*child)))?],
                    )
                    .map_err(|error| sql(&error))?;
                exactly_one(inserted, "register object edge")?;
            }
        }
    }
    Ok(())
}

fn reserve(
    connection: &Connection,
    attempt: AttemptRef,
    objects: &[RegisteredObject],
) -> Result<(), IndexError> {
    let mut earlier_in_batch = std::collections::BTreeSet::new();
    for object in objects {
        let canonical = canonical_object(object)?;
        for child in &canonical.children {
            if !earlier_in_batch.contains(child) {
                require_object(connection, *child)?;
            }
        }
        let payload = encode(&canonical)?;
        let operation = operation_key(attempt.operation)?;
        let key = object_key(object.reference);
        let prior: Option<(Vec<u8>, Vec<u8>)> = connection
            .query_row(
                "SELECT payload,row_digest FROM native_object_intent_v1
             WHERE operation=?1 AND fence=?2 AND object=?3",
                params![&operation, int(attempt.fence)?, &key],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(|error| sql(&error))?;
        if let Some((prior_payload, prior_digest)) = prior {
            check_digest(b"qidx.native.intent.v1", &prior_payload, &prior_digest)?;
            if prior_payload != payload {
                return Err(IndexError::CorruptObject(
                    "reserved object metadata changed".into(),
                ));
            }
        } else {
            let inserted = connection
                .execute(
                    "INSERT INTO native_object_intent_v1(operation,fence,object,payload,row_digest)
                 VALUES (?1,?2,?3,?4,?5)",
                    params![
                        operation,
                        int(attempt.fence)?,
                        key,
                        &payload,
                        digest(b"qidx.native.intent.v1", &payload).to_vec()
                    ],
                )
                .map_err(|error| sql(&error))?;
            exactly_one(inserted, "reserve object")?;
        }
        if !earlier_in_batch.insert(object.reference) {
            return Err(IndexError::InvalidInput(
                "duplicate object in reservation batch".into(),
            ));
        }
    }
    Ok(())
}

fn require_intent(
    connection: &Connection,
    attempt: AttemptRef,
    object: &RegisteredObject,
) -> Result<(), IndexError> {
    let canonical = canonical_object(object)?;
    let payload = encode(&canonical)?;
    let prior: Option<(Vec<u8>, Vec<u8>)> = connection
        .query_row(
            "SELECT payload,row_digest FROM native_object_intent_v1
         WHERE operation=?1 AND fence=?2 AND object=?3",
            params![
                operation_key(attempt.operation)?,
                int(attempt.fence)?,
                object_key(object.reference)
            ],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(|error| sql(&error))?;
    let Some((prior_payload, prior_digest)) = prior else {
        return Err(IndexError::BackendViolation(
            "object confirmation lacks durable allocation intent".into(),
        ));
    };
    check_digest(b"qidx.native.intent.v1", &prior_payload, &prior_digest)?;
    if prior_payload != payload {
        return Err(IndexError::BackendViolation(
            "confirmed object differs from allocation intent".into(),
        ));
    }
    Ok(())
}

fn ensure_roots(connection: &Connection, view: &CatalogView) -> Result<(), IndexError> {
    for reference in roots(view) {
        require_object(connection, reference)?;
    }
    Ok(())
}

fn write_view(connection: &Connection, view: &CatalogView) -> Result<(), IndexError> {
    validate_view_identity(view)?;
    let workspace = workspace_key(view.identity.workspace)?;
    let epoch = int(view.identity.epoch)?;
    let sequence = int(view.identity.sequence)?;
    let payload = encode(view)?;
    let row_digest = digest(b"qidx.native.view.v1", &payload);
    let inserted = connection.execute(
        "INSERT INTO native_view_v1(workspace,epoch,sequence,identity_digest,payload,row_digest)
         VALUES (?1,?2,?3,?4,?5,?6)",
        params![&workspace, epoch, sequence, view.identity.digest.0.to_vec(), &payload, row_digest.to_vec()],
    ).map_err(|error| sql(&error))?;
    exactly_one(inserted, "write view")?;
    for root in distinct_roots(view) {
        let inserted = connection.execute(
            "INSERT INTO native_view_root_v1(workspace,epoch,sequence,object,row_digest) VALUES (?1,?2,?3,?4,?5)",
            params![&workspace, epoch, sequence, object_key(root),
                fields_digest(b"qidx.native.viewroot.v1", &(&workspace, epoch, sequence, object_key(root)))?],
        ).map_err(|error| sql(&error))?;
        exactly_one(inserted, "write view root")?;
    }
    Ok(())
}

fn read_view_row(connection: &Connection, identity: ViewRef) -> Result<CatalogView, IndexError> {
    let row: Option<(Vec<u8>, Vec<u8>, Vec<u8>)> = connection
        .query_row(
            "SELECT identity_digest,payload,row_digest FROM native_view_v1
         WHERE workspace=?1 AND epoch=?2 AND sequence=?3",
            params![
                workspace_key(identity.workspace)?,
                int(identity.epoch)?,
                int(identity.sequence)?
            ],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(|error| sql(&error))?;
    let Some((identity_digest, payload, row_digest)) = row else {
        return Err(IndexError::SnapshotUnavailable);
    };
    check_digest(b"qidx.native.view.v1", &payload, &row_digest)?;
    let view: CatalogView = decode(&payload)?;
    if view.identity != identity || identity_digest != identity.digest.0.to_vec() {
        return Err(IndexError::Storage("native view identity mismatch".into()));
    }
    validate_view_identity(&view).map_err(|error| IndexError::Storage(error.to_string()))?;
    let workspace = workspace_key(identity.workspace)?;
    let mut statement = connection
        .prepare(
            "SELECT object,row_digest FROM native_view_root_v1
         WHERE workspace=?1 AND epoch=?2 AND sequence=?3 ORDER BY object",
        )
        .map_err(|error| sql(&error))?;
    let rows = statement
        .query_map(
            params![&workspace, int(identity.epoch)?, int(identity.sequence)?],
            |row| Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, Vec<u8>>(1)?)),
        )
        .map_err(|error| sql(&error))?;
    let rows = rows
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| sql(&error))?;
    let mut actual = Vec::with_capacity(rows.len());
    for (object, row_digest) in rows {
        if row_digest
            != fields_digest(
                b"qidx.native.viewroot.v1",
                &(
                    &workspace,
                    int(identity.epoch)?,
                    int(identity.sequence)?,
                    &object,
                ),
            )?
        {
            return Err(IndexError::Storage(
                "native view root digest mismatch".into(),
            ));
        }
        actual.push(object);
    }
    let expected = distinct_roots(&view)
        .into_iter()
        .map(object_key)
        .collect::<Vec<_>>();
    if actual != expected {
        return Err(IndexError::Storage(
            "native view root ledger mismatch".into(),
        ));
    }
    Ok(view)
}

fn current_view(
    connection: &Connection,
    workspace: WorkspaceKey,
) -> Result<Option<CatalogView>, IndexError> {
    let row: Option<(i64, i64, Vec<u8>, Vec<u8>)> = connection
        .query_row(
            "SELECT epoch,sequence,payload,row_digest FROM native_workspace_v1 WHERE workspace=?1",
            params![workspace_key(workspace)?],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .optional()
        .map_err(|error| sql(&error))?;
    let Some((epoch, sequence, payload, row_digest)) = row else {
        return Ok(None);
    };
    check_digest(b"qidx.native.view.v1", &payload, &row_digest)?;
    let view: CatalogView = decode(&payload)?;
    if view.identity.workspace != workspace
        || view.identity.epoch != uint(epoch)?
        || view.identity.sequence != uint(sequence)?
    {
        return Err(IndexError::Storage(
            "native current view key mismatch".into(),
        ));
    }
    let historical = read_view_row(connection, view.identity)?;
    if historical != view {
        return Err(IndexError::Storage(
            "native current and history differ".into(),
        ));
    }
    Ok(Some(view))
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct OperationRow {
    immutable: ImmutableOperation,
    events: Vec<SourceEvent>,
    fence: u64,
    status: OperationStatus,
}
quanta_index_contract::native_serde_struct!(OperationRow {
    immutable: ImmutableOperation,
    events: Vec<SourceEvent>,
    fence: u64,
    status: OperationStatus,
});

#[derive(Clone, Debug, Eq, PartialEq)]
struct WorkRow {
    intent: WorkIntent,
    created_operation: OperationKey,
    created_view: ViewRef,
    status: WorkStatus,
    ready_since_ms: u64,
    prepared: Option<OwnerMemberBinding>,
}
type WorkSqlRow = (Vec<u8>, Vec<u8>, i64, i64, i64, i64, i64, Vec<u8>, Vec<u8>);
type OperationSqlRow = (Vec<u8>, i64, Vec<u8>, Vec<u8>);
type PinSqlRow = (Vec<u8>, i64, i64, Option<Vec<u8>>);
quanta_index_contract::native_serde_struct!(WorkRow {
    intent: WorkIntent,
    created_operation: OperationKey,
    created_view: ViewRef,
    status: WorkStatus,
    ready_since_ms: u64,
    prepared: Option<OwnerMemberBinding>,
});

fn work_key(intent: &WorkIntent) -> Result<Vec<u8>, IndexError> {
    Ok(intent
        .identity_digest()
        .map_err(|error| IndexError::Storage(format!("encode work identity: {error}")))?
        .0
        .to_vec())
}

fn work_status_columns(status: &WorkStatus) -> Result<(i64, i64), IndexError> {
    match status {
        WorkStatus::Pending => Ok((0, 0)),
        WorkStatus::Claimed(claim) => Ok((1, int(claim.lease_until_ms)?)),
        WorkStatus::Completed(_) => Ok((2, 0)),
        WorkStatus::Superseded => Ok((3, 0)),
        WorkStatus::Unknown => Err(IndexError::InvalidInput(
            "unknown work status cannot be persisted".into(),
        )),
    }
}

fn read_work(connection: &Connection, intent: &WorkIntent) -> Result<Option<WorkRow>, IndexError> {
    let key = work_key(intent)?;
    let row: Option<WorkSqlRow> = connection
        .query_row(
            "SELECT operation,workspace,epoch,sequence,status,ready_since_ms,lease_until_ms,payload,row_digest
             FROM native_work_v1 WHERE work=?1",
            params![&key],
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
        )
        .optional()
        .map_err(|error| sql(&error))?;
    let Some((
        operation,
        workspace,
        epoch,
        sequence,
        status,
        ready_since,
        lease,
        payload,
        row_digest,
    )) = row
    else {
        return Ok(None);
    };
    check_digest(b"qidx.native.work.v2", &payload, &row_digest)?;
    let decoded: WorkRow = decode(&payload)?;
    let (expected_status, expected_lease) = work_status_columns(&decoded.status)?;
    if decoded.intent != *intent
        || work_key(&decoded.intent)? != key
        || operation_key(decoded.created_operation)? != operation
        || workspace_key(decoded.intent.workspace)? != workspace
        || int(decoded.intent.epoch)? != epoch
        || int(decoded.created_view.sequence)? != sequence
        || decoded.created_view.workspace != decoded.intent.workspace
        || decoded.created_view.epoch != decoded.intent.epoch
        || expected_status != status
        || int(decoded.ready_since_ms)? != ready_since
        || expected_lease != lease
    {
        return Err(IndexError::Storage(
            "native work row identity mismatch".into(),
        ));
    }
    match decoded.status {
        WorkStatus::Claimed(claim)
            if claim.intent != *intent || claim.fence == 0 || claim.lease_until_ms == 0 =>
        {
            return Err(IndexError::Storage(
                "native work claim identity mismatch".into(),
            ));
        }
        WorkStatus::Completed(view)
            if view.workspace != intent.workspace
                || view.epoch != intent.epoch
                || view.sequence < decoded.created_view.sequence =>
        {
            return Err(IndexError::Storage(
                "native work completion view mismatch".into(),
            ));
        }
        WorkStatus::Unknown
        | WorkStatus::Pending
        | WorkStatus::Claimed(_)
        | WorkStatus::Completed(_)
        | WorkStatus::Superseded => {}
    }
    for reference in work_roots(&decoded.intent) {
        verify_work_root(connection, &decoded.intent, reference)?;
    }
    if let Some(prepared) = &decoded.prepared {
        verify_work_root(connection, &decoded.intent, prepared.artifact)?;
    }
    Ok(Some(decoded))
}

fn write_work(connection: &Connection, row: &WorkRow, insert: bool) -> Result<(), IndexError> {
    let key = work_key(&row.intent)?;
    let payload = encode(row)?;
    let (status, lease) = work_status_columns(&row.status)?;
    let checksum = digest(b"qidx.native.work.v2", &payload).to_vec();
    if insert {
        let inserted = connection.execute(
            "INSERT INTO native_work_v1(work,operation,workspace,epoch,sequence,status,ready_since_ms,lease_until_ms,payload,row_digest)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
            params![key, operation_key(row.created_operation)?, workspace_key(row.intent.workspace)?,
                int(row.intent.epoch)?, int(row.created_view.sequence)?, status, int(row.ready_since_ms)?, lease, payload, checksum],
        ).map_err(|error| sql(&error))?;
        exactly_one(inserted, "insert work intent")?;
    } else {
        let changed = connection.execute(
            "UPDATE native_work_v1 SET status=?2,ready_since_ms=?3,lease_until_ms=?4,payload=?5,row_digest=?6 WHERE work=?1",
            params![key, status, int(row.ready_since_ms)?, lease, payload, checksum],
        ).map_err(|error| sql(&error))?;
        exactly_one(changed, "update work intent")?;
    }
    Ok(())
}

fn catalog_now_ms(connection: &Connection) -> Result<u64, IndexError> {
    let elapsed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_conversion_error| {
            IndexError::Storage("catalog clock precedes Unix epoch".into())
        })?;
    let current = u64::try_from(elapsed.as_millis())
        .map_err(|_conversion_error| IndexError::ResourceLimited)?;
    let (last, checksum): (i64, Vec<u8>) = connection
        .query_row(
            "SELECT last_ms,row_digest FROM native_work_clock_v1 WHERE singleton=1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(|error| sql(&error))?;
    if checksum != fields_digest(b"qidx.native.workclock.v1", &(1_i64, last))? {
        return Err(IndexError::Storage(
            "native work clock digest mismatch".into(),
        ));
    }
    if current < uint(last)? {
        return Err(IndexError::Storage("catalog clock regressed".into()));
    }
    let next = int(current)?;
    let changed = connection
        .execute(
            "UPDATE native_work_clock_v1 SET last_ms=?1,row_digest=?2 WHERE singleton=1",
            params![
                next,
                fields_digest(b"qidx.native.workclock.v1", &(1_i64, next))?
            ],
        )
        .map_err(|error| sql(&error))?;
    exactly_one(changed, "advance work clock")?;
    Ok(current)
}

/// Each status lane uses its own covering order index. Reading at most `limit`
/// from each lane suffices for the oldest `limit` across their union.
fn ready_candidates(
    connection: &Connection,
    workspace: WorkspaceKey,
    epoch: u64,
    now: u64,
    limit: u32,
    claimed: bool,
    budget: &mut WorkBudget,
) -> Result<Vec<(u64, Vec<u8>)>, IndexError> {
    let query = if claimed {
        READY_EXPIRED_QUERY
    } else {
        READY_PENDING_QUERY
    };
    let mut statement = connection.prepare(query).map_err(|error| sql(&error))?;
    let mut rows = if claimed {
        statement
            .query(params![
                workspace_key(workspace)?,
                int(epoch)?,
                int(now)?,
                i64::from(limit)
            ])
            .map_err(|error| sql(&error))?
    } else {
        statement
            .query(params![
                workspace_key(workspace)?,
                int(epoch)?,
                i64::from(limit)
            ])
            .map_err(|error| sql(&error))?
    };
    let mut result = Vec::new();
    while let Some(row) = rows.next().map_err(|error| sql(&error))? {
        let key: Vec<u8> = row.get(0).map_err(|error| sql(&error))?;
        let payload_bytes: i64 = row.get(1).map_err(|error| sql(&error))?;
        let due: i64 = row.get(2).map_err(|error| sql(&error))?;
        budget.charge(WorkUsage {
            reads: 1,
            bytes: uint(payload_bytes)?
                .checked_add(64)
                .ok_or(IndexError::ResourceLimited)?,
            ..WorkUsage::default()
        })?;
        result.push((uint(due)?, key));
    }
    Ok(result)
}

fn validate_work_scope(connection: &Connection, intent: &WorkIntent) -> Result<(), IndexError> {
    for reference in work_roots(intent) {
        require_object(connection, reference)?;
    }
    Ok(())
}

fn work_roots(intent: &WorkIntent) -> Vec<ObjectRef> {
    match intent.scope {
        WorkScope::Owner { expected, .. } => [expected.source, expected.projection]
            .into_iter()
            .flatten()
            .collect(),
        WorkScope::ProjectionDomain { source, .. } => {
            [source.owners.0, source.paths.0, source.frontiers.0]
                .into_iter()
                .flatten()
                .collect()
        }
    }
}

fn protect_work_root(
    connection: &Connection,
    intent: &WorkIntent,
    reference: ObjectRef,
) -> Result<(), IndexError> {
    let key = work_key(intent)?;
    let object = object_key(reference);
    let inserted = connection
        .execute(
            "INSERT OR IGNORE INTO native_work_object_v1(work,object,row_digest) VALUES (?1,?2,?3)",
            params![
                &key,
                &object,
                fields_digest(b"qidx.native.workobject.v1", &(&key, &object))?
            ],
        )
        .map_err(|error| sql(&error))?;
    at_most_one(inserted, "protect work root")?;
    if inserted == 0 {
        let stored: Vec<u8> = connection
            .query_row(
                "SELECT row_digest FROM native_work_object_v1 WHERE work=?1 AND object=?2",
                params![&key, &object],
                |row| row.get(0),
            )
            .map_err(|error| sql(&error))?;
        if stored != fields_digest(b"qidx.native.workobject.v1", &(&key, &object))? {
            return Err(IndexError::Storage(
                "work root custody digest mismatch".into(),
            ));
        }
    }
    Ok(())
}

fn verify_work_root(
    connection: &Connection,
    intent: &WorkIntent,
    reference: ObjectRef,
) -> Result<(), IndexError> {
    let key = work_key(intent)?;
    let object = object_key(reference);
    let stored: Vec<u8> = connection
        .query_row(
            "SELECT row_digest FROM native_work_object_v1 WHERE work=?1 AND object=?2",
            params![&key, &object],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| sql(&error))?
        .ok_or_else(|| IndexError::Storage("work root custody missing".into()))?;
    if stored != fields_digest(b"qidx.native.workobject.v1", &(&key, &object))? {
        return Err(IndexError::Storage(
            "work root custody digest mismatch".into(),
        ));
    }
    Ok(())
}

fn live_claim(connection: &Connection, claim: &WorkClaim, now: u64) -> Result<WorkRow, IndexError> {
    let row = read_work(connection, &claim.intent)?.ok_or(IndexError::WorkFenceLost)?;
    let WorkStatus::Claimed(persisted) = row.status else {
        return Err(IndexError::WorkFenceLost);
    };
    if persisted.intent != claim.intent
        || persisted.fence != claim.fence
        || now >= persisted.lease_until_ms
    {
        return Err(IndexError::WorkFenceLost);
    }
    Ok(row)
}

fn validated_lease_ms(lease: Duration) -> Result<u64, IndexError> {
    let lease_ms = u64::try_from(lease.as_millis())
        .map_err(|_conversion_error| IndexError::ResourceLimited)?;
    if lease_ms == 0 || lease_ms > 300_000 {
        return Err(IndexError::InvalidInput(
            "work lease must be 1-300000 ms".into(),
        ));
    }
    Ok(lease_ms)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum EventClaimState {
    Pending,
    Refused,
    Committed,
}
quanta_index_contract::native_serde_enum!(EventClaimState {
    units { Pending = 0, Refused = 1, Committed = 2 }
    newtypes {}
    records {}
});

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct EventClaim {
    operation: OperationKey,
    event: SourceEvent,
    state: EventClaimState,
}
quanta_index_contract::native_serde_struct!(EventClaim {
    operation: OperationKey,
    event: SourceEvent,
    state: EventClaimState,
});

fn event_identity(key: OperationKey, event: SourceEvent) -> Result<Vec<u8>, IndexError> {
    encode(&(
        key.workspace,
        key.epoch,
        event.producer,
        event.stream,
        event.stream_epoch,
        event.sequence,
    ))
}

fn read_event_claim(
    connection: &Connection,
    key: OperationKey,
    event: SourceEvent,
) -> Result<Option<EventClaim>, IndexError> {
    let identity = event_identity(key, event)?;
    let row: Option<(Vec<u8>, Vec<u8>, Vec<u8>)> = connection
        .query_row(
            "SELECT operation,payload,row_digest FROM native_event_claim_v1 WHERE identity=?1",
            params![&identity],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(|error| sql(&error))?;
    let Some((owner, payload, row_digest)) = row else {
        return Ok(None);
    };
    check_digest(b"qidx.native.eventclaim.v1", &payload, &row_digest)?;
    let claim: EventClaim = decode(&payload)?;
    if owner != operation_key(claim.operation)?
        || event_identity(claim.operation, claim.event)? != identity
    {
        return Err(IndexError::Storage(
            "native event claim identity mismatch".into(),
        ));
    }
    Ok(Some(claim))
}

fn write_event_claim(
    connection: &Connection,
    claim: EventClaim,
    insert: bool,
) -> Result<(), IndexError> {
    let identity = event_identity(claim.operation, claim.event)?;
    let payload = encode(&claim)?;
    let row_digest = digest(b"qidx.native.eventclaim.v1", &payload);
    if insert {
        let inserted = connection.execute(
            "INSERT INTO native_event_claim_v1(identity,operation,payload,row_digest) VALUES (?1,?2,?3,?4)",
            params![identity, operation_key(claim.operation)?, payload, row_digest.to_vec()],
        ).map_err(|error| sql(&error))?;
        exactly_one(inserted, "write event claim")?;
    } else {
        let changed = connection.execute(
            "UPDATE native_event_claim_v1 SET operation=?2,payload=?3,row_digest=?4 WHERE identity=?1",
            params![identity, operation_key(claim.operation)?, payload, row_digest.to_vec()],
        ).map_err(|error| sql(&error))?;
        if changed != 1 {
            return Err(IndexError::Storage("native event claim disappeared".into()));
        }
    }
    Ok(())
}

fn claim_events(
    connection: &Connection,
    key: OperationKey,
    events: &[SourceEvent],
    budget: &mut WorkBudget,
) -> Result<(), IndexError> {
    let mut unique = std::collections::BTreeSet::new();
    for event in events {
        budget.checkpoint()?;
        let identity = event_identity(key, *event)?;
        let payload = encode(&EventClaim {
            operation: key,
            event: *event,
            state: EventClaimState::Pending,
        })?;
        let row_bytes = identity
            .len()
            .checked_add(payload.len())
            .and_then(|size| size.checked_add(128))
            .ok_or(IndexError::ResourceLimited)?;
        budget.charge(WorkUsage {
            reads: 1,
            writes: 1,
            bytes: u64::try_from(row_bytes).map_err(|_overflow| IndexError::ResourceLimited)?,
            ..WorkUsage::default()
        })?;
        if !unique.insert(identity) {
            return Err(IndexError::InvalidInput(
                "duplicate source event identity".into(),
            ));
        }
        match read_event_claim(connection, key, *event)? {
            None => write_event_claim(
                connection,
                EventClaim {
                    operation: key,
                    event: *event,
                    state: EventClaimState::Pending,
                },
                true,
            )?,
            Some(mut prior) => {
                if prior.event.predecessor != event.predecessor
                    || prior.event.payload != event.payload
                {
                    return Err(IndexError::DigestConflict);
                }
                match prior.state {
                    EventClaimState::Pending if prior.operation != key => {
                        return Err(IndexError::Conflict);
                    }
                    EventClaimState::Committed => return Err(IndexError::Conflict),
                    EventClaimState::Pending | EventClaimState::Refused => {
                        prior.operation = key;
                        prior.state = EventClaimState::Pending;
                        write_event_claim(connection, prior, false)?;
                    }
                }
            }
        }
    }
    Ok(())
}

fn operation_claims(
    connection: &Connection,
    key: OperationKey,
) -> Result<Vec<EventClaim>, IndexError> {
    let mut statement = connection
        .prepare("SELECT identity,payload,row_digest FROM native_event_claim_v1 WHERE operation=?1")
        .map_err(|error| sql(&error))?;
    let rows = statement
        .query_map(params![operation_key(key)?], |row| {
            Ok((
                row.get::<_, Vec<u8>>(0)?,
                row.get::<_, Vec<u8>>(1)?,
                row.get::<_, Vec<u8>>(2)?,
            ))
        })
        .map_err(|error| sql(&error))?;
    let rows = rows
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| sql(&error))?;
    let mut claims = Vec::with_capacity(rows.len());
    for (identity, payload, row_digest) in rows {
        check_digest(b"qidx.native.eventclaim.v1", &payload, &row_digest)?;
        let claim: EventClaim = decode(&payload)?;
        if claim.operation != key || event_identity(key, claim.event)? != identity {
            return Err(IndexError::Storage(
                "native event claim owner mismatch".into(),
            ));
        }
        claims.push(claim);
    }
    Ok(claims)
}

fn transition_claims(
    connection: &Connection,
    key: OperationKey,
    expected: Option<&[SourceEvent]>,
    state: EventClaimState,
) -> Result<(), IndexError> {
    let claims = operation_claims(connection, key)?;
    if let Some(expected) = expected {
        let mut wanted = std::collections::BTreeSet::new();
        for event in expected {
            if !wanted.insert((
                event_identity(key, *event)?,
                event.predecessor,
                event.payload.0,
            )) {
                return Err(IndexError::InvalidInput(
                    "duplicate expected event identity".into(),
                ));
            }
        }
        let actual = claims
            .iter()
            .map(|claim| {
                Ok((
                    event_identity(key, claim.event)?,
                    claim.event.predecessor,
                    claim.event.payload.0,
                ))
            })
            .collect::<Result<std::collections::BTreeSet<_>, IndexError>>()?;
        if actual != wanted {
            return Err(IndexError::InvalidInput(
                "publication source events differ from admitted claims".into(),
            ));
        }
    }
    for mut claim in claims {
        if claim.state != EventClaimState::Pending {
            return Err(IndexError::Conflict);
        }
        claim.state = state;
        write_event_claim(connection, claim, false)?;
    }
    Ok(())
}

fn read_operation(
    connection: &Connection,
    key: OperationKey,
) -> Result<Option<OperationRow>, IndexError> {
    let row: Option<OperationSqlRow> = connection
        .query_row(
            "SELECT workspace,epoch,payload,row_digest FROM native_operation_v1 WHERE operation=?1",
            params![operation_key(key)?],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .optional()
        .map_err(|error| sql(&error))?;
    let Some((workspace, epoch, payload, row_digest)) = row else {
        return Ok(None);
    };
    check_digest(b"qidx.native.operation.v1", &payload, &row_digest)?;
    let operation: OperationRow = decode(&payload)?;
    if operation.immutable.key != key
        || workspace != workspace_key(key.workspace)?
        || epoch != int(key.epoch)?
    {
        return Err(IndexError::Storage("native operation key mismatch".into()));
    }
    if matches!(operation.status, OperationStatus::Pending(_)) {
        require_object(connection, operation.immutable.request)?;
        let operation_id = operation_key(key)?;
        let request_id = object_key(operation.immutable.request);
        let custody: Option<Vec<u8>> = connection
            .query_row(
                "SELECT row_digest FROM native_operation_object_v1 WHERE operation=?1 AND object=?2",
                params![&operation_id, &request_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| sql(&error))?;
        let Some(custody) = custody else {
            return Err(IndexError::Storage("native request custody missing".into()));
        };
        if custody != fields_digest(b"qidx.native.opobject.v1", &(&operation_id, &request_id))? {
            return Err(IndexError::Storage(
                "native request custody digest mismatch".into(),
            ));
        }
    }
    Ok(Some(operation))
}

fn write_operation(
    connection: &Connection,
    row: &OperationRow,
    insert: bool,
) -> Result<(), IndexError> {
    let payload = encode(row)?;
    let row_digest = digest(b"qidx.native.operation.v1", &payload);
    if insert {
        let inserted = connection
            .execute(
                "INSERT INTO native_operation_v1(operation,workspace,epoch,payload,row_digest)
             VALUES (?1,?2,?3,?4,?5)",
                params![
                    operation_key(row.immutable.key)?,
                    workspace_key(row.immutable.key.workspace)?,
                    int(row.immutable.key.epoch)?,
                    payload,
                    row_digest.to_vec()
                ],
            )
            .map_err(|error| sql(&error))?;
        exactly_one(inserted, "write operation")?;
    } else {
        let changed = connection
            .execute(
                "UPDATE native_operation_v1 SET payload=?2,row_digest=?3 WHERE operation=?1",
                params![
                    operation_key(row.immutable.key)?,
                    payload,
                    row_digest.to_vec()
                ],
            )
            .map_err(|error| sql(&error))?;
        if changed != 1 {
            return Err(IndexError::Storage("native operation disappeared".into()));
        }
    }
    Ok(())
}

fn next_fence(connection: &Connection) -> Result<u64, IndexError> {
    let (next, row_digest): (i64, Vec<u8>) = connection
        .query_row(
            "SELECT next,row_digest FROM native_counter_v1 WHERE name='attempt_fence'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(|error| sql(&error))?;
    if row_digest != fields_digest(b"qidx.native.counter.v1", &("attempt_fence", next))? {
        return Err(IndexError::Storage("native counter digest mismatch".into()));
    }
    let successor = next.checked_add(1).ok_or(IndexError::ResourceLimited)?;
    let changed = connection
        .execute(
            "UPDATE native_counter_v1 SET next=?1,row_digest=?2 WHERE name='attempt_fence'",
            params![
                successor,
                fields_digest(b"qidx.native.counter.v1", &("attempt_fence", successor))?
            ],
        )
        .map_err(|error| sql(&error))?;
    exactly_one(changed, "advance fence counter")?;
    uint(next)
}

fn attempt_ref(key: OperationKey, fence: u64) -> Result<AttemptRef, IndexError> {
    let mut seed = operation_key(key)?;
    seed.extend_from_slice(&fence.to_le_bytes());
    let hash = digest(b"qidx.native.attempt.v1", &seed);
    let mut id = [0u8; 16];
    id.copy_from_slice(&hash[..16]);
    Ok(AttemptRef {
        operation: key,
        attempt: AttemptId(id),
        fence,
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct IngressRow {
    stage: IngressRef,
    sealed: Option<ObjectRef>,
}
quanta_index_contract::native_serde_struct!(IngressRow {
    stage: IngressRef,
    sealed: Option<ObjectRef>,
});

struct IngressSqlRow {
    workspace: Vec<u8>,
    epoch: i64,
    fence: i64,
    payload: Vec<u8>,
    row_digest: Vec<u8>,
}

fn read_ingress(
    connection: &Connection,
    key: OperationKey,
) -> Result<Option<IngressRow>, IndexError> {
    let row: Option<IngressSqlRow> = connection
        .query_row(
            "SELECT workspace,epoch,fence,payload,row_digest FROM native_ingress_v1 WHERE operation=?1",
            params![operation_key(key)?],
            |row| {
                Ok(IngressSqlRow {
                    workspace: row.get(0)?,
                    epoch: row.get(1)?,
                    fence: row.get(2)?,
                    payload: row.get(3)?,
                    row_digest: row.get(4)?,
                })
            },
        )
        .optional()
        .map_err(|error| sql(&error))?;
    let Some(IngressSqlRow {
        workspace,
        epoch,
        fence,
        payload,
        row_digest,
    }) = row
    else {
        return Ok(None);
    };
    check_digest(b"qidx.native.ingress.v1", &payload, &row_digest)?;
    let ingress: IngressRow = decode(&payload)?;
    if ingress.stage.operation != key
        || workspace != workspace_key(key.workspace)?
        || epoch != int(key.epoch)?
        || fence != int(ingress.stage.fence)?
    {
        return Err(IndexError::Storage(
            "native ingress identity mismatch".into(),
        ));
    }
    Ok(Some(ingress))
}

fn write_ingress(connection: &Connection, row: IngressRow, insert: bool) -> Result<(), IndexError> {
    let payload = encode(&row)?;
    let checksum = digest(b"qidx.native.ingress.v1", &payload).to_vec();
    let changed = if insert {
        connection.execute(
            "INSERT INTO native_ingress_v1(operation,workspace,epoch,fence,payload,row_digest) VALUES (?1,?2,?3,?4,?5,?6)",
            params![operation_key(row.stage.operation)?,workspace_key(row.stage.operation.workspace)?,int(row.stage.operation.epoch)?,int(row.stage.fence)?,payload,checksum],
        )
    } else {
        connection.execute(
            "UPDATE native_ingress_v1 SET fence=?2,payload=?3,row_digest=?4 WHERE operation=?1",
            params![operation_key(row.stage.operation)?,int(row.stage.fence)?,payload,checksum],
        )
    }
    .map_err(|error| sql(&error))?;
    exactly_one(changed, "write native ingress")
}

fn check_ingress(connection: &Connection, stage: IngressRef) -> Result<IngressRow, IndexError> {
    let row = read_ingress(connection, stage.operation)?.ok_or(IndexError::FenceLost)?;
    if row.stage != stage {
        return Err(IndexError::FenceLost);
    }
    Ok(row)
}

fn require_open_ingress(connection: &Connection, stage: IngressRef) -> Result<(), IndexError> {
    if check_ingress(connection, stage)?.sealed.is_some() {
        return Err(IndexError::Conflict);
    }
    Ok(())
}

fn ingress_intent(
    connection: &Connection,
    key: OperationKey,
    object: &RegisteredObject,
) -> Result<Option<Vec<u8>>, IndexError> {
    let operation = operation_key(key)?;
    let object_key = object_key(object.reference);
    let row: Option<(Vec<u8>, Vec<u8>)> = connection
        .query_row(
            "SELECT payload,row_digest FROM native_ingress_intent_v1 WHERE operation=?1 AND object=?2",
            params![&operation, &object_key],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(|error| sql(&error))?;
    row.map(|(payload, checksum)| {
        check_digest(b"qidx.native.ingressintent.v1", &payload, &checksum)?;
        let prior: RegisteredObject = decode(&payload)?;
        if prior.reference != object.reference {
            return Err(IndexError::Storage(
                "native ingress intent key mismatch".into(),
            ));
        }
        Ok(payload)
    })
    .transpose()
}

fn confirmed_ingress_object(
    connection: &Connection,
    key: OperationKey,
    reference: ObjectRef,
) -> Result<bool, IndexError> {
    let operation = operation_key(key)?;
    let object = object_key(reference);
    let checksum: Option<Vec<u8>> = connection
        .query_row(
            "SELECT row_digest FROM native_ingress_object_v1 WHERE operation=?1 AND object=?2",
            params![&operation, &object],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| sql(&error))?;
    checksum.map_or(Ok(false), |stored| {
        if stored != fields_digest(b"qidx.native.ingressobject.v1", &(&operation, &object))? {
            return Err(IndexError::Storage(
                "native ingress object digest mismatch".into(),
            ));
        }
        require_object(connection, reference)?;
        Ok(true)
    })
}

fn delete_ingress_rows(
    connection: &Connection,
    operation: &[u8],
    select_sql: &'static str,
    delete_sql: &'static str,
    budget: &mut WorkBudget,
) -> Result<(), IndexError> {
    let mut after = Vec::new();
    loop {
        budget.checkpoint()?;
        let mut statement = connection
            .prepare(select_sql)
            .map_err(|error| sql(&error))?;
        let batch = statement
            .query_map(params![operation, &after], |row| row.get::<_, Vec<u8>>(0))
            .map_err(|error| sql(&error))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| sql(&error))?;
        if batch.is_empty() {
            return Ok(());
        }
        let rows = u64::try_from(batch.len()).map_err(|_overflow| IndexError::ResourceLimited)?;
        let bytes = batch.iter().try_fold(0_u64, |total, object| {
            let row_bytes = object
                .len()
                .checked_add(64)
                .ok_or(IndexError::ResourceLimited)?;
            total
                .checked_add(
                    u64::try_from(row_bytes).map_err(|_overflow| IndexError::ResourceLimited)?,
                )
                .ok_or(IndexError::ResourceLimited)
        })?;
        budget.charge(WorkUsage {
            reads: rows,
            writes: rows,
            bytes,
            ..WorkUsage::default()
        })?;
        let last = batch
            .last()
            .ok_or_else(|| IndexError::Storage("ingress cleanup lost last row".into()))?;
        budget.checkpoint()?;
        let removed = connection
            .execute(delete_sql, params![operation, &after, last])
            .map_err(|error| sql(&error))?;
        if removed != batch.len() {
            return Err(IndexError::Storage(
                "ingress custody changed during cleanup".into(),
            ));
        }
        after.clone_from(last);
        if batch.len() < 256 {
            return Ok(());
        }
    }
}

fn delete_ingress(
    connection: &Connection,
    key: OperationKey,
    budget: &mut WorkBudget,
) -> Result<(), IndexError> {
    let operation = operation_key(key)?;
    delete_ingress_rows(
        connection,
        &operation,
        "SELECT object FROM native_ingress_intent_v1 WHERE operation=?1 AND object>?2 ORDER BY object LIMIT 256",
        "DELETE FROM native_ingress_intent_v1 WHERE operation=?1 AND object>?2 AND object<=?3",
        budget,
    )?;
    delete_ingress_rows(
        connection,
        &operation,
        "SELECT object FROM native_ingress_object_v1 WHERE operation=?1 AND object>?2 ORDER BY object LIMIT 256",
        "DELETE FROM native_ingress_object_v1 WHERE operation=?1 AND object>?2 AND object<=?3",
        budget,
    )?;
    budget.charge(WorkUsage {
        reads: 1,
        writes: 1,
        bytes: 256,
        ..WorkUsage::default()
    })?;
    let removed = connection
        .execute(
            "DELETE FROM native_ingress_v1 WHERE operation=?1",
            params![&operation],
        )
        .map_err(|error| sql(&error))?;
    exactly_one(removed, "delete native ingress")
}

fn transfer_ingress_objects(
    connection: &Connection,
    key: OperationKey,
    budget: &mut WorkBudget,
) -> Result<(), IndexError> {
    let operation = operation_key(key)?;
    let mut after = Vec::new();
    loop {
        budget.checkpoint()?;
        let mut statement = connection
            .prepare("SELECT object,row_digest FROM native_ingress_object_v1 WHERE operation=?1 AND object>?2 ORDER BY object LIMIT 256")
            .map_err(|error| sql(&error))?;
        let batch = statement
            .query_map(params![&operation, &after], |row| {
                Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, Vec<u8>>(1)?))
            })
            .map_err(|error| sql(&error))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| sql(&error))?;
        if batch.is_empty() {
            break;
        }
        let rows = u64::try_from(batch.len()).map_err(|_overflow| IndexError::ResourceLimited)?;
        let bytes = batch.iter().try_fold(0_u64, |total, (object, checksum)| {
            let row_bytes = object
                .len()
                .checked_add(checksum.len())
                .and_then(|size| size.checked_add(64))
                .ok_or(IndexError::ResourceLimited)?;
            total
                .checked_add(
                    u64::try_from(row_bytes).map_err(|_overflow| IndexError::ResourceLimited)?,
                )
                .ok_or(IndexError::ResourceLimited)
        })?;
        budget.charge(WorkUsage {
            reads: rows,
            writes: rows.checked_mul(2).ok_or(IndexError::ResourceLimited)?,
            bytes,
            ..WorkUsage::default()
        })?;
        for (object, checksum) in &batch {
            budget.checkpoint()?;
            if *checksum != fields_digest(b"qidx.native.ingressobject.v1", &(&operation, object))? {
                return Err(IndexError::Storage(
                    "ingress object custody digest mismatch".into(),
                ));
            }
            let changed = connection.execute(
                "INSERT INTO native_operation_object_v1(operation,object,row_digest) VALUES (?1,?2,?3)",
                params![&operation,object,fields_digest(b"qidx.native.opobject.v1", &(&operation,object))?],
            ).map_err(|error| sql(&error))?;
            exactly_one(changed, "transfer ingress object custody")?;
        }
        let last = batch
            .last()
            .map(|(object, _)| object.clone())
            .ok_or_else(|| IndexError::Storage("ingress batch lost last object".into()))?;
        budget.checkpoint()?;
        let removed = connection
            .execute(
                "DELETE FROM native_ingress_object_v1 WHERE operation=?1 AND object>?2 AND object<=?3",
                params![&operation, &after, &last],
            )
            .map_err(|error| sql(&error))?;
        if removed != batch.len() {
            return Err(IndexError::Storage(
                "ingress custody changed during transfer".into(),
            ));
        }
        after = last;
        if batch.len() < 256 {
            break;
        }
    }
    Ok(())
}

fn check_attempt(connection: &Connection, attempt: AttemptRef) -> Result<OperationRow, IndexError> {
    let row = read_operation(connection, attempt.operation)?.ok_or(IndexError::FenceLost)?;
    if row.fence != attempt.fence
        || attempt_ref(attempt.operation, attempt.fence)? != attempt
        || row.status != OperationStatus::Pending(attempt)
    {
        return Err(IndexError::FenceLost);
    }
    Ok(row)
}

fn require_attempt_object(
    connection: &Connection,
    attempt: AttemptRef,
    reference: ObjectRef,
) -> Result<(), IndexError> {
    let operation = operation_key(attempt.operation)?;
    let fence = int(attempt.fence)?;
    let object = object_key(reference);
    let checksum: Option<Vec<u8>> = connection
        .query_row(
            "SELECT row_digest FROM native_attempt_object_v1 WHERE operation=?1 AND fence=?2 AND object=?3",
            params![&operation, fence, &object],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| sql(&error))?;
    let Some(checksum) = checksum else {
        return Err(IndexError::BackendViolation(
            "work action manifest lacks attempt custody".into(),
        ));
    };
    if checksum
        != fields_digest(
            b"qidx.native.attemptobject.v1",
            &(&operation, fence, &object),
        )?
    {
        return Err(IndexError::Storage(
            "attempt object custody digest mismatch".into(),
        ));
    }
    require_object(connection, reference)
}

fn begin(connection: &mut Connection) -> Result<rusqlite::Transaction<'_>, IndexError> {
    connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|error| sql(&error))
}

impl InputStagingPort for SqliteCatalog {
    fn open_ingress(&self, operation: OperationKey) -> Result<IngressRef, IndexError> {
        let mut guard = self
            .lock()
            .map_err(|error| IndexError::Storage(error.to_string()))?;
        let tx = begin(&mut guard)?;
        if read_operation(&tx, operation)?.is_some() {
            return Err(IndexError::Conflict);
        }
        let current =
            current_view(&tx, operation.workspace)?.ok_or(IndexError::SnapshotUnavailable)?;
        if current.identity.epoch != operation.epoch {
            return Err(IndexError::Conflict);
        }
        let prior = read_ingress(&tx, operation)?;
        let stage = IngressRef {
            operation,
            fence: next_fence(&tx)?,
        };
        write_ingress(
            &tx,
            IngressRow {
                stage,
                sealed: prior.and_then(|row| row.sealed),
            },
            prior.is_none(),
        )?;
        tx.commit().map_err(|error| sql(&error))?;
        drop(guard);
        Ok(stage)
    }

    fn reserve_inputs(
        &self,
        ingress: IngressRef,
        objects: &[RegisteredObject],
    ) -> Result<(), IndexError> {
        if objects.len() > 256 {
            return Err(IndexError::ResourceLimited);
        }
        let mut guard = self
            .lock()
            .map_err(|error| IndexError::Storage(error.to_string()))?;
        let tx = begin(&mut guard)?;
        require_open_ingress(&tx, ingress)?;
        let mut earlier = std::collections::BTreeSet::new();
        let operation = operation_key(ingress.operation)?;
        for object in objects {
            let canonical = canonical_object(object)?;
            for child in &canonical.children {
                if !earlier.contains(child) {
                    require_object(&tx, *child)?;
                }
            }
            if !earlier.insert(object.reference) {
                return Err(IndexError::InvalidInput(
                    "duplicate ingress reservation".into(),
                ));
            }
            let payload = encode(&canonical)?;
            if payload.len() > 1_048_576 {
                return Err(IndexError::ResourceLimited);
            }
            if let Some(prior) = ingress_intent(&tx, ingress.operation, object)? {
                if prior != payload {
                    return Err(IndexError::CorruptObject(
                        "ingress allocation metadata changed".into(),
                    ));
                }
            } else {
                let changed = tx.execute(
                    "INSERT INTO native_ingress_intent_v1(operation,object,payload,row_digest) VALUES (?1,?2,?3,?4)",
                    params![&operation,object_key(object.reference),&payload,digest(b"qidx.native.ingressintent.v1", &payload).to_vec()],
                ).map_err(|error| sql(&error))?;
                exactly_one(changed, "reserve ingress object")?;
            }
        }
        tx.commit().map_err(|error| sql(&error))?;
        drop(guard);
        Ok(())
    }

    fn confirm_inputs(
        &self,
        ingress: IngressRef,
        objects: &[RegisteredObject],
    ) -> Result<(), IndexError> {
        if objects.len() > 256 {
            return Err(IndexError::ResourceLimited);
        }
        let mut guard = self
            .lock()
            .map_err(|error| IndexError::Storage(error.to_string()))?;
        let tx = begin(&mut guard)?;
        require_open_ingress(&tx, ingress)?;
        let mut seen = std::collections::BTreeSet::new();
        for object in objects {
            if !seen.insert(object.reference) {
                return Err(IndexError::InvalidInput(
                    "duplicate ingress confirmation".into(),
                ));
            }
            if ingress_intent(&tx, ingress.operation, object)?
                != Some(encode(&canonical_object(object)?)?)
            {
                return Err(IndexError::BackendViolation(
                    "ingress confirmation lacks matching allocation intent".into(),
                ));
            }
        }
        register(&tx, objects)?;
        let operation = operation_key(ingress.operation)?;
        for object in objects {
            let key = object_key(object.reference);
            let changed = tx.execute(
                "INSERT OR IGNORE INTO native_ingress_object_v1(operation,object,row_digest) VALUES (?1,?2,?3)",
                params![&operation,&key,fields_digest(b"qidx.native.ingressobject.v1", &(&operation,&key))?],
            ).map_err(|error| sql(&error))?;
            at_most_one(changed, "confirm ingress object")?;
            if !confirmed_ingress_object(&tx, ingress.operation, object.reference)? {
                return Err(IndexError::Storage(
                    "confirmed ingress object missing".into(),
                ));
            }
        }
        tx.commit().map_err(|error| sql(&error))?;
        drop(guard);
        Ok(())
    }

    fn seal_inputs(&self, ingress: IngressRef, request: ObjectRef) -> Result<(), IndexError> {
        let mut guard = self
            .lock()
            .map_err(|error| IndexError::Storage(error.to_string()))?;
        let tx = begin(&mut guard)?;
        let mut row = check_ingress(&tx, ingress)?;
        if let Some(prior) = row.sealed {
            if prior != request {
                return Err(IndexError::DigestConflict);
            }
        } else {
            if !confirmed_ingress_object(&tx, ingress.operation, request)? {
                return Err(IndexError::InvalidInput(
                    "request root is not confirmed by ingress".into(),
                ));
            }
            row.sealed = Some(request);
            write_ingress(&tx, row, false)?;
        }
        tx.commit().map_err(|error| sql(&error))?;
        drop(guard);
        Ok(())
    }

    fn discard_inputs(
        &self,
        ingress: IngressRef,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        let mut guard = self
            .lock()
            .map_err(|error| IndexError::Storage(error.to_string()))?;
        let tx = begin(&mut guard)?;
        let _checked = check_ingress(&tx, ingress)?;
        delete_ingress(&tx, ingress.operation, budget)?;
        budget.checkpoint()?;
        tx.commit().map_err(|error| sql(&error))?;
        drop(guard);
        Ok(())
    }
}

impl OperationJournalPort for SqliteCatalog {
    fn admit(
        &self,
        immutable: ImmutableOperation,
        ingress: IngressRef,
        events: &[SourceEvent],
        budget: &mut WorkBudget,
    ) -> Result<AdmissionOutcome, IndexError> {
        let mut guard = self
            .lock()
            .map_err(|error| IndexError::Storage(error.to_string()))?;
        let tx = begin(&mut guard)?;
        if let Some(existing) = read_operation(&tx, immutable.key)? {
            if existing.immutable.request != immutable.request || existing.events != events {
                return Err(IndexError::DigestConflict);
            }
            return Ok(AdmissionOutcome::Existing(existing.status));
        }
        if ingress.operation != immutable.key {
            return Err(IndexError::InvalidInput(
                "ingress belongs to another operation".into(),
            ));
        }
        let staged = check_ingress(&tx, ingress)?;
        if staged.sealed != Some(immutable.request) {
            return Err(IndexError::DigestConflict);
        }
        let current =
            current_view(&tx, immutable.key.workspace)?.ok_or(IndexError::SnapshotUnavailable)?;
        if current.identity.epoch != immutable.key.epoch {
            return Err(IndexError::Conflict);
        }
        if !confirmed_ingress_object(&tx, immutable.key, immutable.request)? {
            return Err(IndexError::InvalidInput(
                "sealed request lost ingress custody".into(),
            ));
        }
        require_object(&tx, immutable.request)?;
        budget.checkpoint()?;
        let fence = next_fence(&tx)?;
        let attempt = attempt_ref(immutable.key, fence)?;
        write_operation(
            &tx,
            &OperationRow {
                immutable,
                events: events.to_vec(),
                fence,
                status: OperationStatus::Pending(attempt),
            },
            true,
        )?;
        transfer_ingress_objects(&tx, immutable.key, budget)?;
        claim_events(&tx, immutable.key, events, budget)?;
        delete_ingress(&tx, immutable.key, budget)?;
        budget.checkpoint()?;
        tx.commit().map_err(|error| sql(&error))?;
        drop(guard);
        Ok(AdmissionOutcome::New(attempt))
    }

    fn inspect(&self, key: OperationKey) -> Result<OperationStatus, IndexError> {
        let guard = self
            .lock()
            .map_err(|error| IndexError::Storage(error.to_string()))?;
        Ok(read_operation(&guard, key)?.map_or(OperationStatus::Unknown, |row| row.status))
    }

    fn operation(&self, key: OperationKey) -> Result<ImmutableOperation, IndexError> {
        let guard = self
            .lock()
            .map_err(|error| IndexError::Storage(error.to_string()))?;
        read_operation(&guard, key)?
            .map(|row| row.immutable)
            .ok_or(IndexError::SnapshotUnavailable)
    }

    fn claim_recovery(&self, key: OperationKey) -> Result<AttemptRef, IndexError> {
        let mut guard = self
            .lock()
            .map_err(|error| IndexError::Storage(error.to_string()))?;
        let tx = begin(&mut guard)?;
        let mut row = read_operation(&tx, key)?.ok_or(IndexError::SnapshotUnavailable)?;
        if !matches!(row.status, OperationStatus::Pending(_)) {
            return Err(IndexError::Conflict);
        }
        let current = current_view(&tx, key.workspace)?.ok_or(IndexError::SnapshotUnavailable)?;
        if current.identity.epoch != key.epoch {
            return Err(IndexError::Conflict);
        }
        row.fence = next_fence(&tx)?;
        let attempt = attempt_ref(key, row.fence)?;
        row.status = OperationStatus::Pending(attempt);
        write_operation(&tx, &row, false)?;
        tx.commit().map_err(|error| sql(&error))?;
        drop(guard);
        Ok(attempt)
    }

    fn refuse(&self, attempt: AttemptRef, reason: Refusal) -> Result<OperationStatus, IndexError> {
        let mut guard = self
            .lock()
            .map_err(|error| IndexError::Storage(error.to_string()))?;
        let tx = begin(&mut guard)?;
        let mut row = check_attempt(&tx, attempt)?;
        transition_claims(
            &tx,
            attempt.operation,
            Some(&row.events),
            EventClaimState::Refused,
        )?;
        row.status = OperationStatus::Refused(reason);
        write_operation(&tx, &row, false)?;
        tx.commit().map_err(|error| sql(&error))?;
        drop(guard);
        Ok(row.status)
    }
}

impl ObjectCustodyPort for SqliteCatalog {
    fn reserve_objects(
        &self,
        attempt: AttemptRef,
        objects: &[RegisteredObject],
    ) -> Result<(), IndexError> {
        let mut guard = self
            .lock()
            .map_err(|error| IndexError::Storage(error.to_string()))?;
        let tx = begin(&mut guard)?;
        let _checked_attempt = check_attempt(&tx, attempt)?;
        reserve(&tx, attempt, objects)?;
        tx.commit().map_err(|error| sql(&error))?;
        drop(guard);
        Ok(())
    }

    fn register_objects(
        &self,
        attempt: AttemptRef,
        objects: &[RegisteredObject],
    ) -> Result<(), IndexError> {
        let mut guard = self
            .lock()
            .map_err(|error| IndexError::Storage(error.to_string()))?;
        let tx = begin(&mut guard)?;
        let _checked_attempt = check_attempt(&tx, attempt)?;
        for object in objects {
            require_intent(&tx, attempt, object)?;
        }
        register(&tx, objects)?;
        for object in objects {
            let inserted = tx.execute(
                "INSERT OR IGNORE INTO native_attempt_object_v1(operation,fence,object,row_digest) VALUES (?1,?2,?3,?4)",
                params![operation_key(attempt.operation)?, int(attempt.fence)?, object_key(object.reference),
                    fields_digest(b"qidx.native.attemptobject.v1", &(operation_key(attempt.operation)?, int(attempt.fence)?, object_key(object.reference)))?],
            ).map_err(|error| sql(&error))?;
            at_most_one(inserted, "protect attempt object")?;
        }
        tx.commit().map_err(|error| sql(&error))?;
        drop(guard);
        Ok(())
    }
}

impl WorkClaimPort for SqliteCatalog {
    fn ready_work(
        &self,
        workspace: WorkspaceKey,
        epoch: u64,
        limit: u32,
        budget: &mut WorkBudget,
    ) -> Result<Vec<WorkIntent>, IndexError> {
        if limit > 256 {
            return Err(IndexError::ResourceLimited);
        }
        budget.checkpoint()?;
        let mut guard = self
            .lock()
            .map_err(|error| IndexError::Storage(error.to_string()))?;
        let tx = begin(&mut guard)?;
        let current = current_view(&tx, workspace)?.ok_or(IndexError::SnapshotUnavailable)?;
        if current.identity.epoch != epoch {
            return Err(IndexError::Conflict);
        }
        let now = catalog_now_ms(&tx)?;
        let mut candidates = ready_candidates(&tx, workspace, epoch, now, limit, false, budget)?;
        candidates.extend(ready_candidates(
            &tx, workspace, epoch, now, limit, true, budget,
        )?);
        candidates.sort_unstable();
        candidates.truncate(
            usize::try_from(limit).map_err(|_conversion_error| IndexError::ResourceLimited)?,
        );
        let mut result = Vec::with_capacity(candidates.len());
        for (due, key) in candidates {
            let (payload, checksum): (Vec<u8>, Vec<u8>) = tx
                .query_row(
                    "SELECT payload,row_digest FROM native_work_v1 WHERE work=?1",
                    params![&key],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .map_err(|error| sql(&error))?;
            check_digest(b"qidx.native.work.v2", &payload, &checksum)?;
            let decoded: WorkRow = decode(&payload)?;
            if work_key(&decoded.intent)? != key {
                return Err(IndexError::Storage("ready work identity mismatch".into()));
            }
            let verified = read_work(&tx, &decoded.intent)?
                .ok_or_else(|| IndexError::Storage("ready work disappeared".into()))?;
            if verified.intent.workspace != workspace || verified.intent.epoch != epoch {
                return Err(IndexError::Storage("ready work scope mismatch".into()));
            }
            match verified.status {
                WorkStatus::Pending if verified.ready_since_ms == due => {}
                WorkStatus::Claimed(claim)
                    if claim.lease_until_ms <= now && claim.lease_until_ms == due => {}
                WorkStatus::Unknown
                | WorkStatus::Pending
                | WorkStatus::Claimed(_)
                | WorkStatus::Completed(_)
                | WorkStatus::Superseded => {
                    return Err(IndexError::Storage("ready work status mismatch".into()));
                }
            }
            result.push(verified.intent);
        }
        tx.commit().map_err(|error| sql(&error))?;
        drop(guard);
        Ok(result)
    }

    fn claim_work(&self, intent: &WorkIntent, lease: Duration) -> Result<WorkClaim, IndexError> {
        let intent = *intent;
        let lease_ms = validated_lease_ms(lease)?;
        let mut guard = self
            .lock()
            .map_err(|error| IndexError::Storage(error.to_string()))?;
        let tx = begin(&mut guard)?;
        let current =
            current_view(&tx, intent.workspace)?.ok_or(IndexError::SnapshotUnavailable)?;
        if current.identity.epoch != intent.epoch {
            return Err(IndexError::Conflict);
        }
        let now = catalog_now_ms(&tx)?;
        let mut row = read_work(&tx, &intent)?.ok_or(IndexError::Conflict)?;
        match row.status {
            WorkStatus::Pending => {}
            WorkStatus::Claimed(old) if old.lease_until_ms <= now => {}
            WorkStatus::Unknown
            | WorkStatus::Claimed(_)
            | WorkStatus::Completed(_)
            | WorkStatus::Superseded => return Err(IndexError::Conflict),
        }
        let claim = WorkClaim {
            intent,
            fence: next_fence(&tx)?,
            lease_until_ms: now
                .checked_add(lease_ms)
                .ok_or(IndexError::ResourceLimited)?,
        };
        row.status = WorkStatus::Claimed(claim);
        write_work(&tx, &row, false)?;
        tx.commit().map_err(|error| sql(&error))?;
        drop(guard);
        Ok(claim)
    }

    fn renew_work(&self, claim: &WorkClaim, lease: Duration) -> Result<WorkClaim, IndexError> {
        let claim = *claim;
        let lease_ms = validated_lease_ms(lease)?;
        let mut guard = self
            .lock()
            .map_err(|error| IndexError::Storage(error.to_string()))?;
        let tx = begin(&mut guard)?;
        let now = catalog_now_ms(&tx)?;
        let mut row = live_claim(&tx, &claim, now)?;
        let WorkStatus::Claimed(current) = row.status else {
            return Err(IndexError::WorkFenceLost);
        };
        let renewed = WorkClaim {
            intent: claim.intent,
            fence: claim.fence,
            lease_until_ms: current.lease_until_ms.max(
                now.checked_add(lease_ms)
                    .ok_or(IndexError::ResourceLimited)?,
            ),
        };
        row.status = WorkStatus::Claimed(renewed);
        write_work(&tx, &row, false)?;
        tx.commit().map_err(|error| sql(&error))?;
        drop(guard);
        Ok(renewed)
    }

    fn work_status(&self, intent: &WorkIntent) -> Result<WorkStatus, IndexError> {
        let intent = *intent;
        let guard = self
            .lock()
            .map_err(|error| IndexError::Storage(error.to_string()))?;
        Ok(read_work(&guard, &intent)?.map_or(WorkStatus::Unknown, |row| row.status))
    }

    fn prepared_work(&self, claim: &WorkClaim) -> Result<Option<OwnerMemberBinding>, IndexError> {
        let claim = *claim;
        let mut guard = self
            .lock()
            .map_err(|error| IndexError::Storage(error.to_string()))?;
        let tx = begin(&mut guard)?;
        let now = catalog_now_ms(&tx)?;
        let row = live_claim(&tx, &claim, now)?;
        if let Some(binding) = &row.prepared {
            require_object(&tx, binding.artifact)?;
        }
        tx.commit().map_err(|error| sql(&error))?;
        drop(guard);
        Ok(row.prepared)
    }

    fn stage_work(&self, claim: &WorkClaim, binding: OwnerMemberBinding) -> Result<(), IndexError> {
        let claim = *claim;
        let mut guard = self
            .lock()
            .map_err(|error| IndexError::Storage(error.to_string()))?;
        let tx = begin(&mut guard)?;
        let now = catalog_now_ms(&tx)?;
        let mut row = live_claim(&tx, &claim, now)?;
        let WorkScope::Owner { owner, expected } = claim.intent.scope else {
            return Err(IndexError::InvalidInput(
                "projection domain has no owner artifact result contract".into(),
            ));
        };
        if binding.instance != claim.intent.instance
            || binding.owner != owner
            || binding.input != expected
        {
            return Err(IndexError::BackendViolation(
                "staged owner result does not match work intent".into(),
            ));
        }
        if let Some(prior) = &row.prepared
            && prior != &binding
        {
            return Err(IndexError::DigestConflict);
        }
        require_object(&tx, binding.artifact)?;
        protect_work_root(&tx, &claim.intent, binding.artifact)?;
        row.prepared = Some(binding);
        write_work(&tx, &row, false)?;
        tx.commit().map_err(|error| sql(&error))?;
        drop(guard);
        Ok(())
    }
}

fn install_frontiers(
    connection: &Connection,
    workspace: WorkspaceKey,
    events: &[SourceEvent],
) -> Result<(), IndexError> {
    let workspace = workspace_key(workspace)?;
    for event in events {
        let prior: Option<(i64, i64, Vec<u8>)> = connection
            .query_row(
                "SELECT stream_epoch,sequence,row_digest FROM native_frontier_v1
             WHERE workspace=?1 AND producer=?2 AND stream=?3",
                params![
                    &workspace,
                    event.producer.0.to_vec(),
                    event.stream.0.to_vec()
                ],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()
            .map_err(|error| sql(&error))?;
        if let Some((epoch, sequence, row_digest)) = prior.as_ref()
            && *row_digest
                != fields_digest(
                    b"qidx.native.frontier.v1",
                    &(
                        &workspace,
                        event.producer.0.to_vec(),
                        event.stream.0.to_vec(),
                        *epoch,
                        *sequence,
                    ),
                )?
        {
            return Err(IndexError::Storage(
                "native frontier digest mismatch".into(),
            ));
        }
        match prior {
            Some((epoch, sequence, _))
                if uint(epoch)? == event.stream_epoch && uint(sequence)? == event.predecessor => {}
            None if event.predecessor == 0 => {}
            Some(_) | None => return Err(IndexError::Conflict),
        }
        if event.sequence
            != event
                .predecessor
                .checked_add(1)
                .ok_or(IndexError::Conflict)?
        {
            return Err(IndexError::Conflict);
        }
        let changed = connection
            .execute(
                "INSERT INTO native_frontier_v1(workspace,producer,stream,stream_epoch,sequence,row_digest)
             VALUES (?1,?2,?3,?4,?5,?6)
             ON CONFLICT(workspace,producer,stream) DO UPDATE SET
                 stream_epoch=excluded.stream_epoch, sequence=excluded.sequence,
                 row_digest=excluded.row_digest",
                params![
                    &workspace,
                    event.producer.0.to_vec(),
                    event.stream.0.to_vec(),
                    int(event.stream_epoch)?,
                    int(event.sequence)?,
                    fields_digest(b"qidx.native.frontier.v1",
                        &(&workspace, event.producer.0.to_vec(), event.stream.0.to_vec(), int(event.stream_epoch)?, int(event.sequence)?))?
                ],
            )
            .map_err(|error| sql(&error))?;
        exactly_one(changed, "advance source frontier")?;
    }
    Ok(())
}

impl CatalogPublicationPort for SqliteCatalog {
    fn create_workspace(
        &self,
        genesis: CatalogView,
        objects: &[RegisteredObject],
    ) -> Result<ViewRef, IndexError> {
        if genesis.identity.sequence != 0 {
            return Err(IndexError::InvalidInput(
                "genesis sequence must be zero".into(),
            ));
        }
        if genesis.source.owners.0.is_some()
            || genesis.source.paths.0.is_some()
            || genesis.source.frontiers.0.is_some()
            || genesis.projections.0.is_some()
            || genesis.members.0.is_some()
            || genesis.coverage.0.is_some()
        {
            return Err(IndexError::InvalidInput(
                "genesis must have empty source, projection, member and coverage roots".into(),
            ));
        }
        let mut guard = self
            .lock()
            .map_err(|error| IndexError::Storage(error.to_string()))?;
        let tx = begin(&mut guard)?;
        if current_view(&tx, genesis.identity.workspace)?.is_some() {
            return Err(IndexError::Conflict);
        }
        register(&tx, objects)?;
        ensure_roots(&tx, &genesis)?;
        write_view(&tx, &genesis)?;
        let payload = encode(&genesis)?;
        let inserted = tx
            .execute(
                "INSERT INTO native_workspace_v1(workspace,epoch,sequence,payload,row_digest)
             VALUES (?1,?2,0,?3,?4)",
                params![
                    workspace_key(genesis.identity.workspace)?,
                    int(genesis.identity.epoch)?,
                    &payload,
                    digest(b"qidx.native.view.v1", &payload).to_vec()
                ],
            )
            .map_err(|error| sql(&error))?;
        exactly_one(inserted, "create workspace")?;
        tx.commit().map_err(|error| sql(&error))?;
        drop(guard);
        Ok(genesis.identity)
    }

    fn commit_transition(
        &self,
        publication: PreparedPublication,
        actions: &mut dyn WorkActionCursor,
        budget: &mut WorkBudget,
    ) -> Result<OperationStatus, IndexError> {
        budget.checkpoint()?;
        let mut guard = self
            .lock()
            .map_err(|error| IndexError::Storage(error.to_string()))?;
        let tx = begin(&mut guard)?;
        let mut row = check_attempt(&tx, publication.attempt)?;
        let current = current_view(&tx, publication.expected.workspace)?
            .ok_or(IndexError::SnapshotUnavailable)?;
        if current.identity != publication.expected {
            return Err(IndexError::Conflict);
        }
        let next = &publication.next;
        if next.identity.workspace != current.identity.workspace
            || next.identity.epoch != current.identity.epoch
            || next.identity.sequence
                != current
                    .identity
                    .sequence
                    .checked_add(1)
                    .ok_or(IndexError::ResourceLimited)?
        {
            return Err(IndexError::InvalidInput(
                "next view must advance the same workspace epoch by one".into(),
            ));
        }
        if row.immutable.key.workspace != current.identity.workspace
            || row.immutable.key.epoch != current.identity.epoch
        {
            return Err(IndexError::Conflict);
        }
        if !publication.source_events.is_empty() && !publication.projection_events.is_empty() {
            return Err(IndexError::InvalidInput(
                "source and projection events cannot share one operation".into(),
            ));
        }
        let admitted_events = if publication.source_events.is_empty() {
            &publication.projection_events
        } else {
            &publication.source_events
        };
        if row.events.as_slice() != admitted_events.as_slice() {
            return Err(IndexError::InvalidInput(
                "publication events differ from admitted operation".into(),
            ));
        }
        if !publication.projection_events.is_empty() && publication.expected_source.is_some() {
            return Err(IndexError::InvalidInput(
                "projection publication cannot claim a source transition".into(),
            ));
        }
        if let Some(expected_source) = publication.expected_source {
            if expected_source != current.source {
                return Err(IndexError::Conflict);
            }
        } else if next.source != current.source {
            return Err(IndexError::InvalidInput(
                "source transition lacks expected source".into(),
            ));
        }
        if next.source == current.source && !publication.source_events.is_empty() {
            return Err(IndexError::InvalidInput(
                "source events without a source change".into(),
            ));
        }
        if next.source != current.source && publication.source_events.is_empty() {
            return Err(IndexError::InvalidInput(
                "source root changed without a source event".into(),
            ));
        }
        if next.projections != current.projections && publication.projection_events.is_empty() {
            return Err(IndexError::InvalidInput(
                "projection root changed without a projection event".into(),
            ));
        }
        ensure_roots(&tx, next)?;
        transition_claims(
            &tx,
            publication.attempt.operation,
            Some(admitted_events),
            EventClaimState::Committed,
        )?;
        install_frontiers(&tx, current.identity.workspace, admitted_events)?;
        write_view(&tx, next)?;
        let payload = encode(next)?;
        let changed = tx
            .execute(
                "UPDATE native_workspace_v1 SET sequence=?2,payload=?3,row_digest=?4
             WHERE workspace=?1 AND epoch=?5 AND sequence=?6",
                params![
                    workspace_key(current.identity.workspace)?,
                    int(next.identity.sequence)?,
                    &payload,
                    digest(b"qidx.native.view.v1", &payload).to_vec(),
                    int(current.identity.epoch)?,
                    int(current.identity.sequence)?
                ],
            )
            .map_err(|error| sql(&error))?;
        if changed != 1 {
            return Err(IndexError::Conflict);
        }
        let work_set = publication.work_actions;
        if let Some(root) = work_set.root.0 {
            require_attempt_object(&tx, publication.attempt, root)?;
        } else if work_set != WorkActionSet::empty() {
            return Err(IndexError::InvalidInput(
                "nonempty work actions require an attempt-owned manifest".into(),
            ));
        }
        tx.execute_batch(
            "CREATE TEMP TABLE native_work_action_seen_v1(work BLOB PRIMARY KEY) WITHOUT ROWID",
        )
        .map_err(|error| sql(&error))?;
        let mut action_digest = WorkActionDigest::default();
        let mut ready_since_ms = None;
        while let Some(action) = actions.next_action(budget)? {
            budget.charge(WorkUsage {
                mutations: 1,
                ..WorkUsage::default()
            })?;
            action_digest.push(&action)?;
            if action_digest.count() > work_set.count {
                return Err(IndexError::CorruptObject(
                    "work action stream exceeds declared count".into(),
                ));
            }
            let intent = match action {
                WorkAction::Retire(intent) | WorkAction::Create(intent) => intent,
            };
            if intent.workspace != next.identity.workspace || intent.epoch != next.identity.epoch {
                return Err(IndexError::InvalidInput(
                    "work action crosses workspace epoch".into(),
                ));
            }
            let key = work_key(&intent)?;
            let inserted = tx
                .execute(
                    "INSERT OR IGNORE INTO temp.native_work_action_seen_v1(work) VALUES (?1)",
                    params![&key],
                )
                .map_err(|error| sql(&error))?;
            if inserted != 1 {
                return Err(IndexError::InvalidInput(
                    "work intent appears in multiple transitions".into(),
                ));
            }
            match action {
                WorkAction::Retire(_) => {
                    let mut prior = read_work(&tx, &intent)?.ok_or(IndexError::Conflict)?;
                    if matches!(prior.status, WorkStatus::Pending | WorkStatus::Claimed(_)) {
                        prior.status = WorkStatus::Superseded;
                        write_work(&tx, &prior, false)?;
                    }
                }
                WorkAction::Create(_) => {
                    validate_work_scope(&tx, &intent)?;
                    let since = if let Some(since) = ready_since_ms {
                        since
                    } else {
                        let since = catalog_now_ms(&tx)?;
                        ready_since_ms = Some(since);
                        since
                    };
                    if let Some(mut prior) = read_work(&tx, &intent)? {
                        if matches!(
                            prior.status,
                            WorkStatus::Completed(_) | WorkStatus::Superseded
                        ) {
                            prior.status = WorkStatus::Pending;
                            prior.ready_since_ms = since;
                            write_work(&tx, &prior, false)?;
                        }
                    } else {
                        write_work(
                            &tx,
                            &WorkRow {
                                intent,
                                created_operation: publication.attempt.operation,
                                created_view: next.identity,
                                status: WorkStatus::Pending,
                                ready_since_ms: since,
                                prepared: None,
                            },
                            true,
                        )?;
                    }
                    for reference in work_roots(&intent) {
                        protect_work_root(&tx, &intent, reference)?;
                    }
                }
            }
        }
        if action_digest.count() != work_set.count || action_digest.finish() != work_set.digest {
            return Err(IndexError::CorruptObject(
                "work action stream count or digest mismatch".into(),
            ));
        }
        if let Some(claim) = publication.complete_work {
            if claim.intent.workspace != next.identity.workspace
                || claim.intent.epoch != next.identity.epoch
            {
                return Err(IndexError::InvalidInput(
                    "completed work crosses workspace epoch".into(),
                ));
            }
            let inserted = tx
                .execute(
                    "INSERT OR IGNORE INTO temp.native_work_action_seen_v1(work) VALUES (?1)",
                    params![work_key(&claim.intent)?],
                )
                .map_err(|error| sql(&error))?;
            if inserted != 1 {
                return Err(IndexError::InvalidInput(
                    "completed work appears in another transition".into(),
                ));
            }
            let now = catalog_now_ms(&tx)?;
            let mut completed = live_claim(&tx, &claim, now)?;
            if !matches!(claim.intent.scope, WorkScope::Owner { .. })
                || completed.prepared.is_none()
            {
                return Err(IndexError::InvalidInput(
                    "owner work completion requires staged artifact".into(),
                ));
            }
            let artifact = completed
                .prepared
                .as_ref()
                .map(|binding| binding.artifact)
                .ok_or_else(|| IndexError::Storage("prepared artifact missing".into()))?;
            require_object(&tx, artifact)?;
            completed.status = WorkStatus::Completed(next.identity);
            write_work(&tx, &completed, false)?;
        }
        tx.execute_batch("DROP TABLE temp.native_work_action_seen_v1")
            .map_err(|error| sql(&error))?;
        row.status = OperationStatus::Published(next.identity);
        write_operation(&tx, &row, false)?;
        tx.commit().map_err(|error| sql(&error))?;
        drop(guard);
        Ok(row.status)
    }
}

fn ticket_from_integer(value: i64) -> Result<TicketId, IndexError> {
    let value = uint(value)?;
    let mut bytes = [0u8; 16];
    bytes[..8].copy_from_slice(b"QIDX_TKT");
    bytes[8..].copy_from_slice(&value.to_le_bytes());
    Ok(TicketId(bytes))
}
fn ticket_to_integer(ticket: TicketId) -> Result<i64, IndexError> {
    if &ticket.0[..8] != b"QIDX_TKT" {
        return Err(IndexError::InvalidInput("ticket namespace mismatch".into()));
    }
    let mut bytes = [0u8; 8];
    bytes.copy_from_slice(&ticket.0[8..]);
    int(u64::from_le_bytes(bytes))
}

impl CatalogReadPort for SqliteCatalog {
    fn acquire_ticket(&self, selection: ViewSelection) -> Result<TicketRecord, IndexError> {
        let mut guard = self
            .lock()
            .map_err(|error| IndexError::Storage(error.to_string()))?;
        let tx = begin(&mut guard)?;
        let view = match selection {
            ViewSelection::Latest { workspace, epoch } => {
                let current =
                    current_view(&tx, workspace)?.ok_or(IndexError::SnapshotUnavailable)?;
                if current.identity.epoch != epoch {
                    return Err(IndexError::SnapshotUnavailable);
                }
                current
            }
            ViewSelection::Exact(identity) => read_view_row(&tx, identity)?,
        };
        ensure_roots(&tx, &view)?;
        let inserted = tx
            .execute(
                "INSERT INTO native_pin_v1(workspace,epoch,sequence) VALUES (?1,?2,?3)",
                params![
                    workspace_key(view.identity.workspace)?,
                    int(view.identity.epoch)?,
                    int(view.identity.sequence)?
                ],
            )
            .map_err(|error| sql(&error))?;
        exactly_one(inserted, "create view pin")?;
        let id = ticket_from_integer(tx.last_insert_rowid())?;
        let changed = tx
            .execute(
                "UPDATE native_pin_v1 SET row_digest=?2 WHERE ticket=?1",
                params![
                    ticket_to_integer(id)?,
                    fields_digest(
                        b"qidx.native.pin.v1",
                        &(
                            ticket_to_integer(id)?,
                            workspace_key(view.identity.workspace)?,
                            int(view.identity.epoch)?,
                            int(view.identity.sequence)?
                        )
                    )?
                ],
            )
            .map_err(|error| sql(&error))?;
        exactly_one(changed, "seal view pin")?;
        tx.commit().map_err(|error| sql(&error))?;
        drop(guard);
        Ok(TicketRecord { id, view })
    }

    fn release_ticket(&self, id: TicketId) -> Result<(), IndexError> {
        let mut guard = self
            .lock()
            .map_err(|error| IndexError::Storage(error.to_string()))?;
        let tx = begin(&mut guard)?;
        let ticket = ticket_to_integer(id)?;
        let row: Option<PinSqlRow> = tx
            .query_row(
                "SELECT workspace,epoch,sequence,row_digest FROM native_pin_v1 WHERE ticket=?1",
                params![ticket],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .optional()
            .map_err(|error| sql(&error))?;
        let Some((workspace, epoch, sequence, Some(row_digest))) = row else {
            return Err(IndexError::SnapshotUnavailable);
        };
        if row_digest
            != fields_digest(b"qidx.native.pin.v1", &(ticket, workspace, epoch, sequence))?
        {
            return Err(IndexError::Storage("native pin digest mismatch".into()));
        }
        let changed = tx
            .execute("DELETE FROM native_pin_v1 WHERE ticket=?1", params![ticket])
            .map_err(|error| sql(&error))?;
        if changed != 1 {
            return Err(IndexError::SnapshotUnavailable);
        }
        tx.commit().map_err(|error| sql(&error))?;
        drop(guard);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::{
        collections::VecDeque,
        error::Error,
        time::{Duration, Instant},
    };

    use quanta_index_core::index::{
        CollectionId, Digest, IndexInstanceId, MapRoot, OwnerInputBinding, OwnerKey, ProducerId,
        SlotId, SourceCut, StableOwnerId, StorageFormatId, StreamId, TenantId, WorkLimits,
        WorktreeId,
    };

    use super::*;

    type TestResult = Result<(), Box<dyn Error>>;

    fn id(value: u8) -> [u8; 16] {
        [value; 16]
    }
    fn digest_id(value: u8) -> Digest {
        Digest([value; 32])
    }
    fn object(value: u8, children: Vec<ObjectRef>) -> RegisteredObject {
        RegisteredObject {
            reference: ObjectRef(digest_id(value)),
            bytes: 1,
            children,
        }
    }
    fn workspace() -> WorkspaceKey {
        WorkspaceKey {
            tenant: TenantId(id(1)),
            collection: CollectionId(id(2)),
            worktree: WorktreeId(id(3)),
            slot: SlotId(id(4)),
        }
    }
    fn view(sequence: u64, profile: ObjectRef, member: Option<ObjectRef>) -> CatalogView {
        let mut view = CatalogView {
            identity: ViewRef {
                workspace: workspace(),
                epoch: 7,
                sequence,
                digest: digest_id(sequence as u8 + 10),
            },
            profile,
            source: SourceCut {
                owners: MapRoot(None),
                paths: MapRoot(None),
                frontiers: MapRoot(None),
            },
            projections: MapRoot(None),
            members: MapRoot(member),
            coverage: MapRoot(None),
        };
        view.identity.digest = view.identity_digest().expect("test view encoding");
        view
    }
    fn operation(sequence: u64, request: u8) -> ImmutableOperation {
        ImmutableOperation {
            key: OperationKey {
                workspace: workspace(),
                epoch: 7,
                producer: ProducerId(id(8)),
                stream: StreamId(id(9)),
                stream_epoch: 12,
                sequence,
            },
            request: ObjectRef(digest_id(request)),
        }
    }
    fn owner_work(owner_id: u8) -> WorkIntent {
        WorkIntent {
            workspace: workspace(),
            epoch: 7,
            instance: IndexInstanceId(id(40)),
            scope: WorkScope::Owner {
                owner: OwnerKey {
                    producer: ProducerId(id(8)),
                    stable_id: StableOwnerId(id(owner_id)),
                },
                expected: OwnerInputBinding {
                    source: None,
                    projection: None,
                    producer_context: digest_id(4),
                    derivation_policy: digest_id(5),
                },
            },
        }
    }
    fn owner_result(intent: WorkIntent, artifact: ObjectRef) -> OwnerMemberBinding {
        let WorkScope::Owner { owner, expected } = intent.scope else {
            unreachable!("fixture only creates owner work");
        };
        OwnerMemberBinding {
            instance: intent.instance,
            owner,
            input: expected,
            format: StorageFormatId(id(41)),
            artifact,
            logical_commitment: digest_id(42),
        }
    }
    fn open(root: &Path) -> Result<SqliteCatalog, CoreError> {
        SqliteCatalog::open(root, Duration::from_secs(1))
    }
    fn setup(root: &Path) -> Result<(SqliteCatalog, CatalogView), Box<dyn Error>> {
        let catalog = open(root)?;
        let profile = object(30, vec![]);
        let genesis = view(0, profile.reference, None);
        let created = catalog.create_workspace(genesis.clone(), &[profile])?;
        assert_eq!(created, genesis.identity);
        Ok((catalog, genesis))
    }
    fn admit_new(
        catalog: &SqliteCatalog,
        operation: ImmutableOperation,
    ) -> Result<AttemptRef, Box<dyn Error>> {
        admit_new_events(catalog, operation, &[])
    }
    fn admit_new_events(
        catalog: &SqliteCatalog,
        operation: ImmutableOperation,
        events: &[SourceEvent],
    ) -> Result<AttemptRef, Box<dyn Error>> {
        let request = RegisteredObject {
            reference: operation.request,
            bytes: 1,
            children: vec![],
        };
        let ingress = stage_inputs(catalog, operation, &[request])?;
        match catalog.admit(operation, ingress, events, &mut admission_budget())? {
            AdmissionOutcome::New(attempt) => Ok(attempt),
            other => Err(format!("expected new attempt, got {other:?}").into()),
        }
    }
    fn admission_budget() -> WorkBudget {
        WorkBudget::new(
            WorkLimits {
                reads: 100_000,
                writes: 100_000,
                bytes: 32 * 1024 * 1024,
                mutations: 100_000,
            },
            Instant::now() + Duration::from_secs(30),
        )
    }
    fn stage_inputs(
        catalog: &SqliteCatalog,
        operation: ImmutableOperation,
        objects: &[RegisteredObject],
    ) -> Result<IngressRef, IndexError> {
        let ingress = catalog.open_ingress(operation.key)?;
        catalog.reserve_inputs(ingress, objects)?;
        catalog.confirm_inputs(ingress, objects)?;
        catalog.seal_inputs(ingress, operation.request)?;
        Ok(ingress)
    }
    fn replay_ingress(operation: ImmutableOperation) -> IngressRef {
        IngressRef {
            operation: operation.key,
            fence: 0,
        }
    }
    struct VecActions(VecDeque<WorkAction>);

    impl WorkActionCursor for VecActions {
        fn next_action(
            &mut self,
            budget: &mut WorkBudget,
        ) -> Result<Option<WorkAction>, IndexError> {
            budget.checkpoint()?;
            Ok(self.0.pop_front())
        }
    }

    fn stage_action_set(
        catalog: &SqliteCatalog,
        publication: &mut PreparedPublication,
        actions: &[WorkAction],
    ) -> Result<(), IndexError> {
        let mut summary = WorkActionDigest::default();
        for action in actions {
            summary.push(action)?;
        }
        if actions.is_empty() {
            publication.work_actions = WorkActionSet::empty();
        } else {
            let payload = operation_key(publication.attempt.operation)?;
            let reference = ObjectRef(Digest(digest(b"qidx.test.work-actions.v1", &payload)));
            reserve_confirm(
                catalog,
                publication.attempt,
                &[RegisteredObject {
                    reference,
                    bytes: 1,
                    children: vec![],
                }],
            )?;
            publication.work_actions = WorkActionSet {
                root: MapRoot(Some(reference)),
                count: summary.count(),
                digest: summary.finish(),
            };
        }
        Ok(())
    }

    fn commit_actions(
        catalog: &SqliteCatalog,
        mut publication: PreparedPublication,
        actions: &[WorkAction],
    ) -> Result<OperationStatus, IndexError> {
        stage_action_set(catalog, &mut publication, actions)?;
        let mut cursor = VecActions(actions.iter().copied().collect());
        let mut budget = WorkBudget::new(
            WorkLimits {
                reads: 4096,
                writes: 4096,
                bytes: 1 << 20,
                mutations: 4096,
            },
            Instant::now() + Duration::from_secs(5),
        );
        catalog.commit_transition(publication, &mut cursor, &mut budget)
    }

    fn commit_empty(
        catalog: &SqliteCatalog,
        publication: PreparedPublication,
    ) -> Result<OperationStatus, IndexError> {
        commit_actions(catalog, publication, &[])
    }

    fn publish(
        catalog: &SqliteCatalog,
        attempt: AttemptRef,
        current: &CatalogView,
        next: CatalogView,
    ) -> Result<OperationStatus, IndexError> {
        commit_empty(
            catalog,
            PreparedPublication {
                attempt,
                expected: current.identity,
                next,
                work_actions: WorkActionSet::empty(),
                complete_work: None,
                source_events: Vec::new(),
                projection_events: vec![],
                expected_source: None,
            },
        )
    }

    fn reserve_confirm(
        catalog: &SqliteCatalog,
        attempt: AttemptRef,
        objects: &[RegisteredObject],
    ) -> Result<(), IndexError> {
        for object in objects {
            catalog.reserve_objects(attempt, std::slice::from_ref(object))?;
            catalog.register_objects(attempt, std::slice::from_ref(object))?;
        }
        Ok(())
    }

    #[test]
    fn ready_work_lanes_use_ordered_bounded_indexes() -> TestResult {
        let connection = Connection::open_in_memory()?;
        connection.execute_batch(SCHEMA)?;
        for (query, index, parameters) in [
            (READY_PENDING_QUERY, "native_work_pending_idx_v1", 3),
            (READY_EXPIRED_QUERY, "native_work_view_idx_v1", 4),
        ] {
            let explain = format!("EXPLAIN QUERY PLAN {query}");
            let mut statement = connection.prepare(&explain)?;
            let rows = statement.query_map(
                rusqlite::params_from_iter((0..parameters).map(|_| 1_i64)),
                |row| row.get::<_, String>(3),
            )?;
            let plan = rows.collect::<Result<Vec<_>, _>>()?;
            assert!(plan.iter().any(|detail| detail.contains(index)), "{plan:?}");
            assert!(
                plan.iter().all(|detail| !detail.contains("TEMP B-TREE")),
                "{plan:?}"
            );
        }
        Ok(())
    }

    #[test]
    fn invalid_or_failing_work_cursor_rolls_back_head_work_and_receipt() -> TestResult {
        struct FailAfterOne(Option<WorkAction>);
        impl WorkActionCursor for FailAfterOne {
            fn next_action(
                &mut self,
                budget: &mut WorkBudget,
            ) -> Result<Option<WorkAction>, IndexError> {
                budget.checkpoint()?;
                self.0.take().map_or_else(
                    || {
                        Err(IndexError::BackendViolation(
                            "injected cursor failure".into(),
                        ))
                    },
                    |action| Ok(Some(action)),
                )
            }
        }

        let root = tempfile::tempdir()?;
        let (catalog, genesis) = setup(root.path())?;
        let operation = operation(250, 251);
        let attempt = admit_new(&catalog, operation)?;
        let intent = owner_work(6);
        let next = view(1, genesis.profile, None);
        let mut publication = PreparedPublication {
            attempt,
            expected: genesis.identity,
            next: next.clone(),
            work_actions: WorkActionSet::empty(),
            complete_work: None,
            source_events: vec![],
            projection_events: vec![],
            expected_source: None,
        };
        stage_action_set(&catalog, &mut publication, &[WorkAction::Create(intent)])?;
        let new_budget = || {
            WorkBudget::new(
                WorkLimits {
                    reads: 128,
                    writes: 128,
                    bytes: 8192,
                    mutations: 128,
                },
                Instant::now() + Duration::from_secs(5),
            )
        };
        let mut empty = VecActions(VecDeque::new());
        assert!(matches!(
            catalog.commit_transition(publication.clone(), &mut empty, &mut new_budget()),
            Err(IndexError::CorruptObject(_))
        ));
        let mut interrupted = FailAfterOne(Some(WorkAction::Create(intent)));
        assert!(matches!(
            catalog.commit_transition(publication.clone(), &mut interrupted, &mut new_budget()),
            Err(IndexError::BackendViolation(_))
        ));
        assert_eq!(
            catalog.inspect(operation.key)?,
            OperationStatus::Pending(attempt)
        );
        assert_eq!(catalog.work_status(&intent)?, WorkStatus::Unknown);
        let ticket = catalog.acquire_ticket(ViewSelection::Latest {
            workspace: workspace(),
            epoch: 7,
        })?;
        assert_eq!(ticket.view.identity, genesis.identity);
        catalog.release_ticket(ticket.id)?;
        let mut valid = VecActions(VecDeque::from([WorkAction::Create(intent)]));
        assert_eq!(
            catalog.commit_transition(publication, &mut valid, &mut new_budget())?,
            OperationStatus::Published(next.identity)
        );
        assert_eq!(catalog.work_status(&intent)?, WorkStatus::Pending);
        Ok(())
    }

    #[test]
    fn owner_work_is_durable_fenced_and_completed_with_publication() -> TestResult {
        let root = tempfile::tempdir()?;
        let (catalog, genesis) = setup(root.path())?;
        let first = operation(200, 201);
        let first_attempt = admit_new(&catalog, first)?;
        let intent = owner_work(1);
        let queued = view(1, genesis.profile, None);
        assert_eq!(
            commit_actions(
                &catalog,
                PreparedPublication {
                    attempt: first_attempt,
                    expected: genesis.identity,
                    next: queued.clone(),
                    work_actions: WorkActionSet::empty(),
                    complete_work: None,
                    source_events: vec![],
                    projection_events: vec![],
                    expected_source: None,
                },
                &[WorkAction::Create(intent)]
            )?,
            OperationStatus::Published(queued.identity)
        );
        let mut budget = WorkBudget::new(
            WorkLimits {
                reads: 8,
                writes: 8,
                bytes: 8192,
                mutations: 8,
            },
            Instant::now() + Duration::from_secs(5),
        );
        assert_eq!(
            catalog.ready_work(workspace(), 7, 1, &mut budget)?,
            vec![intent]
        );
        assert_eq!(
            catalog.ready_work(workspace(), 7, 257, &mut budget),
            Err(IndexError::ResourceLimited)
        );
        let claim = catalog.claim_work(&intent, Duration::from_secs(30))?;
        assert_eq!(catalog.work_status(&intent)?, WorkStatus::Claimed(claim));
        let artifact = object(202, vec![]);
        let second = operation(201, 203);
        let second_attempt = admit_new(&catalog, second)?;
        reserve_confirm(&catalog, second_attempt, std::slice::from_ref(&artifact))?;
        let mut wrong = owner_result(intent, artifact.reference);
        wrong.input.derivation_policy = digest_id(99);
        assert!(matches!(
            catalog.stage_work(&claim, wrong),
            Err(IndexError::BackendViolation(_))
        ));
        let result = owner_result(intent, artifact.reference);
        catalog.stage_work(&claim, result.clone())?;
        drop(catalog);

        let catalog = open(root.path())?;
        assert_eq!(catalog.prepared_work(&claim)?, Some(result));
        let mut completed_view = view(2, genesis.profile, None);
        completed_view.coverage = MapRoot(Some(artifact.reference));
        completed_view.identity.digest = completed_view.identity_digest()?;
        let publication = PreparedPublication {
            attempt: second_attempt,
            expected: genesis.identity,
            next: completed_view.clone(),
            work_actions: WorkActionSet::empty(),
            complete_work: Some(claim),
            source_events: vec![],
            projection_events: vec![],
            expected_source: None,
        };
        assert_eq!(
            commit_empty(&catalog, publication.clone()),
            Err(IndexError::Conflict)
        );
        assert_eq!(catalog.work_status(&intent)?, WorkStatus::Claimed(claim));
        assert_eq!(
            catalog.inspect(second.key)?,
            OperationStatus::Pending(second_attempt)
        );
        let mut current_publication = publication;
        current_publication.expected = queued.identity;
        assert_eq!(
            commit_empty(&catalog, current_publication)?,
            OperationStatus::Published(completed_view.identity)
        );
        assert_eq!(
            catalog.work_status(&intent)?,
            WorkStatus::Completed(completed_view.identity)
        );
        assert_eq!(
            catalog.claim_work(&intent, Duration::from_secs(1)),
            Err(IndexError::Conflict)
        );
        let mut changed_stamp = intent;
        changed_stamp.scope = match intent.scope {
            WorkScope::Owner {
                owner,
                mut expected,
            } => {
                expected.derivation_policy = digest_id(88);
                WorkScope::Owner { owner, expected }
            }
            WorkScope::ProjectionDomain { .. } => unreachable!("owner fixture"),
        };
        let third = operation(202, 204);
        let third_attempt = admit_new(&catalog, third)?;
        let mut changed_view = view(3, genesis.profile, None);
        changed_view.coverage = MapRoot(Some(artifact.reference));
        changed_view.identity.digest = changed_view.identity_digest()?;
        assert_eq!(
            commit_actions(
                &catalog,
                PreparedPublication {
                    attempt: third_attempt,
                    expected: completed_view.identity,
                    next: changed_view.clone(),
                    work_actions: WorkActionSet::empty(),
                    complete_work: None,
                    source_events: vec![],
                    projection_events: vec![],
                    expected_source: None,
                },
                &[WorkAction::Create(changed_stamp)]
            )?,
            OperationStatus::Published(changed_view.identity)
        );
        let fourth = operation(203, 205);
        let fourth_attempt = admit_new(&catalog, fourth)?;
        let mut restored_view = view(4, genesis.profile, None);
        restored_view.coverage = MapRoot(Some(artifact.reference));
        restored_view.identity.digest = restored_view.identity_digest()?;
        assert_eq!(
            commit_actions(
                &catalog,
                PreparedPublication {
                    attempt: fourth_attempt,
                    expected: changed_view.identity,
                    next: restored_view.clone(),
                    work_actions: WorkActionSet::empty(),
                    complete_work: None,
                    source_events: vec![],
                    projection_events: vec![],
                    expected_source: None,
                },
                &[
                    WorkAction::Retire(changed_stamp),
                    WorkAction::Create(intent)
                ]
            )?,
            OperationStatus::Published(restored_view.identity)
        );
        assert_eq!(catalog.work_status(&intent)?, WorkStatus::Pending);
        assert_eq!(catalog.work_status(&changed_stamp)?, WorkStatus::Superseded);
        let rearmed = catalog.claim_work(&intent, Duration::from_secs(5))?;
        assert_ne!(rearmed.fence, claim.fence);
        assert_eq!(
            catalog.prepared_work(&rearmed)?,
            Some(owner_result(intent, artifact.reference))
        );
        assert_eq!(
            catalog.prepared_work(&claim),
            Err(IndexError::WorkFenceLost)
        );
        assert_eq!(
            catalog.inspect(second.key)?,
            OperationStatus::Published(completed_view.identity)
        );
        Ok(())
    }

    #[test]
    fn expired_work_lease_reclaims_with_new_fence_and_retains_prepared_result() -> TestResult {
        let root = tempfile::tempdir()?;
        let (catalog, genesis) = setup(root.path())?;
        let first = operation(210, 211);
        let attempt = admit_new(&catalog, first)?;
        let intent = owner_work(2);
        let queued = view(1, genesis.profile, None);
        let published = commit_actions(
            &catalog,
            PreparedPublication {
                attempt,
                expected: genesis.identity,
                next: queued.clone(),
                work_actions: WorkActionSet::empty(),
                complete_work: None,
                source_events: vec![],
                projection_events: vec![],
                expected_source: None,
            },
            &[WorkAction::Create(intent)],
        )?;
        assert_eq!(published, OperationStatus::Published(queued.identity));
        let old = catalog.claim_work(&intent, Duration::from_millis(1))?;
        std::thread::sleep(Duration::from_millis(10));
        drop(catalog);
        let catalog = open(root.path())?;
        let forged = WorkClaim {
            lease_until_ms: u64::MAX,
            ..old
        };
        assert_eq!(
            catalog.prepared_work(&forged),
            Err(IndexError::WorkFenceLost)
        );
        let new = catalog.claim_work(&intent, Duration::from_millis(200))?;
        assert_ne!(old.fence, new.fence);
        assert_eq!(catalog.prepared_work(&old), Err(IndexError::WorkFenceLost));
        assert_eq!(catalog.work_status(&intent)?, WorkStatus::Claimed(new));
        let renewed = catalog.renew_work(&new, Duration::from_secs(5))?;
        assert_eq!(renewed.fence, new.fence);
        assert!(renewed.lease_until_ms > new.lease_until_ms);
        std::thread::sleep(Duration::from_millis(250));
        assert_eq!(catalog.prepared_work(&new)?, None);
        let again = catalog.renew_work(&new, Duration::from_secs(1))?;
        assert_eq!(again.fence, new.fence);
        assert_eq!(again.lease_until_ms, renewed.lease_until_ms);
        assert_eq!(catalog.work_status(&intent)?, WorkStatus::Claimed(renewed));
        Ok(())
    }

    #[test]
    fn restart_replays_original_receipt_and_preserves_exact_old_view() -> TestResult {
        let root = tempfile::tempdir()?;
        let (catalog, genesis) = setup(root.path())?;
        let operation = operation(55, 60);
        let attempt = admit_new(&catalog, operation)?;
        let leaf = object(50, vec![]);
        let map = object(51, vec![leaf.reference]);
        reserve_confirm(&catalog, attempt, &[leaf, map.clone()])?;
        let next = view(1, genesis.profile, Some(map.reference));
        assert_eq!(
            publish(&catalog, attempt, &genesis, next.clone())?,
            OperationStatus::Published(next.identity)
        );
        drop(catalog);

        let catalog = open(root.path())?;
        assert_eq!(
            catalog.inspect(operation.key)?,
            OperationStatus::Published(next.identity)
        );
        assert_eq!(catalog.operation(operation.key)?, operation);
        assert_eq!(
            catalog.admit(
                operation,
                replay_ingress(operation),
                &[],
                &mut admission_budget()
            )?,
            AdmissionOutcome::Existing(OperationStatus::Published(next.identity))
        );
        let conflicting = ImmutableOperation {
            request: ObjectRef(digest_id(61)),
            ..operation
        };
        assert_eq!(
            catalog.admit(
                conflicting,
                replay_ingress(conflicting),
                &[],
                &mut admission_budget()
            ),
            Err(IndexError::DigestConflict)
        );
        let old = catalog.acquire_ticket(ViewSelection::Exact(genesis.identity))?;
        assert_eq!(old.view, genesis);
        let new = catalog.acquire_ticket(ViewSelection::Latest {
            workspace: workspace(),
            epoch: 7,
        })?;
        assert_eq!(new.view, next);
        catalog.release_ticket(old.id)?;
        catalog.release_ticket(new.id)?;
        Ok(())
    }

    #[test]
    fn terminal_operation_replays_digest_and_receipt_without_request_object() -> TestResult {
        let root = tempfile::tempdir()?;
        let (catalog, _) = setup(root.path())?;
        let operation = operation(57, 62);
        let attempt = admit_new(&catalog, operation)?;
        assert_eq!(
            catalog.refuse(attempt, Refusal::InvalidInput)?,
            OperationStatus::Refused(Refusal::InvalidInput)
        );
        let direct = Connection::open(catalog.path())?;
        let removed_custody = direct.execute(
            "DELETE FROM native_operation_object_v1 WHERE operation=?1 AND object=?2",
            params![operation_key(operation.key)?, object_key(operation.request)],
        )?;
        assert_eq!(removed_custody, 1);
        let removed_object = direct.execute(
            "DELETE FROM native_object_v1 WHERE object=?1",
            params![object_key(operation.request)],
        )?;
        assert_eq!(removed_object, 1);
        drop(direct);
        drop(catalog);
        let catalog = open(root.path())?;
        assert_eq!(catalog.operation(operation.key)?, operation);
        assert_eq!(
            catalog.inspect(operation.key)?,
            OperationStatus::Refused(Refusal::InvalidInput)
        );
        assert_eq!(
            catalog.admit(
                operation,
                replay_ingress(operation),
                &[],
                &mut admission_budget()
            )?,
            AdmissionOutcome::Existing(OperationStatus::Refused(Refusal::InvalidInput))
        );
        let mismatched_event = SourceEvent {
            producer: ProducerId(id(8)),
            stream: StreamId(id(9)),
            stream_epoch: 1,
            predecessor: 0,
            sequence: 1,
            payload: digest_id(1),
        };
        assert_eq!(
            catalog.admit(
                operation,
                replay_ingress(operation),
                &[mismatched_event],
                &mut admission_budget()
            ),
            Err(IndexError::DigestConflict)
        );
        Ok(())
    }

    #[test]
    fn shared_object_in_two_view_slots_keeps_distinct_logical_identity_and_old_pin() -> TestResult {
        let root = tempfile::tempdir()?;
        let (catalog, genesis) = setup(root.path())?;
        let old_pin = catalog.acquire_ticket(ViewSelection::Exact(genesis.identity))?;
        let operation = operation(56, 61);
        let attempt = admit_new(&catalog, operation)?;
        let next = view(1, genesis.profile, Some(genesis.profile));
        assert_ne!(next.identity.digest, genesis.identity.digest);
        assert_eq!(next.profile, next.members.0.ok_or("member root missing")?);
        assert_eq!(
            publish(&catalog, attempt, &genesis, next.clone())?,
            OperationStatus::Published(next.identity)
        );
        let new_pin = catalog.acquire_ticket(ViewSelection::Exact(next.identity))?;
        assert_eq!(new_pin.view, next);
        assert_eq!(old_pin.view, genesis);
        catalog.release_ticket(old_pin.id)?;
        catalog.release_ticket(new_pin.id)?;
        Ok(())
    }

    #[test]
    fn missing_root_conflict_and_stale_fence_leave_pending_unpublished() -> TestResult {
        let root = tempfile::tempdir()?;
        let (catalog, genesis) = setup(root.path())?;
        let first_operation = operation(2, 70);
        let old_attempt = admit_new(&catalog, first_operation)?;
        let missing = ObjectRef(digest_id(99));
        assert!(matches!(
            catalog.register_objects(old_attempt, &[object(40, vec![])]),
            Err(IndexError::BackendViolation(_))
        ));
        let invalid = view(1, genesis.profile, Some(missing));
        assert!(matches!(
            publish(&catalog, old_attempt, &genesis, invalid),
            Err(IndexError::CorruptObject(_))
        ));
        assert_eq!(
            catalog.inspect(first_operation.key)?,
            OperationStatus::Pending(old_attempt)
        );
        drop(catalog);

        let catalog = open(root.path())?;
        assert_eq!(
            catalog.inspect(first_operation.key)?,
            OperationStatus::Pending(old_attempt)
        );
        let new_attempt = catalog.claim_recovery(first_operation.key)?;
        assert_ne!(new_attempt, old_attempt);
        assert_eq!(
            catalog.register_objects(old_attempt, &[object(41, vec![])]),
            Err(IndexError::FenceLost)
        );
        let leaf = object(42, vec![]);
        reserve_confirm(&catalog, new_attempt, std::slice::from_ref(&leaf))?;
        let next = view(1, genesis.profile, Some(leaf.reference));
        assert_eq!(
            publish(&catalog, old_attempt, &genesis, next.clone()),
            Err(IndexError::FenceLost)
        );
        assert_eq!(
            publish(&catalog, new_attempt, &genesis, next.clone())?,
            OperationStatus::Published(next.identity)
        );
        let second = operation(3, 71);
        let attempt = admit_new(&catalog, second)?;
        let stale = view(2, genesis.profile, Some(leaf.reference));
        assert_eq!(
            commit_empty(
                &catalog,
                PreparedPublication {
                    attempt,
                    expected: genesis.identity,
                    next: stale,
                    work_actions: WorkActionSet::empty(),
                    complete_work: None,
                    source_events: vec![],
                    projection_events: vec![],
                    expected_source: None,
                }
            ),
            Err(IndexError::Conflict)
        );
        assert_eq!(
            catalog.inspect(second.key)?,
            OperationStatus::Pending(attempt)
        );
        Ok(())
    }

    #[test]
    fn source_frontier_has_own_sequence_and_is_atomic_with_view() -> TestResult {
        let root = tempfile::tempdir()?;
        let (catalog, genesis) = setup(root.path())?;
        let first_operation = operation(500, 81);
        let event = SourceEvent {
            producer: ProducerId(id(8)),
            stream: StreamId(id(9)),
            stream_epoch: 1,
            predecessor: 0,
            sequence: 1,
            payload: digest_id(83),
        };
        let attempt = admit_new_events(&catalog, first_operation, &[event])?;
        let frontier = object(82, vec![]);
        reserve_confirm(&catalog, attempt, std::slice::from_ref(&frontier))?;
        let mut next = view(1, genesis.profile, None);
        next.source.frontiers = MapRoot(Some(frontier.reference));
        next.identity.digest = next.identity_digest()?;
        assert_eq!(
            commit_empty(
                &catalog,
                PreparedPublication {
                    attempt,
                    expected: genesis.identity,
                    next: next.clone(),
                    work_actions: WorkActionSet::empty(),
                    complete_work: None,
                    source_events: vec![event],
                    projection_events: vec![],
                    expected_source: Some(genesis.source),
                }
            )?,
            OperationStatus::Published(next.identity)
        );
        let second = operation(501, 84);
        let second_event = SourceEvent {
            predecessor: 1,
            sequence: 2,
            payload: digest_id(86),
            ..event
        };
        let attempt = admit_new_events(&catalog, second, &[second_event])?;
        let another = object(85, vec![]);
        reserve_confirm(&catalog, attempt, std::slice::from_ref(&another))?;
        let mut following = view(2, genesis.profile, None);
        following.source.frontiers = MapRoot(Some(another.reference));
        following.identity.digest = following.identity_digest()?;
        assert_eq!(
            commit_empty(
                &catalog,
                PreparedPublication {
                    attempt,
                    expected: next.identity,
                    next: following.clone(),
                    work_actions: WorkActionSet::empty(),
                    complete_work: None,
                    source_events: vec![second_event],
                    projection_events: vec![],
                    expected_source: Some(next.source),
                }
            ),
            Ok(OperationStatus::Published(following.identity))
        );
        assert_eq!(
            catalog.inspect(second.key)?,
            OperationStatus::Published(following.identity)
        );
        Ok(())
    }

    #[test]
    fn projection_event_advances_frontier_without_source_change_and_replays_receipt() -> TestResult
    {
        let root = tempfile::tempdir()?;
        let (catalog, genesis) = setup(root.path())?;
        let first = operation(601, 151);
        let event = SourceEvent {
            producer: ProducerId(id(12)),
            stream: StreamId(id(13)),
            stream_epoch: 1,
            predecessor: 0,
            sequence: 1,
            payload: digest_id(152),
        };
        let attempt = admit_new_events(&catalog, first, &[event])?;
        let projection = object(153, vec![]);
        reserve_confirm(&catalog, attempt, std::slice::from_ref(&projection))?;
        let mut next = view(1, genesis.profile, None);
        next.projections = MapRoot(Some(projection.reference));
        next.identity.digest = next.identity_digest()?;
        assert_eq!(
            commit_empty(
                &catalog,
                PreparedPublication {
                    attempt,
                    expected: genesis.identity,
                    next: next.clone(),
                    work_actions: WorkActionSet::empty(),
                    complete_work: None,
                    source_events: vec![],
                    projection_events: vec![event],
                    expected_source: None,
                }
            )?,
            OperationStatus::Published(next.identity)
        );
        assert_eq!(next.source, genesis.source);
        drop(catalog);
        let catalog = open(root.path())?;
        assert_eq!(
            catalog.inspect(first.key)?,
            OperationStatus::Published(next.identity)
        );
        assert_eq!(
            catalog.admit(
                first,
                replay_ingress(first),
                &[event],
                &mut admission_budget()
            )?,
            AdmissionOutcome::Existing(OperationStatus::Published(next.identity))
        );
        let exact = catalog.acquire_ticket(ViewSelection::Exact(next.identity))?;
        assert_eq!(exact.view, next);
        catalog.release_ticket(exact.id)?;

        let second = operation(602, 154);
        let second_event = SourceEvent {
            predecessor: 1,
            sequence: 2,
            payload: digest_id(155),
            ..event
        };
        let second_attempt = admit_new_events(&catalog, second, &[second_event])?;
        let following = view(2, genesis.profile, None);
        assert!(matches!(
            commit_empty(
                &catalog,
                PreparedPublication {
                    attempt: second_attempt,
                    expected: next.identity,
                    next: following.clone(),
                    work_actions: WorkActionSet::empty(),
                    complete_work: None,
                    source_events: vec![],
                    projection_events: vec![],
                    expected_source: None,
                }
            ),
            Err(IndexError::InvalidInput(_))
        ));
        assert_eq!(
            catalog.inspect(second.key)?,
            OperationStatus::Pending(second_attempt)
        );
        assert_eq!(
            commit_empty(
                &catalog,
                PreparedPublication {
                    attempt: second_attempt,
                    expected: next.identity,
                    next: following.clone(),
                    work_actions: WorkActionSet::empty(),
                    complete_work: None,
                    source_events: vec![],
                    projection_events: vec![second_event],
                    expected_source: None,
                }
            )?,
            OperationStatus::Published(following.identity)
        );
        assert_eq!(following.source, genesis.source);
        Ok(())
    }

    #[test]
    fn reserved_root_cannot_publish_and_corrupt_edge_refuses_exact_read() -> TestResult {
        let root = tempfile::tempdir()?;
        let (catalog, genesis) = setup(root.path())?;
        let operation = operation(4, 90);
        let attempt = admit_new(&catalog, operation)?;
        let leaf = object(91, vec![]);
        reserve_confirm(&catalog, attempt, std::slice::from_ref(&leaf))?;
        // Two ordered branch slots may legitimately point to one immutable child.
        let parent = object(92, vec![leaf.reference, leaf.reference]);
        catalog.reserve_objects(attempt, std::slice::from_ref(&parent))?;
        let next = view(1, genesis.profile, Some(parent.reference));
        assert!(matches!(
            publish(&catalog, attempt, &genesis, next.clone()),
            Err(IndexError::CorruptObject(_))
        ));
        assert_eq!(
            catalog.inspect(operation.key)?,
            OperationStatus::Pending(attempt)
        );
        catalog.register_objects(attempt, std::slice::from_ref(&parent))?;
        assert_eq!(
            publish(&catalog, attempt, &genesis, next.clone())?,
            OperationStatus::Published(next.identity)
        );

        let direct = Connection::open(catalog.path())?;
        let changed = direct.execute(
            "UPDATE native_object_edge_v1 SET row_digest=zeroblob(32) WHERE parent=?1",
            params![object_key(parent.reference)],
        )?;
        assert_eq!(changed, 1);
        assert!(matches!(
            catalog.acquire_ticket(ViewSelection::Exact(next.identity)),
            Err(IndexError::CorruptObject(_))
        ));
        Ok(())
    }

    #[test]
    fn refused_event_identity_retains_original_payload_across_rebind() -> TestResult {
        let root = tempfile::tempdir()?;
        let (catalog, genesis) = setup(root.path())?;
        let event = SourceEvent {
            producer: ProducerId(id(8)),
            stream: StreamId(id(9)),
            stream_epoch: 3,
            predecessor: 1,
            sequence: 2,
            payload: digest_id(104),
        };
        let first = operation(10, 100);
        let first_attempt = admit_new_events(&catalog, first, &[event])?;
        assert_eq!(
            catalog.refuse(first_attempt, Refusal::InvalidInput)?,
            OperationStatus::Refused(Refusal::InvalidInput)
        );
        let second = operation(11, 101);
        let second_attempt = admit_new_events(&catalog, second, &[event])?;
        assert_eq!(
            catalog.inspect(first.key)?,
            OperationStatus::Refused(Refusal::InvalidInput)
        );
        let competing = operation(12, 102);
        let competing_input = object(102, vec![]);
        let competing_ingress =
            stage_inputs(&catalog, competing, std::slice::from_ref(&competing_input))?;
        assert_eq!(
            catalog.admit(
                competing,
                competing_ingress,
                &[event],
                &mut admission_budget()
            ),
            Err(IndexError::Conflict)
        );
        assert_eq!(
            catalog.refuse(second_attempt, Refusal::Conflict)?,
            OperationStatus::Refused(Refusal::Conflict)
        );
        let changed = SourceEvent {
            payload: digest_id(105),
            ..event
        };
        assert_eq!(
            catalog.admit(
                competing,
                competing_ingress,
                &[changed],
                &mut admission_budget()
            ),
            Err(IndexError::DigestConflict)
        );
        assert_eq!(catalog.inspect(competing.key)?, OperationStatus::Unknown);
        let latest = catalog.acquire_ticket(ViewSelection::Latest {
            workspace: workspace(),
            epoch: 7,
        })?;
        assert_eq!(latest.view, genesis);
        catalog.release_ticket(latest.id)?;
        Ok(())
    }

    #[test]
    fn damaged_fence_counter_refuses_reopen_before_new_admission() -> TestResult {
        let root = tempfile::tempdir()?;
        let (catalog, _) = setup(root.path())?;
        let path = catalog.path().to_path_buf();
        drop(catalog);
        let direct = Connection::open(path)?;
        let changed = direct.execute(
            "UPDATE native_counter_v1 SET next=next+1 WHERE name='attempt_fence'",
            [],
        )?;
        assert_eq!(changed, 1);
        drop(direct);
        assert!(
            matches!(open(root.path()), Err(CoreError::Storage(message)) if message.contains("counter digest mismatch"))
        );
        Ok(())
    }

    #[test]
    fn ingress_reopen_rotates_fence_and_hands_off_confirmed_prefix_atomically() -> TestResult {
        let root = tempfile::tempdir()?;
        let (catalog, _) = setup(root.path())?;
        let operation = operation(700, 170);
        let leaf = object(171, vec![]);
        let request = object(170, vec![leaf.reference]);
        let first = catalog.open_ingress(operation.key)?;
        catalog.reserve_inputs(first, std::slice::from_ref(&leaf))?;
        catalog.confirm_inputs(first, std::slice::from_ref(&leaf))?;
        drop(catalog);

        let catalog = open(root.path())?;
        let resumed = catalog.open_ingress(operation.key)?;
        assert!(resumed.fence > first.fence);
        assert_eq!(
            catalog.reserve_inputs(first, std::slice::from_ref(&request)),
            Err(IndexError::FenceLost)
        );
        assert_eq!(
            catalog.discard_inputs(first, &mut admission_budget()),
            Err(IndexError::FenceLost)
        );
        catalog.reserve_inputs(resumed, std::slice::from_ref(&request))?;
        catalog.confirm_inputs(resumed, std::slice::from_ref(&request))?;
        catalog.seal_inputs(resumed, request.reference)?;
        assert!(matches!(
            catalog.admit(operation, resumed, &[], &mut admission_budget())?,
            AdmissionOutcome::New(_)
        ));
        assert!(matches!(
            catalog.inspect(operation.key)?,
            OperationStatus::Pending(_)
        ));
        assert_eq!(read_ingress(&*catalog.lock()?, operation.key)?, None);
        let operation_key = operation_key(operation.key)?;
        let direct = Connection::open(catalog.path())?;
        let protected: i64 = direct.query_row(
            "SELECT COUNT(*) FROM native_operation_object_v1 WHERE operation=?1",
            params![operation_key],
            |row| row.get(0),
        )?;
        assert_eq!(protected, 2);
        Ok(())
    }

    #[test]
    fn ingress_failure_prefix_cannot_seal_or_change_immutable_root() -> TestResult {
        let root = tempfile::tempdir()?;
        let (catalog, _) = setup(root.path())?;
        let operation = operation(701, 172);
        let leaf = object(173, vec![]);
        let request = object(172, vec![leaf.reference]);
        let ingress = catalog.open_ingress(operation.key)?;
        assert!(matches!(
            catalog.reserve_inputs(ingress, &[request.clone(), leaf.clone()]),
            Err(IndexError::CorruptObject(_))
        ));
        catalog.reserve_inputs(ingress, &[leaf.clone(), request.clone()])?;
        catalog.confirm_inputs(ingress, std::slice::from_ref(&leaf))?;
        assert!(matches!(
            catalog.seal_inputs(ingress, request.reference),
            Err(IndexError::InvalidInput(_))
        ));
        assert_eq!(catalog.inspect(operation.key)?, OperationStatus::Unknown);
        catalog.confirm_inputs(ingress, std::slice::from_ref(&request))?;
        catalog.seal_inputs(ingress, request.reference)?;
        assert_eq!(
            catalog.seal_inputs(ingress, leaf.reference),
            Err(IndexError::DigestConflict)
        );
        let wrong_request = ImmutableOperation {
            request: leaf.reference,
            ..operation
        };
        assert_eq!(
            catalog.admit(wrong_request, ingress, &[], &mut admission_budget()),
            Err(IndexError::DigestConflict)
        );
        assert_eq!(catalog.inspect(operation.key)?, OperationStatus::Unknown);
        assert!(matches!(
            catalog.admit(operation, ingress, &[], &mut admission_budget())?,
            AdmissionOutcome::New(_)
        ));
        Ok(())
    }

    #[test]
    fn admit_budget_rollback_preserves_sealed_ingress_for_retry() -> TestResult {
        let root = tempfile::tempdir()?;
        let (catalog, _) = setup(root.path())?;
        let mut operation = operation(705, 177);
        let request = object(177, vec![]);
        operation.request = request.reference;
        let ingress = catalog.open_ingress(operation.key)?;
        let mut objects = vec![request];
        for ordinal in 0_u16..257 {
            let mut bytes = [0_u8; 32];
            bytes[0] = 1;
            bytes[1..3].copy_from_slice(&ordinal.to_le_bytes());
            objects.push(RegisteredObject {
                reference: ObjectRef(Digest(bytes)),
                bytes: 1,
                children: vec![],
            });
        }
        for batch in objects.chunks(256) {
            catalog.reserve_inputs(ingress, batch)?;
            catalog.confirm_inputs(ingress, batch)?;
        }
        catalog.seal_inputs(ingress, operation.request)?;
        let mut limited = WorkBudget::new(
            WorkLimits {
                reads: 1_000,
                writes: 512,
                bytes: 1_000_000,
                mutations: 1_000,
            },
            Instant::now() + Duration::from_secs(30),
        );
        assert_eq!(
            catalog.admit(operation, ingress, &[], &mut limited),
            Err(IndexError::ResourceLimited)
        );
        assert_eq!(catalog.inspect(operation.key)?, OperationStatus::Unknown);
        assert_eq!(
            read_ingress(&*catalog.lock()?, operation.key)?.map(|row| row.stage),
            Some(ingress)
        );
        let transferred: i64 = catalog.lock()?.query_row(
            "SELECT COUNT(*) FROM native_operation_object_v1 WHERE operation=?1",
            params![operation_key(operation.key)?],
            |row| row.get(0),
        )?;
        assert_eq!(transferred, 0);

        let request_budget =
            quanta_index_core::RequestBudgetV1::for_duration(Duration::from_secs(30));
        request_budget.cancel_handle().cancel();
        let mut cancelled = WorkBudget::with_request(
            WorkLimits {
                reads: 1_000,
                writes: 1_000,
                bytes: 1_000_000,
                mutations: 1_000,
            },
            request_budget,
        );
        assert_eq!(
            catalog.admit(operation, ingress, &[], &mut cancelled),
            Err(IndexError::Cancelled)
        );
        assert_eq!(catalog.inspect(operation.key)?, OperationStatus::Unknown);
        assert!(matches!(
            catalog.admit(operation, ingress, &[], &mut admission_budget())?,
            AdmissionOutcome::New(_)
        ));
        Ok(())
    }

    #[test]
    fn discard_budget_rollback_preserves_stage_and_fence() -> TestResult {
        let root = tempfile::tempdir()?;
        let (catalog, _) = setup(root.path())?;
        let operation = operation(706, 178);
        let ingress = catalog.open_ingress(operation.key)?;
        let mut objects = vec![object(178, vec![])];
        for ordinal in 0_u16..257 {
            let mut bytes = [0_u8; 32];
            bytes[0] = 2;
            bytes[1..3].copy_from_slice(&ordinal.to_le_bytes());
            objects.push(RegisteredObject {
                reference: ObjectRef(Digest(bytes)),
                bytes: 1,
                children: vec![],
            });
        }
        for batch in objects.chunks(256) {
            catalog.reserve_inputs(ingress, batch)?;
            catalog.confirm_inputs(ingress, batch)?;
        }
        catalog.seal_inputs(ingress, operation.request)?;
        let mut limited = WorkBudget::new(
            WorkLimits {
                reads: 1_000,
                writes: 256,
                bytes: 1_000_000,
                mutations: 1_000,
            },
            Instant::now() + Duration::from_secs(30),
        );
        assert_eq!(
            catalog.discard_inputs(ingress, &mut limited),
            Err(IndexError::ResourceLimited)
        );
        assert_eq!(
            read_ingress(&*catalog.lock()?, operation.key)?.map(|row| row.stage),
            Some(ingress)
        );
        for table in ["native_ingress_intent_v1", "native_ingress_object_v1"] {
            let query = format!("SELECT COUNT(*) FROM {table} WHERE operation=?1");
            let retained: i64 = catalog.lock()?.query_row(
                &query,
                params![operation_key(operation.key)?],
                |row| row.get(0),
            )?;
            assert_eq!(retained, 258);
        }
        let request_budget =
            quanta_index_core::RequestBudgetV1::for_duration(Duration::from_secs(30));
        request_budget.cancel_handle().cancel();
        let mut cancelled = WorkBudget::with_request(
            WorkLimits {
                reads: 1_000,
                writes: 1_000,
                bytes: 1_000_000,
                mutations: 1_000,
            },
            request_budget,
        );
        assert_eq!(
            catalog.discard_inputs(ingress, &mut cancelled),
            Err(IndexError::Cancelled)
        );
        catalog.discard_inputs(ingress, &mut admission_budget())?;
        assert_eq!(read_ingress(&*catalog.lock()?, operation.key)?, None);
        assert_eq!(catalog.inspect(operation.key)?, OperationStatus::Unknown);
        Ok(())
    }

    #[test]
    fn event_claim_budget_rollback_retains_original_ingress() -> TestResult {
        let root = tempfile::tempdir()?;
        let (catalog, _) = setup(root.path())?;
        let operation = operation(707, 179);
        let request = object(179, vec![]);
        let ingress = stage_inputs(&catalog, operation, &[request])?;
        let event = SourceEvent {
            producer: operation.key.producer,
            stream: operation.key.stream,
            stream_epoch: operation.key.stream_epoch,
            predecessor: 0,
            sequence: 1,
            payload: digest_id(180),
        };
        let second_event = SourceEvent {
            stream: StreamId(id(10)),
            payload: digest_id(181),
            ..event
        };
        let mut limited = WorkBudget::new(
            WorkLimits {
                reads: 100,
                writes: 3,
                bytes: 100_000,
                mutations: 100,
            },
            Instant::now() + Duration::from_secs(30),
        );
        assert_eq!(
            catalog.admit(operation, ingress, &[event, second_event], &mut limited),
            Err(IndexError::ResourceLimited)
        );
        assert_eq!(catalog.inspect(operation.key)?, OperationStatus::Unknown);
        assert_eq!(
            read_ingress(&*catalog.lock()?, operation.key)?.map(|row| row.stage),
            Some(ingress)
        );
        let claims: i64 = catalog.lock()?.query_row(
            "SELECT COUNT(*) FROM native_event_claim_v1 WHERE operation=?1",
            params![operation_key(operation.key)?],
            |row| row.get(0),
        )?;
        assert_eq!(claims, 0);
        assert!(matches!(
            catalog.admit(
                operation,
                ingress,
                &[event, second_event],
                &mut admission_budget()
            )?,
            AdmissionOutcome::New(_)
        ));
        Ok(())
    }

    #[test]
    fn ingress_event_conflict_rolls_back_admission_without_discarding_stage() -> TestResult {
        let root = tempfile::tempdir()?;
        let (catalog, _) = setup(root.path())?;
        let event = SourceEvent {
            producer: ProducerId(id(8)),
            stream: StreamId(id(9)),
            stream_epoch: 3,
            predecessor: 0,
            sequence: 1,
            payload: digest_id(190),
        };
        let first = operation(702, 174);
        let first_attempt = admit_new_events(&catalog, first, &[event])?;
        let competing = operation(703, 175);
        let input = object(175, vec![]);
        let ingress = stage_inputs(&catalog, competing, &[input])?;
        assert_eq!(
            catalog.admit(competing, ingress, &[event], &mut admission_budget()),
            Err(IndexError::Conflict)
        );
        assert_eq!(catalog.inspect(competing.key)?, OperationStatus::Unknown);
        assert_eq!(
            read_ingress(&*catalog.lock()?, competing.key)?.map(|row| row.stage),
            Some(ingress)
        );
        assert_eq!(
            catalog.refuse(first_attempt, Refusal::Conflict)?,
            OperationStatus::Refused(Refusal::Conflict)
        );
        assert!(matches!(
            catalog.admit(competing, ingress, &[event], &mut admission_budget())?,
            AdmissionOutcome::New(_)
        ));
        Ok(())
    }

    #[test]
    fn ingress_refuses_old_epoch_before_object_intent() -> TestResult {
        let root = tempfile::tempdir()?;
        let (catalog, _) = setup(root.path())?;
        let stale = ImmutableOperation {
            key: OperationKey {
                epoch: 8,
                ..operation(704, 176).key
            },
            ..operation(704, 176)
        };
        assert_eq!(catalog.open_ingress(stale.key), Err(IndexError::Conflict));
        assert_eq!(read_ingress(&*catalog.lock()?, stale.key)?, None);
        Ok(())
    }
}
